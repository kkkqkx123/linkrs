# EXISTS / IN / NOT IN 正式实现设计详案

> 状态：设计方案（2026-08-12）。本文针对
> `docs/plan/query_exists_in_subquery.md` 第 7 节「仍未实施」部分
> （binder→PatternApply 转换、运行时兜底、相关子查询逐行重执行）
> 给出正式实现方案。概览文档的第 1-6 节为背景，不再重复。

## 0. 结论摘要

| 阶段 | 内容 | 决策 |
|------|------|------|
| P0 | PatternApplyNode 双侧键重构（`key_cols` 占位符约定 → `hash_keys`/`probe_keys` 双侧表达式） | **推荐重构**：统一运行时与 unnest 约定，删除 645 行 `replace_all_variables` |
| P1 | binder 放行 + WHERE 合取位置提取 → PatternApply → 既有 SemiJoin/AntiJoin | 主路径，覆盖绝大多数用例 |
| P2 | 相关子查询逐行重执行（CorrelatedApply + correlation frame + 右子树重建） | 键提取失败的兜底，复用既有 `Argument`/`correlation_frame` 半成品 |
| P3 | 表达式级 `execute_subquery` 兜底（OR 位置 / RETURN 中 EXISTS） | 可选；或对非合取位置报精确错误 |

## 1. 核查结论（代码事实，均经确认）

### 1.1 生产链路现状

- **binder 拦截**：`binder_impl.rs:558-577` 对 `Expression::Exists` /
  `Expression::In` 直接返回错误，任何含 EXISTS/IN 的查询在
  `pipeline/frontend.rs` 的 `bind_parsed_statement` 即失败，
  **到达不了规划层**——这是第一道闸。
- **规划层是 AST 驱动的**：`MatchStatementPlanner::plan_bound`
  （`match_statement_planner.rs:161-190`）实际仍走
  `validated.stmt()`（AST），where 条件即 AST 层的
  `ContextualExpression`（`where_clause_planner.rs:78-87`）。
  → **转换应发生在规划层（对 AST 表达式树做变换），binder 只负责
  放行 + 语义验证**。`BoundExpression::Exists/In`（`bound.rs:88-89,136`）
  声明已存在但从未被生产，本次绑定后其消费方仍是空，不构成破坏。
- **PatternApply 物理节点无生产构造者**：`PatternApplyNode` 仅测试
  构造（`subquery_unnesting.rs:751/783/804`、`decorrelation.rs:142/160`）；
  `PlanNodeFactory::create_pattern_apply` 无调用方；
  `LogicalPatternApplyNode` 全仓库无构造点；
  `physical_planner.rs:685-703` 的转换路径右输入被 `StartNode` 占位
  （死代码）。**破坏面 = 仅测试代码，重构安全**。
- **运行时 PatternApply 有约定缺陷**：`apply_operator.rs:175-225`
  用**同一组** `key_expressions` 对左右行分别求值
  （`evaluate_join_key`，`join_helpers.rs:11-27`），而 unnest 的约定是
  每侧变量名不同（`build_semi_join_from_pattern_apply` 用
  `replace_all_variables(key, left_var/right_var)` 拆成
  `hash_keys`/`probe_keys`）。即：**节点层约定与执行层行为不一致**——
  按节点层约定构造的 PatternApply 到运行时必然键解析失败。
- **unnest 链路已完备**：`UnnestSimplePatternApplyRule`
  （`heuristic/decorrelation.rs:34-95`，注册于 `rule_enum.rs:387-388`，
  `OptimizationBatch::Decorrelation`）+ CBO
  `unnest_subqueries`（`optimizer/engine.rs:952-1034`）→
  `SemiJoinNode`（运行时 `cross_semi_join.rs` 已实现）。
- **相关子查询运行机制半成品**：`ExecutionRuntime::set_correlation_frame`
  / `take_correlation_frame`（`runtime.rs:1146-1160`）+ `Argument` 源头
  算子（`source_operator.rs:344-359`，`SourceSpec::Argument` 已注册）
  + `ArgumentNode`（`control_flow.rs`）。**无任何生产使用者**（仅测试）。
  注意 `take_correlation_frame` 是**一次性 take 语义**，且
  `SourceSpec::Argument` 的布局元数据为空
  （`arena_builder/metadata.rs:626`）——P2 需要显式给 Argument 配
  外层布局。
