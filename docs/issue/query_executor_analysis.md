# Query Executor 模块实现分析

> 分析日期: 2026-07-10
> 范围: `crates/graphdb-query/src/query/executor`

## 结论

`graphdb-query` 当前的 executor 已经具备比较完整的模块骨架：包含基础 `Executor` trait、流式执行框架、表达式求值、图算法执行器、EXPLAIN/PROFILE 入口，以及从 `PlanNodeEnum` 构造 `StreamingExecutor` 的 builder。

但从实现质量看，它更接近“可运行的执行器原型”，还没有形成成熟数据库执行引擎的闭环。核心问题是：名义上采用 streaming / chunk / worker / partition 设计，实际执行路径仍然大量预物化数据、全量收集、全局锁串行化，并且 planner 生成的 schema、变量、统计信息、并行度、事务语义没有稳定贯穿到 executor。

## 当前实现方式

### 1. 模块结构

executor 目录主要分为以下部分：

- `base/`: 定义传统 `Executor<S>` trait、`BaseExecutor`、`ExecutionContext`、`ExecutionResult`、统计结构。
- `streaming/`: 当前主执行框架，包含 `StreamingExecutor`、`DataChunk`、`StreamingExecutionEngine`、`PipelineScheduler`、`WorkerPool`、`StreamingExecutorBuilder`。
- `streaming/executor/operators/`: 按 operator 类型拆分实现，包括 access、sources、single_input、stateful、binary、set_ops、graph_traversal、data_modification、search、management、control_flow。
- `expression/`: 表达式求值器和内置函数注册表。
- `algorithms/`: BFS、Dijkstra、A*、多最短路、子图等图算法执行器。
- `explain/`: EXPLAIN / PROFILE 相关执行器和统计上下文。

### 2. 查询执行主路径

当前生产路径大致为：

1. `QueryPipelineManager` 生成并优化 `ExecutionPlan`。
2. 取 root `PlanNodeEnum`。
3. `StreamingQueryExecutor::from_plan_node()` 调用 `StreamingExecutorBuilder::from_plan_node()`。
4. builder 递归把 `PlanNodeEnum` 转换为一个嵌套的 `StreamingExecutor` enum 树。
5. `StreamingExecutionEngine` 创建单 partition 的 `PartitionView::single(0..1)`，注册 root executor。
6. `StreamingExecutionEngine::execute()` 调度 worker 执行 task。
7. worker 对 root executor 调用 `next()`，产生 `Vec<DataChunk>`。
8. `chunks_to_execution_result()` 再把所有 chunk 合并成一个 `DataSet` 返回。

这意味着当前执行结果最终仍然是一次性 materialize 到 `DataSet`，没有向 API 层提供真正的增量输出流。

### 3. 执行模型

局部 operator 是 pull-based：

- 每个 `StreamingExecutor` variant 实现 `open()`、`next()`、`stop()`、`close()`。
- `Filter`、`Project`、`Limit` 等单输入算子从 child 拉取 `DataChunk`。
- `Sort`、`Aggregate`、`GroupBy`、`WindowFunction`、Join 和部分集合算子会先把输入全部收集到内存，再产生结果。

外层执行引擎试图做 task-based parallel execution：

- `StreamingExecutionEngine` 根据 executor id 和 partition id 构造 task。
- `WorkerPool` 通过 channel 接收 task。
- worker 从共享 `executor_registry` 中取 executor 并调用 `next()`。

但当前只有 root executor 被注册，默认只使用单 partition。即使注册多个 executor，worker 执行 `next()` 时也持有全局 `Mutex<HashMap<usize, Box<StreamingExecutor>>>` 的可变锁，实际并行度会被全局锁严重限制。

### 4. 数据表示

执行期数据以 `DataChunk` 表示：

- 行存格式：`Vec<Vec<Value>>`
- schema：`Arc<Schema>`，列名和类型都是简单字符串
- 默认通过首行推断 schema，列名生成 `col_0`、`col_1` 等

