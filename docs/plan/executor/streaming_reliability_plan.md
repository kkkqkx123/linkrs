# Streaming Executor 可靠性改造方案

> 制定日期：2026-07-11
> 前置文档：`docs/issue/streaming_executor_issues.md`
> 目标：将当前单线程 pull executor 收敛为正确、可取消、可释放资源、可观测的执行内核

## 一、范围与原则

本方案不引入线程池、异步算子、trait object 或分区并行。先稳定现有 `open -> next -> stop -> close` 协议，再讨论 pipeline 和并行执行。

原则：

1. `next()` 必须推进状态，有限输入最终必须返回 `None`。
2. 一旦 `open()` 成功，无论后续成功、失败、取消或客户端断开，都必须恰好执行一次资源关闭。
3. 错误不得被转换为空数据或成功完成事件。
4. 内存预算必须保守计账，失败后保持可恢复的一致状态。
5. profile 只保留一个统计所有者，每个指标有明确语义。

## 二、目标执行协议

执行状态定义为：`New -> Open -> Exhausted | Failed | Cancelled -> Closed`。

- `open()`：只允许从 `New` 进入 `Open`；打开子算子时若失败，关闭已成功打开的子算子。
- `next()`：只允许在 `Open` 调用；返回 chunk 后保持 `Open`，返回 `None` 后进入 `Exhausted`。
- `stop()`：用于提前消费终止，停止上游继续生产；必须允许在 `Open`、`Exhausted`、`Failed` 和 `Cancelled` 调用。
- `close()`：无条件释放本算子与全部已打开子算子的资源；必须幂等，不能因取消检查而拒绝执行。
- 执行入口：使用 guard 保证 `close()`、runtime cleanup 和 `profile_end()` 在所有退出路径执行；若执行和关闭都失败，保留执行错误并记录关闭错误。

第一阶段可继续使用 `OperatorBase::opened`，但需补充终止状态；后续可将状态收敛为专用 `OperatorLifecycle` 枚举，避免布尔值不足以表达错误和耗尽状态。

## 三、阶段一：恢复 Source 正确性

### 3.1 统一 source 状态模型

为 source 分为两类实现：

| 类型 | 变体 | 实现方式 |
| --- | --- | --- |
| 游标型 | StorageScanVertices、StorageScanEdges | 保存 cursor；每次最多产生一个固定大小 chunk |
| 一次性读取型 | GetVertices、GetEdges、GetNeighbors、EdgeIndexScan、IndexScan、GetProp、LookupIndex | 在 `open()` 读取并建立 `VecDeque`/迭代器，或保存 `emitted: bool`；读取完成后下一次必须返回 `None` |

不要在 `next()` 中重复执行同一存储查询。结果可能超过 chunk 大小的一次性 source 必须按相同批大小切分，而不是一次返回整个 `Vec`。

### 3.2 错误和递归处理

1. 将所有 `unwrap_or_default()` 替换为 `map_err`，错误消息包含 source 类型、space、索引或 edge type。
2. 将分区空批次的递归调用改为 `loop`。
3. 每次游标循环和大规模结果切分前调用 runtime 的取消检查。
4. source `open()` 负责初始化状态并设置 `base.opened`；`close()` 清空 cursor/buffer，保持幂等。

### 3.3 验收标准

- 每个 source 均测试“非空首次输出、再次调用最终返回 `None`”。
- 结果超过一个 chunk 时，无重复、无遗漏、顺序符合 source 定义。
- 模拟存储错误时返回 `QueryError`，而不是空结果。
- 连续大量空分区批次不产生递归栈增长。

## 四、阶段二：统一生命周期、取消和资源释放

### 4.1 引入执行清理 guard

在 `StreamingExecutionEngine` 中用私有 guard 或单一 `finish_execution` 方法统一处理：

1. 成功耗尽：`close -> profile_end -> release_resources`。
2. `open` 或 `next` 失败：关闭已打开树，结束 profile，释放 runtime 资源，再返回原错误。
3. `ResultStream` 失败、显式关闭、drop、网络客户端断开：先调用 `stop`，再调用 `close`。

`close()` 不得调用 `ensure_not_cancelled()`，并需继续关闭其他子节点，即使某一子节点关闭失败。

### 4.2 Runtime 所有权收敛

保留 runtime 注入到每个算子的设计，删除或降级重复计时逻辑。`register_executor()` 和 `register_partition_executors()` 在 engine 已有 runtime 时必须立即递归注入；`set_runtime()` 继续覆盖已有树。

为长循环提供统一的 `base.ensure_not_cancelled()` 调用点，至少覆盖 source cursor、blocking materialization、join build/probe 和图遍历。

