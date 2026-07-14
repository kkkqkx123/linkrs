# GraphDB Executor 架构设计

> 日期：2026-07-14
> 修订：2026-07-14，补充计划能力闭包、参数绑定、数据访问、事务、取消、spill 和迁移约束。
> 范围：`graphdb-query` 查询规划结果到查询执行、结果输出的完整边界。
> 定位：本文是 executor 的唯一目标架构规范；代码现状见 `executor_current_gap_analysis.md`，实施顺序见 `executor_remaining_work.md`。

## 一、设计目标

GraphDB executor 面向轻量单机图数据库，采用同步、批量化、pull-first 的执行模型。目标架构必须满足：

1. planner 能把查询语义无损映射为明确的物理计划，不支持的语义在执行前返回结构化错误；
2. 物理计划不可变、可验证、可解释、可缓存，并能并发创建互不共享状态的执行实例；
3. 串行和并行执行消费同一个物理计划，并行不是第二套算子构建路径；
4. schema、slot、ordering、distribution、阻塞性、并行能力和内存策略在执行前可验证；
5. transaction scope、参数、storage binding、cursor、buffer、hash table、memory reservation 和生命周期状态均不进入可缓存计划；
6. 物化结果、chunk stream、HTTP、gRPC 和未来 Arrow 输出通过 ResultSink 或 PullHandle 适配，不改变算子语义；
7. 图遍历、路径算法、DML、DDL、全文和向量查询遵循相同计划、资源、错误和生命周期模型。

非目标：短期不引入分布式执行、async operator API、第三方算子 ABI、全量列式重写或完整 Factorized Table。

## 二、总体架构

```text
Parse / Validate / Logical Plan / Optimize
                  │
                  ▼
          PhysicalPlanBuilder
  choose operators + bind slots + derive properties
                  │
                  ▼
          PhysicalPlanValidator
                  │
                  ▼
        Immutable PhysicalPlan
  operator specs + properties + fragment graph
                  │
             instantiate
                  ▼
        QueryExecutionInstance
 bindings + transaction scope + states + memory + profile
                  │
        ┌─────────┴─────────┐
        ▼                   ▼
 SerialFragmentDriver    QueryTaskGroup
     (one task)       (shared scheduler + morsels)
        └─────────┬─────────┘
                  ▼
        Bound ResultSink / PullHandle
 DataSet / chunk stream / discard / future Arrow
```

职责边界：

- planning 决定采用什么物理算子、数据如何分布以及何处插入 Exchange；
- immutable plan 描述执行方式和正确性契约；
- execution instance 保存本次查询的绑定、资源和状态；
- scheduler service 只运行 fragment/task，查询实例只拥有 task group，不拥有独立线程池；
- 具体 sink 属于执行绑定，只处理结果交付和背压，不改变可缓存计划。

## 三、计划层次

### 3.1 LogicalPlan

逻辑计划只表达查询语义，不表达具体执行算法：

- Join 只保存 join kind、condition 和左右输入；
- Scan 只保存数据访问需求；
- Aggregate、Distinct、TopN 不包含 local/final 拆分；
- Exchange、HashJoin、IndexScan 等物理选择不应与逻辑 variant 混在同一枚举层。

`LogicalNodeId` 在逻辑计划内稳定，用于把优化前后节点和用户查询语义关联起来。

### 3.2 PhysicalPlanBuilder

`PhysicalPlanBuilder` 位于 planning 层，固定执行以下步骤：

1. 根据逻辑语义和统计信息选择 scan、join、aggregate、graph traversal 等物理实现；
2. 分配输入输出 `SlotLayout`，把表达式、join key、filter 和 sort key 绑定为 `SlotId`；
3. 推导 physical properties，并根据输入要求插入 Exchange、Sort、Materialize、partial/final operator；
4. 分配查询计划内唯一的 `PhysicalOperatorId`，记录来源 `LogicalNodeId`；
5. 生成完整且不可变的 `PhysicalPlan`。

该阶段不能使用执行错误作为节点类型路由，也不能以空输入、默认方向、空 edge type、字符串 action 或占位表达式掩盖未实现语义。节点类型路由必须穷尽匹配，新增逻辑节点时应触发编译错误。