这是一种简单、易调试的表示，但不是高性能执行格式。它会导致大量 `Value` clone、行级动态分派、缓存局部性差，并且容易丢失 planner 阶段的变量名、别名和类型信息。

## 与其他数据库执行引擎的对比

### PostgreSQL / Volcano Iterator

PostgreSQL 的执行器是典型 Volcano iterator 模型，每个 plan node 暴露类似 `ExecProcNode()` 的接口，父节点逐行拉取子节点结果。它的优点是模型清晰、算子组合简单、生命周期明确；缺点是函数调用和逐行处理开销较高。

当前项目也采用 pull-based 思路，但和成熟 Volcano 模型相比存在差距：

- plan node 到 executor 的映射不完整，很多 variant 是占位或部分语义。
- tuple slot / schema / expression context 没有稳定贯穿。
- 执行状态、取消、错误、资源释放、统计采样没有统一生命周期协议。
- 部分算子错误吞掉表达式失败并返回 `Null` 或 `false`，不利于调试和语义一致性。

### DuckDB / Velox / DataFusion 等向量化执行

现代分析型执行引擎通常使用 vectorized batch：

- 数据按列或 columnar batch 表示。
- 表达式一次处理一批值。
- filter/project/join/aggregate 尽量减少 per-row 虚调用和对象分配。
- 算子之间传递的是 Arrow RecordBatch、Vector、SelectionVector 或类似结构。

当前项目虽然有 `DataChunk`，但内部仍是 `Vec<Vec<Value>>` 行存，表达式求值逐行构造 `ValueRowContext`，每行复制 row 和列名。它具备“批”的外形，但没有获得向量化执行的主要收益。

### HyPer / morsel-driven parallelism

HyPer 一类执行引擎常见设计是把数据切成 morsel，由 worker 独立处理局部分片；调度器负责 work stealing、pipeline breaker、NUMA/cache 局部性等。并行度来自“每个 worker 处理独立数据片段”，而不是多个 worker 争抢同一个 executor。

当前项目有 partition、scheduler、worker、backpressure 等概念，但执行树和状态没有按 partition 实例化。多个 task 最终可能访问同一个 executor 实例，且受全局 mutex 保护。因此当前并行框架更多是结构雏形，还没有形成真正的 morsel-driven execution。

### Neo4j / Memgraph 等图数据库执行器

图数据库执行器通常围绕 graph pattern、expand、variable binding、path state、visited set、index seek、label scan、relationship scan 建模。成熟实现会重点处理：

- variable binding 的槽位布局。
- path expansion 的去重、最短路和可变长度路径语义。
- 从 cardinality 和 selectivity 选择起点、方向、join/expand 顺序。
- traversal 过程中的 early pruning。

当前项目已经有 graph traversal operator 和算法执行器，但执行期仍然主要基于 `Vec<Value>` 和字符串列名处理变量。visited set、路径状态、filter pushdown、方向和边类型选择还没有和 optimizer 形成稳定闭环，图查询执行更偏“功能映射”，不是成熟的 slot/pipeline runtime。

## 主要不足

### 1. “流式执行”没有真正贯穿到结果输出

`StreamingExecutionEngine::execute()` 返回 `Vec<DataChunk>`，随后 `chunks_to_execution_result()` 合并成单个 `DataSet`。这会抵消 streaming 的主要价值：

- 大结果集仍然必须完整驻留内存。
- API 层无法边执行边返回。
- LIMIT、客户端取消、网络背压无法自然传导到存储扫描。

建议后续把执行结果改为 pull/iterator/stream 形式，API 层按 chunk 消费，只有确实需要兼容老接口时才 materialize。

### 2. scan 节点预先全量读取存储

`StreamingExecutorBuilder::from_plan_node()` 对 `ScanVertices` 和 `ScanEdges` 直接调用存储层 scan，把结果收集成 `Vec<Vec<Value>>`，然后交给 scan source 分块输出。

这会导致：

- 存储层无法按需读取。
- filter/limit 无法提前减少 IO。
- 大图扫描会首先消耗大量内存。
- partition 信息没有用于真实分片扫描。

