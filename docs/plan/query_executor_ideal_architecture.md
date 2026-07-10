# Query Executor 理想架构分析

> 分析日期：2026-07-10
> 范围：`crates/graphdb-query/src/query/executor`
> 目标：定义最终应收敛到的 query executor 架构，而不是短期补丁列表。

## 总体目标

理想的 query executor 不应只是“能把 plan node 跑完”的执行层，而应成为 planner、storage、transaction、API、profile 之间稳定的运行时内核。它需要同时满足以下目标：

- 正确性：planner 产生的变量、类型、schema、join 语义、图路径语义在 executor 中不丢失。
- 流式输出：从 storage scan 到 API response 都能按 chunk/stream 消费，只有兼容旧接口时才显式 materialize。
- 可治理：每个 query 有取消、超时、内存预算、事务句柄、资源生命周期。
- 可观测：PROFILE 能看到每个 operator 的估算值、实际行数、耗时、内存、IO、spill。
- 可演进：先有单线程 pull 内核，之后能自然扩展到 pipeline DAG、partition 并行和 spill。

最终架构建议采用“稳定单线程 pull 内核 + 明确 pipeline breaker + 可选 morsel 并行”的路线。不要把并行调度作为第一层抽象；先让每个 operator 的输入输出契约、schema 契约和资源契约稳定。

## 分层架构

理想结构可以分为六层：

```text
API / Embedded / C API / HTTP
        |
ResultStream / DataSet materializer
        |
ExecutionRuntime
        |
PhysicalPlan -> OperatorFactory -> OperatorTree / PipelineDAG
        |
Operator implementations
        |
StorageCursor / IndexCursor / Transaction / Search / Vector
```

### 1. 结果输出层

目标是让执行结果默认以 `ResultStream` 暴露：

- `next_chunk() -> Result<Option<DataChunk>>`
- 支持客户端取消和背压。
- 支持 API 层边执行边返回。
- `ExecutionResult::DataSet` 只作为兼容层，由 `ResultStream` materialize 得到。

当前 `StreamingExecutionEngine::execute() -> Vec<DataChunk>` 会把所有 chunk 收集起来。理想状态下 engine 不再返回 `Vec<DataChunk>`，而是返回一个可拉取的 stream handle。

建议接口形态：

```text
QueryExecutor::execute(plan, runtime) -> ResultStream
ResultStream::next_chunk() -> Option<DataChunk>
ResultStream::close()
```

### 2. ExecutionRuntime 层

每个 query 执行时应创建一个 `ExecutionRuntime`，贯穿所有 operator。

它负责：

- `query_id` / `session_id`
- 当前 space 信息
- storage / transaction / search / vector 资源句柄
- cancel token 和 deadline
- memory tracker
- profile/instrumentation sink
- resource owner，用于统一关闭 cursor、临时文件、spill 文件、锁和事务资源

理想情况下，operator 不再各自零散持有上下文，而是持有 runtime 引用和自己的 operator state。

建议抽象：

```text
ExecutionRuntime
  - QueryIdentity
  - RuntimeResources
  - Cancellation
  - MemoryManager
  - ProfileCollector
  - ResourceOwner
```

这样可以把取消、内存、PROFILE、事务、资源释放从“约定”变成“运行时强制路径”。

### 3. PhysicalPlan 与 OperatorFactory 层

Planner/Optimizer 输出的 plan 应在进入 executor 前形成明确的 physical contract：

- 每个 physical node 有唯一 `plan_node_id`。
- 每个 node 有确定的 input schema 和 output schema。
- 每个表达式已经绑定到 slot。
- 每个 join 已明确 join kind、build side、probe side、key slot、null 语义。
- 每个 blocking operator 标记为 pipeline breaker。
- 每个 scan/index seek 带 pushdown 条件、projection、limit、partition/range。

`StreamingExecutorBuilder` 最终应演进为 `OperatorFactory`：

- 输入：physical plan node + runtime + child operator。
- 输出：operator 实例。
- 不做语义补全，不隐式丢弃 plan 字段。
- 不支持的 physical node 必须显式报错。
- partial 支持的 node 必须在支持矩阵中标注。

## 数据模型

### 1. Slot-based schema

最终 executor 不应依赖字符串列名查找变量，而应使用 slot layout。

核心结构：

```text
SlotLayout
  - slots: Vec<SlotInfo>
  - name_to_slot: HashMap<Symbol, SlotId>

SlotInfo
  - slot_id
  - name / alias
  - type
  - nullability
  - origin: variable / property / expression / system column
```

