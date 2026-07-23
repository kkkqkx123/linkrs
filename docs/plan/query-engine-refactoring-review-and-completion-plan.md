# 查询引擎重构审查与完整收敛方案

## 1. 审查范围

本次审查以提交 `611b8f3`（`executor phase0-1`）为基线，对以下内容进行检查：

1. `611b8f3..HEAD` 的已提交变更；
2. 当前 index 中已暂存但尚未提交的变更；
3. `docs/plan/query-engine-refactoring-plan.md` 定义的目标、阶段依赖和完成标准；
4. 当前代码实际使用的 query pipeline、physical plan、materializer、DML、cache、PROFILE、projection 和 transaction 路径；
5. query crate 的格式、编译、单元测试和集成测试结果。

基线之后只有一个已提交变更 `e8f92fe`，主要是 storage context/spiller 和 storage issue 文档，不是查询引擎重构主体。查询引擎的 direct arena builder、prepared request 草稿、metadata 和 DML 相关变化目前都仍是已暂存改动。因此，不能用 `HEAD` 的提交历史代替对当前 index 的审查。

## 2. 总体结论

当前修改的架构目标基本正确，但实现尚未形成可合入的闭环。

正确方向包括：

- 从 `PlanNodeEnum` 直接构造 arena `PhysicalPlan`，消除 `PhysicalNode` 中间树；
- 引入内部 prepared request，统一 parse、validate、compile/cache、instantiate 和 sink 选择；
- 为物理算子建立 typed `InputContract`、稳定 operator id、capability 和 compatibility metadata；
- 将 DML 输出收敛为单行 `operation/count` 摘要；
- 删除逐点回表的 `PropertyBatchReader`，统一 projected cursor 契约；
- 保留 pull-based streaming，并让 PROFILE/EXPLAIN ANALYZE 使用真实 `QueryExecutionInstance`。

当前主要问题不是目标选错，而是实施顺序违反了原计划的“正确性门禁 -> 入口统一 -> 计划边界收敛”依赖：阶段 1 尚未真正接线，就同时展开阶段 2、4、5；新契约只写入 plan，没有被 runtime 消费；旧路径没有删除；进度文档又把草稿或目标写成已完成事实。结果是静态编译通过，但真实 DML 集成测试大面积回归。

当前状态应判定为：

> direct arena builder 原型已进入生产编译路径，但阶段 0 正确性门禁未通过，阶段 1、2、5 均未完成，不应继续扩展 pure LogicalPlan、optimizer batch 或 typed projection。

## 3. 实际验证结果

### 3.1 已通过

- `cargo check -p graphdb-query`：通过，但有 18 个 query warning；
- `cargo test -p graphdb-query --lib`：1437 passed；
- `cargo test -p graphdb-query --test integration_streaming`：通过；
- `cargo test -p graphdb-query --test integration_pipeline`：95 passed，2 failed，整体失败。

### 3.2 未通过

- `cargo fmt --check`：失败，当前暂存代码尚未格式化；
- `cargo test -p graphdb-query --test '*'`：在 `integration_data_flow` 失败并停止；
- `integration_data_flow`：8/8 失败；
  - 7 项是 DML 输入 chunk 宽度与 `operation/count` 两列输出 layout 冲突；
  - 1 项是 numeric vertex ID 与 String vertex ID schema 的类型冲突；
- `integration_pipeline`：2 项失败；
  - delete statement：numeric vertex ID 与 String schema 冲突；
  - update statement：1 列输入 chunk 被 2 列 sink layout 校验拒绝。

由于集成测试已经失败，尚无必要把 workspace feature check 和全量 clippy 的结果视为当前阶段验收依据。应先修复 P0 回归，再执行完整验证矩阵。

## 4. 关键问题

### 4.1 P0：DML sink 的输入契约与输出契约混用

arena metadata 将所有 `Sink` 的 output layout 固定为 `operation/count` 两列，这是最终目标的一部分。但当前 `SinkOperator::next` 在消费到上游 chunk 时仍执行写入后直接返回原始输入 chunk，只有输入耗尽时才返回 summary。

