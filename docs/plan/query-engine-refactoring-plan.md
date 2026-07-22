# Query Engine 分阶段修改方案

## 1. 文档目标与基线

本文参考以下分析文档，并以 2026-07-22 的现有代码为最终依据：

- `docs/analysis/query/columnar_vectorization_analysis.md`
- `docs/analysis/query/database_design_comparison.md`
- `docs/analysis/query/plan_optimizer_executor_integration.md`

目标是识别当前查询实现仍然存在的问题，明确保留、删除和延后的设计决策，并给出可独立验收的分阶段修改方案。

当前代码已经完成了一部分分析文档提出的迁移，因此不能直接照搬原有问题清单。本文特别区分“已经完成”“只完成表面接线”和“仍未处理”三种状态。

## 2. 总体结论

查询引擎不需要推倒重来。以下主干应保留：

- CSR 边存储与 ColumnStore 属性存储；
- parse → validate → plan → optimize → physical build → execute 的阶段结构；
- pull-based、chunk-at-a-time 的流式执行模型；
- 不可变物理算子配置与每次执行的可变状态分离；
- arena `PhysicalPlan`、`QueryExecutionInstance`、统一 runtime、取消和资源清理机制；
- 保守的单机分区执行策略。

下一轮修改的重点不是全面列式化，而是让已有主干真正形成单一事实来源：

```text
QueryRequest
  -> semantic binding
  -> pure LogicalPlan
  -> logical rewrite
  -> physical selection
  -> validated Arc<PhysicalPlan>
  -> QueryBindings
  -> QueryExecutionInstance
  -> materialized or streaming sink
```

实施顺序必须是：正确性门禁 → 请求与入口统一 → 计划边界收敛 → 可信优化器 → 真正的存储投影下推 → 观测和资源治理。全面向量化只能由 profile 结果触发，不进入当前主线。

## 3. 对原分析结论的现状校正

| 分析文档中的问题 | 当前状态 | 现有代码依据 | 本方案处理 |
| --- | --- | --- | --- |
| 生产执行仍使用递归 `PhysicalNode` | 部分完成 | pipeline 已缓存并执行 arena `PhysicalPlan`，但 `PhysicalPlanBuilder` 仍先构造完整 `PhysicalNode` 再转 arena | 阶段 2 删除中间树 |
| `QueryExecutionInstance` 未接入生产 | 已完成 | materialized 与 streaming 均调用 `QueryExecutionInstance::instantiate_plan` | 保留并补契约测试 |
| 参数未进入 `ExecutionContext` | 部分完成 | pipeline 会复制 `QueryRequestContext.parameters`，但 API 未把 `QueryRequest.parameters` 写入该上下文 | 阶段 0、1 修复 |
| transaction scope 未贯穿执行 | 基本完成 | 已有 `StorageOperationContext`、`TransactionScope`、operation-bound storage 和 stream finalizer | 阶段 1 收敛唯一绑定入口 |
| traversal optimizer 可能覆盖语义方向 | 已修复 | 当前策略显式保留原方向并禁用 bidirectional 改写 | 阶段 0 固化差分测试，后续移除无效策略 |
| heuristic 最大迭代数配置不生效 | 已修复 | engine 会把配置同步到 `PlanRewriter` | 阶段 3 增加 fingerprint 循环检测 |
| cache 保存 `ExecutionPlan` | 已修复 | cache 已保存 `Arc<PhysicalPlan>` | 阶段 1 补全入口、版本和参数类型接线 |
| EXPLAIN 不是物理事实来源 | 部分完成 | EXPLAIN 已展示 `PhysicalPlan`；PROFILE 仍走单独的 `ProfileExecutor` | 阶段 5 统一 |
| ColumnStore 缺少 Validity Bitmap | 已完成 | Column 和多种 encoding 已维护 null bitmap | 不再改造 |
| 扫描未做投影下推 | 仍未真正完成 | query 只在完整 `Vertex` 返回后 `retain`；打开 cursor 时未设置 `ScanOptions.projection` | 阶段 4 完成端到端下推 |

## 4. 现有实现问题

### 4.1 P0：参数绑定在 API 边界丢失

