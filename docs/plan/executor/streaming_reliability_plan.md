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

### 6.1 现状分析

**Profile 问题：**

| # | 问题 | 位置 | 严重程度 |
|---|------|------|----------|
| A | `total_rows` 三重计数：OperatorBase + ExecutorDriver + ResultStream 各自累加 | `operator_base.rs:71` `driver.rs:72` `stream.rs:70` | 高 — total_rows 膨胀 2-3 倍 |
| B | 算子名称显示为 `"unknown"`：OperatorBase 先创建 entry，硬编码 name | `operator_base.rs:46` | 中 — profile 不可读 |
| C | `input_rows` 恒为 0：Driver 仅以 `is_output=true` 调用 | `driver.rs:110-119` | 低 — 该字段无实际语义 |
| D | `into_stream()` 路径缺少 `profile_end()`：只有 `execute()` 调用 | `engine.rs:198` vs `engine.rs:323` | 中 — 流式查询 profile 无结束时间 |
| E | Profile 数据不向外暴露：ProfileCollector 随 runtime 销毁 | 全局 | 低 — 后续统一观测平台再做 |

**传输层问题：**

| # | 问题 | 位置 | 严重程度 |
|---|------|------|----------|
| F | SSE 静默吞错误：`Err(_)` 不发送 error 事件，metadata/done 照发 | `stream.rs:132` | 高 — 客户端以为查询成功 |
| G | 客户端断连不触发 cancel：channel send error 只退出拉取循环 | `stream.rs:122-127` `server.rs:237-240` | 高 — 孤立查询继续执行 |
| H | KILL QUERY 不传到 ExecutionRuntime：只删除 HashMap，不调 cancel | `query_context.rs:60` | 中 — KILL 无效 |
| I | SSE metadata columns 恒空：hardcoded `Vec::new()` | `stream.rs:141` | 中 — 客户端无法获取列名 |
| J | `batch_size` 命名误导：实际控制 channel 容量，不是 batch 大小 | `stream.rs:25-26` | 低 — 名称误导 |
| K | gRPC `QueryResultChunk` proto 缺少 column_names | `server.rs:232-235` | 中 — 客户端缺列名 |

### 6.2 修改方案

#### 6.2.1 Profile 改造（子阶段 4A）

**4A.1 删除三重计数，只保留 root 一处统计**

改动清单：

1. `operator_base.rs:71` — 删除 `profile.add_rows(count);` 行。OperatorBase 只负责 `output_rows += count`（算子本级统计），不负责 `total_rows`（全局最终输出）。
2. `stream.rs:70` — 删除 `self.runtime.profile_add_rows(count);` 行。ResultStream 不直接接触 profile counter。
3. `driver.rs:72` — 保持 `self.runtime.profile_add_rows(count);` 不变。ExecutorDriver 在 root 算子 `next()` 返回后累加，是唯一 `total_rows` 统计点。

**变更后数据流：**

```
SourceOperator.next() → output DataChunk
  → StreamingExecutor.advance() → OperatorBase.record_profile_rows()
    → entry.output_rows += count     (per-operator)
  → ExecutorDriver.next() → runtime.profile_add_rows(count)   (唯一 total_rows)
  → (ResultStream 不再计数)
```

**4A.2 修复算子名称**

1. `operator_base.rs` — 新增 `name: &'static str` 字段，默认 `"unknown"`：
   ```rust
   pub struct OperatorBase {
       pub plan_node_id: i64,
       pub name: &'static str,        // ← 新增
       pub runtime: Option<Arc<ExecutionRuntime>>,
       pub opened: bool,
       pub is_global: bool,
   }
   ```
2. `operator_base.rs:46` — 改为使用 `self.name` 而非 `"unknown"` 字面量。
3. `executor.rs` — 各 `StreamingExecutor` 变体的构造函数在创建 `OperatorBase` 时传入对应名字，或批量通过 `set_profile_name()` 设置。
4. `driver.rs:93-108` — 删除 `extract_operator_name` 和 `record_timing` 中的 entry 创建逻辑（入口由 OperatorBase 统一管理），仅保留更新计时字段。
5. `driver.rs:139-256` — 删除 `extract_operator_name` 函数（不再需要）。

