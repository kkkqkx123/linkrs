# GraphDB Executor 现状差距分析

> 日期：2026-07-14
> 代码基线：`graphdb-query` 当前 production facade、streaming executor、partition builder、runtime、transaction 和 spill 实现。
> 目标规范：`executor_ideal_architecture.md`。
> 实施计划：`executor_remaining_work.md`。

## 一、结论

当前 executor 已经具备同步批量 pull、storage cursor、immutable operator spec、slot layout、query runtime、分区执行、有界并行队列、内存预算和 profile 等重要基础，架构方向适合轻量单机图数据库。

但它还不是具备完整数据库语义的正式执行内核。当前最主要的问题是契约未闭合，而不是缺少更多算子类型：planner 可以构建若干 executor 没有正式实现的节点；参数、事务、KILL 和 query registry 没有贯穿 production path；spill 对部分算子只有接口；新的 `PhysicalPlan`/`QueryExecutionInstance` 与当前生产入口并存。

因此现阶段判断为：

- 执行模型：合理，应保留；
- 局部算子和分区语义：已有较好测试基础；
- 端到端正确性：不完整；
- 事务和资源边界：不完整；
- 统一物理计划：迁移中；
- production readiness：未达到。

## 二、当前生产调用链

### 2.1 普通查询

```text
QueryPipelineManager::execute_plan
  -> ExecutionPlan.root: PlanNodeEnum
  -> StreamingQueryExecutor::from_plan_node
  -> operator_plan_builder::build_plan_node
  -> PhysicalNode::materialize
  -> StreamingExecutor tree
  -> StreamingExecutionEngine
  -> ResultStream::collect / StreamingQueryResult
```

`QueryPipelineManager` 创建默认 `ExecutionContext`，当前主要填充 storage、space、全文/向量组件、worker 数和 queue 容量。随后 factory 创建一个 `ExecutionRuntime`，注入 operator tree 和 engine。物化出口最终把全部 chunk 合并成 `DataSet`；流式出口保留 pull handle。

### 2.2 分区查询

```text
ExecutionPlan.root + PartitionSpec
  -> PartitionedPhysicalPlan::from_logical
  -> build_partitioned_physical_node
  -> local trees + Gather/Exchange + global tree
  -> StreamingExecutionEngine
```

该路径已经实现全局 Limit、两阶段 Aggregate、两层 Distinct/TopN、分区 Sort merge 和跨 partition join 等正确性处理。并行执行由 query 内 `MorselWorkerPool` 驱动，但当前 task 粒度仍主要是完整 partition executor tree，不是 storage scan morsel。

### 2.3 正在建设的新路径

项目已经存在：

- arena 形式的 `PhysicalPlan`、fragment 和 properties；
- `PhysicalPlanValidator`；
- `QueryBindings`、`QueryExecutionInstance`、`ResultSink`；
- `TransactionScope` 和 `SessionTransactionController`；
- typed state arena。

但是 `QueryExecutionInstance::instantiate` 仍要求调用方额外传入 `PhysicalNode`，文件注释也明确说明 arena 到 runtime operator 的桥尚未完成。production facade 仍直接使用 `StreamingQueryExecutor`，所以新路径尚不是事实来源。

## 三、已有基础评估

| 区域 | 当前状态 | 评价 |
|---|---|---|
| pull protocol | `open -> advance -> close` | 合理 |
| batch | `DataChunk<Vec<Vec<Value>>>` | 已批量化，但仍是行式而非列式 vector |
| storage scan | vertex/edge cursor 分批读取 | 合理 |
| immutable spec | `PhysicalNode` 可重复 materialize | 合理的迁移基础 |
| slot | chunk 始终有 `SlotLayout` | 有基础，尚未完成全计划 slot binding |
| parallel | 有界 channel、动态领取 partition task | 局部合理，pool ownership 不理想 |
| memory | query budget、operator tracker、queue reservation | 有基础，统计不完整且无全局治理 |
| cancellation | runtime token、deadline、循环检查 | 局部完整，未接通 production KILL |
| teardown | stream error/drop 清理、首错保留 | 较好 |
| profile | operator + partition identity | 有用，缺 fragment/task 和完整指标 |
| tests | streaming 定向测试 133 项通过 | 局部覆盖较好，端到端契约不足 |

## 四、正确性缺口

### 4.1 planner 与 executor 能力不闭合

当前 source executor 中 `GetProp` 和 `LookupIndex` 直接返回 `Ok(None)`。这会把“未实现”表现成合法空结果，调用方无法区分真实零行和能力缺失。

`IndexScan` builder 会创建 `IndexScan` spec，但将 `index_value` 设置为 `None`。runtime 只有同时取得 index name 和 value 才调用 `lookup_index`，因此该路径通常直接返回零行。

