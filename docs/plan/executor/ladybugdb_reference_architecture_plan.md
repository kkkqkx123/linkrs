# 参考 LadybugDB 的 Executor 架构调整建议

> 调研日期：2026-07-13  
> 参考：用户提供的 LadybugDB Query Executor 设计说明，以及当前 `graphdb-query` 的 streaming executor 实现。  
> 前置文档：`executor_functional_completion_plan.md` 处理现有语义缺口；本文处理其完成后仍需要的执行架构演进。

## 一、结论

LadybugDB 的核心价值不在于“拥有 60 多种 C++ 物理操作符”，而在于它把以下边界明确成独立契约：

```text
逻辑计划
  -> 不可变物理计划（operator + layout + physical properties）
  -> 每查询执行实例（global state + local state + transaction/runtime）
  -> pipeline fragments + morsel tasks
  -> result sink（物化、网络流或 Arrow）
```

当前项目已经具备 batch pull 内核、`DataChunk`、`SlotLayout`、`ExecutionRuntime`、受限分区 Gather，以及正在引入的 `OperatorSpec` / `OperatorState` / `PhysicalNode`。方向正确，但物理计划、执行状态、分区并行、结果输出仍分散在两套构建路径和多个特例中。

因此应吸收 LadybugDB 的**边界设计**，但不应照搬其 C++ 类数量、通用 `dyn PhysicalOperator` 或完整 Factorized Table。项目当前更合适的目标是：**单机 Rust、封闭 enum、批量 pull、显式 physical properties、全局线程池和 morsel 调度**。

## 二、当前实现与目标模型的差距

| 主题 | 当前状态 | LadybugDB 参照 | 需要的调整 |
|---|---|---|---|
| 物理计划 | `PhysicalNode` 仅覆盖 Source / Unary / Blocking / Join 试点；其余节点走 legacy builder | PlanMapper 对全部逻辑节点生成物理计划 | 统一为一次 lowering，禁止同一查询在两条构建路径间隐式切换 |
| 不可变性 | `OperatorSpec` 已出现，但 Source spec 仍携带 storage handle；state 中仍有部分不应重复的 schema 数据 | Physical operator 配置与 runtime state 严格分离 | 划分 plan constants、query bindings、global state、local state 四层 |
| 执行身份 | `PhysicalNode::materialize()` 目前以 node id `0` 构造算子 | 每个物理 operator 有稳定身份和 profile 指标 | 将逻辑 node id、physical operator id、pipeline id、partition/task id 显式保留 |
| 并行 | Gather 下方为静态分组 `thread::spawn`；白名单决定是否并行 | TaskScheduler 运行 pipeline/morsel task | 以全局 worker pool 和 morsel 队列替代每个 Gather 私有线程 |
| physical properties | `PartitionedPhysicalPlan` 能表达部分 Local/Global 和 Gather，但顺序、分布、阻塞性没有统一属性 | operator/collector 以 order、pipeline、parallelism 驱动实现 | 为每个物理节点声明 distribution、ordering、pipeline capability、memory/spill policy |
| 数据布局 | `DataChunk` 已总是携带 SlotLayout，但主体仍是 `Vec<Vec<Value>>` | 向量化数据块、factorized result | 先稳定 slot ABI 和所有权，再渐进列式化；不直接重写为 Factorized Table |
| 结果输出 | `ResultStream` 直接拉根节点；物化路径收集为 `DataSet` | ResultCollector / ArrowResultCollector 是显式 sink | 引入统一 ResultSink，并把输出顺序和背压作为 sink 契约 |
| 事务 | `ExecutionRuntime` 有取消、预算、profile、cleanup；事务算子仍未承载真实事务状态 | 所有 query 运行于 transaction context | 建立 QueryExecutionInstance 持有 transaction scope，DML/DDL 不再是普通文本结果算子 |

## 三、保留与不照搬的设计选择

### 3.1 应保留：封闭 enum 和 pull 接口

LadybugDB 的多态基类适合其 C++ 生态；当前项目使用 `StreamingExecutor` + 领域 operator enum，可以获得静态分发、穷尽匹配和清晰的跨 crate 依赖。短期不应为模仿 `PhysicalOperator` 而改为 trait object 树。

也不需要抛弃 `open -> advance -> stop -> close`。它与 LadybugDB 的 `initGlobalState -> initLocalState -> getNextTuples` 在职责上相容；需要调整的是状态归属和调度层，而非把同步 pull 改为 async 流处理。