这造成同一个 operator 声明输出两列，却在执行过程中返回 1 至 5 列不等的输入数据。release-mode chunk/layout 检查正确地把它识别为错误，所以不能通过放宽检查或恢复 padding 规避。

正确实现必须同时满足：

1. DML sink 在一次 `next` 调用中循环拉取并消费上游，不能向下游透传输入 chunk；
2. 输入耗尽后只输出一次严格两列的 summary；
3. 后续调用返回 `None`，不能重复输出 summary；
4. 所有 insert/update/delete/pipe delete/DeleteTags 使用同一 fallible summary helper；
5. summary helper 要求 layout 精确等于 `operation/count`，不能按输入宽度补 `Null`；
6. 写入中途失败时不得输出成功 summary，并必须触发 operation finalizer 的 rollback 路径。

这是当前最直接的行为回归，也是下一步必须优先修复的内容。

### 4.2 P0：`prepared.rs` 是未接线且当前不可直接接线的草稿

`query/pipeline/prepared.rs` 已新增 `PreparedRequest` 和 prepared execute 方法，但 `query/pipeline/mod.rs` 没有声明 `mod prepared;`，因此该文件完全没有参与编译和生产执行。

若直接增加 module 声明，还会出现以下问题：

- 与 `execution.rs`、`compiler.rs` 重复定义 `invalidate_after_ddl`、`is_read_only_cacheable`、`scope_for_request` 和 statement 分类函数；
- 调用了当前不存在的 `plan_cache_context`；
- 调用了当前 `OptimizerEngine` 不存在的 `cache_compatibility`；
- 公开入口仍然继续执行原来的重复 parse/validate/compile 流程；
- direct DML 的 operation storage commit/rollback 并没有被完整迁移到 prepared lifecycle。

因此该文件不能被标记为“入口统一已完成”。正确做法是先定义唯一的数据结构和 ownership，再把旧入口逐个改为薄 adapter，最后删除重复实现，而不是保留两套同名 helper。

### 4.3 P0：typed `InputContract` 没有成为 materializer 的事实来源

builder 已为 operator 填充 `InputContract`，但 materializer 仍然：

1. 按 `FragmentSpec.inputs` 的裸顺序从 map 取 producer；
2. 使用通用 stack 的 `pop` 次序推断 unary/binary/exchange 输入；
3. 当 stack 留下多个 root 时执行猜测性 flatten 和 child swap。

这与类型注释中“materializer uses this instead of fragment inputs order”相矛盾，也与进度文档中“严格消费 contract、拒绝多 root”相矛盾。

当前 validator 也没有完整证明 contract port 与 materializer 消费次序一致。尤其 binary operator 的 left/right、exchange partition member、fragment 内前后算子关系仍依赖隐式顺序。

正确方向是：

- `InputContract` 明确表示 `UnaryInput`、`BinaryInputs { left, right }` 和 `PartitionedInputs`；
- materializer 按 contract 中的 fragment id 精确取 producer；
- fragment 内第一个 operator 消费 external contract，后续 operator 只能消费前一个 operator；
- 每个 fragment 完成后必须恰好产生一个 root；
- 删除 flatten、swap 和“多余 stack 自动拼接”逻辑；
- validator 对 contract、fragment inputs、operator arity、layout 和 root 做同一套一致性检查。

### 4.4 P0：进度文档包含多项与代码不一致的完成声明

`query-engine-refactoring-progress.md` 不能作为当前事实来源。至少以下声明不成立：