### 3.3 Immutable PhysicalPlan

`PhysicalPlan` 是缓存和 EXPLAIN 的唯一计划对象：

```text
PhysicalPlan
  ├── operators: PhysicalOperatorSpec arena
  ├── fragments: FragmentSpec graph
  ├── root_fragment: FragmentId
  ├── output: OutputContract
  ├── compatibility: schema/layout/feature versions
  └── required_capabilities
```

每个物理节点至少包含：

- 唯一 `PhysicalOperatorId` 和可选的来源 `LogicalNodeId`；
- 强类型 `PhysicalOperatorSpec`；
- 输入和输出 `SlotLayout`；
- 输入要求与输出 physical properties；
- 内存、spill、阻塞和并行能力；
- 估算 cardinality 和可解释名称。

物理计划不能持有：

- `StorageClient` 或 transaction/session handle；
- cursor、buffer、hash table 或 accumulator；
- `ExecutionRuntime`、取消状态或 deadline；
- memory tracker/reservation；
- 本次查询参数值或权限上下文。

上述内容由稳定对象标识、parameter slot 和 capability 描述代替，并在实例化时通过 `QueryBindings` 解析。

### 3.4 强类型 OperatorSpec

使用封闭 Rust enum，不引入通用 `dyn PhysicalOperator` 树：

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
  | ResultBoundary(ResultBoundarySpec)
```

DDL、事务、全文和向量管理操作使用强类型 command enum。禁止使用 `String action + Option<String>` 表达操作类型；新增 action 必须触发 planner、builder 和 executor 的穷尽匹配检查。

`ResultBoundarySpec` 只声明稳定的输出 schema、layout、ordering 和是否允许流式交付，不选择 `DataSetSink`、`ChunkStreamSink` 或网络协议。具体 sink 在实例化时绑定，因此相同缓存计划可用于物化、流式和丢弃结果，而不需要复制或改写计划。

## 四、Physical properties

Physical properties 是正确性契约，不是装饰元数据。

| 属性 | 最小取值 | 作用 |
|---|---|---|
| `Distribution` | `Single`、`Partitioned`、`Hash(keys, buckets)`、`Broadcast` | 验证 Join、Aggregate 和 Exchange 输入 |
| `Ordering` | `Unordered`、`Ordered(keys, directions)` | 验证 Sort、TopN、Limit、Merge 和结果保序 |
| `PipelineMode` | `Source`、`Streaming`、`Blocking`、`Sink`、`Exchange` | 划分 fragment boundary |
| `ParallelMode` | `Single`、`MorselParallel`、`PartitionLocal` | 决定 task 数和 local state 范围 |
| `MemoryPolicy` | `StreamingBounded`、`RequiresBudget`、`Spillable` | 防止 blocker 绕过内存预算 |
| `Cardinality` | estimate + optional upper bound | 选择 build side、morsel 和 queue 大小 |

属性推导遵循以下规则：

- Filter 等不改变行布局的流式算子继承 distribution 和 ordering；
- Project 只有在保留对应 slot 时才能继承 ordering/distribution key；
- Sort 输出明确 ordering，blocking 与 distribution 独立表达；
- HashRepartition 两侧使用相同 hash 规则和 bucket count；
- GatherMerge 只接受相同 ordering 的输入；
- FinalAggregate 只接受兼容的 partial state；
- 全局 Limit 不依赖无序 concatenate 的偶然输出顺序。

`PhysicalPlanValidator` 在实例化前验证输入数量、slot/schema、properties、feature capability、事务模式、内存策略和 ID 唯一性。

### 4.1 计划能力闭包

planner、physical builder 和 executor 必须形成能力闭包：每一种可生成的 `PhysicalOperatorSpec` 都必须存在完整、经过契约测试的实例化和执行语义。未完成能力只能采用以下两种状态之一：

1. planner 不生成该 spec，并返回 `UnsupportedFeature` 或 `CapabilityUnavailable`；
2. feature capability 未启用，validator 在实例化前拒绝计划。

禁止使用 `Ok(None)`、空字符串、空 key、默认方向、空 target、虚假成功消息或无操作实现表示“暂未实现”。`None` 只能表达规范明确允许缺省的字段，不能同时承担 unresolved、unsupported 和 empty 三种语义。

项目维护一份由测试生成或静态枚举驱动的 capability matrix，至少覆盖：logical node、physical spec、required feature、transaction mode、parallel mode、spill mode 和对应 executor。新增 logical/physical variant 时，穷尽匹配和 capability 测试必须同时失败，直到实现闭包完成。

## 五、执行实例与状态

### 5.1 QueryExecutionInstance

每次执行 immutable plan 都创建独立实例：

```text
QueryExecutionInstance
  ├── plan: Arc<PhysicalPlan>
  ├── bindings: QueryBindings
  ├── runtime: cancellation, deadline, resource owner, profile
  ├── transaction: TransactionScope
  ├── memory: hierarchical MemoryPool
  ├── global_states: GlobalStateRegistry
  ├── task_group: SerialDriver or scheduler task group
  └── result_delivery: ResultSinkState or PullHandleState
