# Streaming Executor 现存问题

> 分析日期：2026-07-11
> 范围：`crates/graphdb-query/src/query/executor/streaming/`，以及 HTTP SSE、gRPC 的流式结果适配层
> 关联方案：`docs/plan/executor/streaming_refactor.md`

## 结论

Streaming Executor 已完成领域枚举拆分和 `OperatorBase` 抽取，结构上比原有大枚举更易维护。当前主执行路径仍是单线程 pull：`open -> next* -> close`。该模型适合单节点数据库的第一阶段实现，但执行协议尚未完整闭环：部分 source 不会结束，错误路径不保证关闭，内存预算与 profile 不可信，HTTP SSE 会隐藏执行错误。

在修复这些问题前，不应基于当前实现启用分区并行或将其视为稳定的流式查询接口。

## 一、正确性问题

### 1. 非游标 Source 会无限返回相同数据

**严重程度：致命**

`GetVertices`、`GetEdges`、`GetNeighbors`、`EdgeIndexScan`、`IndexScan`、`GetProp` 和 `LookupIndex` 在每次 `next()` 调用中都重新读取存储并返回完整结果。它们没有 cursor、offset、结果迭代器或 `emitted` 标记。

执行引擎会持续调用 `next()` 直到获得 `None`，因此只要这些 source 首次返回非空 chunk，查询就会无限执行并反复输出相同数据。计划构建器已经把 `GetVertices`、`GetEdges`、`GetNeighbors`、`IndexScan` 等计划节点映射到这些变体，问题可从正常查询路径触发。

相关位置：

- `crates/graphdb-query/src/query/executor/streaming/operators/source_operator.rs`
- `crates/graphdb-query/src/query/executor/streaming/builder.rs`
- `crates/graphdb-query/src/query/executor/streaming/engine.rs`

### 2. 存储错误被转换为空结果

**严重程度：高**

多个 source 使用 `unwrap_or_default()` 处理 `scan_vertices`、`scan_all_edges`、`scan_edges_by_type` 和 `lookup_index` 错误。存储读取失败会被伪装为无匹配结果，调用方无法区分“没有数据”和“读取失败”。

这会造成静默漏数，尤其会误导上层 SSE/gRPC 客户端和事务调用方。

### 3. 分区过滤使用递归跳过空批次

**严重程度：高**

`StorageScanVertices` 和 `StorageScanEdges` 在一个批次经分区过滤后为空时递归调用自身的 `next()`。若大量批次都不属于当前分区，递归深度与批次数线性增长，可能导致栈溢出。

应改为循环继续读取，且每次循环检查取消状态。

## 二、生命周期、错误与取消问题

### 4. 执行失败时不会保证 close 与资源释放

**严重程度：高**

`StreamingExecutionEngine::execute`、单 root 执行和分区执行均通过 `?` 直接返回。若 `open()` 或 `next()` 失败，不会执行 `close()`、`profile_end()` 或 `release_resources()`。`ResultStream::next_chunk()` 的错误路径也不会自动关闭 stream。

后果包括：

- cursor、临时资源和运行时 cleanup callback 可能持续保留；
- blocking operator 的内存计账无法归还；
- profile 没有结束时间，无法判断查询失败；
- 保持 `StreamingQueryResult` 的调用方可以在错误后长期占用资源。

### 5. Runtime 注入顺序存在公共 API 陷阱

**严重程度：中**

`set_runtime()` 只向调用时已注册的执行器树递归传播 runtime；之后调用 `register_executor()` 或 `register_partition_executors()` 不会为新树注入 runtime。

此时 driver 仍可在 root 调用前检查取消，但深层阻塞循环没有 runtime，无法在算子内部响应取消或记录 profile。工厂当前采用“先注册、后设置 runtime”的正确顺序，但公共引擎 API 没有保证该不变量。

### 6. stop 生命周期未被主路径使用

**严重程度：中**

`StreamingExecutor` 定义了 `stop()`，多数算子也实现了向下游传播，但 `execute()`、`ResultStream::close()` 和网络端点均只调用 `close()`。提前满足 `LIMIT`、客户端断开和取消时没有显式的“停止拉取”语义。

应定义 `stop` 与 `close` 的职责：前者停止上游生产，后者无条件释放已打开资源；二者都必须幂等且允许在取消后执行。

## 三、内存与可观测性问题

### 7. 内存预算不能有效限制真实内存

**严重程度：高**

`MemoryBudget::try_reserve()` 先增加共享计数，再检查是否越界；失败预留不会回滚。行内存估算只覆盖 `Value` 容器本身，未包含字符串、列表、地图、路径、哈希表、排序临时空间以及 `Vec` 容量。因此大量 blocking operator 即使遵守接口，也可能明显低估真实内存。

此外，失败后 `MemoryTracker::current_bytes` 与共享 budget 的计数可能不一致，后续 close 无法可靠归还。

### 8. Profile 统计重复且名称不正确

**严重程度：中**

`StreamingExecutor`、`ExecutorDriver` 和 `ResultStream` 都会累计行数。`ProfileCollector::total_rows` 因而同时包含每层算子输出和 root/stream 再次计数，无法表示最终返回行数。

同时，执行器自身的计时先创建名字为 `unknown` 的 profile 条目，driver 后续只在条目不存在时填写算子名称，导致实际 profile 可能长期显示 `unknown`。`input_rows` 也没有一致的记录来源。

## 四、网络流式接口问题

### 9. HTTP SSE 隐藏查询执行错误

**严重程度：高**

HTTP SSE 拉取任务遇到 `Err(_)` 时只停止拉取，外层仍发送 metadata 和 done 事件。客户端会将部分结果当作成功完成，无法得知查询失败。gRPC 已返回 `Status::internal`，但两条传输路径的错误语义不一致。

### 10. batch_size 只控制 channel 容量，不控制执行批大小

**严重程度：低**

HTTP 请求的 `batch_size` 被用作 SSE event channel 的容量；执行器 source 与 blocking operator 使用固定批大小。名称会误导调用方以为能够控制返回 chunk 大小或数据库读取批大小。

## 五、分区执行的边界

分区代码当前没有接入生产执行计划。它会顺序执行每棵完整执行器树并拼接结果，`OperatorBase::is_global` 也没有参与构建或调度。对于全局 `Sort`、`Aggregate`、`Distinct`、`Join` 等算子，这种模型没有 merge 阶段，不能保证与单树执行等价。

分区设施应保持实验状态，待单线程执行协议和全局 merge 语义稳定后再接入主路径。

## 六、测试缺口

当前 streaming 单元测试覆盖了普通 scan、简单链路、部分取消和分区缓冲区，但没有覆盖以下场景：

- 每个 source 的第二次 `next()` 必须返回 `None` 或后续不同批次；
- `open`、`next`、`close` 任一失败后的关闭和资源释放；
- 超出内存预算后计数恢复；
- 算子内部循环中的取消与 deadline；
- SSE/gRPC 查询中途失败、客户端断开和部分结果语义；
- profile 的算子名称、输入/输出行数及最终行数；
- 分区执行与单树执行在全局算子上的结果等价性。
