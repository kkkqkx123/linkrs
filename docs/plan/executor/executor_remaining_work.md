# GraphDB Executor 正式实施方案

> 日期：2026-07-14
> 目标架构：`executor_ideal_architecture.md`。
> 差距依据：`executor_current_gap_analysis.md`。
> 原则：项目不要求向后兼容；迁移完成后删除旧类型和旧入口，不保留永久双轨。

## 一、实施原则

1. 正确性优先于性能和并行度；先消除静默空结果、虚假事务和未绑定参数。
2. planner 能生成的每个 physical spec 必须有完整 executor；未完成能力在 build/validate 阶段明确拒绝。
3. 一个 production request 只能走一条执行路径；新路径失败不得回退旧路径。
4. 每个阶段遵循“定义契约 -> 增加测试 -> 实现 -> 切换 facade -> 删除旧路径”。
5. immutable plan 不保存 storage、transaction、cursor、runtime、parameter value 或 mutable state。
6. runtime 资源必须有唯一 owner；取消、错误和 cleanup 保留首个原因。
7. 只有完成写出和读回算法的 operator 才能声明 `Spillable`。
8. 不为未证明的性能收益提前引入列式化、factorized table 或 async operator API。

## 二、目标生产链路

最终只保留：

```text
Query request
  -> Parse / Validate
  -> LogicalPlan
  -> PhysicalPlanBuilder
  -> StructuralPlanValidation
  -> PlanCache<Arc<PhysicalPlan>>
  -> QueryBindings + BindingValidation
  -> QueryExecutionInstance
  -> SerialFragmentDriver | SchedulerTaskGroup
  -> ResultBoundary
  -> DataSetSink | PullHandle | StreamSink | DiscardSink
```

实例化顺序固定为：

```text
QueryRegistry registration
  -> TransactionScope acquisition
  -> ParameterFrame binding
  -> MemoryPool / SpillOwner / CancellationSource
  -> GlobalState allocation
  -> LocalState/task creation
  -> operator open
```

teardown 逆序执行，并在返回前等待本查询 task group 退出。

## 三、M0：立即封闭错误语义

### 3.1 建立 capability matrix

新增由封闭 enum 驱动的测试矩阵，每个 `PlanNodeEnum`/未来 `LogicalNode` 映射到：

- physical spec；
- required feature；
- transaction mode；
- serial/parallel capability；
- memory policy；
- executor implementation status。

顶层 physical builder 使用穷尽 match。当前未实现节点直接返回独立错误类型：

```text
PlanBuildError::UnsupportedNode
PlanBuildError::MissingRequiredValue
PlanBuildError::CapabilityUnavailable
PlanBuildError::InvalidTransactionMode
PlanBuildError::ExpressionBinding
```

禁止把这些错误再次包装成丢失类别信息的通用字符串。

验收：遍历所有 node/spec variant 的测试能证明没有 silent fallback；新增 enum variant 会导致编译或 capability test 失败。

### 3.2 暂时拒绝未完成 source

在正式实现前，builder 对以下路径返回结构化错误：

- 无 typed predicate 的 `IndexScan`；
- 当前返回 `Ok(None)` 的 `GetProp`；
- 当前返回 `Ok(None)` 的 `LookupIndex`；
- 没有 correlation frame 的 `Argument`；
- 任何使用空 space、空 edge type、空 target 或默认方向代替必填语义的节点。

这一步不追求功能数量，目标是确保成功查询不会静默缺行。

### 3.3 实现 Start 和 Argument

`StartState` 保存 `emitted: bool`：

- `open` 重置为 false；
- 首次 `next` 返回一行零列、带稳定 empty layout 的 chunk；
- 后续返回 `None`；
- `close` 删除 state。

`ArgumentSpec` 保存 input layout 和 parameter/correlation slot 映射。`Apply` 为右侧实例创建 immutable `CorrelationFrame`；`Argument` 按 frame 输出一行。禁止从全局 variable map 临时猜测列。

验收：无输入 projection、command seed、correlated apply、nested apply 和 empty outer input 测试通过。

### 3.4 清理 panic 和代码约定