```

`QueryBindings` 保存本次执行的参数、space/data source 解析、权限和 session variables。相同物理计划的多个实例之间不能共享任何可变状态。

参数表达式在 build 阶段编译为 `ParameterSlot`，不能在热路径按名称查找。实例化执行以下步骤：校验参数是否齐全、拒绝未知参数、按声明类型转换、写入不可变 `ParameterFrame`，再把 frame 共享给所有 task。literal 仍属于 plan；prepared parameter value 只属于 binding。缺失、重复、类型不兼容或超出范围必须在 operator open 前失败。

并行 scheduler 是数据库引擎级共享服务，负责全局线程上限和查询间公平性。查询实例只持有可取消、可等待的 task group；关闭查询必须等待该 task group 退出，但不能销毁或 join 共享 worker 线程。

### 5.2 GlobalState 与 LocalState

| 状态 | 生命周期 | 示例 |
|---|---|---|
| Spec | plan，只读 | 表达式、layout、operator 类型、properties |
| Binding | query，只读 | 参数、space、storage、权限、snapshot |
| GlobalState | query + physical operator | hash build table、global aggregate、sort runs、exchange、result collector |
| LocalState | task/worker/morsel | scan cursor、chunk buffer、probe cursor、partial aggregate |

每个 state 通过 `(PhysicalOperatorId, FragmentId, TaskId)` 寻址。registry 使用与 `PhysicalOperatorSpec` 对应的封闭强类型 state enum 或按 operator category 划分的 typed arena，禁止以 `dyn Any`、字符串 key 或不受约束的 downcast 作为主状态模型。算子 enum 不直接混入 spec 和 mutable state；memory tracker 从执行实例的分层内存池派生。

### 5.3 生命周期

保留同步 pull 协议：

```text
New -> Opening -> Open -> Exhausted
                 |          |
                 v          v
              Failed / Stopped -> Closing -> Closed