| 文档声明 | 当前代码事实 |
| --- | --- |
| prepared request 已开始统一生产入口 | `prepared.rs` 未被 module 引用 |
| `QueryContext::space_name()` 已 fallback 到 request context | 当前只读取 `SpaceInfo` |
| cache key 已加入 optimizer version/config hash | `PlanCacheKey` 仍只有 query、space、schema、parameter type、index version 等字段 |
| pipeline 已持有并注入实例级 `SharedScheduler` | `QueryPipelineManager` 没有 scheduler 字段，`ExecutionContext` 默认仍为 `None` |
| `QueryBindings` 已传递 session/user/query text | 新字段在 `from_context` 中全部被设为 `None`，registry metadata 也写死为 `None` |
| 独立 `ProfileExecutor` 已删除 | 文件、module、re-export 和 diagnostics 调用都仍存在 |
| materializer 已严格消费 typed contract | materializer 仍使用裸 fragment input stack 和猜测性 flatten |
| `PhysicalNode`、`operator_plan_builder` 已删除 | 两个 module 仍存在并参与编译，`ProfileExecutor` 仍依赖旧 builder 路径 |
| DML 已消费完整输入后只输出 summary | sink 仍逐 chunk 透传输入，已导致集成测试失败 |
| summary helper 已返回 `Result` | 当前返回 `DataChunk`，并按 layout 宽度 padding |

后续进度文档必须由测试和可搜索的删除/调用证据更新，不得记录尚未编译的草稿状态。

### 4.5 P1：direct arena builder 的切换不完整

新 builder 已直接遍历 `PlanNodeEnum`，这是符合阶段 2 的改进。但当前仍有以下缺口：

- `PhysicalNode` 和 `operator_plan_builder` 仍被导出和编译；
- `ProfileExecutor` 仍通过旧路径生成另一份计划；
- `PartitionedPhysicalPlan` 仍是重复 planning representation；
- `Loop`、`PassThrough`、`Select`、`AppendVertices` 仍明确 unsupported，需要先证明 planner 不会产生，或补齐 capability matrix；
- direct builder 将 `PhysicalProperties` 大量统一设置为 `single_streaming` 或 `single_blocking`，没有逐 shape 证明与旧 builder 等价；
- fingerprint 由整个 operator 的 `Debug` 字符串计算，不是显式、版本化、可长期稳定的 plan fingerprint；
- capability 只按 operator 大类汇总，尚未与 runtime 实际 feature/capability 做完整 compatibility check；
- cardinality 只有少数 source 的裸 `Option<f64>`，没有 source/version/reason。

因此可保留 direct builder，但暂时不能删除旧 builder。必须先完成 planner-shape 差分测试和行为回归，再一次性切换并删除旧路径。

### 4.6 P1：PROFILE、EXPLAIN ANALYZE 与普通执行仍未统一

当前：

- 普通 EXPLAIN 会重新 compile 内部语句；
- EXPLAIN ANALYZE 会独立 compile 并手动 instantiate；
- PROFILE 仍生成 `ExecutionPlan`，再构造旧 `ProfileExecutor`；
- diagnostics 没有统一复用 `compile_or_get_cached`；
- PROFILE 的 context、transaction、query registry、parameters 和 cache 行为与普通执行不一致。

这违反了“arena `PhysicalPlan` 是唯一物理事实来源”的目标。该问题应在 prepared request 和 direct builder 稳定后修复，不能与当前 DML 修复交叉扩大范围。

### 4.7 P1：cache compatibility 仍不完整

当前 cache lookup 的 key 维度包含 query text、space、共享 generation、parameter type signature 和 index version，但：

- optimizer version、planning config hash 和 feature set 不在 key 中；
- schema/index 仍共用同一个本地 generation；
- compatibility check 只比较 layout version；
- 未绑定 space 时仍可能使用 `Some(0)` generation，不能代表真实 catalog compatibility；
- DDL invalidation 仍是入口相关的 space/all invalidation；
- cache execution stats 尚未统一到完整 context。

应建立单一 `PlanCacheContext`，lookup、put、stats 和 compatibility validator 全部接收同一对象，禁止各调用点临时拼 key。

### 4.8 P1：query identity 和共享资源字段只增加了结构，没有完成绑定

`QueryBindings` 新增 query text、session id 和 user name 是正确方向，但：

- `QueryRequestContext.session_id` 是 `Option<i64>`，bindings 草稿却定义为 `Option<String>`，identity 类型未统一；
- `QueryBindings::from_context` 把三个字段全部设为 `None`；
- query registry 构造 metadata 时也忽略 bindings 中的新字段；
- pipeline 没有持有数据库实例级 `SharedScheduler`。