- production `unwrap()` 改为状态错误或通过类型结构消除不可能分支；
- poisoned standard mutex 转换为 query execution error，或统一使用不 poison 的锁并保持错误边界；
- 删除代码文件中的中文注释；
- CI 增加 executor production source 的 unwrap/中文扫描规则。

## 四、M1：正式实现数据访问和参数

### 4.1 扩展 storage cursor contract

不要在 executor 内先读取全部 ID 再伪装流式。storage 层提供以下只读 cursor：

```text
VertexScanCursor
EdgeScanCursor
IndexCursor<RowId | CoveringRow>
PropertyBatchReader
```

共同契约：

- cursor 绑定 transaction snapshot；
- `next_batch(limit)` 只有在 exhausted 时表达结束；
- 支持 cancellation/deadline 检查；
- 返回稳定 output schema；
- index cursor 支持 equality/range/prefix 中 storage 实际具备的能力；
- partition/morsel range 是显式 typed range，不使用字符串。

### 4.2 IndexScan

`IndexScanSpec` 至少保存：

```text
index_id
predicate: BoundIndexPredicate
projection: IndexProjection
residual_filter: Option<BoundExpression>
output_layout
```

实例化时校验 index version 和 capability，使用 transaction snapshot 打开 cursor。covering index 直接形成输出；非 covering index 按 chunk 批量回表。stale row id 被计数并跳过，继续读取下一 batch；是否把 stale entry 视为 corruption 由 storage policy 决定，但不能提前 EOF。

### 4.3 GetProp

`GetProp` 应是 unary operator，不是独立零输入 source。spec 保存 entity slot、property ids、missing-property policy 和 output layout。

执行流程：

1. 拉取输入 chunk；
2. 从 entity slot 提取 vertex/edge id；
3. 按 entity kind 和 property projection 批量读取；
4. 按输入行序拼接属性值；
5. 对 missing entity/property 应用明确 null/error 语义；
6. 保持一对一行基数，除非 spec 明确声明过滤。

workspace 和输出 chunk 都从 operator memory pool 预留内存。

### 4.4 LookupIndex

如果语义与 `IndexScan` 相同，删除重复 operator，统一为一个 typed access path；如果 `LookupIndex` 是管理/metadata lookup，则用不同 spec 名称表达不同语义。禁止维护两个字段略有差异、执行行为不一致的索引算子。

### 4.5 ParameterFrame

physical expression 引入：

```text
BoundExpression::Literal
BoundExpression::SlotRef
BoundExpression::ParameterRef(ParameterSlot)
BoundExpression::Call
...
```

`PhysicalPlan` 保存 parameter schema：名称、slot、value type、nullable 和可选 default。`QueryBindings` 实例化时：

1. 检查缺失和未知参数；
2. 做受控类型转换；
3. 校验长度、数值范围、vector dimension 等约束；
4. 构造 `Arc<ParameterFrame>`；
5. expression evaluator 按 slot 读取。

不得把参数值写入 plan cache entry，也不得让 operator 访问 API request 对象。

验收：literal/parameter differential、重复执行同一 cached plan、并发使用不同参数、错误类型和 vector parameter 测试通过。

## 五、M2：接通真实事务和查询生命周期

### 5.1 TransactionScope API

transaction 层提供 executor 所需的最小接口：

```text
snapshot()
read_handle()
write_handle()
create_statement_savepoint()
release_statement_savepoint()
rollback_statement_savepoint()
commit_auto_transaction()
rollback_auto_transaction()
mark_rollback_only()
```

`TransactionScope` 分为：

- `ExplicitBorrowed`：由 session controller 持有；
- `AutoCommitOwned`：由 query instance 持有；
- `ReadOnlySnapshot`：只读且不需要写 transaction；
- command scope：只用于 BEGIN/COMMIT/ROLLBACK 状态迁移。

删除容易把“无 scope”和“无需事务”混淆的宽泛 `None`，由 plan capability 明确声明是否允许无事务执行。

### 5.2 SessionTransactionController

controller 不能只保存 transaction id。它必须保存 transaction manager handle、状态、read/write mode、rollback-only 标志和 session ownership。

状态迁移由 controller 串行化：

