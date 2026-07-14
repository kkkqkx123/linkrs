# GraphDB Executor 剩余任务

> 日期：2026-07-13
> 目标架构：`executor_ideal_architecture.md`
> 差距依据：`executor_current_gap_analysis.md`

## 已完成基线（不再列为任务）

以下基础设施已存在：

- `PhysicalPlan` arena 类型 + `PhysicalOperatorSpec` + `FragmentGraph`/`FragmentSpec`/`FragmentKind`
- `PhysicalOperatorIdAllocator` / `FragmentIdAllocator`
- `PhysicalPlanBuildContext`（只读 build context）
- `PhysicalPlanValidator`（两阶段：Structural/Full，包含 ID 唯一性、连通性、input count、output layout、property consistency、memory policy 检查）
- `QueryExecutionInstance` + `QueryBindings`
- `ResultSink`（Materialize/Stream/Discard）
- `TransactionScope`（Explicit/AutoCommit/None）+ `SessionTransactionController`
- `SlotLayout` + `SlotInfo`（含 name/alias/data_type/nullable/origin）+ `combine_layouts`
- `DataChunk` 始终携带 `Arc<SlotLayout>` + `memory_reservation`，无无条件 `Clone`
- `ExecutionRuntime`（取消、profile、memory、storage、spill manager、worker pool）
- `MorselWorkerPool`（查询级有界 worker pool，原子计数器 morsel 分配）
- `GlobalStateArena` / `LocalStateArena` / `StateArenaSet`（typed arena，无 `dyn Any`）
- `StreamingExecutor` 生命周期（New/Opened/Exhausted/Stopped/Failed/Closed）
- `operator_plan_builder::build_plan_node` 穷尽 match，领域错误不被吞
- `Spillable` trait
- `PhysicalProperties` 结构定义
- `StreamingExecutionEngine`（pull-driven，支持 Gather/Exchange/partitioned roots）

执行原则：

1. 先修复生产路径直接失败、泄漏或返回错误结果的问题；
2. 统一并验证物理计划后再引入 scheduler/spill 等复杂机制；
3. 先建立资源所有权，再扩大并行度；
4. 不使用空列表、默认方向、空字符串 action 或 silent fallback 伪装未实现语义。

---

## P0：恢复可靠的生产执行路径

### 修复非分区执行的双 runtime

`StreamingExecutorBuilder::from_plan_node` 创建 runtime A 并注入 operator tree，factory 随后创建 runtime B，`engine.set_runtime` 未被调用。`execute()`/`into_stream()` 要求 engine 持有 runtime，非分区计划可能直接返回 `No ExecutionRuntime attached`。

- runtime 只能由 execution instance/factory 创建一次
- engine、operator、result handle、cancel/profile handle 引用同一 `Arc<ExecutionRuntime>`
- builder 停止创建 runtime，只构建 immutable plan 或从 bindings 实例化
- 分区与非分区入口使用同一 runtime 装配顺序

验收：无 partition spec 的 scan/filter/empty result 可通过物化和 stream 执行；cancel 可停止 operator tree；profile/resource/memory 对象身份一致；EXPLAIN ANALYZE 和 PROFILE 不走缺失 runtime 的单独路径。

### 修复 worker teardown 和隐藏线程 panic

worker self-join 曾触发 `Resource deadlock avoided` 但测试静默通过。runtime 持有 pool，pool 在 worker 所持最后一个 runtime 引用释放时 Drop 可能自我 join。

- 禁止 worker 执行 pool 的最终 Drop/join（共享 scheduler 到位前）
- query handle 只等待本查询 task group，不 join 当前线程
- worker panic 转为查询首个错误，测试不得静默通过
- 关闭 queue 后等待本查询所有 task 退出，清理错误只记日志不覆盖原始错误
- 增加取消、消费者提前断开、worker error、open failure、Drop-only 五类并发测试