应先定义统一 `QueryIdentity`，明确 query id、session id、user、space、query text 的类型和来源，再贯穿 request、bindings、runtime、registry 和 profile。

### 4.9 P1：projection 只完成了部分 storage 接线

当前正向变化是 vertex/edge scan 打开 cursor 时会设置 `ScanOptions.projection`，VertexTable 已有 projected row read，删除逐点 `PropertyBatchReader` 也合理。

但仍存在：

- projection identity 仍是 `Vec<String>`；
- semantic binding 没有提供 entity/property/slot identity；
- source 中仍保留未使用的 `projected_properties` 绑定和重复接口痕迹；
- 没有 decode/materialize counter 测试证明未投影列未读取；
- alias、同名属性、多变量和 schema change 的行为没有 typed 保证。

所以阶段 4 只能标记为“storage projected read 原型接通”，不能标记为端到端完成。

### 4.10 P2：当前变更集混入大量非 query 清理

当前 index 同时包含 storage encoding、compression、WAL、transaction、undo、issue 文档删除等大量改动。它们可能各自合理，但会显著扩大 query 重构的验证面和回归定位成本。

后续提交应按 concern 拆分：

1. 原有 storage/transaction 清理单独提交并独立验证；
2. query correctness 修复单独提交；
3. prepared request 单独提交；
4. direct arena builder 单独提交；
5. projection storage contract 单独提交；
6. 文档进度修正单独或随对应实现提交。

不应回滚用户已有修改，但也不应把所有 staged change 作为一个 query refactor 完成单元。

## 5. 分阶段修改方案

以下顺序是强制依赖顺序。前一阶段未满足完成标准时，不得开始下一阶段的生产切换。

### 阶段 A：恢复正确性门禁并建立可信基线

#### 目标

消除当前 direct builder 引入的行为回归，使所有已有 query 测试重新通过，并让进度文档反映真实状态。

#### 修改内容

1. 重写所有 DML sink 的 drain/summary 状态机：
   - 循环消费全部输入；
   - 不透传输入 chunk；
   - 只输出一次 `operation/count`；
   - summary 输出后进入 exhausted；
   - helper 返回 `Result<DataChunk, QueryError>` 并校验精确 layout。
2. 建立 DML capability matrix，覆盖：
   - standalone/pipe insert vertices/edges；
   - standalone/pipe update vertices/edges；
   - standalone/pipe delete vertices/edges；
   - DeleteTags；
   - 空输入、多 chunk 输入、中途 storage error。
3. 修复 vertex ID 类型来源：
   - 从 space/schema 的真实 VID type 绑定；
   - 禁止在 planner、literal source 或 validator 中硬编码 String；
   - String 与 numeric VID 分别测试；
   - 不允许用无条件隐式字符串化掩盖 schema 错误。
4. 给 direct builder 增加 representative DML plan tests，校验 source input layout、sink input contract 和 summary output contract。
5. 修正格式和本轮新增 warning。
6. 将 progress 文档中的错误“已完成”声明改为“草稿/未接线/未验证”。

#### 完成标准

- `cargo fmt --check` 通过；
- `cargo check -p graphdb-query` 通过；
- query lib tests 全绿；
- `integration_data_flow` 8 项全绿；
- `integration_pipeline` 97 项全绿；
- `integration_dml`、`integration_streaming` 全绿；
- DML 每个语句只向 client 返回一行 summary；
- 任一失败写入都走 rollback/finalizer，且不返回成功 summary。

### 阶段 B：完成唯一 prepared request 和执行入口

#### 目标

实现计划中的唯一内部主链：

```text
bind_request
  -> parse_and_validate_once
  -> compile_or_get_physical_plan
  -> instantiate(plan, bindings, sink)
  -> finalize
```

#### 修改内容

1. 将 `prepared.rs` 从草稿改成可编译的唯一实现，不直接叠加 module：
   - 先删除与 `execution.rs`、`compiler.rs` 重复的 helper；
   - 定义 `StatementClass`，集中表达 read-only、DML、DDL、transaction、diagnostic；
   - 定义 `PreparedRequest`，至少包含 query text、validated AST、query context、statement class、transaction scope、operation storage、cache context、query identity、deadline/cancel 和 sink policy。
