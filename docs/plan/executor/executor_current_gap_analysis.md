# GraphDB Executor 现状差距与架构评审

> 日期：2026-07-13
> 分析基线：当前 `graphdb-query` production facade、streaming executor、plan cache、EXPLAIN/PROFILE 和定向单元测试。
> 目标规范：`executor_ideal_architecture.md`。
> 后续任务：`executor_remaining_work.md`。

## 一、结论

当前实现是一个正在迁移的 streaming executor，而不是目标架构的完整实现。它已经验证了同步批量 pull、immutable operator spec 试验、slot layout、query runtime、显式 Exchange 和有界并行输出等方向，但生产系统仍由逻辑 `ExecutionPlan`、单树 `PhysicalNode`、分区 `PartitionedPhysicalPlan` 和带状态 `StreamingExecutor` 四层交叉驱动。

目标架构的核心判断不变：应统一为 immutable、可验证的 PhysicalPlan，并让每次执行创建独立 QueryExecutionInstance。经过本次评审，对原设计作五项修正：

1. 具体 ResultSink 是执行绑定，不进入可缓存计划；计划只保存 ResultBoundary/OutputContract；
2. 显式事务跨查询存在，session controller 持有 transaction handle，查询实例只持有语句 scope；
3. worker pool 是引擎级共享服务，查询只拥有可取消、可等待的 task group；
4. Global/Local state registry 使用封闭 enum 或 typed arena，避免 `dyn Any` 主导的弱类型注册表；
5. statistics version 通常影响计划质量而非正确性，应触发 freshness/replan 策略，不应和 schema/layout 一样无条件使计划失效。

## 二、当前生产调用链

### 2.1 非分区路径

```text
ExecutionPlan.root: PlanNodeEnum
  → StreamingExecutorBuilder::from_plan_node
  → operator_plan_builder::build_plan_node
  → PhysicalNode::materialize
  → StreamingExecutor tree
  → StreamingExecutionEngine
  → ResultStream / DataSet
```

这条路径确实经过 `PhysicalNode`，但 `PhysicalNode` 只是 executor 模块内的临时树：没有顶层 compatibility、output contract、fragment graph、input requirements 和 validator，也没有作为 cache/EXPLAIN 的事实来源。

此外，该路径存在双 runtime 接线错误：builder 创建并注入一个 runtime，factory 又创建另一个 runtime，但没有把后者设置给 engine。engine 的 `into_stream` 要求自身持有 runtime，因此普通执行入口可能在开始拉取前失败。

### 2.2 分区路径

```text
ExecutionPlan.root + PartitionSpec
  → PartitionedPhysicalPlan::from_logical
  → physical_builder::build_partitioned_physical_node
  → directly construct partial/final/Gather/HashShuffleJoin executors
  → StreamingExecutionEngine
```

`PartitionedPhysicalPlan` 名称虽然是 physical plan，节点仍保存完整 `PlanNodeEnum`。builder 会直接创建 runtime operator、插入占位 Start、替换输入并分配私有 synthetic ID。因此它是执行树构造脚本，不是可验证、可缓存、可解释的 immutable PhysicalPlan。

### 2.3 缓存与解释路径

plan cache 保存 `ExecutionPlan`，其根仍是 `PlanNodeEnum`。EXPLAIN/PROFILE 通过 `DescribeVisitor` 遍历逻辑根节点，只额外显示 partition spec，无法展示实际 PhysicalNode、partial/final operator、完整 Exchange、slot layout 和 properties。

这意味着“被缓存的计划”“被解释的计划”和“真正执行的计划”不是同一个对象，发生偏差时没有单一事实来源。

## 三、逐项差距

