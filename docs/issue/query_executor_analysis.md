# Query Executor 模块实现分析

> 分析日期：2026-07-10
> 范围：`crates/graphdb-query/src/query/executor`

## 结论

当前 query 包的 executor 已经形成了完整的执行入口：`QueryPipelineManager` 负责 parse / validate / plan / optimize / execute，执行阶段统一走 `StreamingQueryExecutor`，再由 `StreamingExecutorBuilder` 把 `PlanNodeEnum` 递归转换成一棵嵌套的 `StreamingExecutor` enum 树。

但从成熟数据库执行引擎的标准看，当前实现仍然更接近“功能型原型”而不是稳定的执行内核。它名义上是 chunk streaming，实际只有单线程 root pull；scan、sort、aggregate、join、window、部分图遍历等算子仍大量全量 materialize；结果最后也会合并成一个 `DataSet` 返回。主要短板集中在执行模型、数据模型、资源治理、schema/slot 绑定、算子语义完整性和 profile 可观测性。

## 当前实现方式

### 1. 模块结构

`crates/graphdb-query/src/query/executor` 主要包括：

- `base/`：保留传统 `Executor<S>` trait、`BaseExecutor`、`ExecutionContext`、`ExecutionResult`、`ExecutorStats`、`MemoryBudget` 等基础结构。
- `streaming/`：当前主执行框架，包含 `DataChunk`、`StreamingExecutor`、`StreamingExecutionEngine`、`StreamingExecutorBuilder`、`StreamingQueryExecutor`。
- `streaming/executor/operators/`：按 operator 类型拆分实现，包括 `sources`、`single_input`、`stateful`、`binary`、`set_ops`、`graph_traversal`、`data_modification`、`search`、`management`、`control_flow` 等。
- `expression/`：表达式求值、内置函数注册和 evaluation context。
- `algorithms/`：BFS、Dijkstra、A*、双向 BFS、多最短路、子图等算法实现。
- `explain/`：EXPLAIN / PROFILE 相关结构和执行器。

`executor/mod.rs` 对外重新导出这些能力，并明确把 streaming executor 标为 primary execution framework。

### 2. 查询执行主路径

当前普通查询的执行链路如下：

1. `QueryPipelineManager::execute_query_with_space()` 创建 `QueryContext`。
2. 解析、验证、生成 `ExecutionPlan`，再调用 optimizer。
3. `execute_plan()` 取 optimized plan 的 root `PlanNodeEnum`。
4. 构造 `ExecutionContext`，注入 storage、space、fulltext/vector manager 等运行时资源。
5. `StreamingQueryExecutor::from_plan_node()` 调用 `StreamingExecutorBuilder::from_plan_node()`。
6. builder 递归构造一个嵌套的 `StreamingExecutor`。
7. `StreamingExecutionEngine::execute()` 对 root executor 执行 `open -> next loop -> close`。
8. root 每次 `next()` 返回 `Option<DataChunk>`。
9. `chunks_to_execution_result()` 把所有 chunk 合并为一个 `DataSet`，包装成 `ExecutionResult::DataSet`。

因此当前执行路径已经统一，但对外结果仍是 materialized result，不是真正面向 API 层的流式结果。

### 3. 执行模型

当前 `StreamingExecutionEngine` 是简化后的单线程 pull engine：

- 只保存一个 `root_executor`。
- `register_executor()` 实际只替换 root。
- `execute()` 直接驱动 root，不再使用 task scheduler、worker pool 或 partition machinery。
- 源码注释也说明 parallel execution infrastructure 仅保留为参考，不在主路径使用。

`StreamingExecutor` 本身是一个大型 enum，包含数据源、单输入、阻塞算子、join、集合操作、图遍历、DML、全文/向量搜索、DDL/管理、控制流等大量 variant。生命周期方法 `open()`、`next()`、`stop()`、`close()` 通过大 `match` 分发到对应 operator 模块。

这种设计的优点是实现直观、没有 trait object 调度成本、便于在单文件中看到完整算子表面；缺点是 enum 过大，新增算子需要修改 variant、四个生命周期分发、builder 和 operator 实现，维护成本较高。

### 4. 数据表示

执行期数据以 `DataChunk` 表示：

- 行存格式：`Vec<Vec<Value>>`
- schema：`Arc<Schema>`
- 列信息：`ColumnInfo { name: String, data_type: String }`
- 默认通过首行推断类型，没有列名时生成 `col_0`、`col_1`
- chunk size 在 source operator 中固定为 1024