`QueryRequest` 包含 `parameters`，`execute_with_params` 也会构造带参数的请求，但 materialized 和 streaming API 创建 `QueryRequestContext` 时没有复制 `ctx.parameters`。因此 pipeline 的 `build_execution_context` 虽然具备参数传递能力，实际生产入口仍会得到空参数表。

另一个缺口是 `PhysicalPlanBuildContext::from_execution_context` 总是创建空 `ParameterSchema`。这导致：

- 实例化前的缺参、未知参数和类型校验通常不会运行；
- `ParameterFrame` 无法建立，所谓 slot-based 参数路径没有进入生产；
- plan cache 声称按参数类型签名隔离，但 lookup 没有传类型签名，parameterized plan 无法稳定命中；
- 当前参数测试只验证 API 存在，没有执行带参数的真实查询。

这是结果正确性问题，必须先于任何性能优化修复。

### 4.2 P0：现有“投影下推”混淆了语义投影与存储列裁剪

`PushProjectDownScanVerticesRule` 会删除 `Project`，把 projection alias 写进 `ScanVertices.col_names`。物理 builder 再通过字符串是否包含 `.` 等启发式规则推断属性名。

但 `StorageScanVertices` 实际每行仍只产生一个 `Value::Vertex`，并不会产生与多个 alias 一一对应的标量列。与此同时，`DataChunk::new_with_layout` 要求 row width 与 layout width 一致。当前规则因此存在以下风险：

- 普通表达式投影被误当成可消除的 identity projection；
- alias、函数、计算表达式或重命名破坏输出语义；
- 一行一个 Vertex 与多列 layout 不一致；
- release 构建中 debug assertion 不提供保护，错误可能延迟到下游算子。

存储投影的正确语义应是“Vertex 内部只加载后续表达式需要的属性”，而不是“删除负责计算输出的 Project”。在证明等价之前，相关规则不能消除 Project。

### 4.3 P0：缓存与 DDL 失效仍取决于调用入口

`execute_query_with_space` 使用 cache，并用 space/schema/index 维度构造 key；`execute_query_with_request_scope` 和 streaming 主入口不使用 cache。DDL 的 `schema_generation` 更新与 cache invalidation 也只存在于 `execute_query_with_space`。

这会带来两个问题：

- 同一查询通过不同入口执行时，编译、缓存和观测语义不同；
- API 路径执行 DDL 后，另一路径中已有的缓存计划可能没有失效。

此外，DDL 当前在执行前就增加 generation 并失效缓存。失败的 DDL 虽不会导致错误结果，但会产生无必要的全量重编译。失效动作应在 DDL 成功提交后发生。

### 4.4 P1：`LogicalPlan` 仍是包装层，并存在双份 root

当前 `LogicalPlan` 只是 `PlanNodeEnum` 的薄包装；`ExecutionPlan` 同时保存 `root: Option<PlanNodeEnum>` 与 `logical_plan: Option<LogicalPlan>`。`ExecutionPlan::set_root` 只更新前者，optimizer rewrite 后两份 root 可以立即分叉。

`PlanNodeEnum` 本身仍混合：

- 逻辑关系/图算子；
- `IndexScan` 等访问路径；
- `HashInnerJoin` 等物理算法；
- DDL/DML/transaction command；
- 分区相关执行属性。

因此当前不存在可强制验证的 logical/physical 阶段契约，新增 optimizer 规则仍可能跨越语义层和物理层。

### 4.5 P1：arena `PhysicalPlan` 仍由第二棵物理树转译

生产执行虽然以 arena `PhysicalPlan` 为入口，但 builder 当前执行：

```text
PlanNodeEnum -> PhysicalNode tree -> PhysicalPlan arena
```

`PhysicalNode`、arena operator spec、fragment graph 和 properties 仍表达重叠事实。arena builder 中大量字段采用默认值：

- `estimated_cardinality` 全部为 `None`；
- `logical_to_physical` 为空；
- `query_fingerprint` 为 0；
- `required_capabilities` 为空；
- schema layout version 由 space 是否存在映射为固定值 0；
- compatibility check 没有生产调用点。