- BEGIN：创建 transaction 后再切 Active；
- COMMIT：Active -> Committing，成功后清除 handle；失败进入 Failed/RollbackOnly；
- ROLLBACK：幂等释放 handle；
- session disconnect：按策略回滚 active transaction。

transaction operator 删除虚假文本实现，改为调用 controller；成功后由 command result adapter 生成状态行。

### 5.3 DML/DDL 提交协议

DML sink 只通过 `TransactionScope::write_handle` 写入。DDL 明确是否支持 transactional catalog change。全文和向量索引采用 transaction 层已有能力选择：

- 同一事务内可提交的 durable participant；或
- transaction commit 后写 outbox，再由同步组件消费。

禁止在 DML operator 成功返回 chunk 前独立提交外部索引，避免 storage 回滚后外部状态已可见。

### 5.4 QueryRegistry 和统一取消

API 层不自行构造互不关联的 query id。新增进程级 `QueryRegistry`：

- 分配 `QueryId`；
- 保存 query metadata、统一 cancel source、deadline 和弱 handle；
- KILL 根据 ID 设置原因；
- query guard 在完整 teardown 后注销。

`QueryExecutionManager::killed` 和 runtime cancel token 合并为同一对象。取消原因至少区分：user kill、deadline、client disconnect、memory limit、worker failure、shutdown。

验收：KILL 能停止 storage cursor、graph traversal 和 parallel task；query id 不为 0 且并发唯一；registry 不遗留完成查询。

## 六、M3：统一 PhysicalPlan 和唯一实例化入口

### 6.1 完成 arena materializer

实现：

```text
PhysicalPlan arena + FragmentGraph + QueryBindings
  -> validate bindings/capabilities
  -> allocate GlobalState
  -> construct fragment drivers/local state factories
```

materializer 只接受 `Arc<PhysicalPlan>`，不再接受额外 `PhysicalNode`。所有 runtime operator 通过 `PhysicalOperatorId` 读取 spec；storage、transaction、parameter frame 和 memory pool 从 bindings/runtime 注入。

### 6.2 统一 ID

- `PhysicalOperatorIdAllocator` 为全部真实和 synthetic operator 分配 ID；
- `LogicalNodeId` 只作为来源信息；
- 删除 hardcoded `0`、负数 synthetic ID 和同一 local/final operator 共用 ID；
- fragment/task/profile 全部使用 typed ID。

### 6.3 合并普通与分区 builder

`PhysicalPlanBuilder` 一次生成完整 fragment graph。serial/parallel 不改变 plan shape，仅改变有效 task 数。partial/final Aggregate、Distinct、TopN、Gather、Repartition 和 Merge 都是 immutable spec，不在 runtime builder 临时插入。

切换顺序：

1. scan/filter/project/limit；
2. blocking relational operators；
3. join/set/apply；
4. graph traversal/path；
5. DML/DDL/transaction/fulltext/vector；
6. production facade；
7. 删除 `PartitionedPhysicalPlan`、旧 `PhysicalNode` materialize 和直接 executor builder。

每组使用 old/new differential test，但 production 请求不做 runtime fallback。

### 6.4 完成 validator

Structural tier 必须真正检查：

- operator ID 唯一；
- fragment DAG 连通、无非法 cycle；
- child reference 和 input count；
- input/output slot compatibility；
- expression slot 和 parameter slot；
- distribution、ordering、pipeline、parallel、memory properties；
- Exchange 两端协议；
- required capability 和 feature；
- transaction requirement；
- root output contract。

Binding tier 检查：

- catalog/schema/layout/index version；
- storage capability；
- permissions 和会改变计划形状的 policy version；
- parameter frame；
- transaction mode；
- runtime limits。

compatibility mismatch 返回 cache miss/replan，不允许直接执行旧计划。

### 6.5 切换 cache、EXPLAIN 和 PROFILE

- cache 只保存 `Arc<PhysicalPlan>` 和 parameter schema；
- EXPLAIN 输出 operator spec、slot、properties、fragment 和 Exchange；
- PROFILE 复用相同 plan，按 `(PhysicalOperatorId, FragmentId, TaskId)` 汇总；
- EXPLAIN ANALYZE 使用 `DiscardSink`，不额外复制 executor path；
- 删除对逻辑 root 的 output schema fallback。

