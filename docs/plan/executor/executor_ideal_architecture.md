# GraphDB Executor 理想架构设计

> 日期：2026-07-13  
> 范围：`graphdb-query` 的查询执行层。本文定义目标架构和稳定边界；功能缺口见 `executor_functional_completion_plan.md`，LadybugDB 对标与迁移依据见 `ladybugdb_reference_architecture_plan.md`。  
> 定位：单机图数据库的同步、批量化、pull-first 执行器，而不是分布式流处理系统。

## 一、设计目标

目标执行器应同时满足以下要求：

1. **语义完整**：每个可由 planner 生成的逻辑节点都能无损 lower 为物理节点，或在 planner 阶段被明确拒绝。
2. **可复用**：物理计划可缓存、解释、比较和并发实例化，不持有 cursor、buffer、hash table 或生命周期状态。
3. **批量执行**：以带固定 slot layout 的 `DataChunk` 为传输单位，保留当前 pull 模型的低复杂度。
4. **可验证并行**：并行由物理属性和 Exchange 决定，不由算子类型白名单或每个 Gather 自建线程隐式决定。
5. **资源有上界**：内存、临时文件、队列、取消和错误在每次查询范围内统一治理。
6. **输出一致**：物化结果、嵌入式迭代、HTTP SSE、gRPC 和未来 Arrow 仅是不同 ResultSink，不改变算子语义。
7. **图查询一等公民**：Expand、路径、递归和图算法使用同一物理计划、资源和调度模型，不绕过通用 executor。

非目标：短期不引入分布式执行、async operator API、第三方算子插件 ABI、全量列式重写或完整 Factorized Table。

## 二、总体结构

```text
Parse / Validate / Logical Plan / Optimize
                  │
                  ▼
          PhysicalPlanCompiler
       lower + bind slots + validate
                  │
                  ▼
          Immutable PhysicalPlan
  operator specs + properties + fragment graph
                  │
             instantiate
                  ▼
          QueryExecutionInstance
  bindings + transaction + state + memory + profile
                  │
        ┌─────────┴─────────┐
        ▼                   ▼
 SerialFragmentDriver   FragmentScheduler
     (one worker)      (worker pool + morsels)
        └─────────┬─────────┘
                  ▼
              ResultSink
  DataSet / chunk stream / discard / future Arrow
```

物理计划描述“做什么、具有什么属性”；执行实例保存“这一次怎么做、当前做到哪里”；scheduler 只负责运行 fragment/task；sink 只负责接收结果。四者不能互相携带对方的可变状态。

## 三、不可变物理计划

### 3.1 PhysicalPlan 的组成

`PhysicalPlan` 是缓存和 EXPLAIN 的唯一计划对象。它由 operator tree 和 fragment graph 组成，但本身没有任何本次执行的状态。

```text
PhysicalPlan
  ├── operators: PhysicalOperatorSpec arena
  ├── fragments: FragmentSpec graph
  ├── root_fragment: FragmentId
  ├── output: OutputContract
  ├── plan_version: schema/statistics/layout compatibility key
  └── required_capabilities: feature and storage capabilities
```

每个 `PhysicalOperatorSpec` 必须包含：

- `PhysicalOperatorId`：稳定、唯一，保留来源 `LogicalNodeId`；
- `OperatorKind` 和强类型配置；
- 输入和输出 `SlotLayout`；
- 已绑定为 `SlotId` 的表达式、join key、filter、sort key；
- 输入要求和输出 physical properties；
- 内存、spill、阻塞和并行能力；
- 可解释的名称和估算统计。

物理计划不能持有 storage client、session、transaction、参数值、cursor、运行时、内存 reservation 或闭包。它们分别属于 query binding 或执行状态。

### 3.2 强类型 OperatorSpec

保持 Rust enum，而不是建立一个巨型 trait-object 层次。推荐按领域拆分，但在 `PhysicalOperatorSpec` 层形成封闭集合：

```text
PhysicalOperatorSpec
  = Source(SourceSpec)
  | Relational(RelationalSpec)
  | Join(JoinSpec)
  | Set(SetSpec)
  | Apply(ApplySpec)
  | Graph(GraphSpec)
  | Write(WriteSpec)
  | Command(CommandSpec)
  | Exchange(ExchangeSpec)
  | ResultSink(ResultSinkSpec)
```

所有 command、DDL、全文和向量操作均使用强类型字段，不允许通过 `String action` 加若干可选名称表达语义。所有 binary operator 都必须在 spec 中显式拥有左右输入；lowering 不得以空输入、默认方向、空 edge type 或字符串占位代替未完成实现。