> **备选方案**：不改 `OperatorBase` 结构体，而是在 `Engine::build_partitioned` / `register_executor` 时通过 runtime 统一设置 profile entry name。但这需要与 OperatorBase 的 entry 创建时序协调，更复杂。推荐直接存 name 字段。

**4A.3 删除 `input_rows` 字段**

- `runtime.rs:27` — 删除 `pub input_rows: u64` 行。
- `driver.rs:110-119` — 简化 `record_row_count` 为纯 output_rows 更新。

**4A.4 补充 `into_stream()` 路径的 `profile_end()`**

1. `stream.rs:93-101` `close_inner()` — 在 `release_resources()` 前添加 `self.runtime.profile_end();`。

   ```rust
   fn close_inner(&mut self) -> Result<(), QueryError> {
       let result = if let Some(ref mut engine) = self.engine {
           engine.stop_root();
           engine.close_root()
       } else {
           Ok(())
       };
       self.runtime.profile_end();       // ← 新增
       self.runtime.release_resources();
       result
   }
   ```

2. `engine.rs:196-200` — `execute()` 路径已有 `profile_end()`，无需改动。

#### 6.2.2 传输层改造（子阶段 4B）

**4B.1 SSE 错误事件修复**

`stream.rs:99-134` — 改造 `spawn_blocking` 拉取任务，区分耗尽与错误：

```rust
let pull_handle = tokio::task::spawn_blocking(move || {
    let mut row_index: usize = 0;
    loop {
        match stream_result.next_chunk() {
            Ok(Some(chunk)) => {
                // ... 现有行发送逻辑不变 ...
            }
            Ok(None) => return Ok(row_index),   // ← 正常耗尽
            Err(e) => return Err(e),             // ← 返回错误
        }
    }
});
```

`stream.rs:137-155` — 改造 handle 等待逻辑：

```rust
match pull_handle.await {
    Ok(Ok(total_rows)) => {
        // 正常耗尽：发送 metadata + done
        let metadata = StreamMetadata { ... };
        tx.send(Ok(Event::default().event("metadata").data(...))).await;
        tx.send(Ok(Event::default().event("done").data("{}"))).await;
    }
    Ok(Err(e)) => {
        // 执行错误：发送 error + done
        let error_msg = json!({"error": true, "message": e.to_string(), "code": "QUERY_ERROR"});
        tx.send(Ok(Event::default().event("error").data(error_msg))).await;
        tx.send(Ok(Event::default().event("done").data("{}"))).await;
    }
    Err(_) => {}  // 任务 panic 或取消，channel 已关闭
}
```

**4B.2 客户端断连触发 cancel**

`stream.rs:122-127` — channel send 失败时调用 cancel：

```rust
if tx_pull.blocking_send(Ok(Event::default().data(data))).is_err() {
    stream_result.cancel();      // ← 新增：触发取消链
    return Ok(row_index);        // ← 改为 Ok 以区分异常断开
}
```

`server.rs:237-240` — gRPC 同样：

```rust
if tx_pull.blocking_send(Ok(proto_chunk)).is_err() {
    stream_result.cancel();      // ← 新增
    return;
}
```

注意：`stream_result` 需要 `clone` 后传入闭包（当前已 clone 在 `tx_pull` 定义后）。

**4B.3 KILL QUERY 连接 cancel**

`query_context.rs` 或 `session.rs` — `kill_query` 路径中，找到对应 `QueryId` 持有的 `StreamingQueryResult` handle，调用 `.cancel()`。需要 `QueryContext` 维护 `HashMap<QueryId, Weak<ExecutionRuntime>>` 或持有 `StreamingQueryResult` 的弱引用。

具体实现方式待定，取决于 `QueryContext` 与 `StreamingQueryResult` 的生命周期关系。最低可行方案：

1. `QueryContext` 增加 `active_queries: HashMap<u32, Weak<ExecutionRuntime>>`
2. 创建 `StreamingQueryResult` 时注册 `Weak<ExecutionRuntime>` 到 `QueryContext`
3. `kill_query()` 时通过 weak 引用调用 `.cancel()`