- **子查询 pattern 是字符串**：`SubqueryBody.patterns: Vec<String>`
  （`graphdb-core/types/expr/def.rs:187-200`），而 pattern 规划
  （`pattern_planner.rs::plan_path_pattern`）接收 `Pattern` AST——存在
  **字符串 → AST 再解析**缺口。

### 1.2 语义核对（空键 PatternApply = 非相关 EXISTS）

`keys_match(&[], &[])` 恒真（`apply_operator.rs:399-404`）：空键时
左行存在 ⇔ 右表非空，恰为非相关 EXISTS / NOT EXISTS 语义；unnest
后空 `hash_keys`/`probe_keys` 的 SemiJoin 退化为 cross-semi-join，
语义一致。**非相关子查询零键直接可用**。

## 2. P0：PatternApplyNode 双侧键重构（推荐，先行）

### 2.1 动机

占位符约定（`key_cols` 中写 `_`，`left_input_var`/`right_input_var`
两侧重命名）存在三个缺陷：

1. 运行时无法按节点约定执行（§1.1），必须靠 spec 构建期补一次替换；
2. 只能表达「两侧表达式同形、仅变量名不同」的键
   （如 `p.age = t.age + 1`、`IN` 的 `return_expr ≠ 左表达式`
   均无法表达）；
3. unnest 需要 645 行递归 `replace_all_variables`（`subquery_unnesting.rs:479-645`）。

重构后：节点直接携带 `hash_keys`（外层侧键表达式）与
`probe_keys`（子查询侧键表达式），与 `SemiJoinNode` 的
`hash_keys`/`probe_keys` 完全同构。

### 2.2 改动点

| 文件 | 改动 |
|------|------|
| `graph_operations_node.rs:483-525` | `PatternApplyNode`：`key_cols` → `hash_keys`/`probe_keys`；删除 `left_input_var`/`right_input_var`（其唯一用途就是键替换）；`is_anti_predicate` 保留 |
| `logical_nodes/graph_ops.rs:52-61` | `LogicalPatternApplyNode` 二进制化（仿 `LogicalApplyNode`：`left`/`right`/`deps` 三字段模式），字段同上；`deps = [left, right]` |
| `physical_planner.rs:685-703` | 右输入改为 `deps[1]`（删 StartNode 占位），键直通 |
| `plan_node_factory.rs:299-308` | `create_pattern_apply` 签名同步 |
| `subquery_unnesting.rs:415-462` | `build_semi_join_from_pattern_apply` 变为直通：`SemiJoinNode::new_semi(left, right, apply.hash_keys(), apply.probe_keys())`；**删除 `replace_all_variables` 整个函数** |
| `specs.rs:1158-1169` + `apply_operator.rs:22-28` | `ApplySpec::PatternApply { hash_keys, probe_keys, anti }`；执行循环左侧 `evaluate(hash_keys, left_layout)`、右侧 `evaluate(probe_keys, right_layout)`（改一处 `join_helpers::evaluate_join_key` 的调用参数） |
| `decorrelation.rs:86-91` | 不变（调 `build_semi_join_from_pattern_apply`） |
| 测试 | `subquery_unnesting.rs:774-800`、`decorrelation.rs:132-174` 构造同步（`test_unnest_produces_semi_join_with_split_keys` 的占位符用例改为双侧键直断） |

### 2.3 验证

- 运行时单测：双侧键 PatternApply 与 SemiJoin 对同一数据输出一致
  （含 anti；空键非相关 EXISTS 用例）。
- unnest 单测：双侧键直通后 `SemiJoinNode.hash_keys == PatternApply.hash_keys`。
- 回归：`cargo test -p graphdb-query`（基线 1430 lib）、clippy 全 features。