这说明 validator 虽已接入，但只能验证当前已填充的信息，无法证明缓存兼容性、估算一致性和 logical/physical 映射正确。

### 4.6 P1：optimizer 名称和实际能力不一致

启发式规则数量较多，但仍采用单一顺序列表和节点局部固定点。规则生成的新子树不一定重新经过完整 batch，也没有 plan fingerprint 检测规则振荡。

成本阶段仍只注册 `TraversalDirectionOptimizer`，并且只作用于 root。当前策略为了正确性会保留原方向，因此常见查询中该阶段基本没有物理选择作用。与此同时：

- optimizer statistics manager 默认没有生产统计装载；
- join order、index choice、aggregate strategy 等组件没有接入主链；
- arena plan 不保存 estimated rows/cost/reason；
- 执行 actual rows 没有回灌到 versioned statistics。

当前应明确称为“heuristic optimizer + 保守 partition selection”，不应继续扩展名义上的 CBO 组件而不接生产闭环。

### 4.7 P1：PROFILE 仍绕过统一执行入口

EXPLAIN 已从 arena `PhysicalPlan` 生成描述，但 PROFILE 重新生成 `ExecutionPlan`，再交给单独的 `ProfileExecutor`。该路径自行创建基础 `ExecutionContext`，没有复用统一的 bindings、transaction scope、query registry 和 sink。

`execute_explain_analyze` 虽然会执行 `PhysicalPlan`，主路由仍没有把 ExplainStmt 的 analyze 语义统一到该入口。最终表现是 EXPLAIN、PROFILE 与普通执行无法保证共享相同的 operator id 和 runtime metrics。

### 4.8 P1：投影接口已有三套，但没有形成一个存储契约

当前同时存在：

- `ScanOptions.projection`；
- `PropertyBatchReader`；
- `SourceSpec::StorageScanVertices.projected_properties`。

query source 打开 cursor 时没有把 `projected_properties` 写入 `ScanOptions.projection`，而是在 cursor 返回完整 Vertex 后再次 `retain`。默认 `PropertyBatchReader` 又只是持锁后逐个调用 `get_vertex`，并非 ColumnStore 的批量列读取。

即使把 `ScanOptions.projection` 接上，当前 `GraphVertexCursor` 仍先获得带完整 properties 的 record，再遍历过滤属性；它减少了 Vertex 复制，却未必减少 ColumnStore 解码和读取。需要把投影推进到 VertexTable/ColumnStore 的实际取列点，并删除重复契约。

### 4.9 P2：调度、统计与观测骨架尚未被数据库实例持有

`QueryBindings` 和 runtime 已支持 `SharedScheduler`，但 `QueryPipelineManager::build_execution_context` 从未设置它，多 worker 查询仍可能退回每查询 worker pool。

物理算子 profile、core `StatsManager`、optimizer `StatisticsManager` 和 selectivity feedback 仍是多套状态。它们没有通过 physical operator id 和 catalog version 形成闭环。

### 4.10 P2：执行层全面列式化缺少依据

`DataChunk` 仍是 `Vec<Vec<Value>>`，`get_column` 和 `column_ref` 没有生产调用点。这个实现对简单图查询和不确定基数的 expand 是合理折中，但对大规模 Aggregate/Sort/Join 可能产生 Value clone、递归表达式解释和 cache locality 问题。

目前没有 profile 证明它是主要瓶颈。直接引入双布局 DataChunk、selection vector 或 Arrow 风格 Vector 会扩大所有算子接口和状态空间，因此不进入 P0/P1 阶段。

## 5. 明确修改决策

