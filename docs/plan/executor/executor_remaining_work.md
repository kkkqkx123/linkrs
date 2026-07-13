# GraphDB Executor 剩余任务

> 日期：2026-07-13  
> 状态：根据当前代码重新核对，删除已经完成的旧任务。
> 目标架构：见 `executor_ideal_architecture.md`。
> 差距依据：见 `executor_current_gap_analysis.md`。

## 一、当前基线

当前 streaming executor 已经具备批量 pull、`DataChunk`、slot layout、spec/state 初步分离、operator lifecycle、query runtime、显式 Exchange、并行 worker 和有界输出队列等基础设施。以下旧问题已经完成，不再列为剩余任务：

- 顶层 operator plan builder 已对 `PlanNodeEnum` 穷尽分派，不再把领域构建错误吞成 unsupported；
- `UNION ALL` 已根据 distinct 标志选择独立 spec；
- Apply family 已使用强类型 kind，并消费右输入；
- Begin、Commit、Rollback 已具备一次性输出状态；
- BFSShortest、ShortestPath 和 AllPaths 已开始保留双输入和主要配置。

这些局部完成项不代表目标架构已经形成。当前主要问题是非分区执行 runtime 接线错误、两套物理构建路径、缓存对象仍为逻辑 `ExecutionPlan`、计划持有执行资源、properties 不可验证，以及缺少 execution instance、transaction scope、fragment DAG 和统一结果边界。

执行顺序遵循四条原则：

1. 先修复生产路径会直接失败、泄漏或返回错误结果的问题；
2. 在引入 scheduler、spill 等复杂机制前统一并验证物理计划；
3. 先建立资源所有权，再扩大并行度；
4. 不使用空列表、默认方向、空字符串 action 或 silent fallback 伪装未实现语义。

## 二、P0：恢复可靠的生产执行路径

### 2.1 修复非分区执行的双 runtime

当前 `StreamingExecutorBuilder::from_plan_node` 创建 runtime A 并注入 operator tree，`StreamingQueryExecutor::from_plan_node` 随后创建 runtime B，却没有为 engine 调用 `set_runtime`。`execute()` 和 `into_stream()` 最终要求 engine 持有 runtime，因此普通无分区计划可能直接返回 `No ExecutionRuntime attached`；即使绕过该错误，取消和 profile 也可能操作错误实例。

处理要求：

- runtime 只能由 execution instance/factory 创建一次；
- engine、operator、result handle 和公开 cancel/profile handle 必须引用同一个 `Arc<ExecutionRuntime>`；
- 删除 builder 内部创建 runtime 的行为，builder 只构建 immutable plan 或从显式 bindings 实例化；
- 分区与非分区入口使用同一 runtime 装配顺序；
- 执行失败不得用新建的 execution error 覆盖原始错误 kind。

验收：

- 无 partition spec 的 scan/filter/empty result 可通过物化和 stream 两种生产入口执行；
- 从 result handle 发出的 cancel 能停止 operator tree；
- profile、resource owner 和 memory budget 的对象身份在整条调用链一致；
- EXPLAIN ANALYZE 和 PROFILE 不再走缺失 runtime 的单独路径。

### 2.2 修复 worker teardown 和隐藏线程 panic

定向测试 `p8_parallel_gather_preserves_partition_order_and_bounds_buffers` 中曾出现 worker 自己 join 自己导致的 `Resource deadlock avoided`，但测试仍报告通过。当前 query runtime 持有 pool，pool 又可能在 worker 所持最后一个 runtime 引用释放时 Drop，所有权形成环状 teardown 风险。

处理要求：

- 在迁移到共享 scheduler 前，禁止 worker 线程执行 pool 的最终 Drop/join；
- query handle 只等待本查询 task group，不 join 当前线程；
- 捕获 worker panic 并转为查询首个错误，测试不得静默通过；
- 关闭 queue 后等待本查询所有 task 退出，清理错误只记录日志，不覆盖原始执行错误；
- 增加取消、消费者提前断开、worker error、open failure 和 Drop-only 五类并发测试。

验收：相同测试循环运行至少 100 次，无子线程 panic、死锁、遗留 task 或未释放 reservation。

### 2.3 清除仍会制造错误语义的占位值

当前至少仍有以下已知占位：

- `MultiShortestPath` 生成空 target、空 edge types 和默认 Both 方向；
- DDL Migrate 运行时使用空 space、空 action 和空 migration data；
- 合成 Start/DML source 仍使用硬编码 physical ID `0`；
- 部分 command 和 scan 在缺少 space 时使用空字符串。