验收：相同测试循环 100 次，无子线程 panic、死锁、遗留 task 或未释放 reservation。

### 清除占位语义

- `MultiShortestPath` 空 target、空 edge types、默认 Both 方向
- `DdlSpec::Migrate` 使用 `action: String` 违反 §3.4
- 合成 Start/DML source 仍用硬编码 physical ID `0`
- 部分 command/scan 缺少 space 时使用空字符串

无法无损构建的节点在 planner/physical builder 阶段返回结构化错误，语义完整后才能进入 executor。

---

## P1：统一 PhysicalPlan 并消除双路径

### 建立 arena → StreamingExecutor 桥接

当前 `QueryExecutionInstance::instantiate` 仍接受 `PhysicalNode` tree 参数。`plan.rs:14` 注释承认 bridge not yet built。

- 实现从 `PhysicalPlan` arena（`PhysicalOperatorSpec` + `FragmentGraph`）到 `StreamingExecutor` tree 的 materialization
- 删除 `PhysicalNode` 参数，`instantiate(plan, bindings, delivery, scheduler)` 成为唯一入口
- 所有 operator 仅通过 `PhysicalOperatorId` 从 arena 读取配置

### 统一 ID 系统

`PhysicalNode` 使用 `PhysicalNodeId = i64`，与 `PhysicalOperatorId(usize)` 双轨并存。`SyntheticNodeIdAllocator` 从 `i64::MIN` 向下分配。

- 所有物理节点（Start、Gather、partial/final、synthetic）统一使用 `PhysicalOperatorIdAllocator`
- 删除 `SyntheticNodeIdAllocator` 和硬编码 `0`/`i64::MIN + n`
- logical node 拆分时保留来源 `LogicalNodeId` 但不复用 physical ID

### 分离 LogicalPlan 与 PhysicalPlan

`PlanNodeEnum` 同时包含语义层（InnerJoin/Scan）和算法层（HashInnerJoin/IndexScan）节点。

- logical join 只保留 join kind、condition 和输入
- logical scan 只描述访问需求
- HashJoin、IndexScan、partial/final Aggregate、TopN、Distinct、Exchange 仅存在于 physical spec
- planner/optimizer 不生成 executor 专用 variant

### 合并单树和分区构建路径

当前两条路线：
1. `PlanNodeEnum → PhysicalNode → materialize → StreamingExecutor`
2. `PlanNodeEnum → PartitionedPhysicalPlan → physical_builder → replace_single_input`

统一为：
```
LogicalPlan → PhysicalPlanBuilder → PhysicalPlanValidator
            → Arc<PhysicalPlan> → QueryExecutionInstance::instantiate
```

Gather、Merge、HashRepartition、partial/final Aggregate、Distinct、TopN 先成为 immutable physical spec。删除 `replace_single_input`、直接创建 `BlockingOperator`、专用 `HashShuffleJoin` tree。

验收：串行和并行查询在实例化前可 EXPLAIN 同一 PhysicalPlan；执行模式只影响 task 数，不改变 operator 选择和 fragment graph。

### 清理 spec，移除执行期资源

`SinkSpec` 所有 DML variant 持有 `storage: Option<Arc<RwLock<dyn StorageClient>>>`，违反不可变性。

- SinkSpec 只保存 space name、列名、表达式等不可变描述
- storage/transaction handle 在实例化时从 `QueryBindings` 注入
- `DdlSpec::Migrate` 的 `action: String` 改为封闭 enum

### 完成 slot binding、property derivation、validator

- 所有 expression/join key/filter/sort key 在构建期绑定到 `SlotId`
- 每个 operator 声明输入/输出 `SlotLayout`，空结果沿用 output contract
- Filter 继承 distribution/ordering；Project 只继承仍存在的 key
- Sort 明确输出 ordering
- local partition 不得标记为 Single
- HashRepartition、GatherMerge、FinalAggregate 验证完整输入契约
- 阻塞 operator 必须选择 `RequiresBudget` 或 `Spillable`
- `PhysicalPlanValidator::check_compatibility`（当前是 TODO）实现 cache 命中校验