```

`stop` 和 `close` 在所有适用状态必须幂等。打开失败、取消、客户端断连和 worker 错误均由 instance 统一 teardown。一次性 command 输出一次后必须进入 Exhausted。

## 六、DataChunk、slot 与内存

### 6.1 DataChunk ABI

`DataChunk` 是所有生产算子的批量传输 ABI，并始终携带编译期确定的 `SlotLayout`。算子热路径通过 `SlotId` 访问数据，不按字符串重复解析列位置。空结果同样携带稳定 schema/layout。

近期允许 `Vec<Vec<Value>>` 作为数据主体，但必须满足：

- chunk 转移时显式转移 reservation；
- 生产 `DataChunk` 不提供无条件 `Clone`；需要复制时使用 `deep_copy(pool) -> Result<DataChunk>` 建立新 reservation。测试辅助数据可以使用明确标记的无计账复制接口；
- expression workspace、hash key、sort buffer 和 exchange queue 进入内存账户；
- operator 声明输出是复用、切片、复制还是新分配。

### 6.2 分层内存和 spill

执行实例持有根 `MemoryPool`，向 fragment、operator、task 和 queue 派生子池。阻塞算子必须声明：

- 可持续输出且内存有界；
- 必须整体缓存、不可 spill，超预算时报错；
- 可 spill，并定义临时文件 owner、格式和 cleanup。

优先实现一种可复用的外排 Sort 或 hash partition spill 基础设施，再由 Aggregate 和 Join 复用。

spill 是算子算法的一部分，而不是把内存容器序列化后清空。每个 `Spillable` 算子必须定义写出、读取/归并、取消、校验和恢复内存 reservation 的完整状态机：

- external sort 生成有序 run，最终执行 k-way merge；
- hash aggregate/join 对输入按稳定 hash seed 和 partition count 分区，逐分区装载和处理；
- set operator 保存可重建的 typed row/key，而不是调试字符串；
- 临时文件由 query resource owner 持有，成功、失败、取消、客户端断连和进程启动清理均有明确策略；
- 磁盘空间不足、文件损坏和反序列化失败保留结构化错误，不得退化为缺行结果。

声明为 `Spillable` 的 operator 必须具备端到端测试；否则其 `MemoryPolicy` 必须是 `RequiresBudget`，达到预算后确定性报错。

### 6.3 渐进列式化

只有 profiling 证明收益后，才将 scan、Filter、Project、aggregate key 和 join key 逐步转为列式 vector + validity。Vertex、Edge 和 Path 可先保存 ID 或轻量引用，在属性访问和最终输出时 materialize。

Factorized result 仅在多跳图查询 benchmark 证明重复前缀是主要成本后评估，并限制在 graph fragment 内部。

## 七、Fragment、Exchange 与调度

### 7.1 Fragment 图

物理算子树按 pipeline boundary 划分为 `FragmentSpec` DAG：

- Source 产生 morsel；
- 连续 streaming unary operator 位于同一 fragment；
- Sort、Distinct、HashJoin build、global Aggregate、Window 和 Materialize 形成 blocker boundary；
- Gather、Repartition、Broadcast 和 Merge 形成 Exchange boundary；
- ResultBoundary、DML sink 和事务 command 是 terminal fragment。

串行执行同样消费 fragment 图，只由 `SerialFragmentDriver` 使用一个 task 运行。

### 7.2 Morsel 与 worker pool

可切分 scan 产生多个小 morsel，而不是用一个完整 partition tree 作为最小调度单位。引擎级固定大小、query-aware 的共享 worker pool 动态领取 morsel并创建独立 LocalState。

查询只能向 scheduler 提交 task，算子和 Exchange 不自行创建线程。首个错误、取消和内存超限会关闭上下游 queue，并在返回调用方前等待本查询所有任务退出。

### 7.3 Exchange

最小 Exchange 集合：

- `GatherConcatenate`：合并无顺序要求的输入；
- `GatherMerge`：合并具有相同 ordering 的输入；
- `RepartitionHash(keys, buckets)`：支持 hash join 和分组聚合；
- `Broadcast`：复制小 build side 或常量数据；
- `Barrier`：等待 blocking/global state；
- `Materialize`：仅用于显式重扫或生命周期隔离。

Exchange queue 容量固定、占用可计账，并支持取消和错误传播。

### 7.4 图递归

Expand 可作为普通 streaming fragment。变长路径和多轮 BFS 使用显式 `RecursiveFragmentSpec`，包含 seed、frontier、visited/global state、step bound、终止条件和输出策略，不能由无界通用 Loop 模拟。

## 八、结果、事务与命令

### 8.1 ResultSink 与 PullHandle

物理计划以抽象 `ResultBoundarySpec` 结束；实例化时绑定具体结果交付方式。主动驱动查询时，ResultSink 接收 schema/layout、chunk、完成和错误事件：

- `DataSetSink`：嵌入式物化结果；
- `ChunkStreamSink`：HTTP、gRPC、C API 流式输出；
- `DiscardSink`：EXPLAIN ANALYZE、DML 和 benchmark。

调用方直接 pull 时，`PullHandle` 暴露同一 output contract，并由调用方拉取驱动根 fragment。两种方式共享 operator、fragment、生命周期和错误模型，只允许在执行绑定层转换，禁止为 HTTP、gRPC 或物化结果重建算子树。

sink 或 pull handle 在首个数据 chunk 前发布 schema；空结果也发布 schema。同步 pull 本身形成自然背压；需要跨 async/网络边界时使用有界 bridge queue，队列属于执行实例并进入内存账户。网络背压不得修改 operator 语义。

### 8.2 TransactionScope

transaction 不属于可缓存计划，也不是产生文本行的普通 operator。`TransactionScope` 是本次语句对事务的执行视图，负责：

- 显式和自动提交事务；
- snapshot、读写模式和 transaction id；
- DML/DDL 成功提交、失败或取消回滚；
- storage 与全文/向量同步的一致提交边界。

Begin、Commit、Rollback 可作为 terminal command spec，但实际状态迁移由 TransactionScope 执行。

显式事务可跨越多个查询，因此实际 transaction handle 由 session 级 `SessionTransactionController` 持有；每个 `QueryExecutionInstance` 的 `TransactionScope` 只借用并校验该 handle。自动提交事务由 scope 创建并拥有。查询结束不得自动提交或销毁仍由 session 持有的显式事务，但语句失败必须按既定策略标记事务失败或回滚到语句保存点。

正式事务状态机为：

```text
NoTransaction -> Active -> Committing -> Committed
                      |  -> RollingBack -> RolledBack
                      `  -> Failed -> RollingBack / RollbackOnly
```