```
// QueryContext
pub struct QueryContext {
    active_queries: HashMap<u32, Weak<ExecutionRuntime>>,
    ...
}

pub fn track_query(&mut self, query_id: u32, runtime: &Arc<ExecutionRuntime>) {
    self.active_queries.insert(query_id, Arc::downgrade(runtime));
}

pub fn kill_query(&mut self, query_id: u32) -> bool {
    if let Some(weak) = self.active_queries.remove(&query_id) {
        if let Some(rt) = weak.upgrade() {
            rt.cancel();
            true
        } else { false }
    } else { false }
}
```

**4B.4 SSE metadata 列名修复**

`stream.rs:141` — 跟踪首个 chunk 的列名：

```rust
// 在 spawn_blocking 外部
let first_columns: Arc<Mutex<Option<Vec<String>>>> = Arc::new(Mutex::new(None));

// 在 spawn_blocking 内部，首次收到 chunk 时
let mut cols = first_columns.lock();
if cols.is_none() {
    *cols = Some(chunk.col_names());
}

// 在 pull_handle 等待后
let columns = first_columns.lock().take().unwrap_or_default();
let metadata = StreamMetadata { columns, ... };
```

**4B.5 `batch_size` 重命名**

`stream.rs:22-27`：

```rust
pub struct StreamQueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default = "default_buffer_capacity")]
    pub event_buffer_capacity: usize,
}
fn default_buffer_capacity() -> usize { 100 }
```

**4B.6 gRPC 增加 column_names**

`proto/graphdb.proto:214-217`：

```protobuf
message QueryResultChunk {
    repeated Row rows = 1;
    bool is_last = 2;
    repeated string column_names = 3;    // 新增
}
```

`server.rs:232-235` — 从首个 chunk 提取 column_names 并在所有后续 chunk 中携带：

```rust
// 在 spawn_blocking 外部
let schema: Arc<Mutex<Option<Vec<String>>>> = Arc::new(Mutex::new(None));

// 在循环内
let mut cols = schema.lock();
if cols.is_none() {
    *cols = Some(chunk.col_names());
}
let column_names = cols.clone().unwrap_or_default();
let proto_chunk = super::proto::QueryResultChunk {
    rows: proto_rows,
    is_last: false,
    column_names,          // ← 新增
};
```

### 6.3 验收标准

| 验收项 | 验证方式 |
|--------|----------|
| `total_rows` 等于 root 最终输出行数（非 2-3 倍） | 搭建两层 chain (Source → Filter)，对比 `total_rows` 与客户端收到行数 |
| profile 不出现 `"unknown"` 算子名 | 任意查询后 profile 中所有 entry 的名称非空且非 "unknown" |
| SSE mid-stream 错误发送 error 事件 | 模拟 `next_chunk()` 在第 3 个 chunk 返回错误；客户端收到 error + done，无 metadata |
| gRPC mid-stream 错误返回 grpc-status: Internal | 同上，验证 gRPC 流终止于错误状态码 |
| 客户端断连后 runtime 被 cancel | 验证断连后 `stream_result.is_cancelled()` 为 true |
| KILL QUERY 导致正在执行的 next() 返回取消错误 | 执行长查询同时在另一连接执行 KILL |
| SSE metadata 包含列名 | 验证 `"columns"` 字段非空 |
| `event_buffer_capacity` 参数生效 | 改小后 SSE 背压行为变化（channel 满时阻塞） |

## 七、阶段五：分区执行的设计与实现

### 7.1 现状与约束

当前分区基建状态：

| 组件 | 状态 | 备注 |
|------|------|------|
| `PartitionView` | ✅ 已完成 | range-based 分片 |
| `build_partitioned()` | ✅ 已实现未接入 | builder.rs:2047-2149 |
| `register_partition_executors()` | ✅ 已实现 | engine.rs:66-82 |
| `execute_partitions()` | ✅ 已实现 | 单线程顺序执行 |
| `is_global` 标注 | ✅ 已定义未使用 | operator_base.rs:12 |
| `ScanVertices`/`ScanEdges` partition-aware | ✅ 已完成 | source_operator.rs |
| 生产路径接入 | ❌ 未做 | factory.rs/query_pipeline_manager.rs |
| 并行执行 | ❌ 未做 | 当前单线程 |
| Merge/Gather/Exchange 算子 | ❌ 未做 | 不存在 |
| 分区 profile 聚合 | ⚠️ 有 bug | HashMap 用 plan_node_id 做 key，分区互相覆盖 |