#### PhysicalProperties 枚举语义对齐

| 属性 | 规范要求 | 当前实现 | 修正 |
|---|---|---|---|
| `Distribution` | `Hash(keys, buckets)` 含 bucket 数 | `HashPartitioned(Vec<String>)` 无 bucket | 增加 bucket count |
| `PipelineKind` | `Source/Streaming/Blocking/Sink/Exchange` | 仅 `Streaming/Blocking` | 增加 Source/Sink/Exchange |
| `ParallelMode` | `Single/MorselParallel/PartitionLocal` | `Parallelism { min, max }` | 替换为并行模式枚举 |
| `Cardinality` | estimate + optional upper bound | `estimated_cardinality: Option<f64>` | 增加 `upper_bound` |

### cache、EXPLAIN、PROFILE 切换到 PhysicalPlan

- plan cache 只保存 `Arc<PhysicalPlan>` + parameter metadata
- cache compatibility 包含 query fingerprint、schema/layout version、feature/capability、planning config 和可能改变计划形状的策略版本
- statistics version 作为 freshness/replan 信息，不强制 correctness miss
- 权限每次实例化时重新校验
- EXPLAIN 输出 slot、properties、fragment、Exchange
- PROFILE 按 `(PhysicalOperatorId, FragmentId, TaskId)` 聚合

---

## P1：完成仍缺失的查询语义

### 图路径和递归

- `MultiShortestPath` 无损保存 source/target、edge types、方向、hop 范围、环策略
- 明确 graph operator 的输入/输出 slot
- 长循环定期检查取消和 memory budget
- weighted shortest path 实现前返回 feature unavailable
- 变长路径迁移到 `RecursiveFragmentSpec`，不得用无界通用 Loop 模拟

### 强类型 command

- DDL、Fulltext、Vector、Migration 全部使用封闭 enum，删除字符串 action
- schema 变更发布 catalog version，使依赖计划失效
- 明确哪些 command 不进 plan cache，遵循相同 validation/lifecycle/result contract

### 遗留控制流

`Loop`、`PassThrough`、`Select`、`AppendVertices` 在 planner 阶段明确拒绝。只有定义终止条件、取消、内存边界和输出 schema 后才能新增。

### 语义测试矩阵

Union、Apply、路径、事务、管理命令覆盖：
- parser/planner → PhysicalPlan → validation → execution instance
- materialized vs chunk stream vs PullHandle 差分
- serial vs parallel 差分
- error、empty input、cancel、memory exceeded、feature matrix

---

## P2：建立 QueryExecutionInstance 和资源边界

### 迁移内联 mutable state 到 arena

`SourceOperator`、`UnaryOperator`、`BlockingOperator` 等运行时 enum 直接嵌入 cursor/buffer/hash table。`state.rs` 已定义 `SourceState`/`UnaryState`/`BlockingState` 但未被使用。

- runtime operator enum 只保留不可变 spec，可变状态移到 `StateArenaSet`
- operator 通过 `(PhysicalOperatorId, TaskId)` 从 arena 读写 state
- 删除内联状态字段（如 `SourceOperator` 的 `buffer/current_index`）
- arena state 由 `QueryExecutionInstance` 统一分配和清理

### ResultBoundarySpec

可缓存 plan 只保存 `OutputContract`/`ResultBoundarySpec`，作为 `OperatorKindSpec` 变体或 `OutputContract` 字段。实例化时绑定具体 sink。

- 所有交付方式在首个数据前提供 schema，空结果保持 schema
- 跨 async/HTTP/gRPC 的 bridge queue 有界、可取消、可计账
- 更换交付方式不重建 PhysicalPlan

### 分层内存