## 七、M4：状态、结果和内存所有权

### 7.1 分离 spec、global state 和 local state

runtime operator enum 不再同时保存 config 和 mutable buffer/cursor。typed arena 按 ID 分配：

- Global：hash build、global aggregate、sort runs、Exchange、result status；
- Local：scan cursor、probe state、partial accumulator、expression workspace。

state variant 与 spec variant 在 instantiate 阶段匹配；不允许热路径 `unwrap` 假定 variant 一致。错误计划必须在 open 前失败。

### 7.2 ResultBoundary

plan root 保存稳定 `OutputContract`：layout、types、nullable、ordering 和 streaming capability。实例化绑定：

- `DataSetSink`；
- `PullHandle`；
- `ChunkStreamSink`；
- `DiscardSink`。

零行结果仍发布 schema。sink error 进入统一取消；网络 bridge queue 有界且计账。`DataSetSink` 的累计内存属于 query memory pool。

### 7.3 分层 MemoryPool

建立：

```text
DatabaseMemoryPool
  -> QueryPool
     -> FragmentPool
        -> OperatorPool
           -> TaskPool / QueuePool
```

所有 reservation 使用 RAII。`DataChunk` 删除无条件 `Clone`，提供 move、slice/view 和 `deep_copy(pool)`。内存估算覆盖容器 capacity、hash table、typed key、expression workspace、frontier/visited 和 queue envelope。

database pool 支持 admission control：无法授予 query 初始 reservation 时排队或返回 resource exhausted，不能让每个查询独立认为自己可使用 512 MiB。

## 八、M5：正式 spill

### 8.1 第一阶段只实现 external sort

选择 sort 作为第一个闭环，原因是算法和正确性边界清晰：

1. 内存达到 operator threshold 后排序当前 buffer；
2. 写出带 version、schema fingerprint、row count、checksum 的 run；
3. 清空 buffer 并释放 reservation；
4. 输入结束后对内存 run 和磁盘 run 执行 k-way merge；
5. merge 输出继续遵守 chunk size、ordering、cancel 和 memory budget；
6. resource owner 清理所有 run。

磁盘 quota 与 memory quota 分开。测试覆盖多 run、相同 key、null ordering、limit/top-N、cancel、disk full 和 corrupt run。

### 8.2 hash partition spill

在 sort 稳定后实现共享 `HashPartitionSpiller`：

- 固定 hash algorithm/seed/version；
- 根据预算选择 partition count；
- typed row/key serialization；
- skew detection 和递归 repartition 上限；
- partition-at-a-time load/probe/aggregate；
- outer join unmatched row 状态持久化；
- set operator 保留完整 typed key，不使用字符串编码。

Aggregate、HashJoin、Distinct/Set 复用该基础设施。每个 operator 在支持前保持 `RequiresBudget`。

### 8.3 启动清理

spill path 使用数据库实例 id、query id 和随机 execution nonce。正常 cleanup 由 resource owner 完成；启动时只清理由本实例规则确认的 orphan directory，避免删除其他进程文件。

## 九、M6：共享 scheduler、morsel 和 Exchange

### 9.1 引擎级 scheduler

数据库启动时创建固定 worker pool。query 注册 task group，包含 quota、priority、cancel token、错误槽和 completion barrier。worker 不拥有 scheduler，query teardown 不 join worker。

调度策略先采用简单公平 round-robin/query queue，再根据 profile 决定是否需要 work stealing。线程数、active query 和 memory admission 由全局配置控制。

### 9.2 scan morsel

storage scan 暴露可切分 range/page/segment。morsel 描述不可变数据范围，不持有 cursor。worker claim 后创建 local cursor/state；失败是否可重试由 transaction snapshot 和 operator side effect 明确决定。写算子默认不可透明重试。

### 9.3 通用 Exchange

实现并验证：

- `GatherConcatenate`；
- `GatherMerge`；
- `RepartitionHash`；
- `Broadcast`；
- `Barrier`；
- 显式 `Materialize`。

queue 同时受 chunk count 和 byte budget 限制。关闭协议传播 EOF、first error 和 cancellation，不允许 receiver 依靠 channel disconnect 猜测正常结束。

