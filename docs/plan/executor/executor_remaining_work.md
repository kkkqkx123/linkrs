# GraphDB Executor 剩余任务

> 日期：2026-07-13  
> 范围：仅记录尚未完成的 executor 功能和架构工作。  
> 目标架构：见 `executor_ideal_architecture.md`。已完成的基础设施不在本文重复记录。

## 一、当前结论

当前 executor 已具备可编译、可测试的 streaming 主路径，但仍存在会影响查询正确性的语义缺口，以及两套物理构建路径、不可验证属性和状态边界混杂等架构问题。

剩余工作按以下原则排序：

1. 先修复会吞错或返回错误结果的正确性问题；
2. 再统一 immutable physical plan 和实例化路径；
3. 随后建立 execution instance、事务、结果和资源边界；
4. 最后进行 fragment、spill、列式化和图执行优化。

禁止以默认值、空列表、字符串占位或 silent fallback 将未实现语义伪装成成功计划。

## 二、P0：计划构建正确性

### 2.1 修复节点分派和错误吞噬

当前计划转换入口依次调用 scan、relational、graph、write、control、DDL 等领域函数，并把所有 `Err` 当成“该领域不处理此节点”。因此表达式解析、数值转换和子节点构建错误可能被吞掉，最终被错误报告为 unsupported node。

处理要求：

- 将当前 `streaming/lowering` 改为语义明确的过渡模块 `operator_plan_builder`；
- 顶层对 `PlanNodeEnum` 只进行一次穷尽 `match`；
- 禁止顶层和领域路由使用 `_` 捕获新增节点；
- 领域函数接收确定的具体节点类型，`Err` 只表示真实构建失败；
- unsupported、invalid value、expression binding、property mismatch 和 feature unavailable 使用结构化错误；
- 子节点错误保留原始错误类型、node kind 和 node ID。

验收：

- 非法 Limit、Sample 和表达式绑定错误不会变成 unsupported；
- 新增 `PlanNodeEnum` variant 时 builder 编译失败；
- default、`fulltext-search` 和 `qdrant` feature 下均具有穷尽覆盖。

### 2.2 修复 `UNION ALL`

当前已有 `SetSpec::UnionAll` 和运行时算子，但计划构建仍固定生成去重 Union。

处理要求：

- 根据 `UnionNode::distinct()` 选择 Union 或 UnionAll；
- 两侧列数和 layout 不一致时在计划构建阶段失败；
- UnionAll 只顺序拼接，不分配去重 hash set；
- 定义串行和并行情况下的顺序契约。

验收：重复行、NULL、空输入、多 chunk、嵌套值和并行退化路径均有差分测试。

### 2.3 完成 Apply 家族语义

当前 Apply 和 PatternApply 会打开右输入但不消费右输入；apply kind 被转换成字符串表达式，多列关联 key 也可能被替换为占位字符串。RollUpApply 尚未形成完整双输入关联语义。

处理要求：

- `ApplySpec` 保存强类型 `ApplyKind`、左右 layout 和关联 slot；
- Standard、Semi、Anti、Single、All 分别实现明确语义；
- PatternApply 支持 EXISTS/NOT EXISTS，不吞表达式错误；
- RollUpApply 消费左右输入，按 compare key 收集指定右侧列；
- 右侧物化/hash 状态纳入 memory budget；
- 删除字符串 kind 和 `"correlated"` 等占位表达式。

验收：覆盖空右侧、多列 key、重复 key、NULL、左右多 chunk、EXISTS/NOT EXISTS 和所有 ApplyKind。

### 2.4 修复一次性命令终止

Begin、Commit、Rollback 当前可重复返回文本结果。其他 command operator 也必须统一检查一次性输出状态。

处理要求：

- command state 包含明确的 `emitted`/terminal 状态；
- 第一次输出后持续返回 `None`；
- `stop`、`close` 和错误路径幂等；
- 后续 TransactionScope 落地后，事务命令不再自行伪造状态文本。

验收：所有一次性命令在有限次 `advance()` 后终止，物化执行不会无限循环。

## 三、P1：计划参数保真与功能语义

### 3.1 补全图路径计划

当前 BFSShortest、ShortestPath、AllPaths、MultiShortestPath 等节点仍可能只消费左输入，或使用空 target、空 edge types、默认方向。

处理要求：

- 消费逻辑节点声明的全部输入；
- 保留 source/target、edge types、方向、hop 范围、环策略和算法配置；
- 明确输入输出 layout，禁止运行时猜测顶点列；
- 长循环定期检查取消和内存预算；
- 变长路径最终迁移到 `RecursiveFragmentSpec`。

验收：覆盖单源/多源、定向边、多 edge type、hop 范围、不可达目标、环控制和空输入。

### 3.2 将管理命令改为强类型 spec

当前 DDL、全文和向量管理 spec 仍使用 `String action`，并丢失部分 ALTER、用户、角色、索引和数据源参数。