## 3. P1：binder 放行 + WHERE 合取位置转换（主路径）

### 3.1 binder 放行（`binder_impl.rs:558-577` 替换）

```rust
Expression::Exists { body } => {
    let query = self.bind_subquery_body(body, None)?;   // child scope
    Ok(BoundExpression::Exists { query: Box::new(query) })
}
Expression::In { expr, subquery, negated } => {
    let e = self.bind_inner_expr(expr, None)?;
    let q = self.bind_subquery_body(subquery, None)?;
    Ok(BoundExpression::In { expr: Box::new(e), subquery: Box::new(q), negated: *negated })
}
```

- `bind_subquery_body`：push child scope（`BinderScope::with_parent`，
  `scope.rs:28-33`）→ `build_query_graph` 注册子查询变量 →
  绑 where/return（变量在父 scope 命中即**相关变量**，记录于
  `BoundStatement` 或直接忽略——规划层会重做相关性分析）→ pop scope。
- `expr_converter.rs:280-284` 的转换拒绝可保留（规划层不用 bound 树），
  但错误信息改为「bound 树仅用于验证」。
- 语义验证覆盖：子查询变量类型/属性经 catalog 解析。

### 3.2 子查询 pattern 字符串 → `Pattern` AST（解析期规范化）

`SubqueryBody.patterns` 是 lexeme 拼接串。方案：

- **解析期规范化**：`parse_subquery_body`（`expr_parser.rs:992-1026`）
  得到字符串后，立即用 traversal 解析器
  （`traversal_parser.rs:457::parse_pattern`）round-trip 校验；
  失败即解析错误；成功则把规范化串存回 `SubqueryBody.patterns`
  （保证任何存库字符串均可再解析）。
- **规划期再解析**：exists 规划器对每个 pattern 串调用
  `parse_pattern` 得 `Pattern`，走既有 `plan_path_pattern`
  （`pattern_planner.rs:126`）。
- 备选（round-trip 有坑时）：binder 期解析并缓存于 AST
  `ExpressionAnalysisContext` 的旁路表（按表达式 id 索引）。

### 3.3 exists 规划器（新模块 `planning/statements/clauses/exists_planner.rs`）

`WhereClausePlanner::transform_clause`（`where_clause_planner.rs:44-75`）
改造为：

```
输入：SubPlan（外层计划 root）+ where 条件 ContextualExpression
输出：(residual 条件, Vec<ExistsSpec>)   // 仅提取合取位置的 Exists/In
```

**提取规则**：条件表达式 AND 树中处于**合取位置**（沿 And 链下钻）的
`Expression::Exists` / `Expression::In` 可提取；提取处替换为
`Literal(true)`（与 AND 恒真元可消去，后续常量折叠规则处理）。
非合取位置（OR 之下、RETURN/WITH 表达式内）→ P3。

**相关性分析与键提取**（对每个 ExistsSpec）：

```
V_inner = 子查询 pattern 解析出的变量集
V_outer = input_plan.root().col_names()（缺省时由外层 AST pattern 变量推导）
对子查询 WHERE 的 AND 拆解出的每个等值条件 a = b：
  一侧仅引用单个 V_inner 变量、另一侧仅引用 V_outer 变量
    → 提取为键 (outer_side_expr → hash_key, inner_side_expr → probe_key)
  其余条件保留在子查询计划 Filter 中：
    若其中仍引用 V_outer 变量 → 非简单相关 → P2 路径
若 V_inner 与 V_outer 无交集且无键 → 非相关子查询（空键）
```

**IN / NOT IN 改写**：`expr IN (SUBQ)` 合成等值条件
`return_expr = expr` 进入同一提取流程（与 EXISTS 统一）：
- 非简单形状（return_expr 与左 expr 不同形、多变量）→ P2/P3；
- NULL 语义沿用 `Value::contains` 现状（`NULL IN (…)` 结果偏差已在
  概览文档第 2.3 节声明不在范围）。

**子查询规划**（`plan_subquery`，递归）：