BEGIN、COMMIT、ROLLBACK command 只能调用 session controller 和 transaction manager 完成状态迁移，结果行在迁移成功后生成。DML/DDL 不直接提交事务；它们只通过 scope 获取 snapshot/transaction handle。自动提交语句仅在根 fragment、外部索引同步和结果边界均成功后提交；执行失败、取消或 sink 失败时回滚。显式事务中的语句失败按 transaction manager 能力使用 statement savepoint，无法隔离时将事务标为 rollback-only。

### 8.3 DML 与 DDL

DML sink 消费输入 chunk，使用 instance transaction 写入，不直接持有 storage。DDL 使用强类型 command，并明确事务能力、schema invalidation 和 plan cache invalidation。

## 九、缓存、可观测性和错误

### 9.1 Plan cache

只缓存 `PhysicalPlan`，不缓存 state 或 binding。cache compatibility 至少包含：

- query fingerprint；
- schema 和 storage layout version；
- feature/capability set；
- physical planning config；
- 仅当权限或行级策略会改变计划形状时包含对应策略版本。

schema 改变、索引重建、space 切换和 capability 改变必须使计划失效。statistics version 和代价相关配置通常影响计划质量而非结果正确性，应记录为 freshness/replan 元数据：超过阈值时异步或同步重规划，而不是让每次统计更新都强制失效。权限必须在每次实例化时重新校验；只有会改变 scan/filter 形状的行级策略才参与 cache compatibility。

### 9.2 EXPLAIN 与 PROFILE

EXPLAIN 输出完整 immutable plan、slot layout、physical properties、fragment DAG 和 Exchange。PROFILE 以 `(PhysicalOperatorId, FragmentId, TaskId)` 聚合：

- open/next/close、blocked time 和 queue wait；
- 输入输出行数和 chunk 数；
- local/global peak memory、spill bytes/count；
- morsel/task 数和 worker utilization；
- 估计与实际 cardinality 偏差；
- 取消或失败原因。

合成节点使用统一 allocator 分配物理 ID，并标记为 synthetic。禁止硬编码 ID `0`。

### 9.3 错误和取消

计划构建错误至少区分 unsupported node、invalid plan value、expression binding、property mismatch、feature unavailable 和 invalid synthetic ID。执行错误保留首个原因，清理错误不得覆盖原始错误。

deadline、KILL QUERY、客户端断连和内存超限进入同一取消传播路径。

每次执行必须先从进程级 `QueryRegistry` 获得非零且唯一的 `QueryId`，再创建 runtime 和 task group。`QueryContext::mark_killed`、服务端 KILL、deadline、sink disconnect 和 worker failure 都只调用同一个 `CancellationSource`。取消是原因携带的单调状态，首个原因获胜；operator、cursor、Exchange 和 scheduler 只持有只读 token。查询从 registry 移除前必须完成 task wait、operator close、transaction finalize 和 resource cleanup。