| 主题 | 决策 | 理由 |
| --- | --- | --- |
| 边与属性存储 | 保留 CSR + ColumnStore | 适合单机图遍历，现有列编码和 null bitmap 已具备良好基础 |
| 执行模型 | 保留 pull-based streaming | 自然支持背压、LIMIT 早停和图展开 |
| DataChunk | 近期保留行式布局 | 当前无 profile 证据支持全面改造 |
| Validity Bitmap | 不在 query executor 重复引入 | storage Column 已实现；行式 `Value::Null` 保持现状 |
| 投影下推 | 必须做，但 Project 默认保留 | 存储列裁剪与结果表达式计算是两个不同语义 |
| 投影描述 | 使用 bind 后的 property identity/slot origin，不使用 alias 字符串启发式 | 避免把计算列、重命名和变量名误判为属性 |
| PropertyBatchReader | 与 cursor projection 合并为一个原生 projected scan 契约；完成后删除未使用的重复接口 | 避免三套能力漂移 |
| LogicalPlan | 建立纯逻辑 operator 表示，删除与 `ExecutionPlan.root` 的双份状态 | 形成 optimizer 可验证输入 |
| PhysicalPlan | arena 表示作为唯一可缓存、可解释、可实例化的物理事实来源 | 已接入生产，继续完成收敛成本最低 |
| PhysicalNode | direct arena builder 完成后删除 | 当前只是重复中间树，不保留兼容层 |
| CBO | 暂时关闭名义上的 traversal CBO；统计可用前只保留等价 rewrite 和保守 partition selection | 避免无统计的伪决策 |
| Plan cache | 缓存 validated `Arc<PhysicalPlan>`，所有入口统一使用 | 保持 immutable plan / mutable instance 分离 |
| PROFILE | 从实际 `QueryExecutionInstance` 收集同一 operator id 的指标 | 消除独立执行路径 |
| 向量化 | 仅在 profile 门槛触发后，先试点单个 blocking operator | 控制复杂度和回归范围 |
| LMDB/COW 事务重构 | 不纳入本计划 | 需要独立存储与事务分析，不能从 query 文档直接决策 |

本项目不要求向后兼容。每个替代阶段完成后，应直接删除旧入口、旧表示和重复 adapter，不保留长期双轨。

## 6. 分阶段实施方案

### 阶段 0：建立正确性门禁并封住已知风险（P0）

#### 修改内容

1. 增加真实 parameterized query 端到端测试，覆盖 embedded/API、materialized/streaming、缺参、未知参数和类型错误。
2. 修复 API 构造 `QueryRequestContext` 时遗漏 `QueryRequest.parameters` 的问题。
3. 在参数 schema 未接通前，不允许把“空 schema”解释为“查询没有参数”；compile 后发现 AST 有参数而 schema 为空应报结构化错误。
4. 暂停会删除 Project 的 scan/get projection pushdown 规则；只保留能够证明是 identity projection 的规则。
5. 增加 optimized vs optimizer-disabled 差分测试，至少覆盖属性投影、alias、计算表达式、OUT/IN/BOTH traversal。
6. 增加 `DataChunk` row width 与 output layout width 的 release-mode 错误检查，不能只依赖 `debug_assert`。
7. 固化当前 `cargo check -p graphdb-query` 基线，并记录现有 warning，不在本阶段顺带清理无关 warning。

#### 验收标准

- 带参数的真实查询能够得到不同参数值对应的正确结果；
- 缺参、未知参数、错误类型在 operator open 前失败；
- Project 不会因所谓投影下推而改变输出列数、alias 或表达式结果；
- traversal optimizer 开关不改变结果；
- `cargo test -p graphdb-query` 与相关 API 集成测试通过。

### 阶段 1：统一请求、编译、缓存与执行入口（P0/P1）

#### 修改内容

1. 定义唯一内部请求对象，显式包含 query text、space identity、session/user、parameter values/types、transaction/snapshot、deadline/cancel、sink policy。
2. 将现有入口归一为：

   ```text
   bind_request
     -> compile_or_get_physical_plan
     -> instantiate(plan, bindings, sink)
   ```

3. materialized、streaming、profile 和 direct-with-space 只负责选择 sink/telemetry，不再复制 parse/validate/compile 流程。
4. 从 semantic binding 生成 `ParameterSchema`，写入 `PhysicalPlanBuildContext`；cache lookup 和 put 使用同一参数类型签名。
5. cache key 至少包含 normalized query、space id/name、schema version、index version、parameter type signature、optimizer/config version和相关 feature set。
6. cache hit 后执行 compatibility check；version 不完整时拒绝写 cache，而不是以 0 代替真实版本。
7. DDL 成功提交后统一更新 catalog generation 并按 dependency invalidation；失败 DDL 不改变 generation。
8. 明确 DML/transaction command 的 cache policy。建议开发阶段只缓存只读查询，待 transaction-dependent validation 完整后再开放 DML cache。