2. direct-with-space 允许为判断 DML 做一次轻量 parse，但解析结果必须传入 prepared request，禁止再次 parse。
3. materialized、streaming、direct、request-scope、PROFILE、EXPLAIN ANALYZE 只做 binding 和 sink 选择。
4. operation storage 的 commit/rollback 由统一 finalizer 管理；streaming 的正常耗尽、error、cancel、early drop 都必须触发一次且仅一次 finalize。
5. transaction command 明确只允许的 sink/入口策略，并统一 SessionTransactionController 的建立与复用。
6. query identity 从 request 原样写入 bindings、runtime 和 registry，统一 session id 类型。
7. database/pipeline 实例持有共享 `SharedScheduler`，每次查询只创建 task group 和 quota。

#### 完成标准

- 所有公开入口最终调用同一个 prepared execute 方法；
- query text 只 parse/validate 一次；
- materialized 与 streaming collect 的结果和错误一致；
- transaction、DDL、DML 的 finalize 次数有计数测试；
- stream 正常耗尽和 early drop 都清理 registry、memory、task 和 operation storage；
- registry 中能观察到真实 query id、session、user、space 和 query text。

### 阶段 C：完成 cache context 与 compatibility 闭环

#### 目标

确保任何 cache hit 都能证明计划与当前 request/catalog/runtime 兼容。

#### 修改内容

1. 定义唯一 `PlanCacheContext`：
   - normalized query；
   - space identity；
   - schema version；
   - index version；
   - parameter type signature；
   - optimizer version；
   - planning config hash；
   - feature/capability set。
2. lookup、put、execution stats 和 invalidation 使用同一 context，不再提供会拼出缺维 key 的旧 helper。
3. semantic binding 生成 `ParameterSchema`；缺参、未知参数、类型错误必须在 operator open 前失败。
4. cache hit 后检查完整 compatibility；任一 version 不可用时禁止写 cache。
5. schema 与 index version 从真实 manager 获取，不再共享本地 generation。
6. DDL 只在成功 commit 后更新对应 version，并按 dependency invalidation；失败 DDL 不改变 version。
7. 开发阶段只缓存只读查询。

#### 完成标准

- 同类型不同参数值命中同一 plan；
- 参数类型、space、schema、index、optimizer config 或 feature 变化必定 miss/replan；
- 失败 DDL 不失效 cache，成功 DDL 从所有入口正确失效；
- cache hit/miss 与 cache disabled 结果差分全绿。

### 阶段 D：严格化 arena fragment/input contract

#### 目标

让 arena `PhysicalPlan` 成为 materializer 可机械消费、validator 可证明的唯一物理事实来源。

#### 修改内容

1. 重新定义 external fragment port：binary 明确 left/right，exchange 明确 partition id 和 side。
2. materializer 按 `InputContract` 精确取 producer，不读取裸 inputs 顺序推断语义。
3. fragment 内 operator 采用严格线性规则：
   - 首算子消费 external input；
   - 后续算子只消费前一算子；
   - source 不得同时拥有 external input；
   - fragment 结束必须恰好一个 root。
4. 删除 materializer 的多 root flatten、child swap 和 stack 猜测。
5. validator 同时检查 arity、port identity、layout、type/nullability、fragment reachability、root、state ownership 和 capability。
6. 为每种 planner shape 建立 capability matrix；unsupported shape 必须证明 planner 不生成，或返回稳定结构化 capability error。
7. physical properties 从 node/spec 真实推导，不能统一套默认值。
8. 使用显式版本化 fingerprint 结构，禁止以完整 `Debug` 字符串作为长期 cache fingerprint。

#### 完成标准

- 故意交换 join left/right、遗漏 producer、增加多 root 的测试会被 validator 拒绝；
- materializer 中不存在 flatten/swap 猜测代码；
- representative planner shape 的旧/新 builder 结果差分全绿；
- serial/partitioned、stream/materialized 结果一致；
- EXPLAIN 展示的 operator/fragment 与实际 runtime identity 一致。