所有 operator 都使用 slot id 读写值：

- Filter：表达式读取 input slot。
- Project：生成新的 output slot。
- Join：拼接左右 slot，并处理同名冲突。
- Aggregate：group key slot + aggregate output slot。
- Traversal：新增 vertex/edge/path slot。

`DataChunk::from_rows()` 的首行推断只应保留给测试。生产路径必须显式传入 `SlotLayout`。

### 2. Chunk 表示

短期可以保留 row-based chunk：

```text
RowChunk
  - layout: Arc<SlotLayout>
  - rows: Vec<Vec<Value>>
```

但架构上应允许未来切换为 columnar chunk：

```text
ColumnarChunk
  - layout: Arc<SlotLayout>
  - columns: Vec<ValueVector>
  - selection: Optional<SelectionVector>
```

因此 operator 代码不应长期绑定到 `Vec<Vec<Value>>`。建议抽象一个最小 chunk access trait：

- 按 slot 读取单行值。
- 追加输出列。
- 获取行数。
- 迭代 selected rows。

这能让当前实现先保持简单，又为未来向量化留出空间。

### 3. 表达式执行

理想表达式执行分两阶段：

1. Planner/Builder 阶段：把 AST expression 绑定为 slot-based physical expression。
2. Runtime 阶段：表达式解释执行或 JIT/向量化执行。

表达式不应每行 clone row 和 col_names，也不应在运行时解析变量名。错误策略需要统一：

- 字段不存在：plan/build 阶段错误。
- 类型不匹配：validate 或 runtime 类型错误。
- null 传播：按表达式语义返回 null。
- 函数错误：带函数名、参数类型、plan_node_id。

## Operator 模型

### 1. 生命周期

理想 operator 接口：

```text
Operator
  - open(runtime)
  - next(runtime) -> Option<DataChunk>
  - close(runtime)
```

`stop()` 不应只是递归调用，而应与 cancel/backpressure/limit 协同：

- `Limit` 达到上限后向上游发出 early stop。
- API 取消时 runtime cancel token 生效。
- blocking operator 在 collect/build/sort/hash/traversal 循环中周期性检查 cancel。

### 2. Operator 分类

建议按行为而不是语法分类：

- Source：scan、index seek、fulltext search、vector search、argument。
- Streaming transform：filter、project、limit、assign、remove、append。
- Binary streaming/blocking：hash join、nested loop join、apply。
- Pipeline breaker：sort、aggregate、distinct、window、materialize、set ops。
- Graph runtime：expand、variable length traversal、shortest path、subgraph。
- Sink / side-effect：insert、update、delete、DDL、transaction control。

每类 operator 要有统一的资源和统计规则。例如 pipeline breaker 必须声明：

- 是否需要全量读取输入。
- 是否可 spill。
- 使用哪个 memory tracker。
- 输出是否保持顺序。

### 3. Enum 与 trait 的取舍

当前大 enum 适合早期开发，但最终会有两个选择：

方案 A：继续 enum-based。

- 优点：静态分发、调试直观、Rust 类型检查强。
- 缺点：variant 过多，生命周期 match 膨胀。

方案 B：trait object + operator state。

- 优点：扩展算子容易，factory 清晰。
- 缺点：动态分发、对象生命周期复杂。

建议路线：短中期继续 enum-based，但拆分领域 enum；成熟后再评估是否引入 trait object。

可演进形态：

```text
PhysicalOperator
  - Source(SourceOperator)
  - Unary(UnaryOperator)
  - Binary(BinaryOperator)
  - Blocking(BlockingOperator)
  - Graph(GraphOperator)
  - Sink(SinkOperator)
```

这样比一个 80+ variant 的单体 enum 更容易维护，同时保留静态分发。

## Storage 与 Scan 架构

Scan 是执行器性能和资源模型的底座。理想状态下 storage 不应返回全量 `Vec`，而应提供 cursor/batch reader。

建议接口能力：

- `open_vertex_scan(space, options) -> VertexCursor`
- `open_edge_scan(space, options) -> EdgeCursor`
- `open_index_scan(space, index, predicate, options) -> IndexCursor`
- cursor 支持 `next_batch(max_rows)`
- scan options 支持 projection、predicate、limit、partition/range、snapshot/transaction。

Scan operator 行为：

- `open()` 创建 cursor。
- `next()` 从 cursor 读取一个 batch 并转换成 `DataChunk`。
- `close()` 关闭 cursor。
- cancel 时及时释放 cursor。