### 3.3 Physical properties

每个节点的输出声明下列最小属性，compiler 在连接节点前验证输入要求：

| 属性 | 典型取值 | 作用 |
|---|---|---|
| `Distribution` | `Single`、`Partitioned`、`Hash(keys, buckets)`、`Broadcast` | 选择 Aggregate、Join、Exchange 方案 |
| `Ordering` | `Unordered`、`Ordered(keys, directions)` | 验证 Sort、TopN、Limit、Merge 和结果保序 |
| `PipelineMode` | `Source`、`Streaming`、`Blocking`、`Sink`、`Exchange` | 形成 fragment 边界、表达首行延迟 |
| `ParallelMode` | `Single`、`MorselParallel`、`PartitionLocal` | 决定能否分配多个 local task |
| `MemoryPolicy` | `StreamingBounded`、`RequiresBudget`、`Spillable` | 确保 blocker 不绕过预算 |
| `Cardinality` | estimate + upper-bound hint | 选择 build side、morsel 大小和队列容量 |

属性是 correctness contract，而不只是优化建议。例如 `Gather(Merge)` 只能接收相同 ordering 的输入；最终聚合只能接收 partial state；全局 LIMIT 不得建立在无序并行 concatenate 的顺序假设上。

### 3.4 PhysicalPlanCompiler

编译过程固定为四步：

1. **Lower**：按 scan、relational、graph、write、control 模块将 logical node 无损转成 spec；
2. **Bind**：分配 slot，绑定表达式、常量和 parameter slot，推导 schema；
3. **Optimize physical properties**：插入 local/final aggregate、TopN、Exchange、Merge 等物理节点；
4. **Validate**：检查 slot、输入数量、properties、feature、事务模式和内存策略。

任何节点不支持、参数缺失或 properties 无法满足时，必须在这一步返回结构化错误。生产路径不存在 legacy builder fallback；若需要迁移对照，应通过仅测试可用的 feature gate 显式选择。

## 四、QueryExecutionInstance 与状态模型

### 4.1 每查询实例

每次执行 immutable `PhysicalPlan` 都创建一个独立的 `QueryExecutionInstance`：

```text
QueryExecutionInstance
  ├── plan: Arc<PhysicalPlan>
  ├── bindings: QueryBindings
  ├── runtime: cancellation, deadline, resource owner, profile
  ├── transaction: TransactionScope
  ├── memory: hierarchical MemoryPool
  ├── global_states: GlobalStateRegistry
  ├── scheduler: SerialDriver or FragmentScheduler
  └── result_sink: ResultSinkState
```

`QueryBindings` 保存本次的 space、storage/data source 解析结果、prepared parameters、用户权限和会话变量。它与可缓存物理计划分离，防止计划缓存意外共享 session 或 cursor。

### 4.2 GlobalState 与 LocalState

| 状态类别 | 归属 | 例子 |
|---|---|---|
| GlobalState | 一个查询、一个 physical operator 或一个 exchange | hash join build table、global aggregate table、最终 sort run、result collector、barrier |
| LocalState | 一个 task / worker / morsel | scan cursor、chunk buffer、probe cursor、局部 aggregate、表达式临时区 |
| Binding | 一个查询、只读 | 参数、space、storage 解析、权限、snapshot |
| Spec | plan、只读 | 表达式、slot layout、operator 类型、physical properties |

禁止让 LocalState 直接作为可缓存 plan 的字段；禁止让 GlobalState 被未声明同步策略的多个 task 同时修改。每个 state 通过 `(PhysicalOperatorId, FragmentId, TaskId)` 寻址，便于重复实例化、profile 和故障定位。

### 4.3 生命周期

保留 `open -> advance -> stop -> close` 作为单个 fragment driver 的执行协议，但其状态机必须独立于 spec：

```text
New -> Opening -> Open -> Exhausted
                 |          |
                 v          v
              Failed / Stopped -> Closing -> Closed
```

`stop` 和 `close` 必须在所有非 New 状态幂等；打开失败、取消、sink 断连和 worker 错误均由 instance 触发统一 teardown。一次性 command 在产生一个结果后必须转为 Exhausted，不能仅依赖“已关闭”标记阻止再次输出。

## 五、数据块、slot 与内存模型

### 5.1 DataChunk ABI

生产 `DataChunk` 始终携带不可变 `SlotLayout`，且 layout 在物理编译期确定。算子执行期通过 `SlotId` 访问数据，不再按字符串列名解析。空结果也必须带有输出 layout/schema。