### 阶段 E：切断旧 physical/profile 路径

#### 目标

在 direct builder 经过阶段 D 验证后，一次性删除重复物理事实来源。

#### 修改内容

1. PROFILE 和 EXPLAIN ANALYZE 通过 prepared request 获取同一个 cached/compiled `Arc<PhysicalPlan>`。
2. runtime profile 按 `PhysicalOperatorId + partition id + catalog version` 回填同一 plan description。
3. 删除：
   - `ProfileExecutor`；
   - `PhysicalNode`；
   - `operator_plan_builder`；
   - 相关 public re-export、测试和 adapter。
4. 评估并删除或重构 `PartitionedPhysicalPlan`，避免保留第二份 physical representation。
5. 编译期禁止新增代码依赖被删除类型。

#### 完成标准

- `rg` 搜索不到生产 `PhysicalNode`、`ProfileExecutor`、`operator_plan_builder` 引用；
- EXPLAIN、PROFILE、cache 和 executor 使用同一个 arena plan；
- PROFILE 不发生第二次 planning；
- operator id 与 actual rows/time/memory/spill 指标稳定对齐。

### 阶段 F：建立 pure LogicalPlan

#### 目标

完成原计划阶段 2 的逻辑/物理类型边界，而不只删除中间物理树。

#### 修改内容

1. 定义纯 logical operator tree/arena，只包含逻辑语义、bound expression、logical schema 和 required properties。
2. command logical node 与 relational logical node 分开。
3. 将 IndexScan、Hash/Merge Join、exchange、partition 和 memory policy 移出逻辑 enum。
4. planner 只返回一个 `LogicalPlan`；optimizer 只接收并返回该类型。
5. 删除 `ExecutionPlan` 中残留的重复 root/partition physical 职责。
6. direct physical converter 从 logical node 和 required properties 选择并分配 physical spec。

#### 完成标准

- 类型系统阻止物理算法进入 LogicalPlan；
- planner、optimizer 和 physical builder 各只有一个 root/schema 来源；
- logical-to-physical mapping 完整且有稳定 id 测试；
- 所有现有查询结果保持一致。

### 阶段 G：可信 optimizer batch

#### 目标

在 pure LogicalPlan 上建立可诊断、可收敛的 optimizer，而不是扩展当前混合节点规则。

#### 修改内容

1. 拆分 normalize、predicate pushdown、property pruning、decorrelation、cleanup batch。
2. 每个 batch 使用 whole-plan fixed point、显式 fingerprint、iteration limit 和 stop reason。
3. 记录 rule hit、before/after fingerprint 和循环诊断。
4. 统计闭环完成前保持名义 CBO 关闭。
5. 首批 physical choice 仅接 scan/index、两表 join、aggregate，并保存 statistics version、estimated rows、cost 和 reason。

#### 完成标准

- optimizer enabled/disabled 结果差分全绿；
- 规则振荡能稳定检测；
- 空统计 fallback 稳定，有统计时能观察到预期 plan 切换；
- EXPLAIN 展示 estimate/cost/reason/version。

### 阶段 H：typed property projection

#### 目标

完成真正的 storage 列裁剪，同时保持 Project 的表达式和输出语义。

#### 修改内容

1. semantic binding 生成 typed `RequiredProperty`：entity identity、property identity、source slot、schema version。
2. scan spec 和 storage cursor 使用 typed projection，不再依赖 alias/name 启发式。
3. Project 始终负责最终表达式计算和 alias，不因 projection pruning 被错误删除。
4. VertexTable/ColumnStore 和 edge property path 只读取请求列。
5. 删除 query/source/storage 中剩余的重复 projection 字段和 post-read retain。
6. 增加 per-column decode/materialize counter 测试。

#### 完成标准

- 未投影列 decode/materialize 次数为 0；
- alias、计算表达式、nullable、同名属性、多变量均正确；
- vertex/edge projection enabled/disabled 结果差分一致；
- schema version 变化后旧 typed identity 不可复用。

### 阶段 I：全量观测、故障与 workspace 验收