#### 验收标准

- 同一查询通过 materialized/streaming/direct API 使用同一个 compile/cache 主链；
- 参数值变化但类型相同可以命中同一计划，类型变化会重新编译；
- 不同 space 或 schema/index version 绝不共享不兼容计划；
- 任意入口执行成功 DDL 后旧计划失效；
- cache miss/hit、stream collect/materialized 结果完全一致。

### 阶段 2：收敛 LogicalPlan 与 PhysicalPlan（P1）

#### 修改内容

1. 将 `LogicalPlan` 从 wrapper 改为纯逻辑 operator tree/arena，只保留逻辑语义、bound expression、logical schema 和 required properties。
2. 从逻辑层移出 `IndexScan`、Hash/Merge Join 变体、exchange、partition layout、memory policy 等物理选择。
3. DDL/DML/transaction command 使用明确的 command logical node，不与关系查询物理算法混排。
4. 删除 `ExecutionPlan.root` 与 `logical_plan.root` 双份状态；optimizer 只接收并返回一个 `LogicalPlan`。
5. 让 physical converter 直接向 arena 分配 `PhysicalOperatorSpec` 和 fragment edge，不再先构造 `PhysicalNode`。
6. direct builder 同时填充：logical-to-physical mapping、真实 compatibility、required capabilities、estimated cardinality（可为空但必须注明来源）、output contract 和稳定 operator id。
7. direct arena builder 覆盖全部 planner 可生成 shape 后，删除 `PhysicalNode`、旧 tree builder 和重复 partitioned representation。

#### 验收标准

- 编译期类型上不能把物理 join/index variant 放入 LogicalPlan；
- planner/optimizer/physical builder 各只有一个 root 和一个 schema 来源；
- 每个 logical node 可映射到零个或多个稳定 physical operator id；
- EXPLAIN、cache 和 executor 消费同一个 `PhysicalPlan`；
- 全量 planner-to-physical capability matrix 测试通过或返回明确 capability error。

### 阶段 3：重建可信的 optimizer 主链（P1）

#### 修改内容

1. 将启发式规则拆为命名 batch：normalize、predicate pushdown、property pruning、decorrelation、cleanup。
2. 每个 batch 运行 whole-plan fixed point，并用 plan fingerprint 检测循环；记录命中规则和停止原因。
3. 明确区分：
   - logical equivalence rewrite；
   - required property 推导；
   - physical alternative selection。
4. 在统计闭环完成前关闭当前 traversal cost phase，避免把 no-op 策略描述为 CBO。
5. 第一批 CBO 只实现三项：scan/full-index choice、两表 join algorithm、aggregate algorithm。
6. 从 storage/catalog 装载带 space/schema version 的 cardinality、NDV、min/max、index state 和 degree distribution。
7. 每个物理选择保存 estimated rows、cost、统计版本和 decision reason；无统计时使用明确 fallback，而不是伪精确数值。
8. PROFILE 反馈只触发 replan 或更新对应版本统计，不直接无版本修改共享估算。

#### 验收标准

- optimizer-disabled 与 enabled 结果差分测试全绿；
- 规则振荡可被 fingerprint 检测并输出诊断；
- EXPLAIN 能展示每个受选择影响算子的 estimate/cost/reason；
- full scan/index scan、不同 join algorithm 的结果一致；
- 空统计时计划稳定且可预测，有统计时测试能够观察到预期方案切换。

### 阶段 4：完成端到端属性投影下推（P1）

#### 修改内容