处理要求：无法无损构建的节点必须在 planner/physical builder 阶段返回结构化错误；只有语义完整后才能进入 executor。

## 三、P1：建立唯一、可验证的 PhysicalPlan

### 3.1 定义顶层 PhysicalPlan 和稳定 ID

在现有 `PhysicalNode` 试验结构上建立正式计划对象：

```text
PhysicalPlan
  operators: arena<PhysicalOperatorSpec>
  fragments: fragment graph
  root_fragment: FragmentId
  output: OutputContract
  compatibility: PlanCompatibility
  required_capabilities: CapabilitySet
```

每个 operator 必须包含独立 `PhysicalOperatorId`、可选 `LogicalNodeId`、输入/输出 layout、输入要求、输出 properties、cardinality、memory policy 和 explain name。

处理要求：

- 使用统一 allocator 分配所有物理 ID，包括 Start、Gather、partial/final 和其他 synthetic node；
- logical node 一对多拆分时保留来源 logical ID，但不复用 physical ID；
- 移除所有硬编码 `0`、`i64::MIN + n` 和领域私有 synthetic ID 空间；
- `PhysicalPlan` 构建完成后只读共享，不能通过替换 child 修改。

### 3.2 分离 LogicalPlan 与 PhysicalPlan

当前 `PlanNodeEnum` 同时包含 InnerJoin/HashInnerJoin、Scan/IndexScan 等语义层和算法层节点。

处理要求：

- logical join 只保留 join kind、condition 和输入；
- logical scan 只描述访问需求；
- HashJoin、IndexScan、partial/final Aggregate、TopN、Distinct 和 Exchange 只存在于 physical spec；
- planner/optimizer 不生成 executor 专用 variant；
- executor 不再根据 logical variant 选择算法。

### 3.3 合并单树和分区构建路径

删除当前两条生产路线：

- `PlanNodeEnum → PhysicalNode → StreamingExecutor`；
- `PlanNodeEnum → PartitionedPhysicalPlan → physical_builder → StreamingExecutor`。

统一为：

```text
LogicalPlan → PhysicalPlanBuilder → PhysicalPlanValidator
            → Arc<PhysicalPlan> → QueryExecutionInstance::instantiate
```

Gather、Merge、HashRepartition、partial/final Aggregate、Distinct 和 TopN 必须先成为 immutable physical spec。删除构建 executor 后再 `replace_single_input`、直接创建 `BlockingOperator` 和专用 `HashShuffleJoin` tree 的路径。

验收：串行和并行查询在实例化前可 EXPLAIN 同一个完整 PhysicalPlan；执行模式只影响 task 数，不改变 operator 选择和 fragment graph。

### 3.4 缩小 plan build context 和清理 spec

引入只读 `PhysicalPlanBuildContext`，只包含 schema catalog、statistics snapshot、capability、planning config 和稳定对象标识。

必须从 spec 移除：

- `StorageClient`、transaction/session handle；
- runtime、memory tracker、cursor、buffer 和 emitted state；
- 本次执行的 parameter value、权限上下文和当前 snapshot；
- 可由 binding 解析的临时 space/storage 引用。

注意：SQL/GQL 文本中的常量属于查询语义，可以作为 immutable literal 进入计划；prepared parameter 的实际值必须保留为 parameter slot，不能在构建缓存计划时固化。

### 3.5 完成 slot binding、property derivation 和 validator

处理要求：

- 所有 expression、join key、filter、sort key 和 graph input column 在构建期绑定到 `SlotId`；
- 每个 operator 声明输入/输出 `SlotLayout`，空结果沿用 output contract；
- Filter 继承 distribution/ordering，Project 只继承仍存在的 key；
- Sort 明确输出 ordering；blocking 与 distribution 分开表达；
- local partition 不得标记为 Single；
- HashRepartition、GatherMerge 和 FinalAggregate 验证完整输入契约；
- 阻塞 operator 必须选择 `RequiresBudget` 或 `Spillable`，不能使用无语义默认值；
- 实现 `PhysicalPlanValidator`，验证 input count、ID 唯一性、schema/slot、properties、capability、transaction mode 和 memory policy。

validator 必须在缓存写入前和实例化前运行；cache load 后仍检查 compatibility，但不重复执行不依赖 binding 的昂贵验证。

### 3.6 让 cache、EXPLAIN 和 PROFILE 使用物理计划

处理要求：