| 目标区域 | 当前状态 | 判断 | 主要差距 |
|---|---|---|---|
| 同步批量 pull | `open → advance → close` 和 DataChunk 已工作 | 基本符合 | lifecycle 仍分散在 operator tree，缺少 instance 统一状态 |
| Logical/Physical 分离 | `PlanNodeEnum` 同时含 HashJoin、IndexScan 等算法节点 | 不符合 | optimizer、builder、executor 边界混杂 |
| Immutable PhysicalPlan | 有可 clone/materialize 的 `PhysicalNode` | 部分符合 | 无顶层计划、fragment、output、compatibility、validator |
| 唯一构建路径 | 单树和分区分别构建 | 不符合 | 分区路径直接创建 runtime operator |
| 强类型 spec | 大多数 operator 有 enum spec | 部分符合 | SinkSpec 持有 storage；Migrate 仍是字符串/空值；spec 仍含执行绑定 |
| 独立执行实例 | 有 `ExecutionRuntime` | 部分符合 | 无 QueryExecutionInstance/QueryBindings；当前甚至可能创建两个 runtime |
| Global/Local state | 有 `operator_state.rs` | 部分符合 | state 仍直接嵌在 runtime operator enum，无 typed arena 和 ID 寻址 |
| Slot ABI | DataChunk 始终带 SlotLayout | 部分符合 | plan node 无输入/输出 layout；表达式仍按名称在执行期解析 slot |
| Physical properties | 已定义 distribution/order/pipeline/memory 字段 | 仅骨架 | 大量固定 `single_streaming/single_blocking`，无推导、输入要求和 validator |
| ID | 大多复用 logical ID | 不符合 | synthetic Start 为 0，Gather 使用私有负数区间，partial/final 可重复 ID |
| 并行调度 | query-level worker pool、有界 channel | 部分符合 | task 单位仍是完整 partition tree；每查询线程池；出现 self-join panic |
| Exchange | 有 Concatenate 和 MergeSort | 部分符合 | RepartitionHash/Broadcast/Barrier 不统一；HashShuffleJoin 是特例路径 |
| 内存 | MemoryBudget、MemoryTracker、queue reservation | 部分符合 | 无分层 pool；DataChunk clone 不重新 reservation；workspace 计账不完整 |
| Spill | 有 Spillable 接口 | 不符合 | 固定返回未实现 |
| Result delivery | materialized 和 stream 共用 pull engine | 部分符合 | 无 OutputContract/ResultBoundary；空结果 schema 依赖 facade fallback |
| Transaction | command 已一次性输出 | 不符合 | 没有真实 TransactionScope/session controller，只返回文本 |
| DML/DDL | 进入 streaming operator tree | 部分符合 | DML spec 直接持有 storage，事务和 cache invalidation 边界缺失 |
| 图递归 | 有 BFS/paths operator | 部分符合 | 无 RecursiveFragmentSpec；MultiShortestPath 仍丢参数 |
| Plan cache | 有线程安全 cache 和 partition fingerprint | 部分符合 | 缓存逻辑 ExecutionPlan，compatibility 不完整，参数/权限边界未统一 |
| EXPLAIN/PROFILE | 有逻辑 plan 展示和 node profile | 部分符合 | 不展示实际 physical/fragment；profile key 缺 fragment/task identity |
| 错误与取消 | runtime cancel、deadline、首错清理已有基础 | 部分符合 | builder 多用通用 execution error；worker panic 可被测试隐藏 |

## 四、已经完成且应保留的基础

### 4.1 构建错误传播

`operator_plan_builder::build_plan_node` 已使用顶层穷尽 match，领域 builder 的真实错误不会再被轮询式路由吞掉。这个结构可作为未来 LogicalPlan → PhysicalPlanBuilder 的过渡入口，但领域函数最终应接收具体 logical node 类型，避免内部 `_` 路由错误分支长期存在。

### 4.2 immutable spec 与 fresh state

`PhysicalNode::materialize` 每次创建新的 runtime operator/state，证明同一 spec 可以生成互不共享 cursor/hash/buffer 的执行树。未来应保留“spec 只读、state 每次新建”的性质，但把 materialize 迁移到 QueryExecutionInstance，并删除 spec 中的 storage 和本次执行数据。