1. semantic binding 为每个 scan 计算 `RequiredProperties`，使用 tag/edge identity + property id/name 表达，不复用输出 alias。
2. Project 继续负责表达式计算和最终输出；required-property pruning 只修改 scan 的内部读取集合。
3. `SourceSpec::StorageScanVertices/Edges` 将 typed projection 原样传入 `ScanOptions.projection`，删除 source 返回后的二次 `retain`。
4. 为 VertexTable/ColumnStore 增加 projected row/batch read，使 cursor 只从指定 Column 读取 Value；不能先构造完整 record 再过滤。
5. 顶点扫描仍可输出一个瘦身后的 `Value::Vertex`，保持现有 property-access 表达式语义；不要伪装成多个标量 slot。
6. edge cursor 同步实现 projection，避免只优化 vertex scan。
7. 将 `PropertyBatchReader` 合并到原生 projected cursor/batch API；若没有独立调用场景，直接删除该 trait 和逐点默认实现。
8. 增加读取计数器测试，证明未投影列没有发生 decode/materialize，而不仅是最终 Vertex 中看不到该属性。

#### 验收标准

- 查询只引用 1 个属性时，其他列的读取/解码计数为 0；
- `RETURN p.name`、`RETURN p.name AS n`、`RETURN p.age + 1`、filter + project 组合结果正确；
- 未知属性、nullable 属性和 schema change 行为明确；
- 投影开启/关闭的结果差分一致；
- vertex 和 edge scan 均有端到端覆盖。

### 阶段 5：统一 PROFILE、生命周期与共享资源（P1/P2）

#### 修改内容

1. 删除独立 `ProfileExecutor` 执行主链，PROFILE/EXPLAIN ANALYZE 通过正常 `QueryExecutionInstance` 执行。
2. 在同一 physical operator id 上采集 estimated rows、actual rows、loops、time、memory、spill 和 task 指标。
3. 由数据库实例创建并持有 `SharedScheduler`，pipeline 只为查询创建 task group、quota 和 admission token。
4. query registry 记录真实 session/user/query text/space，KILL、client disconnect、deadline 和 runtime cancel 使用同一 token。
5. 合并 core stats、optimizer stats 和 runtime profile 的数据接口，保留职责分离但统一 identity/version。

#### 验收标准

- EXPLAIN 与实际执行的 operator/fragment 结构一致；
- PROFILE 不重编译第二份不同计划；
- query id 非零且能用于 cancel/KILL 和 profile 对齐；
- 并发查询共享实例级 scheduler，不创建独立线程池；
- drop、错误和 cancel 后 registry、task、memory reservation、transaction finalizer 均被清理。

### 阶段 6：profile 驱动的局部执行优化（P2，可选）

#### 进入条件

只有当真实 workload profile 同时证明以下任一问题占主导时才启动：

- blocking operator 的 Value clone/row traversal 占查询 CPU 的显著比例；
- 大 chunk 的 filter/project 表达式解释成为瓶颈；
- memory profile 显示 `Vec<Vec<Value>>` 是主要占用来源。

#### 候选修改

1. 先为一个算子引入 typed column view，优先 Aggregate，其次 Sort/HashJoin build side。
2. column view 必须有单一所有权与物化边界，禁止长期维护 rows/columns 两份可变真相。
3. 通过 slot-bound expression bytecode 或 compiled evaluator 消除热路径字符串查找。
4. 只有多个算子共享收益后，才评估统一 columnar DataChunk、selection vector 和 validity bitmap。

#### 明确不做

- 不因 DuckDB/Arrow 的通用优势直接改写图遍历算子；
- 不紧凑化当前 frontier 而增加 vertex 回表；
- 不替换已存在的 storage null bitmap；
- 不在单线程、浅遍历场景引入数据级 morsel 调度。

## 7. 阶段依赖与并行关系

| 阶段 | 前置条件 | 可并行工作 |
| --- | --- | --- |
| 0 正确性门禁 | 无 | 参数测试与投影差分测试可并行 |
| 1 入口统一 | 阶段 0 | cache key 与 request binding 可在接口确定后并行 |
| 2 计划收敛 | 阶段 1 的内部请求契约稳定 | direct arena builder 与 pure logical node 设计可分支开发，最终一起切换 |
| 3 optimizer | 阶段 2 | catalog statistics 装载可提前开发 |
| 4 投影下推 | 阶段 0；最好复用阶段 2 的 required properties | storage projected read 可与 logical property pruning 并行 |
| 5 观测治理 | 阶段 1、2 | shared scheduler 可独立推进 |
| 6 局部列式优化 | 阶段 5 profile 数据 | 无固定承诺 |