- plan cache 只保存 `Arc<PhysicalPlan>` 和 parameter metadata；
- correctness compatibility 包含 query fingerprint、schema/layout version、feature/capability、planning config，以及会改变计划形状的策略版本；
- statistics version 作为 freshness/replan 信息，不因每次统计更新强制 correctness miss；
- 权限在每次实例化时重新校验；
- EXPLAIN 输出 slot、properties、fragment 和 Exchange；
- PROFILE 按 `(PhysicalOperatorId, FragmentId, TaskId)` 聚合，并保留 synthetic/source logical 标记。

## 四、P1：完成仍缺失的查询语义

### 4.1 图路径和递归

- 无损保存 source/target、edge types、方向、hop 范围、环策略、权重和算法配置；
- `MultiShortestPath` 禁止使用空配置占位；
- 明确 graph operator 的输入/输出 slot，不猜测顶点列；
- 长循环定期检查取消和 memory budget；
- weighted shortest path 在实现前返回 feature unavailable；
- 变长路径最终迁移到 `RecursiveFragmentSpec`，不得用无界通用 Loop 模拟。

### 4.2 强类型 command

- 定义封闭的 Space、Tag、Edge、Index、User、Fulltext、Vector 和 Migration command enum；
- 无损传递 create/alter/drop/show/rebuild/migrate 的全部字段；
- 删除 runtime 中的字符串 action 和空字符串 fallback；
- schema 变更发布 catalog version，并使依赖计划失效；
- 明确哪些 command 不进入 plan cache，但仍遵循相同 validation、lifecycle 和 result contract。

### 4.3 遗留控制流

删除或在 planner 阶段明确拒绝 `Loop`、`PassThrough`、`Select`、`AppendVertices` 等 executor 不支持的可生成节点。只有定义了终止条件、取消、内存边界和输出 schema 后才能新增实现。

### 4.4 语义测试矩阵

为 Union、Apply、路径、事务和管理命令建立：

- parser/planner → PhysicalPlan；
- PhysicalPlan validation；
- PhysicalPlan → execution instance；
- executor → real storage；
- materialized 与 chunk stream/PullHandle 差分；
- serial 与 parallel 差分；
- error、empty input、cancel、memory exceeded 和 feature matrix。

## 五、P2：建立 QueryExecutionInstance 和资源边界

### 5.1 QueryBindings 与单一实例入口

`QueryExecutionInstance` 必须拥有本次执行的 bindings、runtime、root memory pool、typed states、transaction scope、task group 和结果交付状态。相同 `Arc<PhysicalPlan>` 并发实例化时不得共享任何 mutable state。

生产环境只保留：

```text
QueryExecutionInstance::instantiate(plan, bindings, delivery, scheduler)
```

测试 builder 可以保留，但必须明确标注 test-only，不能绕过 validator 和资源绑定进入 API。

### 5.2 Typed GlobalState/LocalState

- hash build、global aggregate、sort runs、exchange 和 result collector 进入 GlobalState；
- cursor、probe cursor、chunk buffer 和 partial aggregate 进入 LocalState；
- 使用封闭 state enum 或 typed arena，禁止 `dyn Any`/字符串 downcast registry；
- state 通过 physical/fragment/task ID 寻址；
- spec 和 state 不重复保存 schema/layout；
- 所有 tracker 从 instance memory pool 派生。

### 5.3 TransactionScope 与 session transaction

显式事务跨查询存在，不能完全由单个 QueryExecutionInstance 拥有。

处理要求：

- session 级 `SessionTransactionController` 持有显式 transaction handle；
- QueryExecutionInstance 的 `TransactionScope` 借用显式事务，或拥有自动提交事务；
- Begin/Commit/Rollback command 只触发 controller/scope 状态迁移，不产生伪造文本；
- 明确语句失败时显式事务是整体失败、自动回滚还是回到 savepoint；
- DML/DDL、storage、全文和向量同步使用一致提交边界；
- 客户端断开只回滚本实例拥有的自动提交事务，不擅自结束 session 显式事务。

### 5.4 结果交付边界

可缓存计划只保存 `OutputContract`/`ResultBoundarySpec`。实例化时绑定：

- `DataSetSink`；
- `ChunkStreamSink`；
- `DiscardSink`；
- 同步 `PullHandle`。

所有交付方式必须在首个数据前提供 schema，空结果也保持 schema。同步 pull 使用自然背压；跨 async/HTTP/gRPC 的 bridge queue 必须有界、可取消、可计账。更换交付方式不得重建 PhysicalPlan。

### 5.5 分层内存和可失败复制