近期仍允许 `Vec<Vec<Value>>` 作为兼容存储；但 schema、layout 和 reservation 的所有权必须唯一且可追踪：

- chunk 转移时显式转移 reservation；
- clone 要么建立新的 reservation，要么被禁止在生产热路径使用；
- operator 输出必须声明是否复用、切片、复制或新分配输入数据；
- expression、hash key、排序工作区和 exchange queue 均进入内存账户。

### 5.2 渐进列式化与图值

按收益顺序将 scan、Filter、Project、聚合键和 join key 转为列式 vector + validity；复杂 Vertex/Edge/Path 先存储 id 或轻量引用，仅在属性访问或最终输出时 materialize。这样可降低图遍历中的重复对象复制，而不要求所有 operator 立刻支持完整 columnar `Value`。

Factorized intermediate result 不是基础 ABI。只有 benchmark 证明多跳路径的重复前缀是主要成本后，才将 factorization 限制在 graph fragment 内部，并通过 `DataChunk` 适配器与通用关系算子对接。

### 5.3 分层内存与 spill

每个 instance 持有根 `MemoryPool`，向 fragment、operator、task 和 queue 派生子池。所有 reservation 自下而上计入根预算。阻塞算子必须在 spec 中明确：

- 可持续输出且有界；
- 必须整体缓存但不可 spill，超预算时失败；
- 可 spill，指定临时文件 owner、分区格式和 cleanup。

优先实现外排 Sort 或 hash aggregate/hash join 中一种通用 spill 路径，随后让其他 blocker 复用，不为每种算子建立互不兼容的临时文件机制。

## 六、Fragment、Exchange 与调度

### 6.1 Fragment 图

`PhysicalPlan` 的 operator tree 按 pipeline boundary 切分为 `FragmentSpec` DAG：

- Source 产生 morsel；
- 连续 streaming unary operator 融合在同一个 fragment；
- Sort、Distinct、HashJoin build、global Aggregate、Window、Materialize 形成 blocker boundary；
- Gather、Repartition、Broadcast、Merge 是 Exchange boundary；
- ResultSink、DML sink、事务 command 是 terminal fragment。

单线程执行也使用同一 fragment 图，只是由 `SerialFragmentDriver` 按拓扑顺序、一个 task 运行。这样并行模式是同一计划的调度策略，不是第二套 executor。

### 6.2 Morsel 驱动任务

可分区 scan 将数据拆为许多小 morsel（id range、cursor range、index range 或 frontier slice），而不是每 partition 复制一棵执行树。全局 worker pool 动态领取 morsel 并创建本 task 的 LocalState。

```text
Source morsels -> streaming fragment tasks -> bounded Exchange
                                               │
               global/blocking fragment <-----┘
                                               │
                                       ResultSink task
```

这能消除当前静态 partition-to-thread 分配的倾斜问题。worker pool 必须按全局资源策略限制线程数；查询只能提交 task，不能在 Gather 内部自行 `thread::spawn`。

### 6.3 Exchange 类型

最小 Exchange 集合：

- `GatherConcatenate`：仅用于无顺序契约的合并；
- `GatherMerge`：输入必须具有相同 ordering；
- `RepartitionHash(keys, bucket_count)`：hash join 与分组聚合；
- `Broadcast`：小 build side、常量和维表；
- `Barrier`：等待 blocking/global state 完成；
- `Materialize`：仅在显式需要重扫或隔离生命周期时使用。

Exchange queue 有固定容量，queue 占用进入 memory pool；正常完成、取消和首个错误都关闭上下游队列。所有 task 收到停止信号后必须退出，scheduler 在向调用方返回前等待任务清理。

### 6.4 图递归

Expand 可作为普通 streaming fragment；变长路径和多轮 BFS 则是显式的 `RecursiveFragmentSpec`，包含 seed、frontier、visited/global state、step bound、终止条件和输出策略。不要用通用无界 `Loop` 节点模拟递归。

visited/semi-mask/bitmap 等图优化属于该 fragment 的 GlobalState；每轮 frontier 可用 morsel 并行，但必须定义重复消除、顺序和内存上界。

## 七、结果、事务与命令

### 7.1 ResultSink

ResultSink 是物理计划的终端节点，统一接收 schema/layout、chunk、完成和错误事件。首批实现：

- `DataSetSink`：嵌入式物化结果；
- `ChunkStreamSink`：HTTP SSE、gRPC、C API 的逐 chunk 输出；
- `DiscardSink`：EXPLAIN ANALYZE、DML、benchmark 的消费器。