### 3.2 不应直接引入 Factorized Table

图查询的确可能从 factorization、late materialization 和 ID/属性分离中获益，但当前热点首先是：行存 `Value` clone、字符串列名查找、可选布局语义、内存 reservation 不完整。应先让 slot 在生产路径成为唯一 ABI，再将高频标量列演进为列向量。复杂的 Vertex/Edge/Path 仍可暂存为 `Value` 或引用句柄。

### 3.3 不应立即构建通用 pipeline DAG

当前 Gather 并行仅覆盖 formal partition boundary，直接增加一个没有 worker/morsel/exchange 的 DAG 只会复制现有树。应先定义 physical properties 和 fragment 边界，随后让 scheduler 消费 fragment；此时 DAG 才是调度图而不是第二套计划表示。

## 四、目标执行边界

### 4.1 不可变 PhysicalPlan

将目前的 `PhysicalNode` 扩展为完整物理计划节点。每个节点必须包含：

- 稳定 `PhysicalOperatorId`，并保留来源 logical node id；
- 输入/输出 `SlotLayout`，以及已绑定的表达式 slot；
- 不可变 operator spec；
- 输出属性：`Distribution`、`Ordering`、`Boundedness`、预估 cardinality；
- 执行能力：`Pipeline`、`Blocking`、`Source`、`Sink`、`Exchange`、`Spillable`、`ParallelLocal`；
- 内存和 spill 策略，而不是仅在运行时临时创建 tracker。

建议定义的最小属性如下：

| 属性 | 推荐取值 | 用途 |
|---|---|---|
| `Distribution` | `Single` / `Partitioned(key)` / `Hash(keys, buckets)` / `Broadcast` | 验证 Join、Aggregate、Gather、Exchange 的输入是否正确 |
| `Ordering` | `Unordered` / `By(keys, directions)` | 决定 LIMIT、MergeSort、网络输出能否保序 |
| `PipelineKind` | `Source` / `Streaming` / `Blocking` / `Sink` / `Exchange` | 划分 fragment 和 first-row latency 语义 |
| `Parallelism` | `Single` / `Morsel` / `PartitionLocal` | 取代手写 `is_parallel_safe()` 白名单 |
| `MemoryPolicy` | `Bounded` / `Spillable` / `RequiresBudget` | 使 planner 在构建阶段拒绝不安全组合 |

`PhysicalPlan` 只能保存 plan constants 和稳定的对象标识。storage client、session variables、参数值、transaction handle、cursor、runtime、memory reservation 都不应成为可缓存 plan 的可变部分。需要运行时解析的内容应表示为 data source id、parameter slot 或 query binding。

### 4.2 QueryExecutionInstance

每次执行一个 `PhysicalPlan` 时创建 `QueryExecutionInstance`，统一取代“PhysicalNode materialize 时直接拼 StreamingExecutor”的临时边界：

```text
QueryExecutionInstance
  ├── ExecutionRuntime：取消、deadline、profile、资源清理、根内存池
  ├── TransactionScope：自动提交/回滚、读写模式、snapshot
  ├── QueryBindings：参数、当前 space、数据源解析结果
  ├── GlobalOperatorState：hash table、aggregate table、result sink、exchange
  ├── LocalOperatorState：每 task 的 cursor、morsel、表达式临时区、局部累加器
  └── SchedulerHandle：任务取消、错误汇聚、完成等待
```

具体调整：

- `OperatorSpec` 不持有 mutable state；`SourceState` 不应重复保存 `col_names` 等 spec 数据；
- Hash join build、全局 aggregate、全局 sort、result collector 应有明确的 GlobalState；
- scan cursor、chunk buffer、probe cursor、局部 aggregate 应属于 LocalState，每个 task 独立创建；
- `MemoryTracker` 应由 instance 的分层 memory pool 创建，不能只散落在各算子 enum 内；
- `PhysicalNode::materialize()` 必须使用真实 physical node id、slot layout、partition/task identity，以便 profile 可追踪。

### 4.3 单一 lowering 入口

当前 `lowering/` 对少数节点生成 `PhysicalNode`，失败后回退到 `StreamingExecutorBuilder::from_plan_node()`。这个试点机制可暂时保留，但不能成为长期状态，因为它会导致：缓存、EXPLAIN、profile、partition 和 state 生命周期在不同节点上有不同语义。

目标入口应为：