- 建立 instance → fragment → operator → task/queue 子池；
- 删除生产 `DataChunk` 的无条件 Clone，提供需要 reservation 的可失败 deep copy；
- chunk transfer 显式转移 reservation；
- expression workspace、hash key、sort buffer、graph frontier 和 queue 全部计账；
- 资源 owner 统一清理 cursor、临时文件和 bridge queue。

## 六、P2：Fragment、共享调度和 Exchange

### 6.1 FragmentSpec DAG

按 source、blocking、exchange、result boundary 和 write/command terminal 划分 fragment。串行 driver 与并行 task group 必须消费同一个 DAG。

### 6.2 引擎级共享 scheduler

当前每查询创建线程池不利于全局限流，并产生复杂 Drop/join 所有权。

处理要求：

- 数据库引擎持有固定大小共享 worker pool；
- 查询实例只持有 task group、取消令牌和配额；
- scheduler 提供查询间公平性、全局线程上限和 worker panic 隔离；
- operator 和 Exchange 只能提交 task，不能创建线程；
- query teardown 等待自己的 task，不关闭共享 pool。

### 6.3 从 partition tree 演进到 scan morsel

- vertex、edge 和 index scan 提供可切分 morsel；
- worker 动态领取 morsel并创建独立 LocalState；
- partition/layout 只描述数据域，不再等同于一个完整 executor tree；
- profile 记录 morsel 数、task 数、倾斜和 worker utilization。

### 6.4 通用 Exchange

在 Concatenate 和 MergeSort 基础上实现 `GatherConcatenate`、`GatherMerge`、`RepartitionHash`、`Broadcast`、`Barrier` 和显式 Materialize boundary。Hash shuffle join 收敛到通用 RepartitionHash，不保留第二套 join scheduler。

每个 queue 必须固定容量、分层计账、传播首个错误，并在 cancel 时同时唤醒 producer 和 consumer。

### 6.5 Spill

优先实现可复用的外排 Sort 或 hash partition spill，定义临时文件 owner、格式、校验、清理和 profile。Aggregate 与 Join 在基础设施稳定后复用。不可 spill 的 blocker 超预算时必须返回结构化错误。

## 七、P3：有证据后再做的优化

以下工作不阻塞架构完成的正确性阶段：

1. scan/filter/project、aggregate key 和 join key 的渐进列式化；
2. Vertex/Edge/Path ID 或轻量引用的 late materialization；
3. frontier/visited bitmap 和递归 fragment 专项优化；
4. factorized graph result；
5. 针对 NUMA、SIMD 或 work stealing 的进一步调优。

每项优化必须报告吞吐、首行延迟、分配次数、peak memory、queue wait、worker utilization 和 spill bytes，并保持语义差分测试通过。

## 八、推荐里程碑

1. **M0 可靠性**：修复双 runtime、worker self-join/panic、占位语义和生产入口测试。
2. **M1 统一计划**：正式 PhysicalPlan、ID allocator、build context、slot/properties、validator。
3. **M2 唯一入口**：合并单树/分区构建，cache、EXPLAIN、PROFILE 切换到 PhysicalPlan。
4. **M3 执行边界**：QueryExecutionInstance、typed state、transaction scope、result delivery、分层内存。
5. **M4 调度**：FragmentSpec、共享 scheduler、scan morsel、通用 Exchange。
6. **M5 资源与图**：spill、RecursiveFragmentSpec、真实 storage 和并发压力测试。
7. **M6 优化**：只实施 benchmark 能证明收益的列式化和图数据布局优化。

每个里程碑必须先完成结构和 validator，再迁移 production facade，最后删除旧路径。禁止长期保留双写、silent fallback 或“新路径失败后回退旧 executor”的兼容逻辑。

## 九、完成定义

以下条件全部满足后，本文可以归档：

- 所有 planner 可生成语义都能正确执行，或在 planner/physical builder 阶段被结构化拒绝；
- 一个 immutable、可验证、可缓存的 PhysicalPlan 覆盖所有生产查询和 command；
- executor 只有一条实例化路径，串行和并行消费同一 fragment graph；
- 每次执行独占 bindings、state、memory、profile、task group 和结果交付状态；
- 显式事务由 session controller 持有，语句 scope 和自动提交事务边界清晰；
- 所有跨 task 通信可计账、可取消、可等待并传播首个错误；
- schema、slot、properties、capability 和 memory policy 在执行前验证；
- cache、EXPLAIN 和 PROFILE 只以 PhysicalPlan/physical ID 为事实来源；
- 语义差分、真实 storage、取消、超限、worker panic 和 feature matrix 测试全部通过；
- benchmark 证明 fragment/scheduler/spill 等新增复杂度带来可衡量收益。