`IndexScan` 还会一次解析完整 ID 列表并 clone；如果一个 batch 内的 ID 全部 stale 或转换失败，它返回 `None`，可能在后续还有 ID 时提前结束流。

正式实现要求：

- build 阶段生成 typed predicate/range，而不是 `Option` 拼装状态；
- storage 提供 index cursor 或 batched lookup cursor；
- stale row id 只被跳过，不能作为 stream exhausted；
- 未实现的 `GetProp`/`LookupIndex` 在 builder 阶段返回结构化 unsupported，直到完整实现上线；
- 建立 logical node -> physical spec -> executor capability matrix 测试。

### 4.2 Start 和 Argument 缺少正式 seed/correlation 语义

当前 `Start` 和 `Argument` 的 `next` 都返回 `None`。`Start` 在关系执行模型中通常应提供单例零列行，作为无输入 Project、command 或 write pipeline 的 seed；`Argument` 应读取 correlated apply frame。

正式实现不能把两者视为相同空 source：

- `Start` 每次 open 后只输出一次单例 empty row；
- `Argument` 必须绑定 apply/correlated frame，并保持 slot layout；
- validator 检查 `Argument` 只能出现在具有 correlation binding 的 fragment。

### 4.3 参数只存在于上下文定义

`ExecutionContext` 和 `QueryBindings` 保存 parameters，但当前 production runtime 不保存 parameter frame；`ExpressionEvaluator` 遇到 `Expression::Parameter` 会返回缺少 runtime context 的错误。

正式实现应在 physical build 阶段把名称参数编译为有类型的 `ParameterSlot`，实例化时一次性校验并生成不可变 `ParameterFrame`。operator 热路径只按 slot 读取，不能访问字符串 map。

### 4.4 空结果 schema 仍由 facade 补偿

流式 facade 会从逻辑 root 预填列名，说明 output contract 尚未由 physical plan 贯穿到 result handle。直接使用较低层 `ResultStream::collect` 时，零行结果仍可能丢失 schema。

正式实现要求 root `ResultBoundarySpec` 始终提供 output layout，result handle 在首个 chunk 前发布 schema，不依赖是否产生数据。

## 五、事务与写入缺口

### 5.1 transaction command 是文本模拟

当前 BEGIN、COMMIT、ROLLBACK operator 只输出状态文本，没有调用 transaction manager 或 storage transaction。operator 中虽然保存 `transaction_id`，但没有执行真实状态迁移。

### 5.2 production query 没有 TransactionScope

`TransactionScope` 只进入尚未成为 production path 的 `QueryExecutionInstance`。普通查询和 DML 仍主要通过 storage client 直接执行，无法证明同一语句的读写使用相同 snapshot/transaction handle。

### 5.3 正式实现要求

- session controller 持有跨语句显式 transaction handle；
- query instance 绑定 explicit、auto-commit 或 none scope；
- storage scan、index、DML、DDL、全文和向量同步从 scope 获取相同 snapshot/transaction；
- command 先完成真实状态迁移，再生成结果行；
- auto-commit 只在执行、外部索引同步和结果交付边界成功后提交；
- 失败、取消和 sink failure 回滚；
- 显式事务失败使用 statement savepoint，或标记 rollback-only；
- transaction command 和 DML 增加真实 storage integration test。

## 六、取消、标识和生命周期缺口

`QueryContext::mark_killed` 修改 `QueryExecutionManager` 中的 atomic flag，streaming executor 检查的是 `ExecutionRuntime` 的另一套 token，两者没有在 production 装配时连接。

production `ExecutionContext` 也没有设置 server-assigned query id，默认值为 0。`ExecutionRuntime` 已有 query manager、finish guard 和 deadline 能力，但没有成为所有请求的强制生命周期。

正式实现顺序：

1. `QueryRegistry::begin` 分配非零唯一 `QueryId`；
2. 用同一 `CancellationSource` 创建 context、runtime 和 task group；
3. KILL、deadline、disconnect、memory error 和 worker error 写入同一 source；
4. query instance 负责 stop、wait、close、transaction finalize、resource cleanup；
5. 最后从 registry 移除 query；
6. 返回首个执行/取消原因，cleanup error 仅附加记录。

## 七、内存和 spill 缺口

### 7.1 内存账户不是完整所有权模型

当前 `MemoryBudget` 是 query 级共享 atomic counter，blocking operator 和 queue 有一定计账，但仍有以下缺口：

- `DataChunk::clone` 深拷贝 rows，却不建立新的 reservation；
- row memory estimate 没有完整覆盖 Vec capacity、hash bucket、temporary key 和 workspace；
- materialized `DataSet` 的累计内存没有严格纳入同一 budget；
- 没有 database/global pool 和 query admission control；
- operator/task/queue 缺少分层子池。

### 7.2 spill 尚不是完整算法