处理要求：

- 定义 `SpaceCommand`、`TagCommand`、`EdgeCommand`、`IndexCommand`、`UserCommand`、`FulltextCommand` 和 `VectorCommand`；
- 无损传递 create/alter/drop/show/rebuild 的所有字段；
- executor 穷尽匹配 command enum，不匹配字符串；
- plan spec 不持有 `StorageClient`；
- schema 变更触发 plan cache invalidation；
- 明确仍保留的管理语法，删除不可达 plan variant。

验收：每个领域至少完成“创建 → 查询 → 修改 → 验证 → 删除”端到端测试，并断言实际对象字段。

### 3.3 处理遗留控制流节点

`Loop`、`PassThrough`、`Select` 仍存在于计划枚举，但执行阶段不支持。

优先选择删除或禁止生成：

- 若 parser/planner 不再需要，删除 node、visitor 和 optimizer 分支；
- 若仍有公开语法依赖，在 planner 阶段返回明确 unsupported error；
- 只有存在确定语义时才实现；Loop 必须有步数上限、取消和内存边界。

验收：任何 parser/planner 可生成的计划都不会在 executor 才首次发现“不支持”。

### 3.4 完成 planner 到 storage 的语义测试

现有单元测试不能替代语义完整性验证。为 Union、Apply、路径、事务和管理命令建立：

- parser/planner → physical plan 测试；
- physical plan → executor tree 测试；
- executor → real storage 端到端测试；
- 物化接口与 chunk stream 差分测试；
- 串行与并行退化路径差分测试；
- 错误、空输入、取消和内存超限测试。

## 四、P1：统一物理计划

### 4.1 缩小计划构建上下文

当前计划构建接收完整 `ExecutionContext`，部分 spec 直接保存 storage 和当前 space。

处理要求：

- 引入只读的 plan build context，只包含 schema、statistics、capability 和稳定数据源标识；
- 参数值、storage、transaction、session、memory 和 cancellation 在实例化时通过 `QueryBindings` 注入；
- 建立 schema/layout/statistics/feature compatibility key；
- 验证同一 plan 的并发实例互不共享状态。

### 4.2 分离 LogicalPlan 与 PhysicalPlan

当前 `PlanNodeEnum` 同时包含 InnerJoin/HashInnerJoin、普通 Scan/IndexScan 等不同层次概念。

处理要求：

- 逻辑节点只表达语义；
- physical builder 选择 HashJoin、IndexScan 等具体算法；
- local/final Aggregate、Distinct、TopN 和 Exchange 只出现在物理计划；
- executor 不再根据逻辑 variant 选择算法。

### 4.3 合并两套物理构建路径

当前普通路径为 `PlanNodeEnum → PhysicalNode → StreamingExecutor`，分区路径又通过 `PartitionedPhysicalPlan` 和 `physical_builder` 直接创建部分运行时算子。

处理要求：

- 统一 `PhysicalNode` 与 `PartitionedPhysicalNode` 的表达能力；
- Gather、Merge、HashRepartition、partial/final Aggregate、Distinct 和 TopN 全部成为 immutable physical node；
- 删除构造 executor 后再 `replace_single_input` 的方式；
- 删除直接从 logical/partitioned node 创建生产 executor 的入口；
- 生产环境只保留 `PhysicalPlan → ExecutorFactory::instantiate`。

验收：串行和分区查询都先生成可 EXPLAIN 的完整 PhysicalPlan。

### 4.4 实现属性推导和验证

当前 `PhysicalProperties` 大量使用 `single_streaming()` 或 `single_blocking()`，没有系统推导和消费。

处理要求：

- 实现 source、unary、blocking、join 和 exchange 属性推导；
- Sort 输出 ordering，Filter/Project 正确继承或失效属性；
- blocking 与 distribution 分离；
- 分区 local 节点不得标记为 Single；
- HashRepartition、GatherMerge 和 FinalAggregate 验证输入契约；
- 删除不被 planner、validator、EXPLAIN 或 scheduler 消费的虚假字段；
- 实现 `PhysicalPlanValidator`。

### 4.5 统一物理节点 ID

当前合成 Start 等节点仍存在硬编码 ID `0`。

处理要求：

- 区分 `LogicalNodeId` 和 `PhysicalOperatorId`；
- 使用统一 allocator 为所有物理节点分配唯一 ID；
- 一对多拆分保留来源 logical ID；
- EXPLAIN 标记 synthetic node；
- PROFILE 以 physical ID 聚合。

## 五、P2：执行实例、状态与事务

### 5.1 引入 QueryExecutionInstance

建立统一的每查询执行边界，持有：

- immutable plan；
- QueryBindings；
- ExecutionRuntime；
- TransactionScope；
- hierarchical MemoryPool；
- GlobalStateRegistry；
- scheduler；
- ResultSinkState。

### 5.2 接入 GlobalState/LocalState