```text
PlanNodeEnum
  -> PhysicalPlanLowerer（按 scan / relational / graph / write / control 分模块）
  -> PhysicalPlanValidator（schema、slot、properties、transaction、memory）
  -> QueryExecutionInstance::instantiate()
  -> FragmentScheduler 或 serial pull driver
```

lowerer 必须是无损转换：任何未处理节点、未传递输入、未传递 action 参数都在 lowering 阶段返回结构化错误。不得以默认值、空列表或字符串占位来“成功构建”。

## 五、从 pull tree 到 pipeline/morsel 调度

### 5.1 Fragment 划分规则

LadybugDB 把 source 和 sink 视为任务边界。当前项目可采用更明确的规则：

- Source（storage scan、index scan、图扩展的可切分访问）产生 morsel；
- Streaming unary chain（Filter、Project、Limit、轻量 Expand）融合在同一 fragment；
- Blocking operator（Sort、Distinct、HashJoin build、global Aggregate、Window、Materialize）形成阶段屏障；
- Exchange（Gather、Repartition、Broadcast、Merge）形成跨 fragment 边界；
- ResultSink、DML sink、事务控制是最后阶段的单独 sink fragment。

初版依然可以在一个 worker 内用 `advance()` 驱动一个 fragment；这保留当前成熟的 pull 语义。关键是 fragment 具备显式输入、输出、状态范围和任务数量。

### 5.2 Morsel 与任务池

将“复制整棵分区子树并静态分配给 `thread::spawn`”替换为：

1. scan 根据 partition/range/cursor 创建许多小 morsel，而非一个 partition 对应一个 worker；
2. 固定大小的 query-aware worker pool 从并发队列领取 morsel；
3. 每项任务以 LocalState 执行一个 fragment，输出写入 bounded exchange queue；
4. GlobalState 只由受控同步原语访问；
5. query cancel、首个错误、内存超限必须关闭所有 queue，并等待本 query 的任务退出。

这样能处理数据倾斜，也避免每个 Gather 启动线程。现有 `ParallelPartitionCoordinator` 中的有界 channel、取消传播、队列内存计账和 profile 指标可保留为 Exchange 的基础实现，但静态分组和按 partition 顺序阻塞应逐步移除。

### 5.3 Exchange 与正确性

需要显式 operator/spec，而不是仅有 Gather 的内部特例：

- `Gather(Concatenate)`：只在输入顺序不重要时使用；
- `Gather(Merge)`：要求输入均满足同一 ordering；
- `Repartition(Hash(keys))`：为 hash join 和分组聚合分发；
- `Broadcast`：为小 build side 或常量表复制；
- `Barrier`：为全局 blocking state 的完成信号。

physical plan validator 应检查这些前置条件。例如全局 `LIMIT` 不能位于无顺序的并行 concatenate 后；HashJoin 两侧必须使用一致 hash 函数和 bucket 数；最终 Aggregate 必须消费 partial state 而不是原始行。

## 六、结果、顺序和事务

### 6.1 ResultSink

将 `ResultStream::collect()`、HTTP SSE、gRPC 输出与结果 schema 协调为统一 ResultSink 协议。建议 sink 在执行开始前公开 schema/layout，在执行中接收 `DataChunk`，在结束时公开正常完成或错误。

初始实现只需三种 sink：

- `DataSetSink`：嵌入式/当前物化接口；
- `ChunkStreamSink`：SSE、gRPC 和 C API 的逐 chunk 输出；
- `DiscardSink`：EXPLAIN ANALYZE、DML 或 benchmark 中仅消费结果。

Arrow sink 可作为后续格式适配层，而不是要求所有算子立即依赖 Arrow。所有 sink 都必须遵守 `Ordering` 属性：无序结果可并行写入；有 `ORDER BY` 或客户端顺序契约时使用单一有序汇合或 deterministic merge。

### 6.2 事务 scope

把事务从当前 `TxnOperator` 的输出文本升级为 `QueryExecutionInstance` 的执行范围：

- 自动事务由 instance 在执行开始、成功、失败和取消时统一处理；
- 显式 Begin/Commit/Rollback 仅改变 session transaction context，且是一次性 command sink；
- DML 和 DDL 的所有 local task 在同一事务/写锁策略下执行；
- 一旦任务失败，scheduler 停止接收新的 morsel，再统一等待、回滚和释放资源；
- query result 在事务提交前的可见性要有明确契约。

## 七、数据布局演进

### 7.1 近期：把 SlotLayout 变成执行 ABI