runtime 有 `SpillManager` setter，但 production factory 没有创建并绑定 manager。join spill 是 no-op，`spilled_bytes` 固定为 0。部分 set spill 写出后清空内存状态，却没有完整保存、重新读取和归并的执行协议。

正式实现必须从一种算法闭环开始：

- external sort：run generation + k-way merge；或
- hash partition：稳定分区 + partition-at-a-time aggregate/join。

在闭环完成前，对应 operator 必须声明 `RequiresBudget`，预算超限返回错误，不能标记为 `Spillable`。

## 八、计划、缓存和解释缺口

### 8.1 多种计划对象并存

当前同时存在 `ExecutionPlan`、`PlanNodeEnum`、`PartitionedPhysicalPlan`、`PhysicalNode` 和 arena `PhysicalPlan`。被 optimizer 处理、被 cache 保存、被 EXPLAIN 展示和真正被 executor 执行的对象尚未完全统一。

### 8.2 validator 多数是结构骨架

`PhysicalPlanValidator` 已有两阶段接口，但：

- full tier 的 permission、parameter binding 和 stats freshness 尚未实现；
- operator input count 没有真正核对引用关系；
- compatibility check 直接返回成功；
- properties 的取值较弱，很多节点使用默认 `single_streaming` 或 `single_blocking`；
- 当前 production path 不经过 arena plan validator。

### 8.3 正式收敛目标

唯一链路必须是：

```text
LogicalPlan
  -> PhysicalPlanBuilder
  -> PhysicalPlanValidator::structural
  -> Arc<PhysicalPlan> / PlanCache
  -> PhysicalPlanValidator::bindings
  -> QueryExecutionInstance::instantiate
  -> SerialDriver / QueryTaskGroup
```

EXPLAIN、PROFILE、cache key 和 executor 全部以同一个 `PhysicalPlan` 为事实来源。

## 九、并行调度缺口

当前每个 runtime 可以创建自己的 `MorselWorkerPool`。并发查询数增加时，线程数会按查询相乘，也不具备全局公平、优先级和 admission control。

此外当前所谓 morsel 主要是 partition tree 的动态领取，而不是 storage range/page/index segment 等小数据单元。

正式实现应改为进程级共享 scheduler：

- 引擎启动时创建固定 worker；
- query 只拥有 task group、quota 和 cancellation；
- scan 生成可重试、可计量的 morsel；
- Exchange 是唯一跨 task 数据通道；
- query close 等待自己的 task，不 join 或关闭共享 worker；
- worker panic 转换为 task/query error。

## 十、工程质量和测试缺口

当前 executor 定向单元测试共 133 项通过，覆盖分区 sort、aggregate、join、distinct、parallel gather、取消、stream cleanup 和 spill codec 等局部行为。这是可靠迁移的基础。

仍缺少以下 gate：

- planner -> physical plan -> real storage -> result 的 capability contract test；
- materialized 与 streaming differential test；
- serial 与 parallel differential test；
- transaction commit/rollback/savepoint integration test；
- parameters、empty schema、stale index row、empty cursor batch 测试；
- memory exceeded、disk full、corrupt spill、client disconnect 测试；
- worker panic、取消竞争和重复 close 压力测试；
- feature matrix 测试。

生产代码还存在多个 `unwrap()`，与项目约定不符；executor 目录也存在中文代码注释，与代码文件只能使用英文的约定不符。这些应作为迁移阶段的强制清理项，但优先级低于错误结果和事务缺口。

## 十一、风险优先级

| 优先级 | 风险 | 影响 |
|---|---|---|
| P0 | `GetProp`/`LookupIndex`/`IndexScan` 静默空结果 | 数据正确性 |
| P0 | transaction command 不执行事务 | ACID 语义 |
| P0 | 参数和 KILL 未接入 production runtime | 功能和运维正确性 |
| P1 | 多套计划和入口并存 | 架构漂移、cache/EXPLAIN 偏差 |
| P1 | validator 和 properties 不构成强契约 | 错误计划进入执行 |
| P1 | spill capability 名不副实 | 超限、缺行或 OOM 风险 |
| P2 | query 级线程池和粗粒度 partition task | 并发吞吐和资源公平性 |
| P2 | 行式 chunk、字符串/动态表达式路径 | CPU 和分配性能 |

性能优化必须排在语义闭包、事务和资源所有权之后。

## 十二、验证记录

本次分析执行：

```shell
cargo check -p graphdb-query --lib
cargo test -p graphdb-query --lib query::executor::streaming -- --nocapture
```

结果：编译通过；streaming executor 相关测试 133 项通过、0 失败，存在一个测试代码 unused-variable warning。该结果证明局部算子基础可用，不代表 production facade、真实事务、参数、索引和资源压力已经满足目标架构。