1. patterns → `plan_path_pattern` + `cross_join_plans`（复用
   `PlanningContext`；validation_info 暂用外层——子查询 tag 不在
   其中，索引选择退化为全扫，见 §6 风险）；
2. 子查询 WHERE 自身递归调用本规划器（嵌套 EXISTS/IN 自然支持）；
3. 子查询计划不引用外层变量（相关条件已被提取为键）。

**PatternApply 构建**：

```
apply = PatternApplyNode::new(left=input_plan.root, right=sub_plan.root,
                              hash_keys, probe_keys, is_anti_predicate)
plan  = apply
（多个 EXISTS 顺序嵌套：PatternApply(PatternApply(…, sub1), sub2)）
最终 = Filter(residual) 叠在 PatternApply 链之上
```

物理节点直接构建（与 `WhereClausePlanner` 现有 Filter 构建一致）；
逻辑镜像（`LogicalPatternApplyNode`）同步构建以保持
`SubPlan.logical_root` 一致（P0 已二进制化）。

### 3.4 端到端链路

```
WHERE EXISTS { MATCH (p:Person) WHERE p.name = t.name }
  → binder 放行（child scope 验证）
  → exists_planner：键 (hash=t.name, probe=p.name)，残差条件为空
  → PatternApply(left, right=Scan(p)→Filter(p 相关条件已提取))
  → Decorrelation batch：UnnestSimplePatternApplyRule
    （is_simple_subquery_shape 通过）→ SemiJoinNode
  → 运行时 cross_semi_join 执行
```

保留路径（`should_unnest` 拒绝时，如 TooManyRows/Complex）：
PatternApply 算子执行（P0 双侧键后与嵌套循环 SemiJoin 等价），
反馈闭环（`instance.rs:364-372` 的 apply_rows/join_rows）生效。

### 3.5 EXPLAIN

- `describe_visitor.rs` 的 PatternApply 行增加
  `keys: [hash=…, probe=…]` 与 `anti` 标注；
- SemiJoin 转换后自然显示 `SemiJoin`/`AntiJoin`（已有）。

## 4. P2：相关子查询逐行重执行（键提取失败的兜底）

**触发条件**：子查询存在无法提取为键的相关条件（非等值、多变量、
表达式形状不一致）——即 §3.3 分析后仍引用外层变量的子查询计划。

**计划形状**：

```
左子树 = 外层计划（每行触发一次重执行）
右子树 = Argument（外层布局）→ CrossJoin(基行, 子查询 pattern 计划)
         → Filter(保留的相关条件，引用外层变量)
         → 可选 Project(return_expr)
```

**执行**（`ApplyOperator` 新增变体 `CorrelatedApply`，
或给 `PatternApply` 增加 `Option<SubqueryPlan>`）：

1. 实例化期：右子树 `PlanNodeEnum` 缓存于算子（materializer
   提供「按 PlanNodeEnum 重建 StreamingExecutor」工厂——把
   `PhysicalPlanBuilder` + `PhysicalPlanMaterializer` 对子计划
   的执行路径抽成可复用函数）；
2. 每行：`runtime.set_correlation_frame(left_layout, left_row)` →
   **重建**右子树 executor（fresh open）→ 拉尽收集右行 →
   存在性判定（semi/anti）→ 行级输出；
3. `take_correlation_frame` 一次性取帧：右子树 root 仅一个
   `Argument` 源头，基行携带外层全部变量，单次消费正确；
4. 永不 unnest（`is_simple_subquery_shape` 不含 Argument/CrossJoin，
   天然避开）；`BatchPlanAnalyzer` 对含外层变量的 Filter 做确定性
   分析时无碍（不参与 unnest 判定即可）。

**性能说明**：逐行重建子树为 O(行数 × 子树成本)，正确性优先；
后续优化项（文档化，不在本次范围）：按 chunk 重建 + 算子状态
重置协议（`SourceOperator` 的 buffered 状态在算子结构体内，当前
无可重置机制，需引入 rewind 协议后方可复用子树实例）。