这是一种易实现、易调试的数据表示，但不是高性能执行格式。表达式求值经常按行构造 `ValueRowContext`，复制 row 和 col_names，然后通过字符串列名访问变量。

### 5. 代表性算子实现

数据源：

- `ScanVertices` / `ScanEdges` 可从预加载 buffer 分块输出。
- `StorageScanVertices` / `StorageScanEdges` 在第一次 `next()` 时调用 storage scan，把全部 vertex/edge 收集到 `Vec<Vec<Value>>`，之后再按 1024 行切 chunk。
- 这意味着 scan 在 executor 边界是 lazy 的，但底层 storage API 仍然是 materializing 调用。

单输入算子：

- `Filter`、`Project`、`Limit` 等按 chunk 从 child 拉取。
- 表达式按行求值，依赖 `ValueRowContext` 和列名映射。

阻塞算子：

- `Aggregate`、`Sort`、`GroupBy`、`WindowFunction` 会先拉完全部输入。
- `Aggregate`、`Sort`、`HashJoin`、`NestedLoopJoin` 等部分算子接入 `MemoryBudget`，超过预算会报错。
- 但没有外部排序、hash aggregate spill、hash join spill，也没有统一释放预算的生命周期。

Join：

- `HashJoin` 会先全量读取 right side，构建 hash table，再 probe left chunk。
- `NestedLoopJoin` 同样全量读取 right side。
- `InnerJoin` 在 builder 中映射为 `NestedLoopJoin`，`HashInnerJoin` 才映射为 `HashJoin`。
- 右侧列名、join key 和条件表达式仍比较依赖字符串列名和临时 schema 拼接。

图遍历：

- `Expand`、`Traverse`、`ShortestPath` 等 operator 直接持有 storage，执行期在 operator 内调用 `get_node_edges()`、`get_vertex()` 等接口。
- visited set、frontier、path state 分散在具体 variant 字段里。
- planner/optimizer 的图模式、起点选择、方向选择、边类型过滤与执行期 traversal runtime 还没有形成强约束闭环。

Builder：

- `StreamingExecutorBuilder::from_plan_node()` 覆盖了大量 `PlanNodeEnum`。
- 但不少映射是简化实现：例如部分 plan node 的 right input 被解析但没有真正用于输出，路径目标或 edge type 使用默认值，`Sample` 忽略 input，部分 DML/控制流只是 start/pass-through 风格。
- 这说明 executor 表面覆盖很宽，但各算子的语义成熟度不一致。

## 与其他数据库执行引擎的对比

### PostgreSQL / Volcano Iterator

PostgreSQL 的执行器是经典 Volcano iterator：每个 plan node 有明确生命周期和 `ExecProcNode()` 类似接口，父节点逐行拉取子节点。它的强项是 plan node、tuple slot、expression context、executor state、resource owner 和 instrumentation 之间关系稳定。

当前项目也采用 pull-based 思路，但差距在于：

- 没有稳定的 tuple slot / slot id layout，仍大量依赖字符串列名。
- schema 可能在执行期从首行推断，而不是 planning 后固定。
- 生命周期中没有统一记录 input/output rows、时间、内存、IO。
- 资源释放、取消、事务状态没有深入所有 operator。
- builder 到 executor 的映射存在简化和语义缺口。

### DuckDB / Velox / DataFusion 等向量化执行

现代分析型执行器通常按 vectorized batch 处理：

- 数据以 columnar vector / Arrow RecordBatch / selection vector 表示。
- 表达式对一批值执行，而不是逐行构造上下文。
- filter/project/join/aggregate 尽量减少对象 clone 和字符串查找。
- pipeline breaker 明确管理内存和 spill。

当前项目虽然有 `DataChunk`，但它是 row-oriented `Vec<Vec<Value>>`，表达式也是 row-at-a-time。它具备“批”的外形，但没有获得列式向量化的主要收益。

### HyPer / morsel-driven parallelism

HyPer 类执行器通常把数据切成 morsel，由 worker 独立处理不同分片；pipeline breaker 会切断 pipeline，调度器做 work stealing 和局部性控制。并行的关键是 per-partition operator state，而不是多个 worker 推进同一个 mutable executor。

当前主路径没有并行调度，`engine.rs` 已经退化为单 root 单线程 pull。这个选择比“假并行”更清晰，但也意味着当前 executor 还没有真正利用 partition、worker、morsel 或 pipeline DAG。

### Neo4j / Memgraph 等图数据库执行器

图数据库执行器通常围绕 variable binding、slot、expand、path state、visited set、index seek、label scan、relationship scan、路径去重和 early pruning 建模。成熟实现会让 optimizer 的 cardinality/selectivity 估计影响起点、方向、expand 顺序和 join/expand 策略。

