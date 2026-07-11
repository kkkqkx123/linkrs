# Query Executor 模块实现分析

> 分析日期：2026-07-10  
> 范围：`crates/graphdb-query/src/query/executor` 以及 query pipeline 中调用 executor 的主路径。

## 结论

当前 `graphdb-query` 的 executor 已经不是单纯的原型入口：普通查询会经过 `QueryPipelineManager::execute_plan()`，统一构造 `StreamingQueryExecutor`，再由 `StreamingExecutorBuilder` 将 `PlanNodeEnum` 递归转换成一棵 `StreamingExecutor` enum 执行树。执行引擎是单线程 pull-based 模型，具备 `open -> next -> close` 生命周期、`DataChunk` 批处理、`ResultStream` 流式句柄、`ExecutionRuntime`、基础取消检查、内存预算和 profile 结构。

但是，和 PostgreSQL、DuckDB、DataFusion、Velox、Neo4j、Memgraph 这类成熟数据库执行引擎相比，当前 executor 仍更接近“宽表面、浅语义”的功能型执行层，而不是稳定的数据库执行内核。主要问题不是缺少入口，而是执行模型、数据模型、资源模型、算子语义、profile 统计和 optimizer/runtime 闭环都还没有形成强约束。

特别需要注意：项目已经引入了一些正确方向的结构，例如 `ResultStream`、`ExecutionRuntime`、`SlotLayout`、storage cursor scan、`ProfileCollector`、pipeline graph。但是这些结构多数还没有完全贯穿主路径或全部 operator，因此不能简单等同于“已经具备成熟执行引擎能力”。

## 当前实现方式

### 1. 模块结构

`crates/graphdb-query/src/query/executor` 主要由以下部分组成：

- `base/`：传统执行基础设施，包括 `Executor` trait、`ExecutionContext`、`ExecutionResult`、`ExecutorStats`、`MemoryBudget`、`MemoryTracker` 等。
- `streaming/`：当前主执行框架，包括 `DataChunk`、`StreamingExecutor`、`StreamingExecutionEngine`、`StreamingExecutorBuilder`、`StreamingQueryExecutor`、`ExecutionRuntime`、`ResultStream`、`SlotLayout` 等。
- `streaming/executor/operators/`：按算子类型拆分实现，包括 source、single input、stateful、binary join、set ops、graph traversal、DML、search、management、control flow 等。
- `expression/`：表达式求值、内置函数和 evaluation context。
- `algorithms/`：BFS、Dijkstra、A*、bidirectional BFS、多最短路、子图等图算法。
- `pipeline/`：pipeline breaker 分析和单线程 runner。目前更偏分析/实验路径，不是普通查询默认执行路径。
- `explain/`：EXPLAIN / PROFILE 相关执行器和格式化。
- `traversal/`：图遍历 runtime、reader、stats 等结构。

`executor/mod.rs` 明确把 streaming executor 作为 primary execution framework 对外导出，同时也导出 `ExecutionRuntime`、`ResultStream`、`SlotLayout`、`ProfileCollector` 等新执行期结构。

### 2. 主执行链路

普通查询的主链路如下：

1. `QueryPipelineManager` 创建 `QueryContext`。
2. 执行 parse、validate、plan、optimize。
3. `execute_plan()` 取优化后 plan 的 root node。
4. 构造 `ExecutionContext`，注入 storage、space name、fulltext/vector manager 等资源。
5. `StreamingQueryExecutor::from_plan_node()` 调用 `StreamingExecutorBuilder::from_plan_node()`。
6. builder 递归将 `PlanNodeEnum` 转换成嵌套的 `StreamingExecutor` enum 树。
7. `StreamingQueryExecutor::execute()` 将 engine 转成 `ResultStream`。
8. `ResultStream::collect()` 逐 chunk 拉取结果，最终合并为 `DataSet`。
9. 对外返回 `ExecutionResult::DataSet`。

因此，内部已经有 chunk stream 句柄，但默认 query pipeline 仍会 materialize 成完整 `DataSet`。API 层还没有把 `ResultStream` 作为结果协议贯通。

### 3. 执行模型

`StreamingExecutionEngine` 是明确的单线程 pull engine：

- 只保存一个 root executor。
- `register_executor()` 实际注册 root。
- `execute()` 或 `ResultStream::next_chunk()` 驱动 root。
- 没有 worker pool、task scheduler、work stealing 或 morsel/partition 调度。
- 注释明确说明并行基础设施保留为参考，不在主执行路径使用。

