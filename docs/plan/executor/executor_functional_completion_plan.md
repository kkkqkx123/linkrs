# Executor 功能补全计划

> 调研日期：2026-07-13  
> 范围：`temp/executor/` 旧实现语义、`crates/graphdb-query/src/query/executor/` 当前实现、planner 到 streaming builder 的 lowering 链路。  
> 本文只列出功能等价性缺口；内存、并行、数据布局等架构演进见 `streaming_current_state_review.md`。

## 一、结论与边界

当前项目已经完成主执行框架的切换：查询通过 `QueryPipelineManager` 进入 `StreamingQueryExecutor`，再由 `StreamingExecutorBuilder` 降级为流式算子树。扫描、过滤、投影、基础聚合、Join、集合操作、图操作、DML 和 DDL 都有结构上的对应分支。

但“每个 `PlanNodeEnum` 有一个 match 分支”不等于功能等价。当前 builder 覆盖全部计划节点，其中 `Loop`、`PassThrough`、`Select` 会显式报不支持；此外若干分支构造了算子，却丢弃计划的输入或关键参数。结论是：**当前 executor 具备可用主路径，但尚未完成对旧 executor 功能语义的迁移。**

`temp/executor/` 是语义参考而非可执行回归基线：它不属于当前 Cargo module tree，且依赖已删除的 `ExecutorEnum`、`admin`、`data_access` 等模块。因此验收必须以新的端到端差分测试为准，不能尝试重新接回旧目录。

## 二、功能缺口清单

| 优先级 | 范围 | 现状与影响 | 处理目标 |
|---|---|---|---|
| P0 | `UNION ALL` | planner 已保存 `UnionNode::distinct=false`，builder 却始终构造去重的 `SetOperator::Union`，重复行被错误删除 | 按 `distinct()` 选择 `Union` 或 `UnionAll` |
| P0 | `Apply` / `PatternApply` / `RollUpApply` | 当前实现不消费右输入；`RollUpApply` 甚至在 builder 阶段丢弃右子树 | 以左右输入、关联键和 apply kind 实现真实关联子查询语义 |
| P0 | 事务输出终止 | Begin、Commit、Rollback 算子没有“已输出”状态；Begin 和 Rollback 会持续返回结果行 | 每个事务语句只产生一个终止结果 chunk，并对接真实事务状态 |
| P1 | 路径与图算法 lowering | 多个路径节点仅消费左输入，目标顶点、边类型、方向、步数等被硬编码或置空 | 完整传递计划配置并消费两侧输入 |
| P1 | DDL 参数保真 | builder 多数只提取名称；空间、标签、索引、用户、全文索引的选项被丢弃或被默认值替代 | 以强类型 command/spec 传递全部计划参数 |
| P1 | 控制流 | `Loop`、`PassThrough`、`Select` 有计划节点但不可执行 | 明确删除不可达节点，或实现并测试其执行语义 |
| P1 | 全文索引 action | `ALTER` 无实现；`show_fulltext_index` 与执行器匹配的 `show_fulltext_indexes` 不一致；非创建操作缺少定位参数 | 统一 action 枚举与 index identity |
| P2 | 索引状态与管理输出 | 旧目录有 `ShowTagIndexStatusExecutor`，当前无专用计划/输出契约；部分 show/create 管理节点也没有 DDL action | 确认语法是否保留；保留时补 plan、builder、执行器与结果 schema |
| P2 | 测试基础 | 多数缺口没有 planner -> builder -> storage 的端到端测试 | 建立按计划节点和语义分类的回归矩阵 |

## 三、P0：集合操作与关联子查询

### 3.1 修复 `UNION ALL`

`SetOperationPlanner` 以 `UnionNode::new(left, right, false)` 表示 `UNION ALL`，该信息已完整保留在逻辑计划中。`StreamingExecutorBuilder` 必须读取 `union_node.distinct()`：

- `true`：构造去重 `SetOperator::Union`；
- `false`：构造顺序拼接的 `SetOperator::UnionAll`；
- 两种模式均要验证列数、列名/schema 与 layout 的一致性；
- `UnionAll` 不应分配去重 hash set，也不应占用 distinct 内存预算。

验收：左右输入含同一行时，`UNION` 返回一行、`UNION ALL` 返回两行；空输入、多 chunk 输入、NULL、嵌套 `Value` 和流式输出均覆盖。

### 3.2 实现双输入 Apply 家族

旧实现的语义是：`Apply` 依据关联列连接左右输入；`PatternApply` 实现 EXISTS/NOT EXISTS 过滤；`RollUpApply` 按 key 从右输入聚合并附加到左行。当前 `ApplyOperator` 打开右子树但不拉取它，因此不能作为等价实现。

建议先定义不可变的 `ApplySpec`，至少包含：

- `apply_kind`；
- 左右输出 schema/layout；
- 关联键表达式或已绑定 slot；
- `PatternApply` 的 anti predicate；
- `RollUpApply` 的 compare columns、collect column、聚合输出列。

执行期创建独立状态：右侧物化/hash 状态、输出迭代位置、内存 reservation。不得把 `apply_kind` 转为字符串常量后当作表达式执行。

验收：标准关联、零关联键笛卡尔组合、Semi、Anti、Single、All、EXISTS、NOT EXISTS、空右侧、多列关联键、重复 key、NULL，以及左/右各多 chunk 的输出均需覆盖。

### 3.3 修复事务算子

为 `TxnOperator` 增加一次性输出状态，或统一为带 `emitted: bool` 的命令状态。`next()` 在输出结果后必须返回 `None`；不能依赖 `mark_closed()` 隐式终止，因为 executor dispatch 不会自动检查该标记。