### 4.3 DataChunk 和 slot layout

DataChunk 始终带 layout，ValueRowContext 通过 layout 把名称解析为 slot，已消除每行临时构建 name-index map 的主要问题。下一步不是立刻列式化，而是让 PhysicalPlan 在编译期固定每个 operator 的 input/output layout，并把 expression 直接绑定为 slot expression。

### 4.4 生命周期和首错保留

OperatorLifecycle 已覆盖 New、Opened、Exhausted、Stopped、Failed、Closed。ResultStream 在执行错误后执行 teardown，并避免 cleanup error 覆盖原始错误。这些语义应上移到 QueryExecutionInstance，operator 只实现本地 open/next/close，不各自决定全查询终态。

### 4.5 有界并行输出

并行 channel 有固定 chunk 容量，同时对排队 bytes 建立 reservation，并记录 queue peak。这一机制可以迁移为 Exchange queue 基础设施。需要替换的是 pool 所有权和 partition-tree task 粒度，而不是丢弃有界队列设计。

## 五、正确性与可靠性风险

### 5.1 双 runtime 是当前最高优先级问题

非分区 factory 没有 `engine.set_runtime`，而 engine `into_stream` 明确拒绝无 runtime。EXPLAIN ANALYZE、PROFILE、物化和 stream 都可能触发这条路径。修复时不能只补一行 setter：还必须删除 builder 创建的另一个 runtime，确保 operator、engine、result handle、cancel 和 profile 引用同一对象。

### 5.2 worker self-join 说明所有权错误

定向 streaming 测试共 130 项报告通过，但其中一个并行 Gather 测试的 worker 子线程出现 `failed to join thread: Resource deadlock avoided`。pool 在 Drop 中 join workers；若最后一个 pool/runtime 引用在 worker 内释放，就可能由 worker 销毁并 join 自己。测试框架没有把该子线程 panic 计为用例失败，因此既是 teardown bug，也是测试可观测性缺口。

### 5.3 properties 当前可能产生错误信心

字段存在但未被系统推导和验证，比完全没有字段更容易让后续代码误以为契约成立。例如 local partition 节点仍常用 `Distribution::Single`，blocking operator 的 memory policy 默认没有语义。validator 完成前，这些字段只能视为草案，不应作为 scheduler 或 cache correctness 的依据。

### 5.4 空 schema 和 DataChunk clone 破坏资源契约

空结果 schema 目前由 facade 从逻辑 root 手工回填；直接 `ResultStream::collect` 则可能得到空列名。DataChunk clone 深拷贝 rows，却把 memory reservation 设为 None。两者都说明 output/memory contract 尚未从计划贯穿到执行结果。

### 5.5 command 和图计划仍有 silent fallback

Migrate 的空 action/space 和 MultiShortestPath 的空 target/edge type 会把“未实现”伪装成可执行计划。目标架构明确禁止这种 fallback；短期正确方案是提前返回结构化 unsupported/invalid plan，而不是尝试从 runtime 猜测。

## 六、目标架构合理性评审

### 6.1 保留：同步、批量、pull-first

对轻量单机图库，pull-first 能保持 operator API 简单，并天然支持嵌入式接口的背压。并行 fragment 可以在内部通过有界 Exchange 转换为 task push，不需要把全部 operator 改为 async。这个选择合理。

### 6.2 调整：ResultSink 不应固化在缓存计划

原设计一方面要求 PhysicalPlan 是唯一缓存对象，另一方面把 DataSet/stream 等 ResultSink 作为物理终端。如果具体 sink 在计划内，相同查询从 embedded、HTTP 和 gRPC 执行时需要不同缓存计划，违背“结果格式不改变算子树”。

调整后，PhysicalPlan 只包含 `ResultBoundarySpec` 和 `OutputContract`；QueryExecutionInstance 绑定 DataSetSink、ChunkStreamSink、DiscardSink 或 PullHandle。结果交付不同，计划和 operator 不变。

### 6.3 调整：显式事务必须分成 session 与 statement 两层