`StreamingExecutor` 是大型 enum，源码注释称包含 79 个左右 operator variant。生命周期通过 `open()`、`next()`、`stop()`、`close()` 的大 `match` 分发到不同 operator 模块。这种设计的优点是没有 trait object 调度成本，结构直观；缺点是扩展算子需要同时修改 enum、生命周期分发、builder 映射和具体 operator，实现容易分散且难以强制语义一致。

### 4. 数据表示

当前执行期批数据为 `DataChunk`：

- 行存格式：`Vec<Vec<Value>>`。
- schema：`Arc<Schema>`，字段为 `ColumnInfo { name, data_type }`。
- 可选 slot layout：`Option<Arc<SlotLayout>>`。
- `DataChunk::new_with_layout()` 可以从 `SlotLayout` 构造 schema。
- 但 `from_rows()` / `from_rows_with_col_names()` 仍存在按首行推断类型、无列名时生成 `col_N` 的兼容路径。
- `get_or_create_layout()` 会在缺少 layout 时按 schema 临时生成 slot layout。

这说明 slot-based 方向已经开始引入，但尚未成为强制执行协议。大量 operator 和表达式求值仍依赖列名、`Vec<Value>` 行、schema 推断或临时列名。

### 5. Source 和 scan

当前有两类 scan：

- `ScanVertices` / `ScanEdges`：预加载 buffer，主要用于测试或内部包装。
- `StorageScanVertices` / `StorageScanEdges`：持有 `VertexCursor` / `EdgeCursor`，在 `next()` 中按 `CHUNK_SIZE = 1024` 拉 batch。

相比旧的“一次 scan 全部收集进 buffer”已经有明显改进：executor 边界上 scan 是 cursor/chunk 形态，`Limit` 有机会提前停止。但是源码注释也说明底层 cursor 可能仍是 Vec-backed bridge，真实 IO 是否流式取决于 storage 层 cursor 能力。当前 scan 也还没有完整的 predicate/projection/index condition 下推协议。

### 6. Runtime、取消和 profile

`ExecutionRuntime` 集中了：

- query identity。
- cancel token 和 deadline。
- per-query memory budget。
- `ProfileCollector`。
- `ResourceOwner`。

`ExecutorDriver` 在 root operator 的 `open`、`next`、`close` 外层做取消检查和计时统计。`ResultStream::next_chunk()` 每次拉取也会检查 cancel token。

但当前 driver 只是包住被 engine 直接调用的 root executor。子 executor 的递归调用仍发生在各 operator 内部，除非 operator 自己显式接入 runtime，否则不会自动得到 per-operator cancel/profile。`extract_plan_node_id()` 也没有覆盖所有 variant，feature-gated search variant 甚至退回 0。profile 当前能记录入口级 output rows 和部分 peak memory，但还不是成熟的 per-plan-node instrumentation。

### 7. Pipeline 模块

`pipeline/` 可以分析 plan tree，识别 pipeline breaker，生成 `PipelineGraph`。`PipelineRunner` 支持：

- `execute_flat()`：从 root plan 构造单棵 `StreamingExecutor` 执行。
- `execute_pipelined()`：实验性地按 pipeline 分段执行，在 breaker 边界 materialize。

源码注释已经说明 pipelined mode 是验证/实验路径，复杂 plan 输出可能不完全一致。普通查询默认没有走 pipeline runner，因此它目前更像未来并行/分段执行的结构准备，而不是生产级 pipeline execution engine。

### 8. Builder 映射状态

`StreamingExecutorBuilder` 覆盖了大量 `PlanNodeEnum`，包括 scan、filter、project、limit、aggregate、sort、join、set ops、DDL/DML、全文/向量、图遍历、控制流等。

但覆盖面不等于语义完整。当前仍能看到一些明显的 partial 映射：

- `InnerJoin` 映射到 `NestedLoopJoin`，只有 `HashInnerJoin` 映射到 `HashJoin`。
- `HashLeftJoin` 映射到普通 `LeftJoin`。
- `Sample` 构造了 input executor，但最终返回的 `StreamingExecutor::Sample` 不保存 input。
- `Apply` 构造了右输入但未挂入 executor，只把 apply kind 转成 literal。
- `PatternApply` 使用 `Null` literal 作为 pattern。
- `ShortestPath`、`BFSShortest`、`AllPaths` 等目标点常为 `None`，方向默认 `"both"`。
- `MultiShortestPath` 的 target vertices、edge type 为空。
- `GetVertices`、`GetEdges`、`GetNeighbors` 多数参数为 `None` 或默认值。
- 部分 DML 仍使用 `col_0`、`col_1` 这种临时列名约定。