**改动面**：`apply_operator.rs`（新变体）、`arena_builder`（子计划
spec 构建 + 重建工厂）、`runtime.rs`（无改动，帧机制已有）、
`exists_planner.rs`（§3.3 非简单相关 → 此路径）。

## 5. P3：表达式级 `execute_subquery` 兜底（可选，独立阶段）

**触发条件**：非合取位置的 EXISTS/IN（`WHERE a = 1 OR EXISTS {…}`、
`RETURN EXISTS {…}`）——无法用 PatternApply 过滤表达。

**方案 A（推荐实现）**：运行时子查询执行器注入。

- 编译期：无法提取的 `SubqueryBody` 编译为独立子 `PhysicalPlan`
  （复用 arena builder），按表达式指纹注册于 `ExecutionRuntime`
  的 `subquery_runners` 表；
- 求值期：`chunk/eval.rs` 的 `evaluate_expression` 增加
  `Option<&Arc<ExecutionRuntime>>` 参数（Filter/Project 经
  `OperatorBase.runtime` 传入）；`Exists/In` 命中时走
  `evaluate_expression_per_row`（已存在，`eval.rs:275-290`）兜底，
  per-row 上下文携带子计划执行器：设帧（当前行）→ 重执行 →
  收集 RETURN 投影列（`In` 用 `results.contains`，`Exists` 用非空）；
- 列式/typed 快路径对 `Exists/In` 直接返回 `None`（回退 per-row），
  不改动既有快路径性能。

**方案 B（保守，可先落地）**：非合取位置返回精确错误
「EXISTS/IN in this position is not supported; move it to a
conjunctive WHERE condition」（比现状「binding not yet implemented」
可诊断性好得多），P3 整体推迟。

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 子查询 pattern 字符串 round-trip 再解析失败 | §3.2 解析期规范化 + 单测锁定；备选 binder 期缓存 |
| 子查询 tag 不在外层 `ValidationInfo`，索引选择退化 | 功能不受影响（全扫兜底）；后续扩展 validation 覆盖子查询 pattern（独立任务） |
| PatternApply 重构破坏面 | 无生产调用方，仅测试代码；P0 先行独立落地 + 全量回归 |
| `take_correlation_frame` 一次性语义 | P2 设计为单 Argument 消费；若右子树多源头需扩展为计数式帧（文档化） |
| OR 位置 / RETURN 中 EXISTS 语义 | P3 方案 A 或 B |
| NOT IN 的 NULL 语义 | 沿用 `Value::contains`，概览文档已声明不在范围 |

## 7. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| 1 | P0 重构（§2） | `cargo test -p graphdb-query` 全量 + clippy |
| 2 | P1 binder 放行（§3.1） | 单测：EXISTS/IN 绑定成功、child scope 变量解析、错误信息保留 |
| 3 | P1 解析规范化（§3.2） | 单测：`EXISTS { MATCH (a)-[:r]->(b) WHERE … RETURN … }` 字符串 round-trip |
| 4 | P1 exists_planner（§3.3） | 计划单测：非相关 EXISTS → 空键 PatternApply；相关简单等值 → 键提取；IN 改写；嵌套 EXISTS |
| 5 | P1 端到端 | e2e：`WHERE EXISTS/NOT EXISTS/IN/NOT IN` 各 2-3 例；EXPLAIN 断言 SemiJoin/AntiJoin 或 PatternApply；结果与手算一致 |
| 6 | P2 CorrelatedApply（§4） | 单测：非等值相关 `p.age > t.age`；e2e 结果正确性（与子查询单独执行对比） |
| 7 | P3（§5） | 方案 B 先落地（精确错误），方案 A 视需要 |
| 8 | 全量回归 | `cargo test -p graphdb-query`、e2e 全量、clippy 全 features 零新增警告 |

## 8. 与其他计划的衔接

- `query_exists_in_subquery.md` 第 7 节的「未实施」三项分别对应
  本文 P0+P1、P2、P3；
- CBO/反馈闭环（`apply_rows`/`join_rows`）在 P1 后自然生效；
- FoldConstantsRule（`query_expression_optimization.md`）可顺手处理
  残差条件中的 `AND true` 消去。