#### 目标

证明重构在正常、并发、取消、错误和 feature-specific 场景下都完成闭环。

#### 修改内容

1. 统一 core stats、optimizer stats 和 runtime profile 的 identity/version 接口。
2. 覆盖 KILL、deadline、client disconnect、worker failure、memory exceeded 和 spill failure。
3. 覆盖 explicit transaction、auto-commit、read-only snapshot、stream early drop。
4. 检查 feature-specific fulltext、qdrant、grpc、c_api 路径。
5. 清理所有 transition/deprecated 字段、旧 helper、dead code 和临时文档声明。

#### 完成标准

按顺序全部通过：

```shell
cargo fmt --check
cargo check -p graphdb-query
cargo test -p graphdb-query --lib
cargo test -p graphdb-query --test '*'
cargo test --test integration_streaming_executor
cargo check --workspace --features server,fulltext-search,c_api,grpc,qdrant
cargo clippy --all-targets --all-features
```

同时满足：

- 无 query refactor 新增 warning；
- 无未清理 registry/task/memory reservation/transaction finalizer；
- 无旧 plan/profile 生产引用；
- 所有 capability matrix 和结果差分测试全绿。

## 6. 每阶段提交与回归规则

为确保修改能够正确完成，每个阶段都必须遵守以下规则：

1. 一个提交只完成一个可独立验收的阶段或子阶段；
2. 替代路径接线和旧路径删除应在同一阶段完成，禁止长期双轨；
3. 新结构只有在被生产入口引用、参与编译并有行为测试后，才能写入“已完成”；
4. 不允许通过删除失败测试、放宽 layout/type validator 或恢复 debug-only assertion 解决回归；
5. 每个 bug 先增加最小失败测试，再修复实现；
6. 每个阶段先跑定向测试，再跑 query 全量测试；
7. 涉及 storage/API/transaction 时必须追加对应 crate 或 workspace feature 验证；
8. progress 文档记录命令、通过数量、失败项和对应 commit，不使用“曾通过”替代当前结果；
9. 阶段失败时只修复本阶段，不继续叠加后续架构改动；
10. 所有删除操作先用 `rg` 证明没有生产引用，再删除 module/re-export/file。

## 7. 推荐的近期执行顺序

当前应立即按以下顺序推进：

1. 停止扩展 pure LogicalPlan、optimizer 和 typed projection；
2. 完成阶段 A：DML drain/summary、VID type、格式和现有集成测试；
3. 将 progress 文档纠正为实际状态；
4. 完成阶段 B：把 prepared request 真正接入并删除重复入口逻辑；
5. 完成阶段 C：统一 cache context；
6. 完成阶段 D：让 materializer 真正消费 typed contract；
7. direct builder capability matrix 全绿后执行阶段 E，删除旧 physical/profile 路径；
8. 再开始 pure LogicalPlan、optimizer、typed projection 和最终观测治理。

这个顺序与原计划的依赖关系一致，也能将当前最大风险从“多条半完成主链相互漂移”收敛为“每个阶段只有一个可验证事实来源”。

## 8. 最终完成定义

只有同时满足以下条件，查询引擎重构才能标记完成：

1. 所有入口共享唯一 prepared request、compile/cache、instantiate 和 lifecycle；
2. 参数、space、schema/index version、transaction、snapshot、identity、cancel 来源唯一；
3. planner 输出 pure LogicalPlan，optimizer 不操作混合物理节点；
4. arena PhysicalPlan 是 cache、EXPLAIN、PROFILE 和 executor 的唯一物理事实来源；
5. materializer 完全按 typed contract 工作，不存在顺序猜测和多 root flatten；
6. PhysicalNode、operator_plan_builder、ProfileExecutor 和重复 partition representation 已删除；
7. 所有 DML 正确消费输入、只输出一次 summary，并在错误/取消/drop 时正确 finalize；
8. projection 能证明减少 ColumnStore/edge property 的实际读取；
9. optimizer 的生产决策有统计版本、估算、理由和结果差分测试；
10. 本文阶段 I 的所有验证命令和生命周期/故障测试全部通过。