这些实现可能足以通过部分 happy path，但对完整数据库语义而言风险较高。

## 与其他数据库执行引擎的对比

### PostgreSQL / Volcano Iterator

PostgreSQL 是典型 Volcano iterator 执行器，每个 plan node 有稳定的 executor state、tuple slot、expression context、resource owner、instrumentation。父节点逐行拉取子节点，但执行期 tuple layout、表达式上下文、资源释放和统计是强约束。

当前项目同样采用 pull-based 思路，但差距明显：

- 没有强制 slot layout，仍保留字符串列名和 `col_N` 兼容路径。
- schema 可能执行期从首行推断，而不是 planning 后固定。
- `ExecutionRuntime` 尚未贯穿每个 operator。
- profile 不是完整 per-node instrumentation。
- resource owner 结构存在，但 cursor、buffer、临时内存、事务资源没有统一接入。
- builder 存在多个 partial 映射，plan node 到 operator 的语义不够可验证。

### DuckDB / DataFusion / Velox

现代分析型执行器通常以 vectorized batch 或 columnar batch 为基础：

- 数据使用 typed vector、Arrow RecordBatch、selection vector 或类似列式结构。
- 表达式按 batch/vector 执行。
- filter/project/join/aggregate 尽量避免逐行 clone 和字符串查找。
- pipeline breaker 明确管理内存、spill、并行分区和统计。

当前项目虽然有 `DataChunk`，但底层仍是 row-based `Vec<Vec<Value>>`。它具备“批”的外形，但没有获得列式向量化的核心收益。`SlotLayout` 是正确方向，但还没有替代 `ValueRowContext`、列名映射、首行 schema 推断等旧路径。

### HyPer / morsel-driven parallelism

HyPer 类执行器通常围绕 pipeline DAG 和 morsel 调度工作：

- source 将数据切成 morsel。
- worker 独立处理分片。
- pipeline breaker 切断流水线。
- state 按 partition/local/global 分层。
- 调度器处理 work stealing、NUMA/locality 和 backpressure。

当前主路径是单 root 单线程 pull。这个选择比半成品并行更清晰，但也意味着现有 `partition`、`pipeline`、`ExecutionMode` 相关结构还没有提供真实并行执行能力。

### Neo4j / Memgraph 等图数据库

成熟图数据库执行器通常围绕 variable binding、slot、expand、path state、visited policy、index seek、label scan、relationship scan、路径唯一性和 early pruning 建模。optimizer 的 cardinality/selectivity 估计会影响起点、方向、expand 顺序、join/expand 策略和路径剪枝。

当前项目有图遍历 operator 和独立 algorithms 模块，但整体仍是 `Vec<Value>` 行、字符串方向、operator 内直接 storage call、部分目标参数默认值。图查询可以覆盖一部分功能，但还不是成熟 graph pattern runtime。

## 主要不足

### 1. 对外查询结果仍默认 materialize

虽然已有 `ResultStream`，但 `QueryPipelineManager::execute_plan()` 调用 `StreamingQueryExecutor::execute()`，后者会 `collect()` 成 `DataSet`。

影响：

- 大结果集仍需要完整驻留内存。
- HTTP/gRPC/embedded API 层不能自然 chunk streaming。
- 客户端取消、网络背压、分页拉取无法作为执行协议传入 executor。
- `ResultStream` 的价值主要停留在内部接口，未成为主结果模型。

建议：把 `ExecutionResult` 扩展为支持 streaming result，API 层按 chunk 消费；需要兼容旧接口时再显式 collect。

### 2. Slot layout 没有成为强制协议

当前 `DataChunk` 支持 `layout: Option<Arc<SlotLayout>>`，但 optional 本身说明执行协议还不稳定。大量路径仍可通过 schema/col name/首行推断运行。

影响：

- 变量、别名、tag/edge 属性和表达式输出没有统一 slot id 绑定。
- join/project/filter/aggregate 容易出现列名不一致或 `right_N` 之类临时约定。
- 空结果、首行 null、混合类型会导致 schema 不稳定。
- 表达式执行难以高效化。

建议：validation/planning 后生成固定 `SlotLayout`，executor 禁止从首行推断生产 schema；表达式按 slot id 访问，列名只用于输出展示。

### 3. Row-based `Vec<Vec<Value>>` 限制性能上限

当前 `DataChunk` 是 row-oriented，`Value` 是动态类型，表达式和算子常按行处理。

影响：

- cache locality 差。
- clone 成本高。
- typed comparison、hash、sort、aggregate 难以优化。
- 无法获得现代向量化执行器的 SIMD/vector/batch expression 优势。