**设计前提**：分区执行的前提是单线程执行协议完全闭环（阶段一至四）、source 一次性语义正确（阶段一）、以及 `close`/`stop`/`drop` 资源释放正确（阶段二）。在阶段四完成前，分区 API 保持实验状态，不接入生产执行计划。

### 7.2 分区语义定义

#### 7.2.1 算子分类

| 类别 | 定义 | 示例 | 分区行为 |
|------|------|------|----------|
| 局部算子 | 结果不依赖跨分区数据 | Scan, Filter, Project, LocalLimit, PassThrough, Unwind | 每个分区独立执行整棵树 |
| 全局算子 | 结果需要全量数据 | Sort, Aggregate, Distinct, Join, TopN, GlobalLimit | 需 exchange + merge 阶段 |
| 交换边界 | 在分区之间重分布数据 | Shuffle (hash/range) | 将数据路由到正确分区 |

#### 7.2.2 两阶段执行模式

全局算子需要拆分为两个阶段：

```
第一阶段（局部）            第二阶段（全局）
┌──────────┐              ┌──────────┐
│ Scan(P0) │──┐           │ Merge    │
├──────────┤  │           ├──────────┤
│ Scan(P1) │──┤─── 局部 ──→│ Sort     │
├──────────┤  │  结果     ├──────────┤
│ Scan(P2) │──┤           │ GlobalLimit│
├──────────┤  │           ├──────────┤
│ Scan(P3) │──┘           │ 输出     │
└──────────┘              └──────────┘
```

即：`局部树 × N` → Exchange（Gather + 可选 Shuffle） → `全局树 × 1`

**关键设计点**：两棵树的 runtime（MemoryBudget、cancel_token）必须共享，以保证内存预算全局一致、取消全树传播。

### 7.3 实现方案

#### 7.3.1 Exchange 算子设计

新增 `Gather` 算子（非阻塞 gather + merge sort）：

```
GatherOperator {
    inputs: Vec<StreamingExecutor>,   // N 棵局部子树
    mode: GatherMode,
}

enum GatherMode {
    /// 无顺序要求：直接拼接（UNION ALL 语义）
    Concatenate,
    /// 需要全局有序：多路归并
    MergeSort {
        sort_keys: Vec<SortKey>,
        limit: Option<usize>,
    },
}
```

`Gather` 不是 `StreamingExecutor` 变体，而是在 `StreamingExecutionEngine` 层面管理的执行协调器。根算子变成 `Gather` 时，引擎的 `execute()` `next_chunk_from_root()` 等入口需要适配。

**备选方案**：不引入新算子类型，而是在 engine 层面实现 `execute_partitions()` 的 merge 版本。但对于 `into_stream()` 路径（逐个 chunk 输出），engine 层 merge 更自然——`next_chunk_from_root()` 内部轮询各分区的 `next()`。

#### 7.3.2 全局算子拆分

**Sort** → LocalSort（每个分区内排序）+ MergeSort（多路归并合并有序流）

**Aggregate/GroupBy**：
- 可下推的聚合（SUM, COUNT, MIN, MAX）：局部 partial aggregate → 全局 final aggregate
- 不可下推的聚合（AVG,  DISTINCT 聚合, 百分位数）：局部全量分组 → 全局再聚合

**Distinct** → LocalDistinct（分区内去重）+ GlobalDistinct（全局去重，使用 BloomFilter 或 hash set）

**Join**：
- HashJoin：build 阶段需要在单个分区收集全部一侧数据 → exchange 后确保 join key 落在同一分区，或使用 broadcast join（小表复制到所有分区）
- NestedLoopJoin：不适合分区，退化为单线程

**Limit** → LocalLimit（每个分区取 N 行）+ GlobalLimit（取前 N 行）

#### 7.3.3 Startup API

在 `StreamingExecutionEngine` 上新增：

```rust
pub fn execute_partitioned(
    &mut self,
    local_trees: Vec<StreamingExecutor>,
    global_tree: Option<StreamingExecutor>,
) -> Result<Vec<DataChunk>, QueryError>
```