推荐主路径为 `0 -> 1 -> 2 -> 3 -> 5`。阶段 4 的 storage 侧可在阶段 1 后开始，但 query 侧最终接口应基于阶段 2 的 typed required properties。

## 8. 必须建立的测试门禁

### 8.1 编译与能力闭包

- representative AST 覆盖每种 `Stmt`；
- planner 可生成的每种 logical shape 均能物理化或明确拒绝；
- physical validator 检查 arity、layout、slot type/nullability、ordering、distribution、capability 和 output contract；
- clone/rewrite 后 logical id 稳定且唯一。

### 8.2 结果差分

- optimizer enabled/disabled；
- cache hit/miss；
- streaming collect/materialized；
- serial/partitioned；
- scan/index scan；
- projection enabled/disabled；
- 不同 join/aggregate algorithm；
- OUT/IN/BOTH traversal。

### 8.3 绑定与隔离

- 参数缺失、未知和类型不匹配；
- 不同 space、schema/index version、parameter type；
- compile 与 instantiate 之间发生 DDL；
- explicit transaction、auto-commit、read-only snapshot；
- 全文/向量副作用与 transaction finalize 一致。

### 8.4 生命周期与故障

- parse/validate/rewrite/physical build/open/next/close 故障注入；
- client disconnect、KILL、deadline、worker failure；
- memory exceeded、spill failure；
- streaming handle 未耗尽即 drop；
- 空结果保留正确 output schema。

## 9. 每阶段通用验证命令

```shell
cargo fmt --check
cargo test -p graphdb-query --lib
cargo test -p graphdb-query --test '*'
cargo test --test integration_streaming_executor
cargo check -p graphdb-query
cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant
```

涉及 API、storage 或 feature-specific operator 时，再分别运行对应 crate 测试。阶段 2、3、4 完成后应运行 workspace clippy；阶段 0 不应为了清理无关 warning 扩大修改范围。

## 10. 完成定义

本计划完成时必须满足：

1. 所有查询入口共享同一 request binding、compile/cache、instantiate 和 lifecycle 主链；
2. 参数、space、schema/index version、transaction/snapshot、query id、cancel token 来源唯一；
3. planner 输出纯 `LogicalPlan`，optimizer 不再修改混合物理节点；
4. arena `PhysicalPlan` 是 cache、EXPLAIN、PROFILE 和 executor 的唯一物理事实来源；
5. `PhysicalNode` 和重复 plan root 已删除；
6. 投影下推能够证明减少 ColumnStore 实际列读取，同时保持 Project 语义；
7. CBO 的每个生产决策都有统计版本、估算、理由和差分测试；
8. 行式 DataChunk 是否继续演进，由统一 PROFILE 数据决定，而不是由架构类比决定。

## 11. 主要影响范围

- pipeline 与入口：`crates/graphdb-query/src/query/pipeline/`、`crates/graphdb-api/src/api/core/query_api.rs`
- request/binding：`crates/graphdb-query/src/query/context/`、`executor/streaming/instance.rs`
- logical plan：`crates/graphdb-query/src/query/planning/plan/`
- optimizer：`crates/graphdb-query/src/query/optimizer/`
- physical plan：`crates/graphdb-query/src/query/executor/streaming/plan/`
- legacy physical builder：`crates/graphdb-query/src/query/executor/streaming/operator_plan_builder/`
- scan executor：`crates/graphdb-query/src/query/executor/streaming/operators/source_operator.rs`
- storage cursor：`crates/graphdb-storage/src/storage/cursor.rs`、`storage/engine/graph_storage/cursor_impl.rs`
- ColumnStore：`crates/graphdb-storage/src/storage/vertex/`
- cache：`crates/graphdb-query/src/query/cache/plan_cache.rs`
- EXPLAIN/PROFILE：`crates/graphdb-query/src/query/executor/explain/`、`query/pipeline/diagnostics.rs`