建议：短期先稳定 slot-based row layout；中长期引入 columnar chunk 或 typed vector，并保留 row adapter 作为兼容层。

### 4. Runtime 只在入口包装，未深入 operator 树

`ExecutorDriver` 包装 root 的生命周期调用，但子 operator 的 `next()` 是由父 operator 直接调用的。长循环内部是否检查取消、是否记录统计，取决于具体 operator 实现。

影响：

- 长时间 sort/hash join/graph traversal 不能保证及时响应 cancel。
- profile 统计不能精确到每个 plan node。
- resource owner 难以管理所有 cursor、临时文件和 blocking buffer。
- memory budget 是存在的，但不是统一 query runtime 的强约束入口。

建议：让 operator 生命周期接收 `ExecutionRuntime`，或给每个 operator 包装 driver；长循环固定周期检查 cancel；所有 memory tracker、cursor、temp resource 都注册到 runtime。

### 5. Blocking operator 只有内存预算，没有 spill

`Aggregate`、`Sort`、`GroupBy`、`WindowFunction`、`HashJoin`、`NestedLoopJoin`、set ops、`Materialize` 等都可能收集大量输入。部分 operator 有 `MemoryTracker`，但策略主要是预算检查和报错。

缺失：

- external sort。
- hash aggregate spill。
- hash join partition/spill。
- TopN bounded heap 与完整 sort 的明确边界。
- blocking operator peak memory 的全树统计。
- optimizer 的 memory estimate 与 executor 的实际内存反馈闭环。

建议：先统一 blocking operator memory accounting 和 profile，再为 sort/hash aggregate/hash join 设计 spill trait 和 temp storage。

### 6. Join 策略和语义成熟度不足

当前 join 的主要风险：

- `InnerJoin` 默认 nested loop，大输入下容易退化。
- `HashInnerJoin` 才映射到 `HashJoin`，optimizer 是否稳定生成该节点会直接影响性能。
- `HashLeftJoin` 实际映射到 `LeftJoin`，命名和执行策略不一致。
- right side 列名通过 `right_N` 改写，属于临时约定。
- outer/semi/full/cross join 的 null 语义、重复行、空输入行为需要系统测试确认。
- 没有 runtime filter、bloom filter、adaptive build side、spill。

建议：优先收敛 equi-hash-join：固定 build/probe side、slot key、null 语义、输出 layout、内存预算和测试矩阵；再扩展 outer/semi/full。

### 7. PlanNode 到 executor 的映射存在 partial/placeholder

builder 覆盖面很宽，但多个节点没有完整使用 plan 信息。

具体例子：

- `Sample` 丢掉 input。
- `Apply` 丢掉 right input。
- `PatternApply` pattern 为空值。
- shortest path 系列缺目标点。
- `GetVertices` / `GetEdges` / `GetNeighbors` 使用默认参数。
- `MultiShortestPath` target 和 edge type 为空。

影响：

- 用户看到语法/plan 支持，但结果语义可能不完整。
- 新增 plan node 时容易被“能构造 executor”掩盖问题。
- 测试如果只覆盖 happy path，难以发现输入丢失、schema 丢失、条件丢失。

建议：建立 `PlanNodeEnum -> StreamingExecutor` 支持矩阵，分为 `ready`、`partial`、`unsupported`；partial 节点应优先显式报错，除非已有完整语义测试。

### 8. Graph traversal runtime 与 optimizer 闭环弱

图遍历 operator 直接持有 storage，在 operator 内维护 visited/frontier/path 状态。方向、edge type、target vertex 等仍有字符串或默认值路径。

影响：

- 起点选择、方向选择、边类型过滤、深度限制与 executor 参数没有强类型协议。
- 可变长度路径、最短路、all paths 的去重和剪枝策略难以统一。
- traversal 缺少统一 cancel/budget/profile。
- `algorithms/` 和 streaming graph traversal operator 边界不够清晰。

建议：抽象统一 traversal runtime，包括 frontier、visited policy、path uniqueness、edge filter、vertex filter、depth、limit、target、cancel、memory budget、profile。

### 9. Pipeline graph 还不是生产级 pipeline engine

pipeline analyzer 能生成 `PipelineGraph`，但普通查询没有使用它。`execute_pipelined()` 会在 pipeline 边界 materialize，并且源码标注复杂 plan 输出可能不完全一致。

影响：