这样才能让 filter/limit/index seek 真正减少 IO，并让 API 层流式消费有意义。

## Join 架构

Join 应先收敛到稳定的 slot-based hash join，再扩展其他 join。

理想 `HashJoin` contract：

- join kind：inner / left / right / full / semi / anti。
- build side：left 或 right，由 optimizer 决定。
- build key slots。
- probe key slots。
- output layout。
- null equality policy。
- memory budget。
- spill strategy。

执行策略：

1. build 阶段读取 build side。
2. 按 key 构建 hash table，并记录内存。
3. 超预算时进入 partition spill 或返回明确错误。
4. probe 阶段逐 chunk 输出结果。
5. outer join 额外维护 matched bitset。

Nested loop join 只应作为 fallback：

- 非等值 join。
- 小表 apply。
- 显式 cross join。

不要让普通 inner join 默认退化成 nested loop。

## Blocking 与 Spill 架构

Sort、Aggregate、Distinct、Window、HashJoin 都是潜在 pipeline breaker。理想架构中它们共享同一个 blocking runtime 能力：

- memory reservation。
- peak memory tracking。
- spill file 管理。
- external sort。
- partitioned hash aggregate。
- partitioned hash join。

短期可以只实现 memory budget + 明确错误。最终目标是：

- 小数据内存执行。
- 中数据 bounded memory + spill。
- 大数据可取消、可 profile、不会 OOM。

所有 blocking operator 都必须在 PROFILE 中展示：

- build/read rows。
- output rows。
- peak memory。
- spill bytes。
- spill partitions。
- blocking time。

## 图查询执行架构

图数据库的 executor 不能只把 traversal 当成普通 relational operator。理想架构应引入统一 traversal runtime。

核心抽象：

```text
TraversalRuntime
  - frontier
  - visited policy
  - path policy
  - edge filter
  - vertex filter
  - direction
  - edge type set
  - depth range
  - limit
  - memory/cancel/profile hooks
```

不同图算子复用该 runtime：

- Expand：一跳 traversal。
- Traverse：可变长度 traversal。
- ShortestPath：BFS / bidirectional BFS。
- AllPaths：带 path policy 和 limit 的枚举。
- Subgraph：从种子点扩展的图抽取。

Optimizer 需要把以下信息传入 runtime：

- 起点选择。
- expand 方向。
- 边类型过滤。
- vertex/edge predicate pushdown。
- 最大深度和 limit。
- 是否允许重复点/边/路径。

这样图查询才不会停留在“operator 内直接调用 storage”的阶段。

## DML / DDL / Transaction 架构

DML 和 DDL 不应只是返回若干行的普通 operator。它们是 side-effect operator，必须与 transaction runtime 绑定。

理想行为：

- 每个写 operator 从 runtime 获取 transaction handle。
- 写入以 batch 方式执行。
- operator 记录 write set / affected rows。
- 失败时交给 transaction manager rollback。
- DDL 通过 metadata manager 和 storage schema manager 统一执行。
- side-effect operator 在 profile 中记录写入行数、耗时、冲突/约束错误。

事务控制 operator：

- `BeginTransaction` 创建或绑定 transaction context。
- `Commit` / `Rollback` 操作 runtime 中的 transaction handle。
- 普通 auto-commit 查询由 pipeline manager 在外围控制。

## PROFILE / EXPLAIN 架构

理想的 PROFILE 不是单独 executor，而是运行时 instrumentation 的视图。

每个 operator 需要统一记录：

- plan_node_id。
- operator name。
- estimated rows / cost / memory。
- actual input rows。
- actual output rows。
- open time。
- next time。
- close time。
- storage read rows / bytes。
- peak memory。
- spill bytes。
- cancel/error 状态。

执行结束后，PROFILE 把这些统计回填到 plan tree，形成：

```text
PlanNode
  estimated rows/cost
  actual rows/time/memory/io
  children...
```

这样才能回答三个关键问题：

- optimizer 估算是否准确。
- 慢在哪里。
- 内存和 IO 消耗在哪里。

## 并行执行架构

并行不应建立在共享一个 mutable root executor 上。理想路线是先稳定单线程 pull，然后引入 pipeline DAG。

并行模型：

- 把 physical plan 切成 pipeline。
- pipeline breaker 切断 pipeline。
- source 按 partition/range/morsel 产生任务。
- 每个 task 拥有独立 operator state。
- worker 处理 morsel，输出 chunk 到下游 exchange。
- blocking operator 汇总局部状态，再 merge。