当前项目有图遍历 operator 和独立算法实现，但整体仍是 `Vec<Value>` + 字符串列名 + operator 内直接 storage call 的模式。图查询可以执行部分功能，但还不是成熟的 graph pattern runtime。

## 主要不足

### 1. Streaming 只存在于 executor 内部，对外仍 materialize

`StreamingExecutionEngine::execute()` 返回 `Vec<DataChunk>`，`StreamingQueryExecutor::execute()` 随后合并为一个 `DataSet`。

影响：

- 大结果集仍要完整驻留内存。
- API 层不能边执行边返回。
- 客户端取消、网络背压、分页拉取无法自然传导到 executor。
- `LIMIT` 之后的下游 stop/cancel 价值有限。

建议：引入面向 API 的 chunk stream / iterator result。兼容旧接口时再显式 materialize。

### 2. Scan 仍然全量读取 storage

`StorageScanVertices` 和 `StorageScanEdges` 第一次 `next()` 会调用 storage scan，并把全部结果放进 buffer。之后才按 chunk 输出。

影响：

- filter/limit 无法提前减少 IO。
- 大图扫描内存峰值高。
- partition id 没有真实映射到存储范围。
- storage cursor 生命周期没有进入 executor lifecycle。

建议：storage 层提供 cursor / batch reader，scan operator 在 `next()` 中按 chunk 拉取，并让 limit、predicate、projection、index seek 尽早下推。

### 3. Row-based `Vec<Vec<Value>>` 限制性能上限

当前 `DataChunk` 是行存，表达式执行频繁 clone row 和 col_names。

影响：

- CPU cache locality 差。
- `Value` 动态类型开销高。
- 字符串列名查找频繁。
- 批处理无法向量化。

建议：短期建立 slot-based row layout，去掉字符串查找；中长期可引入 columnar chunk 或 typed vector。

### 4. Schema 和变量绑定不稳定

`DataChunk::from_rows()` 默认从首行推断 schema，空结果、首行 null、混合类型都会导致不稳定。join 右侧列名可能临时生成 `right_N`，project/aggregate 又依赖变量名表达式。

影响：

- planner 阶段的变量、别名、tag/edge 字段可能丢失。
- join/project/filter 中变量引用容易和执行期列名不一致。
- 错误会在运行时才暴露，甚至被吞成 `Null` 或 `false`。

建议：validation/planning 后生成固定 schema 和 slot mapping，executor 只按 slot id 访问。

### 5. 阻塞算子只有预算检查，没有 spill 能力

`Aggregate`、`Sort`、`WindowFunction`、`HashJoin`、`NestedLoopJoin` 等会收集全部输入或 build side。部分算子已经使用 `MemoryBudget`，这是进步，但当前策略只是超预算报错。

缺失：

- 外部排序。
- hash aggregate spill。
- hash join partition/spill。
- blocking operator 的统一内存释放和峰值统计。
- optimizer cost/memory estimate 与 executor runtime 的闭环。

建议：先统一所有 blocking operator 的 memory accounting，再为 sort/hash aggregate/hash join 预留 spill 接口。

### 6. Join 语义和执行策略还不成熟

主要问题：

- 普通 `InnerJoin` builder 映射到 `NestedLoopJoin`，容易在大输入上退化。
- `HashJoin` 只覆盖 `HashInnerJoin` 路径，build/probe key、右侧列名、条件表达式仍需严格验证。
- outer/semi/full/cross join variant 很多，但不同 join 的 null 语义、输出 schema、重复行、空输入行为需要系统测试确认。
- 没有 runtime filter、bloom filter、join spill、adaptive build side 选择。

建议：优先收敛 equi-hash-join 的正确性：固定输出 schema、slot key、null 语义、build/probe side、内存预算，再扩展 outer/semi/full。

### 7. 图遍历 runtime 和 optimizer 结合弱

当前图遍历 operator 在执行期直接读 storage，路径状态和 visited set 由各 operator 自己维护。

不足：

- 起点选择、方向选择、边类型过滤、深度限制和 filter pushdown 没有稳定的执行协议。
- 可变长度路径、最短路、all paths 的路径去重和剪枝策略需要更明确。
- traversal 中缺少周期性取消检查和预算检查。
- `algorithms/` 中的算法实现与 streaming graph traversal operator 存在并行体系，边界不够清晰。

建议：抽象统一的 traversal runtime：frontier、visited policy、path policy、edge filter、vertex filter、depth/limit/cancel/budget 都作为执行参数。