成熟做法应是 scan executor 持有存储层 iterator / cursor / batch reader，并在 `next()` 中按 chunk 拉取。

### 3. 并行执行框架基本被全局锁串行化

`WorkerPool` 中所有 executor 存在共享 `HashMap`，worker 执行 task 时获取整个 registry 的 mutex，然后对 executor 调用 `next()`。这意味着同一时刻只有一个 worker 能真正推进 executor。

此外：

- task 的 partition id 没有传入 executor 的 `next()`。
- 默认只注册 root executor 和单 partition。
- `build_tasks()` 对非 source executor 的依赖关系很粗糙，不能表达任意 DAG 的 pipeline 边。
- child executor 嵌套在 parent 的 `Box<StreamingExecutor>` 中，和 registry/task DAG 是两套并存模型。

建议选择一种主模型：要么保留嵌套 pull iterator，先做单线程正确性；要么重构为 pipeline DAG，每个 partition 有独立 operator state。

### 4. schema 和变量绑定不稳定

`DataChunk::from_rows()` 根据首行推断 schema，列名默认是 `col_N`。Filter、Project、Join、Aggregate 再基于这些列名构造 `ValueRowContext`。

问题包括：

- planner 里的变量名、别名、tag/edge 字段名可能丢失。
- join 后右表列名临时生成 `right_N`，容易和表达式中的变量引用不一致。
- 类型由首行推断，遇到空 chunk、null 首行、混合类型时不稳定。
- 每行求值时复制 row 和 col_names，性能成本高。

成熟执行器通常会在 planning/validation 后生成稳定 slot layout：变量、列、表达式输出都映射到固定 slot id，执行期按 slot 访问，不依赖字符串查找。

### 5. 阻塞算子缺少内存控制和 spill 机制

Sort、Aggregate、GroupBy、WindowFunction、NestedLoopJoin、HashJoin、Intersect、Except 等都会全量收集输入或 build side。

当前缺少：

- per-query memory budget。
- operator 级内存估算和限制。
- 外部排序、hash aggregate spill、hash join spill。
- 大结果集的分批输出策略。

这会使查询在稍大数据集上出现内存峰值不可控。项目已有 cost/memory budget 相关 optimizer 模块，但 executor 尚未真正执行这些约束。

### 6. Join 实现语义和性能都较弱

当前 join 主要问题：

- `HashJoin` 使用整行 `format!("{:?}", row)` 作为 hash key，而不是 planner 提供的 join key。
- 条件求值依赖拼接后的列名，右侧列名临时生成 `right_N`。
- `NestedLoopJoin` 预先收集整个 right side，对非小表 join 风险很高。
- left/right/full/semi/cross join 虽有 variant，但语义完整性需要逐一验证。
- 没有 bloom filter、runtime filter、join reordering 执行配合。

建议优先实现基于 slot 的 equi-hash-join，并明确 build/probe side、null 语义、输出 schema 和内存上限。

### 7. 表达式执行成本高且错误处理不一致

Filter 和 Project 等算子逐行创建 `ValueRowContext`，并频繁 clone row / col_names。表达式求值失败时，不同算子分别返回 false、Null 或忽略错误。

这会导致：

- 语义错误被静默吞掉。
- 性能难以优化。
- 调试复杂查询时无法定位真实表达式失败原因。

建议把表达式编译为可执行计划，输入为 slot layout；同时统一表达式错误策略，区分类型转换失败、字段缺失、函数不存在、运行时 null 传播等情况。

### 8. executor enum 过大，扩展成本高

`StreamingExecutor` 是一个包含大量 variant 的大 enum，并通过巨大的 `match` 分发 `open/next/stop/close`。优点是静态分发、容易定位；缺点是：

- 每新增算子需要修改 enum、四个生命周期 match、builder 和 operator 模块。
- 编译期和代码审查成本上升。
- 很多 variant 只是部分实现或占位，真实能力容易被 API 表面掩盖。