`DataChunk` 已强制带 layout，这是正确起点。下一步应使 physical lowering 绑定表达式为 `SlotId`，执行期禁止按字符串列名查找。schema 和 layout 由物理节点产生和传播；空结果也必须携带正确 schema。

### 7.2 中期：选择性列式化与 late materialization

无需一次性替换 `Vec<Vec<Value>>`。先为扫描、Filter、Project、Aggregate key、Join key 等标量路径引入列式 vector/validity；Vertex/Edge/Path 可先保存 id 或轻量引用，到最终需要属性时再 materialize。这样可获得 LadybugDB 向量化与图对象减少复制的主要收益，并保持现有复杂 `Value` 兼容。

### 7.3 长期：评估 factorization

只有在 profiling 显示多跳扩展的重复前缀占主要内存后，才评估 factorized intermediate table。此时应先将其限制在 graph expand / path result 的内部表示，并经由普通 `DataChunk` 适配器进入通用关系算子；不要让 factorization 改变所有算子的公开 ABI。

## 八、分阶段实施计划

### Phase A：恢复语义与完成实例化试点

- 先完成 `executor_functional_completion_plan.md` 的 P0：UNION ALL、Apply 家族、事务终止；
- 让 `PhysicalNode` 试点真正接入 factory，而不是仅作为可选 fallback；
- 补 `PhysicalOperatorId`、input/output layout、node profile identity；
- 校验同一 immutable plan 的两次实例化不会共享 cursor、hash table、lifecycle 或 reservation。

验收：同一 cached physical plan 能并发实例化；serial 结果与 legacy builder 一致；EXPLAIN/PROFILE 不再出现全部 node id 为零。

### Phase B：统一 PhysicalPlan 与 properties

- 为 Set、Apply、Graph、Sink、DDL/Txn、Fulltext/Vector 补充完整 spec；
- 以 domain lowering 模块取代巨型 builder 的业务匹配；
- 加入 distribution、ordering、pipeline kind、parallelism、memory policy validator；
- 删除正常查询的 legacy builder fallback，仅在受控迁移 feature 下保留对照实现。

验收：每个可由 planner 生成的节点都能生成物理 spec；任何属性不满足或参数丢失在 lowering/validation 失败，而不是执行中静默退化。

### Phase C：执行实例、sink 与事务边界

- 引入 `QueryExecutionInstance`、GlobalState/LocalState 和分层 memory pool；
- 实现 ResultSink，统一物化与流式 API；
- 将事务、错误、取消、cleanup 放到 instance/scheduler 层；
- 将 profile 按 physical operator、pipeline、task、exchange 记录。

验收：DML 失败能回滚；客户端断连停止任务并释放全部资源；所有 sink 在空结果和错误时返回稳定 schema/终态。

### Phase D：morsel scheduler 与 Exchange

- 先为 vertex/edge/index scan 提供可切分 morsel；
- 建立固定线程池、bounded exchange queue、动态任务领取；
- 将现有 Gather P8 实现收敛为 `Gather` exchange；
- 依次启用 local/global aggregate、TopN merge、hash repartition join。

验收：串行、morsel 并行和分区并行在值、NULL、重复和顺序契约上等价；倾斜数据不会因静态分配而严重退化；线程数受全局查询资源治理。

### Phase E：布局与图优化

- 绑定 slot 并消除名称查找；
- 对热点标量列实现向量化；
- 为路径扩展增加基于 bitmap/visited set 的选择性过滤，评估类似 semi-mask 的收益；
- 仅在 benchmark 证明收益后引入图结果 factorization。

验收：报告 scan/filter/project、aggregate/join、图扩展三类基准的 rows/s、首行延迟、分配次数、peak memory、worker utilization 和 spill bytes；所有优化保持语义差分测试通过。

## 九、完成标准

架构调整完成的标志不是“有 pipeline 目录”或“算子数量与 LadybugDB 相同”，而是：

- 一个不可变、可验证、可缓存的 PhysicalPlan 覆盖所有生产节点；
- 每次查询创建独立执行实例，全局与局部状态边界清晰；
- planner 能基于显式 physical properties 决定顺序、分布、阻塞和并行；
- scheduler 使用固定 worker pool 和 morsel/exchange，而非每个 Gather 创建线程；
- 物化、HTTP SSE、gRPC 和未来 Arrow 只是在 ResultSink 层的不同输出；
- 事务、取消、内存、profile 与错误能跨所有 task 和 sink 一致传播；
- 图查询优化建立在已稳定的 slot/chunk/physical properties 之上，而不是绕过通用执行模型。