执行流程：
1. 对所有局部树执行 `open → loop next → close`，收集所有分区结果
2. 如果有全局树，将分区结果作为输入执行全局树
3. 聚合 profile、释放资源

#### 7.3.4 Planner 标注

1. `PlanningContext` 新增 `partition_count: usize` 字段
2. 优化器根据表和索引分布确定 `partition_count`
3. Plan 树遍历时标注每个算子的 `is_global` 属性
4. `StreamingExecutorBuilder` 根据 `is_global` 和 `partition_count` 决定是构建单棵树还是 `build_partitioned`

```rust
// PlanNode 新增
pub fn is_global(&self) -> bool {
    matches!(self.kind,
        PlanNodeKind::Sort | PlanNodeKind::Aggregate | PlanNodeKind::Distinct
        | PlanNodeKind::Join | PlanNodeKind::TopN | PlanNodeKind::Limit(None)
    )
}
```

#### 7.3.5 Profile 聚合修正

修复分区 profile 互相覆盖的问题：

`ProfileCollector::record_operator_profile` 使用 `(plan_node_id, partition_id)` 作为 key，或在每个 partition 内建立独立的 profile 记录，最后在 engine 层聚合：

```rust
pub struct OperatorProfile {
    pub node_id: i64,
    pub partition_id: usize,          // ← 新增
    pub name: String,
    ...
}

// 聚合时：
// total_rows = 所有 partition 之和
// peak_memory = 所有 partition 的 max
// timing = 总 wall clock（partition 不适合累加 timing，因为是顺序执行）
```

或者更简单：为每个 partition 创建独立的 ProfileCollector，由 engine 聚合关键指标（sum rows, max peak_memory, wall clock timing）。

### 7.4 分阶段交付

| 子阶段 | 内容 | 前置 |
|--------|------|------|
| 5A | Profile 聚合修复 + partition 基础测试 | Phase 4 |
| 5B | Gather/MergeSort 算子设计实现 + engine 层 support | 5A |
| 5C | 全局算子拆分（Sort/Distinct/Aggregate/Limit） | 5B |
| 5D | Planner 标注 + 生产路径接入 | 5C |
| 5E | HashJoin exchange 和 broadcast join | 5D |
| 5F | 并行执行（可选）：多线程分区执行 | 5E |

### 7.5 验收标准

| 验收项 | 验证方式 |
|--------|----------|
| 分区 scan 结果与全 scan 行数一致（无全局算子时） | 分区 + 单树结果相等 |
| 全局 Sort 的分区结果与单树结果等价 | 排序后内容相等 |
| 全局 Aggregate 的分区结果与单树结果等价 | 聚合值相等 |
| 全局 Distinct 的分区结果与单树结果等价 | 去重后集合相等 |
| Gather MergeSort 在多分区下有正确顺序 | 多路归并后全局有序 |
| 全局 Limit 在分区后不超过指定行数 | 限制成立 |
| 分区 profile 显示正确行数和峰值内存 | 总和正确 |
| 分区下单分区失败 → 全部释放 | 错误测试 |

## 八、建议提交顺序

| 提交 | 内容 | 前置条件 |
| --- | --- | --- |
| 1 | 修复一次性 source 状态、错误传播与递归扫描 | 无 |
| 2 | 引入执行清理 guard，统一 close/stop/drop | 提交 1 |
| 3 | 修复 runtime 注册顺序和长循环取消 | 提交 2 |
| 4 | CAS 内存预算与精确 tracker 释放 | 提交 2 |
| 5 | ✅ 统一 profile 统计（4A） | 提交 2 |
| 6 | ✅ SSE/gRPC 错误、断连和 metadata 语义（4B） | 提交 2 |
| 7 | 🕑 分区 profile 聚合与基础设施加固（5A） | 提交 5 |
| 8 | 🕑 Gather/MergeSort 与全局算子拆分（5B, 5C） | 提交 7 |
| 9 | 🕑 Planner 标注与分区执行接入生产（5D, 5E） | 提交 8 |
| 10 | 🕑 并行多线程执行（5F，可选） | 提交 9 |

每个提交均应包含对应的单元测试和至少一个 API 层集成测试；不要把并行改造与单线程正确性修复混在同一个提交中。