### 4.3 验收标准

- 自定义会在 open、next、close 失败的测试算子，验证每个已打开节点只关闭一次。
- cleanup callback 在成功、失败、取消和提前 drop 下都执行一次。
- 在 blocking source/join 循环中取消后可在有限批次内返回取消错误。
- 先设置 runtime 后注册 executor 与正常顺序具有相同取消/profile 行为。

## 五、阶段三：修复内存预算与阻塞算子资源模型

### 5.1 预算预留

将 `MemoryBudget::try_reserve` 改为 compare-and-swap 预留：只有新值未超过上限时才提交计数。使用 `checked_add` 防止整数溢出。

`MemoryTracker` 记录每次成功预留的精确字节数；close 直接释放 tracker 的 `current_bytes`，而不是根据当前 state 重新估算。状态在错误退出时也由 guard 关闭并释放。

### 5.2 计账边界

第一版采用保守估算，至少覆盖：

- 行向量的容量与 `Value` 元素；
- `String`、Blob、List、Map、Set、Path、Vector 等 `Value` 的递归堆内存；
- hash 表 entry、排序工作区和结果缓冲；
- 聚合和 join 的中间结构。

估算无法可靠覆盖的结构应优先改为增量构建并按容器容量计账。spill 暂不实现，但预算错误必须明确返回，不能继续分配。

### 5.3 验收标准

- 小预算下 Sort、Aggregate、Join、Distinct、Materialize 均在越界前失败。
- 失败后共享 `MemoryBudget::current()` 恢复到零。
- 多个 blocking operator 共享预算时，累计使用不超过上限。
- profile 可记录每个 blocking operator 的峰值内存。

## 六、阶段四：统一 profile 和网络错误语义

### 6.1 Profile

确定指标语义：

- `total_rows`：仅 root 最终输出行数；
- `input_rows`/`output_rows`：仅对应算子的边界吞吐；
- 计时：由执行器 dispatch 记录；
- 算子名称：在 `OperatorBase` 初始化或 profile entry 创建时一次性设置。

删除 `ExecutorDriver`、`StreamingExecutor` 与 `ResultStream` 的重复计数路径，仅保留一处 root 输出累计。profile 结束必须发生在成功和失败两种路径。

### 6.2 HTTP SSE 与 gRPC

定义统一传输语义：

- 正常耗尽才发送完成标记；
- 运行时错误必须发送 SSE `error` 或 gRPC `Status`，不能伪装为成功完成；
- 客户端断开时取消 runtime，随后执行 stop/close；
- metadata 需要在首个 chunk 取得 schema 后发送，或在独立 schema 事件中发送；
- 将 HTTP `batch_size` 重命名为 `event_buffer_capacity`，或真正把它接入 chunk 大小配置。

### 6.3 验收标准

- 两层 unary pipeline 的最终 `total_rows` 等于客户端收到的行数，而每个 operator 的吞吐独立正确。
- profile 不出现 `unknown` 算子名。
- SSE/gRPC 在第 N 个 chunk 失败时均向客户端报告失败，且不会发送成功完成标记。
- 客户端读取首个 chunk 后断开，执行器资源在测试超时前释放。

## 七、阶段五：限定并明确分区能力

在前三个阶段完成前，`build_partitioned` 和 `register_partition_executors` 保持实验 API，不接入生产计划。

后续接入前必须先实现：

1. 由 planner 标注局部算子、全局算子和 exchange 边界；
2. 分区 source 的稳定分片规则，支持数值与字符串 VertexId；
3. `Gather`、`MergeSort`、全局 Aggregate/Distinct/Join 的 merge 算子；
4. 分区错误取消、内存预算和 profile 的聚合；
5. 单树与分区树在语义测试中的结果等价。

在此之前，顺序运行多棵完整树后拼接结果只能用于受限的无全局语义 source scan 测试。

## 八、建议提交顺序

| 提交 | 内容 | 前置条件 |
| --- | --- | --- |
| 1 | 修复一次性 source 状态、错误传播与递归扫描 | 无 |
| 2 | 引入执行清理 guard，统一 close/stop/drop | 提交 1 |
| 3 | 修复 runtime 注册顺序和长循环取消 | 提交 2 |
| 4 | CAS 内存预算与精确 tracker 释放 | 提交 2 |
| 5 | 统一 profile 统计和算子名称 | 提交 2 |
| 6 | SSE/gRPC 错误、断连和 metadata 语义 | 提交 2 |
| 7 | 设计并实现分区 merge/exchange | 提交 1 至 6 |

每个提交均应包含对应的单元测试和至少一个 API 层集成测试；不要把并行改造与单线程正确性修复混在同一个提交中。