### 8. Builder 覆盖面宽但语义深度不一致

`StreamingExecutorBuilder` 支持大量 `PlanNodeEnum`，但不少节点采用默认值或简化映射。

风险：

- API 表面看似支持，实际结果可能不符合完整语义。
- 新 plan node 很容易被“能编译的默认映射”掩盖问题。
- 测试如果只覆盖 happy path，难以发现 builder 丢失输入、目标、schema 或条件。

建议：维护 `PlanNodeEnum -> StreamingExecutor` 支持矩阵，标注 production ready / partial / unsupported，并为 partial 映射补充失败用例或显式 error。

### 9. EXPLAIN / PROFILE 与实际 operator 执行脱节

项目已有 `ExecutionStatsContext`、`NodeExecutionStats`、`ExecutorStats` 等结构，但 streaming operator 没有统一 instrumentation。

缺少：

- 每个 plan node 的 input rows / output rows。
- 每个 operator 的 open/next/close 耗时。
- storage read rows / bytes。
- filtered rows。
- peak memory。
- spill 次数。
- 实际 cardinality 与 optimizer 估算对比。

建议：把 plan_node_id 作为 instrumentation key，所有 operator 生命周期统一记录统计，再让 PROFILE 展示估算与实际差异。

### 10. 取消、事务和资源治理没有贯穿

当前 executor 有 `stop()`，查询上下文也有执行管理结构，但长时间 scan、sort、join、traversal 内部没有统一取消检查。

影响：

- 客户端断开或超时后，长查询可能不能及时停止。
- DML/transaction operator 与 transaction manager 的关系不够强。
- storage cursor、临时 buffer、内存预算缺少统一 close/drop 约束。

建议：建立 `ExecutionRuntime` 或类似上下文，提供 cancel token、transaction handle、memory tracker、profile sink、resource owner，由所有 operator 持有并周期性检查。

## 建议优先级

### P0：收敛执行内核语义

当前主路径已经是单线程 root pull，建议短期明确接受这个模型，先把正确性、schema、统计、资源释放做扎实。不要在当前嵌套 enum tree 上重新叠加半成品 worker 并行。

### P1：建立 slot-based schema

从 planner 输出稳定 layout：

- 每个变量、别名、表达式输出对应 slot id。
- `DataChunk` 保存固定 schema，不从首行推断。
- 表达式按 slot 访问，不按字符串查找。
- join/project/aggregate 明确输出 schema。

这是修复表达式、join、profile、类型推断和性能问题的基础。

### P1：把 scan 改为 cursor/chunk reader

目标：

- `open()` 创建 storage cursor。
- `next()` 最多读取一个 chunk。
- limit/predicate/projection/index seek 尽可能下推。
- partition id 真正映射到数据范围。

### P2：完善 blocking operator 的资源模型

目标：

- 所有 blocking operator 使用同一个 memory tracker。
- 记录 peak memory。
- 超预算错误明确带 operator 和 plan node。
- sort/hash aggregate/hash join 预留 spill 能力。

### P2：建立 executor 支持矩阵和测试矩阵

至少覆盖：

- 每个 `PlanNodeEnum` 的 builder 映射状态。
- 每个 `StreamingExecutor` variant 的实现状态。
- 空输入、null、类型错误、schema 不匹配、大输入、取消、内存超限。
- DML/DDL/search/traversal 的语义边界。

### P3：PROFILE 闭环

把每个 operator 的实际统计接入 EXPLAIN ANALYZE / PROFILE：

- estimated rows vs actual rows。
- time。
- memory。
- storage IO。
- filtered rows。
- blocking/spill 状态。

### P3：再引入真正并行

等单线程 pull executor 正确且可观测后，再设计并行：

- pipeline DAG。
- per-partition operator state。
- morsel/chunk scheduler。
- pipeline breaker。
- work stealing。
- runtime filter。

不要让多个 worker 共享同一个 mutable root executor。

## 总体评价

当前 executor 的最大价值是把数据库执行器的主要表面都搭出来了：统一 pipeline、plan-to-executor builder、chunk、表达式、join、图遍历、DML、全文/向量、EXPLAIN/PROFILE 都有入口。当前最主要的问题不是“缺少某一个算子”，而是执行模型、数据模型、资源模型和可观测性没有形成强约束。

下一阶段建议把目标缩小为一个可靠的最小执行内核：单线程 pull、cursor scan、slot schema、统一 memory tracker、明确 blocking boundary、operator instrumentation。这个内核稳定后，再扩展 spill、图遍历 runtime 和真正的 partition 并行。