sink 在第一个数据 chunk 前发布 schema；空结果也会发布 schema。输出 ordering 由 root property 决定：无序结果可并行收集，固定顺序必须经过 ordered merge 或单一顺序 sink。网络背压应作用于 sink/exchange queue，而不反向修改 operator 语义。

### 7.2 事务和写入

`TransactionScope` 位于 `QueryExecutionInstance`，而不是普通 `TxnOperator` 内：

- 自动事务在 instance 成功时提交、错误或取消时回滚；
- 显式 Begin/Commit/Rollback 修改 session transaction context，并作为一次性 command sink；
- DML/DDL 以强类型 Write/Command spec 执行，参数完整传递；
- 在存储层未证明支持并发写入前，写 fragment 默认单任务、保序执行；
- 未来批量写入可在同一 transaction scope 内分区，但必须由 storage capability 显式开启。

## 八、可观测性、缓存与安全性

### 8.1 Profile 和 EXPLAIN

EXPLAIN 输出 immutable spec、slot layout、physical properties、fragment DAG 和 Exchange。PROFILE 以 `(PhysicalOperatorId, FragmentId, TaskId)` 聚合，至少包含：

- open/next/close 时间、blocked time、queue wait；
- 输入/输出行和 chunk 数；
- local/global peak memory、spill bytes/count；
- morsel/task 数、worker 利用率、取消原因；
- 估计 cardinality 与实际 cardinality 的偏差。

### 8.2 Plan cache

只缓存 `PhysicalPlan`，不缓存 state 或 binding。cache key 至少包含语句 fingerprint、schema/layout version、statistics version、feature set、物理规划配置和权限相关版本。prepared parameters 通过 `QueryBindings` 注入；schema 改变、索引重建、space 切换和 capability 改变必须失效缓存。

### 8.3 错误和取消

任何 operator、task、sink 或 storage error 都记录为 instance 的首个执行错误并触发全局取消；清理错误仅记录日志，不能覆盖原始错误。deadline、KILL QUERY、客户端断连和内存超限走同一取消传播路径。

## 九、推荐模块边界

推荐按职责而非历史 executor 类别组织模块：

| 区域 | 职责 |
|---|---|
| `physical/plan` | immutable PhysicalPlan 与 physical properties |
| `physical/specs` | operator、exchange、sink 的强类型 spec |
| `physical/compiler` | domain lowering、slot binding、property validation |
| `execution/instance` | QueryExecutionInstance 与 TransactionScope |
| `execution/state` | global/local state registry |
| `execution/driver` | serial fragment driver |
| `execution/scheduler` | worker pool、task lifecycle、morsel 调度 |
| `execution/exchange` | bounded queue、gather、merge、repartition、broadcast |
| `runtime` | memory pool、spill owner、profile、cancellation |
| `result` | DataSet、stream、discard、future Arrow sink |
| `graph/recursive` | frontier/visited recursive fragment support |

现有 `streaming` 模块可按此渐进重组；迁移期允许 facade 保持 `StreamingQueryExecutor` 的公开接口稳定，但内部只能委托给新的 plan/instance/driver 边界。

## 十、迁移顺序

1. 先完成功能语义 P0：集合、Apply 家族、事务终止；
2. 将所有生产节点 lower 为无损强类型 spec，补 physical id、slot 和 properties；
3. 引入 `QueryExecutionInstance` 与 GlobalState/LocalState，移除 plan 内的可变状态；
4. 接入 ResultSink、统一事务和 profile；
5. 在同一 fragment 图上实现 serial driver，再引入 Exchange；
6. 为 scan 启用 morsel 和固定 worker pool，逐步并行 Aggregate、TopN、Join、graph frontier；
7. 最后以 profiling 驱动列式化、spill 和图 factorization。

每阶段均要求串行基线、并行路径、物化 sink、流式 sink 的差分测试一致；新 physical properties 必须有正反例验证，不能只依赖集成测试偶然覆盖。

## 十一、架构完成标准

理想架构落地后，应满足：

- 一个 immutable PhysicalPlan 覆盖所有生产查询，并能安全缓存和并发实例化；
- 每次执行独占 state、transaction、memory、profile 和资源清理范围；
- 单线程与并行只是同一 fragment 图的不同调度方式；
- 所有跨 task 通信经过显式、可计账、可取消的 Exchange；
- layout、ordering、distribution、阻塞性和内存策略在物理编译期可验证；
- 图递归、DML/DDL、全文/向量与关系算子遵循相同 lifecycle、错误和结果契约；
- 客户端结果格式的变化不要求改写算子树；
- benchmark 与 profile 能证明每次复杂化确实改善吞吐、首行延迟、内存或并行度。