当前 `operator_state.rs` 尚未成为运行时主状态模型，许多 mutable state 仍直接位于 operator enum。

处理要求：

- hash table、global aggregate、sort runs、exchange 和 result collector 进入 GlobalState；
- cursor、probe state、chunk buffer 和 partial aggregate 进入 LocalState；
- state 以 `(PhysicalOperatorId, FragmentId, TaskId)` 寻址；
- operator spec 和 runtime state 不重复保存 schema/layout；
- memory tracker 从 instance memory pool 派生。

### 5.3 建立 TransactionScope

处理要求：

- 显式事务和自动提交使用同一 scope；
- DML/DDL 成功提交，失败、取消或客户端断连回滚；
- storage 与全文/向量同步使用一致提交边界；
- Begin/Commit/Rollback 只触发 scope 状态迁移；
- 重复提交、重复回滚和超时返回结构化错误。

验收：使用真实 storage 验证提交可见性、失败回滚和取消清理。

### 5.4 实现统一 ResultSink

实现：

- `DataSetSink`；
- `ChunkStreamSink`；
- `DiscardSink`。

要求所有 sink 在空结果和错误时提供稳定 schema/终态，并把网络背压转换为 bounded queue 压力或查询取消。

## 六、P2：Fragment、Exchange 与资源治理

### 6.1 建立 FragmentSpec

从统一 PhysicalPlan 按 source、blocking、exchange 和 sink boundary 构建 fragment DAG。串行 driver 和并行 scheduler 消费同一 DAG。

### 6.2 从 partition task 演进为 scan morsel

当前 worker pool 动态领取的主要单位仍是完整 partition tree。

处理要求：

- vertex、edge 和 index scan 提供可切分 morsel；
- 每个 task 创建独立 LocalState；
- 数据倾斜时 worker 能动态领取更多 morsel；
- 查询级资源限制控制线程、queue 和任务数量。

### 6.3 补齐 Exchange

在现有 Concatenate 和 MergeSort 基础上统一实现：

- `RepartitionHash`；
- `Broadcast`；
- `Barrier`；
- 显式 `Materialize` lifecycle boundary。

Hash shuffle join 等特例应收敛到通用 Exchange contract。

### 6.4 完成内存和 spill

当前 `spill_to_disk` 尚未实现。

处理要求：

- instance → fragment → operator → task/queue 分层计账；
- chunk clone 和 transfer 的 reservation 所有权完整；
- expression workspace、hash key 和临时 buffer 纳入预算；
- 先实现可复用的外排 Sort 或 hash partition spill；
- 临时文件由 query resource owner 统一清理；
- PROFILE 记录 peak memory、spill bytes/count。

## 七、P3：布局和图执行优化

以下工作必须在统一计划、slot binding、资源边界和 profile 稳定后实施：

1. 在表达式热路径消除字符串列名查找，只使用 `SlotId`；
2. 对 scan/filter/project、aggregate key 和 join key 选择性列式化；
3. Vertex/Edge/Path 使用 ID 或轻量引用进行 late materialization；
4. 为图递归引入 frontier、visited bitmap 和 `RecursiveFragmentSpec`；
5. 只有 benchmark 证明收益时才评估 factorized graph result。

每项优化必须报告 rows/s、首行延迟、分配次数、peak memory、worker utilization 和 spill bytes，并保持语义差分测试通过。

## 八、推荐实施顺序

1. 补计划构建错误传播测试，改为穷尽分派；
2. 修复 UnionAll、Apply 和一次性命令终止；
3. 补全路径参数和强类型管理 command；
4. 处理遗留控制流节点，建立语义测试矩阵；
5. 缩小 plan build context，统一 physical ID；
6. 分离 logical/physical node，加入 property derivation 和 validator；
7. 合并分区与非分区物理构建路径；
8. 引入 QueryExecutionInstance、Global/Local State、TransactionScope 和 ResultSink；
9. 建立 FragmentSpec，补齐 Exchange 和 scan morsel；
10. 实现分层内存与 spill；
11. 依据 profile 实施列式化和图执行优化。

每个提交只处理一个可验证边界，必须保持构建通过，并包含对应的计划测试、执行测试和错误边界测试。禁止把目录重命名、语义修复和计划类型重写混在同一提交。

## 九、完成定义

以下条件全部满足后，本文可以删除：

- 所有 planner 可生成语义均能正确执行，或在 planner 阶段被明确拒绝；
- 一个 immutable、可验证的 PhysicalPlan 覆盖全部生产查询；
- executor 只有一条实例化路径；
- 串行和并行消费同一 fragment graph；
- transaction、state、memory、profile 和 result 均属于 QueryExecutionInstance；
- 所有跨 task 通信可计账、可取消并传播首个错误；
- schema、slot、properties、feature 和内存策略在执行前验证；
- 语义差分、真实 storage、取消、超限和 feature 测试全部通过。