现有专用 hash shuffle join 在通用 `RepartitionHash` 上线后删除。

## 十、M7：图递归和高级能力

### 10.1 RecursiveFragmentSpec

变长路径、BFS 和多轮算法使用明确 recursive fragment：

```text
seed
frontier input/output
visited state
step expression
min/max hop
termination condition
path uniqueness policy
output policy
```

frontier、visited、path predecessor 和 result queue 全部计账。每轮和批量扩边内部检查取消。weighted shortest path 在权重类型、负权策略和算法选择完成前保持 capability unavailable。

### 10.2 全文和向量

全文/向量 query 接受相同 transaction visibility、parameter frame、deadline、result boundary 和 profile contract。当前同步 operator 内使用 `block_on` 的路径应在边界层提供明确同步 adapter；不得在持有 executor/global lock 时等待外部 future。

管理命令使用封闭 enum，并发布 catalog/capability version 触发 plan cache compatibility 失效。

## 十一、测试与发布门槛

### 11.1 每个 operator 的契约测试

统一测试套件覆盖：

- open/next/stop/close 状态机和幂等性；
- empty input、empty output schema；
- chunk boundary 不改变结果；
- cancel、deadline、memory error；
- serial/parallel differential；
- materialized/stream/discard differential；
- profile row/chunk/memory 计数。

### 11.2 端到端矩阵

从 parser 或 logical plan 开始，经过 physical build、validation、cache、bindings、真实 storage 和 result boundary。至少覆盖 scan/index/property、join/set/apply、aggregate/window、graph、DML、DDL、transaction、fulltext 和 vector feature 组合。

### 11.3 并发与故障注入

- KILL 与 cursor read、queue send、spill write、commit 竞争；
- client 在首行前和中途断开；
- worker panic 和 task error；
- memory/disk quota；
- corrupt spill；
- session disconnect with active transaction；
- repeated open/close 和 100 次并发稳定性循环。

子线程 panic 必须使测试失败，不能只打印到 stderr。

### 11.4 性能基线

每个调度或数据布局优化报告：吞吐、首行延迟、总延迟、CPU、分配次数、peak memory、queue wait、spill bytes 和 worker utilization。没有明确收益则保留较简单实现。

## 十二、里程碑和依赖

| 阶段 | 核心交付物 | 前置 | 删除项 |
|---|---|---|---|
| M0 | capability matrix、拒绝占位语义、Start/Argument | 无 | silent fallback |
| M1 | storage cursor、IndexScan/GetProp、ParameterFrame | M0 | 全量 ID 收集、运行期参数名查找 |
| M2 | 真实 transaction、QueryRegistry、统一 cancel | M0 | 文本 transaction、独立 killed flag |
| M3 | 唯一 PhysicalPlan 和 instance 入口 | M1、M2 | `PartitionedPhysicalPlan`、额外 `PhysicalNode` 参数、旧 factory |
| M4 | typed state、ResultBoundary、分层内存 | M3 | inline mutable state、schema fallback、chunk 无计账 clone |
| M5 | external sort、hash spill | M4 | no-op/不完整 spill |
| M6 | shared scheduler、morsel、通用 Exchange | M3、M4 | query-level pool、专用 shuffle join |
| M7 | recursive fragment、全文/向量统一 | M3-M6 | 图算法特殊生命周期 |

M1 和 M2 可以并行设计，但在 M3 切换 production facade 前必须同时完成。M5 和 M6 可分别开发，合并时必须统一 memory、cancel 和 task ownership。

## 十三、阶段完成定义

每个里程碑只有同时满足以下条件才完成：

- 目标路径有 unit、contract 和 integration test；
- production facade 已切换；
- cache、EXPLAIN 和 PROFILE 与执行对象一致；
- 旧入口、旧类型和临时 adapter 已删除；
- 没有新路径失败后回退旧实现；
- `cargo clippy --all-targets --all-features` 和 workspace feature check 通过；
- 文档中的 capability matrix 和实际 enum 一致。

整个 executor 重构完成的最终标准以 `executor_ideal_architecture.md` 第十二节为准。