- instance → fragment → operator → task/queue 子池
- 生产 `DataChunk` 不提供无条件 Clone，deep copy 需要 reservation
- expression workspace、hash key、sort buffer、graph frontier、queue 全部计账

---

## P2：Fragment、共享调度和 Exchange

### FragmentSpec DAG

按 source/blocking/exchange/result/terminal 划分 fragment。串行与并行消费同一 DAG。

### 引擎级共享 scheduler

当前每查询创建线程池。改为引擎持有固定大小共享 pool，查询只持有 task group + 取消令牌 + 配额。
- 查询间公平性、全局线程上限、worker panic 隔离
- operator 和 Exchange 只提交 task，不创建线程
- query teardown 等待自己的 task，不关闭共享 pool

### 通用 Exchange

在 `Concatenate`/`MergeSort` 基础上实现 `RepartitionHash`、`Broadcast`、`Barrier`、`Materialize`。
Hash shuffle join 收敛到 `RepartitionHash`，不保留第二套 join scheduler。

### 从 partition tree 演进到 scan morsel

- vertex/edge/index scan 提供可切分 morsel
- worker 动态领取 morsel 并创建独立 LocalState
- partition/layout 只描述数据域，不等同完整 executor tree
- profile 记录 morsel 数、task 数、倾斜、worker utilization

### Spill

实现可复用外排 Sort / hash partition spill。定义临时文件 owner、格式、校验、清理和 profile。
Aggregate 与 Join 复用。不可 spill 的 blocker 超预算返回结构化错误。

---

## P3：有证据后做的优化

1. scan/filter/project、aggregate key、join key 的渐进列式化
2. Vertex/Edge/Path 轻量引用的 late materialization
3. frontier/visited bitmap 和递归 fragment 专项优化
4. factorized graph result
5. NUMA/SIMD/work stealing 调优

每项必须报告吞吐、首行延迟、分配次数、peak memory、queue wait、worker utilization、spill bytes，语义差分测试通过。

---

## 里程碑

| 阶段 | 交付物 |
|---|---|
| **M0** 可靠性 | 双 runtime 修复、worker teardown/panic 修复、占位语义清除、生产入口测试通过 |
| **M1** 统一计划 | PhysicalPlan arena → executor bridge、统一 ID、Logical/Physical 分离、单构建路径、spec 清理、slot/properties/validator 完成 |
| **M2** 唯一入口 | cache/EXPLAIN/PROFILE 切换到 PhysicalPlan，删除 PartitionedPhysicalPlan 和直接 executor builder |
| **M3** 执行边界 | QueryExecutionInstance、typed state arena 切换、ResultBoundarySpec、分层内存 |
| **M4** 调度 | FragmentSpec DAG、共享 scheduler、scan morsel、通用 Exchange |
| **M5** 资源与图 | spill、RecursiveFragmentSpec、真实 storage 和并发压力测试 |
| **M6** 优化 | benchmark 证明收益的列式化和图数据布局 |

每阶段先完成结构和 validator，再迁移 production facade，最后删除旧路径。禁止长期保留双写、silent fallback 或"新路径失败回退旧 executor"的兼容逻辑。

---

## 完成定义

以下条件全部满足后本文可归档：

- 所有 planner 可生成语义正确执行或被结构化拒绝
- 唯一 immutable、可验证、可缓存的 PhysicalPlan 覆盖所有查询和 command
- 串行和并行消费同一 fragment graph
- 每次执行独占 bindings、state、memory、profile、task group、delivery
- 显式事务由 session controller 持有，语句 scope/auto-commit 边界清晰
- 所有跨 task 通信可计账、可取消、可等待、传播首个错误
- schema/slot/properties/capability/memory policy 执行前验证
- cache/EXPLAIN/PROFILE 以 PhysicalPlan/physical ID 为事实来源
- 语义差分、真实 storage、取消、超限、worker panic、feature matrix 测试通过
- benchmark 证明新增复杂度带来可衡量收益