- 目前不能依赖 pipeline 模块获得并行、backpressure 或 breaker-level resource control。
- pipeline breaker 信息没有反馈给默认 executor。
- 未来并行如果直接叠加在当前 enum tree 上，会遇到共享 mutable state 和 operator state 划分问题。

建议：先把 pipeline graph 作为 explain/analysis 工具；等 slot layout、operator runtime、blocking boundary 稳定后，再实现 per-pipeline state 和 scheduler。

### 10. EXPLAIN / PROFILE 未形成估算与实际闭环

已有 `ProfileCollector`、`OperatorProfile`、`ExecutionStatsContext`、`NodeExecutionStats` 等结构，但默认 profile 还不能完整回答成熟数据库 profile 需要的问题。

缺少：

- 每个 plan node 的 actual rows。
- 每个 operator 的 input rows / output rows。
- 每个 operator 的 open/next/close 时间。
- storage read rows / bytes。
- filtered rows。
- peak memory 和 spill 次数。
- estimated rows vs actual rows。

建议：以 plan node id 为 profile key，把 runtime instrumentation 接到所有 operator；PROFILE 输出同时展示 optimizer estimate 和 executor actual。

## 建议优先级

### P0：建立 executor 支持矩阵

短期先把 builder 的真实支持状态写清楚：

- ready：语义完整且有测试。
- partial：可构造但丢失 plan 信息或语义不完整。
- unsupported：应显式报错。

对 `Sample`、`Apply`、`PatternApply`、shortest path、`GetVertices/GetEdges/GetNeighbors` 等 partial 节点，优先补测试或改成显式错误。

### P1：强制 slot-based schema

目标：

- planner 输出固定 `SlotLayout`。
- `DataChunk` 生产路径必须携带 layout。
- 表达式按 slot id 访问。
- join/project/aggregate 明确输出 layout。
- `col_N` 和首行 schema 推断只保留在测试/兼容 adapter。

这是修复表达式、join、profile、类型推断和性能问题的基础。

### P1：贯通外部 ResultStream

目标：

- `ExecutionResult` 或 API 层支持 streaming result。
- HTTP/gRPC/embedded API 能逐 chunk 拉取。
- cancel/backpressure 能传回 executor。
- materialize 变成显式选择，而不是默认行为。

### P2：让 ExecutionRuntime 深入所有 operator

目标：

- operator 生命周期接收 runtime。
- 长循环周期性检查 cancel。
- cursor、临时文件、blocking buffer 注册到 resource owner。
- memory tracker 统一挂到 runtime。
- profile 按 plan node id 记录。

### P2：完善 blocking operator 资源模型

目标：

- 所有 blocking operator 使用统一 memory tracker。
- 记录 peak memory。
- 超预算错误包含 operator 和 plan node。
- sort/hash aggregate/hash join 预留 spill 接口。
- TopN 使用 bounded memory 策略。

### P3：收敛 join 和 traversal 两个核心复杂域

Join：

- 先做好 equi-hash-join 正确性和性能。
- 固定 slot key、输出 layout、null 语义、build/probe side。
- 补 outer/semi/full/cross 的边界测试。

Traversal：

- 建立统一 traversal runtime。
- 明确 path uniqueness、visited policy、target、direction、edge filter。
- 将 optimizer 的起点/方向/选择率决策落到 executor 参数。

### P3：再实现真正 pipeline/parallel execution

等单线程 pull executor 的 schema、runtime、profile 和 blocking boundary 稳定后，再实现：

- pipeline DAG。
- per-pipeline/per-partition operator state。
- morsel/chunk scheduler。
- pipeline breaker。
- runtime filter。
- work stealing。

不要让多个 worker 共享同一棵 mutable executor tree。

## 总体评价

当前 executor 的积极面是：主路径已经统一，算子表面覆盖很宽，scan 已经推进到 cursor/chunk 形态，并且引入了 `ResultStream`、`ExecutionRuntime`、`SlotLayout`、`ProfileCollector`、pipeline graph 等关键结构。

当前最大问题是：这些结构尚未形成不可绕过的执行协议。默认结果仍 materialize，slot layout 仍 optional，profile 主要在 root driver 层，pipeline 不在主路径，builder 存在多处 partial 映射，blocking operator 没有 spill，join 和 graph traversal 的完整语义还需要系统收敛。

下一阶段最务实的目标不是马上做并行或重写全部算子，而是把单线程 pull executor 打磨成可靠内核：固定 slot schema、明确 builder 支持矩阵、贯通 streaming result、让 runtime 深入 operator、统一 blocking memory/profile。这个内核稳定后，再引入 spill、图遍历 runtime 和真正的 pipeline 并行。