随后将 Begin/Commit/Rollback 与会话事务上下文关联，至少保证 transaction id、失败回滚、重复提交和取消语义明确。当前仅输出文本行不足以实现事务控制。

验收：对三个语句连续调用 `advance()`，结果必须是“一个 `Some` 后持续 `None`”；`execute_materialized()` 必须结束；提交、回滚和错误路径须验证实际存储可见性。

## 四、P1：图路径节点参数完整 lowering

路径节点的逻辑计划包含双输入及算法配置，physical lowering 不得用默认值替代：

- `ShortestPath`、`BFSShortest`、`AllPaths`：读取左右顶点输入，保留 edge types、方向、最小/最大步数、是否允许环等参数；
- `MultiShortestPath`：传递 source/target、edge types、步数及算法配置，禁止构造空 `target_vertices`、空 `edge_types`；
- 明确同一节点的输入是“顶点集合”还是“已构造路径”，并为不满足输入 schema 的情况返回可诊断错误；
- 算法长循环中定期检查取消状态和内存预算。

验收：单源/多源、定向边、多个 edge type、不同 hop 范围、目标不可达、多个最短路径、环控制、左右输入为空，以及串行与分区退化路径结果一致。

## 五、P1：DDL 与管理命令保真传递

### 5.1 问题模式

当前 DDL builder 将不同 plan node 压缩为字符串 action 和少量名称字段，造成信息丢失。例如：

- `ALTER SPACE` 丢失 `SpaceAlterOption`，并错误地以 space name 作为 comment；
- `ALTER TAG` 调用存储层时传入空的增加/删除字段列表；
- 创建用户、改密码、授予/撤销角色没有传递密码、角色、space 等参数；
- 标签索引创建只构造空 fields/properties 的默认 `IndexConfig`；
- Fulltext 仅在 Create 传递 tag、field、space id，Drop/Describe/Alter 无法定位目标。

### 5.2 改造方式

不要再以 `String action + Option<String> name` 作为 DDL ABI。为每个领域定义强类型 spec，例如 `SpaceCommand`、`TagCommand`、`IndexCommand`、`UserCommand`、`FulltextCommand`，由 builder 无损地从对应管理 plan node 构造。

执行器匹配 command enum，而非匹配字符串。这样可使新增 action 在编译期触发穷尽匹配，避免 `show_fulltext_index` / `show_fulltext_indexes` 一类拼写漂移。

验收：为每种语法执行“创建 -> 描述/显示 -> 修改 -> 验证 -> 删除”的全链路测试；断言存储对象字段与用户输入完全一致，而不仅断言执行成功。

### 5.3 管理功能取舍

需要产品确认并写入 parser/plan/executor 一致性清单：

- `ShowTagIndexStatus` 是否仍是公开语法；若保留，补充独立 plan node 或在 index command 中显式表示可选 index name；
- `ShowCreateSpace`、`ShowCreateEdge`、`ShowCreateIndex`、`ShowUsers`、`ShowRoles` 是否保留；当前计划中存在部分节点，但 DDL dispatch 未完整覆盖；
- 旧 edge index executor 本身会返回“不支持”，因此不应把“实现 edge index 存储能力”误列为旧功能回归；应保持一致、明确的错误契约，或单独规划存储层能力。

## 六、P1：控制流节点处置

`Loop`、`PassThrough`、`Select` 在计划枚举中可见，但 streaming builder 直接报错。应在以下两种策略中选择一种，并保持 parser、planner、optimizer、executor 一致：

1. **删除或禁止生成**：若这些节点只属于废弃 pipeline 设计，删除枚举/visitor/优化规则，或在 planner 阶段返回明确的“不支持该语法”；
2. **实现执行语义**：为每个节点定义输入、循环终止、状态隔离、取消和内存上限；特别是 `Loop` 不能以无界递归方式实现。

验收：没有任何可由 parser/planner 生成的节点会在执行期才因“not supported”失败；若保留 Loop，必须有上限、取消和异常清理测试。

## 七、实施顺序与提交边界

1. 先恢复构建基线：当前正在进行的 immutable spec/state 改造新增了 `operator_spec` 等文件，必须在引用它们前于 `streaming/mod.rs` 注册，或暂时移除未接线的引用。功能补全测试应建立在可编译工作树上。
2. 单独提交 `UNION ALL` 修复与差分测试。
3. 单独提交 Apply 家族的 spec、状态和端到端测试；不要与路径算法或 DDL 混合。
4. 单独提交事务一次性输出和会话事务集成。
5. 按 Space/Tag/Edge/Index/User/Fulltext 分领域改造 DDL command，逐组添加回归测试。
6. 完成路径算法参数 lowering 与测试矩阵。
7. 最后决定控制流节点的删除或实现，并从计划层建立“不存在 silent fallback”的断言。

每个提交都应包含：计划构造测试、builder lowering 测试、至少一个真实存储端到端测试，以及错误/空输入/取消中的一项边界测试。

## 八、完成标准

满足以下条件才可称 executor 已完成旧功能迁移：

- 所有仍可由 planner 生成的计划节点都有明确、可执行的 lowering，或在 planner 阶段被拒绝；
- builder 不丢弃 plan node 的输入、操作类型、过滤条件、关联键或领域参数；
- `UNION ALL`、关联子查询、路径查询、事务和保留的管理命令均有端到端语义测试；
- 同一查询的物化接口和 chunk 流接口结果一致；
- 每个一次性命令型算子在有限次 `advance()` 后终止；
- 在默认 feature 组合及 `fulltext-search` / `qdrant` feature 组合下，测试均能编译并通过。