## 十、模块边界

推荐按职责组织：

| 区域 | 职责 |
|---|---|
| `planning/logical` | 逻辑计划和逻辑节点 |
| `planning/physical/plan` | immutable PhysicalPlan 和 properties |
| `planning/physical/builder` | 物理算子选择、slot binding 和 property derivation |
| `planning/physical/validator` | schema、slot、properties、feature 和内存策略验证 |
| `executor/instance` | QueryExecutionInstance 与 TransactionScope |
| `executor/state` | GlobalState/LocalState registry |
| `executor/driver` | serial fragment driver |
| `executor/scheduler` | 引擎级共享 worker pool、query task group、task lifecycle 和 morsel 调度 |
| `executor/exchange` | bounded queue、gather、merge、repartition、broadcast |
| `executor/operators` | runtime operators，不可变 spec 的实例化实现 |
| `executor/runtime` | memory、spill owner、profile、cancellation |
| `executor/result` | output contract、pull handle、DataSet、stream、discard 和 future Arrow sink |
| `executor/graph/recursive` | frontier/visited recursive fragment |

公开 facade 可以保持稳定，但内部只能委托给同一 plan/instance/driver 路径。

## 十一、正式实现与迁移约束

目标架构通过逐段替换落地，但不提供查询级 silent fallback：

1. 新旧实现可以在代码中暂时并存，但一个 request 在 plan build 前必须确定唯一执行路径；
2. 新路径 build、validate、instantiate 或 execute 失败时直接返回原始结构化错误，禁止自动转交旧 executor；
3. 每次切换必须先有 old/new differential test，再切 production facade，最后删除旧入口和临时 adapter；
4. `PhysicalPlan` 成为 cache 和 EXPLAIN 的事实来源后，禁止继续缓存或解释另一种 plan；
5. transaction、DML 和 DDL 不允许灰度到缺少真实 transaction handle 的路径；
6. 删除旧路径是每个里程碑的完成条件，不把永久双轨当作兼容方案。

数据访问算子的最低正式契约如下：

| 算子 | open | next | close |
|---|---|---|---|
| `Start` | 创建单例 seed state | 仅一次返回含零列一行的 chunk | 释放 seed state |
| `Argument` | 绑定 correlated input frame | 按 frame 生命周期返回输入行 | 解除 frame 引用 |
| `StorageScan` | 从 transaction snapshot 打开 cursor | 按 chunk/morsel 拉取，空批次仅在 cursor exhausted 时结束 | 关闭 cursor |
| `IndexScan` | 绑定 typed predicate 并打开 index cursor | 流式返回 row id 或 covering row，跳过 stale id 后继续扫描 | 关闭 index cursor |
| `GetProp` | 校验 entity slot 和 property projection | 批量读取并保持输入行基数与 null 语义 | 释放 batch workspace |
| `LookupIndex` | 解析 index id、range 和 projection | 返回 covering data 或批量回表结果 | 关闭 cursor |

这些算子不能通过预先收集全部 ID 实现“流式”接口；storage API 缺少 cursor/batch 能力时，应先扩展 storage contract，再接入 production executor。

## 十二、架构完成标准

以下条件同时满足，才视为目标架构完成：

- 一个 immutable、可验证、可缓存的 `PhysicalPlan` 覆盖所有生产查询；
- logical node 与 physical operator 类型明确分离；
- 每次执行独占 state、transaction、memory、profile 和资源清理范围；
- 串行与并行只是同一 fragment 图的不同调度方式；
- 所有跨 task 通信经过显式、可计账、可取消的 Exchange；
- slot、ordering、distribution、阻塞性和内存策略在执行前验证；
- 图递归、DML/DDL、全文/向量遵循相同 lifecycle、错误和结果契约；
- 客户端结果格式变化只改变执行绑定，不修改或复制可缓存计划和算子树；
- benchmark 和 profile 能证明新增复杂度改善吞吐、首行延迟、内存或并行度。