关键组件：

- `Pipeline`
- `Morsel`
- `Task`
- `Exchange`
- `LocalOperatorState`
- `GlobalOperatorState`

并行阶段的基本规则：

- operator state 必须区分 local 和 global。
- source partition 必须真实对应 storage range。
- memory budget 要支持 per-worker 和 global。
- PROFILE 要能聚合每个 worker 的统计。

在这些条件未满足前，应保持单线程 pull，不引入表面并行。

## 目标执行流程

理想查询执行流程如下：

1. Parser 生成 AST。
2. Validator 绑定 schema、类型、变量。
3. Planner 生成 logical plan。
4. Optimizer 生成 physical plan，确定 slot layout、join strategy、scan pushdown、pipeline breaker。
5. PipelineManager 创建 `ExecutionRuntime`。
6. OperatorFactory 构建 operator tree 或 pipeline DAG。
7. QueryExecutor 返回 `ResultStream`。
8. API 层按需拉取 chunk。
9. Runtime 持续记录 profile、检查取消、管理内存和资源。
10. 查询结束后关闭 stream，释放 resource owner，回填 PROFILE。

## 目标模块拆分

建议最终目录形态：

```text
query/executor/
  runtime/
    execution_runtime.rs
    cancellation.rs
    memory.rs
    resource_owner.rs
    profile_sink.rs
  physical/
    physical_plan.rs
    physical_expr.rs
    slot_layout.rs
    operator_factory.rs
  chunk/
    data_chunk.rs
    row_chunk.rs
    columnar_chunk.rs
  operators/
    source/
    unary/
    binary/
    blocking/
    graph/
    sink/
  traversal/
    runtime.rs
    policies.rs
  stream/
    result_stream.rs
    materialize.rs
  explain/
```

当前 `streaming/` 模块可以逐步迁移到这些子模块，不需要一次性重写。

## 演进路线

### 阶段 1：稳定单线程执行内核

- 保持当前 root pull 模型。
- 引入 `ExecutionRuntime`。
- 引入 `ResultStream`，旧 `DataSet` 由 stream materialize。
- 所有 operator 接入 cancel/memory/profile 的最小接口。

### 阶段 2：Slot schema 和表达式绑定

- Planner 输出 `SlotLayout`。
- `DataChunk` 生产路径强制携带 layout。
- 表达式按 slot 访问。
- Join/project/aggregate/traversal 输出明确 layout。

### 阶段 3：Storage cursor 和 scan pushdown

- storage 提供 batch cursor。
- scan operator 真正按需读取。
- limit/predicate/projection/index seek 下推。

### 阶段 4：Blocking operator 治理

- 统一 memory tracker。
- PROFILE 记录 peak memory。
- sort/hash aggregate/hash join 预留并实现 spill。

### 阶段 5：Graph traversal runtime

- 抽象 frontier/visited/path/filter/depth/limit。
- `Expand`、`Traverse`、`ShortestPath` 等复用 runtime。
- optimizer 把 traversal strategy 传给 executor。

### 阶段 6：Pipeline DAG 和并行

- 切分 pipeline。
- per-partition/per-morsel operator state。
- exchange 和 local/global state。
- PROFILE 聚合 worker 统计。

## 判断标准

理想架构完成后，应满足以下检查项：

- 大扫描查询可以边读边返回，不需要全量 materialize。
- `LIMIT 10` 能让上游 scan 尽早停止。
- Join key 不依赖字符串列名或整行 debug string。
- 空结果、首行 null、同名列、混合类型不会破坏 schema。
- Sort/Aggregate/Join 超预算时有明确错误或 spill，不会 OOM。
- PROFILE 能定位慢算子和内存高峰。
- 客户端取消能中断 scan、join、sort、traversal。
- DML 失败能按事务语义 rollback。
- 图遍历的方向、深度、去重、路径策略由统一 runtime 控制。
- 并行执行时不同 worker 不共享同一个 mutable executor state。

## 总结

最终理想架构的核心不是“把当前 enum 拆得更漂亮”，而是建立四个稳定契约：

1. 数据契约：slot-based schema + chunk。
2. 执行契约：operator lifecycle + streaming result。
3. 资源契约：runtime 管理 cancel、memory、transaction、resource owner。
4. 观测契约：每个 plan node 都有真实 execution stats。

只要这四个契约稳定，当前 enum-based executor 可以继续演进；未来切换到 pipeline DAG、列式 chunk、spill 或并行执行时，也不会推翻上层语义。