显式事务从 Begin 延续到后续多个查询，生命周期长于单个 QueryExecutionInstance。若实例“拥有 transaction”，查询结束时无法判断是否应提交、回滚或继续保留。

调整后，session controller 持有显式 transaction handle；每个查询的 TransactionScope 借用它并实现 statement 级错误策略。自动提交事务仍由 QueryExecutionInstance 创建并拥有。

### 6.4 调整：worker pool 应全局共享

每查询创建固定线程池会使并发查询时线程数相乘，也使 Drop/join 和 worker 持有 runtime 的所有权非常复杂。目标中的“query-aware worker pool”应解释为引擎级共享 pool 感知 query quota/fairness，而不是每查询一个 pool。

查询实例拥有 task group，能够 cancel、wait 和计量自己的 task；数据库关闭时才关闭共享 pool。

### 6.5 调整：state registry 必须类型安全

以 `(operator, fragment, task)` 寻址是合理的，但通用 registry 容易演变为 `HashMap<Id, Box<dyn Any>>`。这会把 spec/state 对应关系推迟到 runtime downcast，并与项目最小化动态分派的约束冲突。

建议使用封闭 `GlobalState`/`LocalState` enum，或按 operator category 使用 typed arena。ID 负责定位，enum variant 负责类型验证，错误 plan 在 validator/instantiate 阶段失败。

### 6.6 调整：statistics freshness 与 correctness compatibility 分开

schema、storage layout 和 capability 变化可能让旧计划无法正确执行，必须强制 miss。statistics 变化一般只让旧计划变慢，不改变结果；每次 analyze 都强制淘汰会降低 cache 命中率。

建议 cache entry 同时保存 correctness compatibility 和 cost freshness。统计偏差或版本跨度超过阈值时触发 replan；在重规划完成前，兼容旧计划仍可执行。

### 6.7 澄清：常量与参数值不是同一类数据

查询文本中的 literal 是语义的一部分，可以安全进入 immutable spec。prepared parameter 的实际值属于执行 binding，不能固化在缓存计划。原设计“本次查询参数值不能进入计划”应按这一边界理解，否则常量折叠和索引范围规划会变得含糊。

### 6.8 保留：fragment graph 同时服务串行和并行

统一 fragment graph 是消除两套构建路径的关键。串行 driver 可以用一个 task 执行同一 DAG；并行 scheduler 只增加 morsel/task 数和 Exchange 并发度。这个复杂度只有在统一 PhysicalPlan 和 validator 之后才值得引入。

## 七、建议迁移策略

采用 strangler migration，但不保留运行时 silent fallback：

1. 先修复 runtime 和 worker teardown，使现有路径可靠；
2. 在 planning 下建立正式 PhysicalPlan 类型、ID 和 validator，不移动 runtime operator；
3. 让当前单树 builder 输出正式 PhysicalPlan，再通过新 factory 实例化现有 operator；
4. 把 partition split、partial/final 和 Exchange 迁入同一个 builder；
5. 切换 cache/EXPLAIN/PROFILE 到 PhysicalPlan；
6. production facade 全部切换后删除 PartitionedPhysicalPlan 和直接 executor builder；
7. 再引入 QueryExecutionInstance、typed state 和 ResultBoundary；
8. 最后建立 fragment DAG、共享 scheduler、morsel 和 spill。

每一阶段允许旧代码仍存在，但一个 production request 只能选择一条明确路径。新路径失败必须返回错误，不能自动回退旧路径，否则语义和 profile 无法验证。

## 八、验证记录

本次分析运行：

```shell
cargo test -p graphdb-query --lib query::executor::streaming -- --nocapture
```

结果为 130 个测试通过、0 个测试失败；同时观察到一个未传播到测试结果的 worker 子线程 self-join panic。该结果证明现有单元覆盖的算子语义多数可工作，但不能证明 production facade、真实 storage、事务、空 schema 和并发 teardown 已满足目标架构。