短期可以接受这种方式，但需要建立“支持矩阵”和测试覆盖，明确哪些算子是 production ready，哪些只是 stub。

### 9. EXPLAIN / PROFILE 与实际执行统计连接不足

项目有 `ExecutionStatsContext`、`NodeExecutionStats`、`ExecutorStats` 等结构，但当前 streaming operator 没有稳定记录：

- 每个 plan node 的 input/output rows。
- 每个 operator 的 wall time / CPU time。
- peak memory。
- storage read rows / bytes。
- filtered rows。
- spill 次数。

成熟数据库的 PROFILE 能回答“哪个算子慢、估算和实际差多少、内存耗在哪里”。当前实现还没有把统计信息作为 executor 生命周期的一部分。

### 10. 事务、取消和资源治理没有深入 executor

`QueryExecutionManager` 有 killed 标志，executor 也有 `stop()`，但两者没有形成完整链路：

- 长时间 scan、sort、join、traversal 内部没有周期性检查取消。
- storage cursor 生命周期没有进入 executor close/drop 管理。
- DML operator 和事务提交/回滚语义没有和 transaction manager 深度绑定。
- backpressure 只是 chunk 计数器，且 add 后马上 remove，无法代表真实消费者压力。

这会影响长查询治理、客户端断开、超时、事务一致性和资源释放。

## 建议优先级

### P0: 收敛执行模型

先明确当前阶段的目标：

- 如果目标是快速保证正确性，建议保留单线程 pull executor，移除或弱化当前 task/worker 并行假象。
- 如果目标是并行执行，建议重构为 pipeline DAG + per-partition operator state，不再让多个 worker 共享同一个 mutable executor。

在模型明确前继续增加 operator，会放大后续重构成本。

### P1: 建立 slot-based row layout

从 planner 输出稳定 schema：

- 每个变量/表达式输出对应 slot id。
- `DataChunk` 保存 schema，而不是每次从首行推断。
- 表达式按 slot 访问，不使用字符串列名查找。
- join/project/aggregate 明确输出 schema。

这是修复表达式、join、project、profile、类型推断问题的基础。

### P1: scan 改为 cursor/chunk reader

把 `ScanVertices` / `ScanEdges` 从 buffer source 改为存储游标 source：

- `open()` 创建 cursor。
- `next()` 从存储层读取最多 N 行。
- limit/filter/index seek 能尽早减少扫描。
- partition id 真实映射到数据范围或 shard/range。

### P2: 给阻塞算子加内存预算

至少先做到：

- sort/aggregate/join 记录估算内存。
- 超过 query memory budget 时返回明确错误。
- 为 sort 预留外部排序接口。
- 为 hash join / aggregate 预留 spill 分区接口。

### P2: 完善 operator 支持矩阵和测试

建议新增文档或测试矩阵：

- 每个 `PlanNodeEnum` 是否可 builder 转换。
- 每个 `StreamingExecutor` variant 是否完整实现。
- 支持哪些 Cypher/nGQL 语义。
- 单元测试、集成测试、错误路径测试、空输入测试、大输入测试覆盖状态。

### P3: 执行统计和 PROFILE 闭环

每个 operator 统一记录：

- open/next/close 时间。
- input/output rows。
- 内存峰值。
- storage IO。
- 错误和取消状态。

然后让 EXPLAIN ANALYZE 展示估算值与实际值差异。

## 总体评价

当前 executor 的价值在于已经把执行器所需的主要概念摆出来了：operator、chunk、builder、scheduler、worker、expression、graph algorithm、profile 都有入口。但这些入口之间还没有形成成熟数据库引擎所需的强约束关系。

最主要的技术债不是“缺少某几个算子”，而是执行模型、数据模型和资源模型没有统一。建议下一阶段优先收敛到一个可验证的最小执行内核：slot-based schema、cursor scan、单线程 pull pipeline、明确阻塞算子边界和完整统计。等正确性和可观测性稳定后，再引入真正的 partition 并行和 spill。
