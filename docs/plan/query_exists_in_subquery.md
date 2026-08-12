# EXISTS / IN / NOT IN 表达式级子查询落地 方案

> 状态：部分实施（2026-08-12）。解析器缺口已修复并测试；
> binder→PatternApply 转换与运行时兜底仍未实施，见文末「实施记录」。

## 1. 现状分析

### 表达式级子查询的现状（运行时必失败）

- 表达式定义：`Expression::Exists { body }` 与
  `Expression::In { expr, subquery, negated }`
  （`graphdb-core/types/expr/def.rs`，`SubqueryBody` 承载子查询体）
- 求值器：`expression_evaluator.rs:309-322` 对这两种表达式调用
  `context.execute_subquery(body)`：
  - `Expression::Exists`：`execute_subquery` 返回空则 False
  - `Expression::In`：对 `expr` 求值后 `results.contains(&value)`，
    `negated` 取反
- **缺口**：`ExpressionContext::execute_subquery` 的默认实现
  （`evaluator/traits.rs:72-77`）返回
  `"Subquery execution not supported in this context"`，且**全仓库无任何
  context 覆盖该默认实现**（grep 确认仅 traits 定义 + evaluator 调用）
  → 任何到达执行层的 EXISTS/IN 表达式都会报错

### 计划级路径已有

- `cost_based/subquery_unnesting.rs` 已实现 PatternApply →
  SemiJoin（EXISTS）/ AntiJoin（NOT EXISTS）转换
  （`SemiJoinNode::new_semi` / `new_anti`，行 456-461），
  启发式 `heuristic/decorrelation.rs::UnnestSimplePatternApplyRule`
  已挂载
- **缺**：binder/planning 阶段把 `Expression::Exists` /
  `Expression::In` 的 `SubqueryBody` 绑定为 `PatternApply` 子计划
  的路径（遗留的「依赖 binder 产生 PatternApply」即指此）

## 2. 方案设计

### 2.1 目标：表达式级子查询 → 计划级 SemiJoin/AntiJoin（推荐）

`(EXISTS | NOT EXISTS | IN | NOT IN) (subquery)` 在 binder 阶段转换：

```
WHERE EXISTS (MATCH (v)-->(p) WHERE p.name = t.name)
        ↓ binder
WHERE (t.name) IN (SELECT p.name FROM PatternApply-子计划)
        ↓ unnest 优化器（既有）
SemiJoin / AntiJoin（既有路径，含反馈闭环）
```

- **binder 转换**（`planning/binder/` 或表达式绑定层）：
  - 检测 `Expression::Exists/In`，其 `SubqueryBody` 解构为独立查询，
    绑定为 `PatternApply` 节点（apply 右输入 = 子查询计划，左输入 =
    外层行，`applied_expr` 关联子查询投影表达式）
  - `In`/`NOT IN` 的 `expr`（左侧）作为半连接探查键的表达式来源
  - 转换成功 → 计划树出现 PatternApply → 既有
    `UnnestSimplePatternApplyRule` / CBO 判定接管
  - 转换失败（子查询形状不满足 `is_simple_subquery` 门槛）→ **回退**
    到 2.2 运行时求值，保证可执行而非报错
- **EXPLAIN**：SemiJoin/AntiJoin 节点标注 `exists`/`in`/`not in` 来源

### 2.2 运行时求值兜底（低成本，保证可执行）

在 `evaluation_context/` 的行上下文实现 `execute_subquery`：

- 执行 `SubqueryBody` 绑定的子计划（复用子计划执行入口），
  收集 RETURN 投影列值列表返回
- 语义：相关子查询逐行重执行（性能差但正确），作为 2.1 转换失败
  的兜底，也直接修复当前「必报错」问题
- 若子计划执行入口不可复用，则在默认实现返回明确的
  「EXISTS/IN 表达式未转换为子查询计划」错误（至少改进错误信息）

### 2.3 不在本次范围

- `NOT IN` 的 NULL 语义精确处理（`NULL IN (…)` 结果）——沿用现有
  `Value::contains` 语义，后续专项
- 相关子查询去关联化（分析文档 4.3-8 建议）——`subquery_unnesting`
  已有 CBO 路径，表达式级先打通转换链

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | binder：`Exists/In` 子查询解构 → PatternApply 绑定 + 探测键表达式接线 | `planning/binder/`（表达式绑定路径） |
| 2 | 形状门控：不满足 `is_simple_subquery` 时保留表达式（回退 2.2） | 同上 + `cost_based/subquery_unnesting.rs`（复用判定） |
| 3 | 行上下文 `execute_subquery` 实现（子计划执行 + RETURN 值收集） | `evaluation_context/default_context.rs`, `row_context.rs` |
| 4 | EXPLAIN 标注 exists/in/not in 来源 | `executor/explain/` |
| 5 | 单元测试 + e2e：转换成功路径（SemiJoin/AntiJoin 计划断言 + 执行结果对比 Apply 与 Join 一致） | `tests/` |

## 4. 验证方法

- 单元测试：
  - binder 转换：`WHERE EXISTS (…)` / `WHERE x IN (…)` / `NOT IN` 各生成
    PatternApply（形状满足时）
  - 回退路径：非简单子查询仍为 `Expression::Exists/In`，执行走
    `execute_subquery` 兜底不报错
  - 结果一致性：同一查询转换前（Apply 重执行）与转换后
    （SemiJoin/AntiJoin）结果集一致（复用 `subquery_unnesting.rs`
    既有断言模式）
- e2e：`tests/e2e/` 增补 EXISTS / IN / NOT IN 查询用例
- 回归：`cargo test -p graphdb-query` 全量；clippy 零新增警告

## 5. 预期收益

- 修复分析文档遗留：EXISTS/IN/NOT IN 表达式级子查询从「必报错」
  变为「转换优先、兜底可执行」
- 打通 binder → PatternApply → SemiJoin/AntiJoin → 执行反馈闭环的
  完整链路，`QueryExecutionFeedback` 的 `apply_rows`/`join_rows`
  在表达式级查询上生效

## 6. 风险与回退

- **风险**：binder 转换破坏现有 `Expression::Exists/In` 使用方
  （构造器、visitor）。缓解：先核查现有构造点（`construction.rs`），
  转换仅发生在绑定层，core 表达式不变
- **风险**：相关子查询语义（外层变量引用）在 PatternApply 绑定中
  未正确传递。缓解：复用既有 `PatternApply` 的变量绑定机制；
  单测覆盖相关性
- **回退**：步骤 1-2 摘除即恢复现状（表达式保留）；步骤 3 兜底
  始终保留，可独立落地

## 7. 实施记录（2026-08-12）

本次核查与落地发现并修复**解析器真实缺陷**，binder 转换保持未实施：

### 已修复：子查询解析缺陷

`parse_pattern_string`（`expr_parser.rs`）用 `match_token` 判断终止符，
会**消费** WHERE / RETURN / RBrace / MATCH token，导致
`EXISTS { MATCH (q:person) WHERE q.age > 30 }` 解析报
"Expected RBrace, found Identifier(q)"。修复：

- 终止符改为 `check`（不消费），`parse_subquery_body` 得以在其后
  正常分派 WHERE / RETURN
- 无 `MATCH` 关键字的裸模式（`EXISTS { a:person-[:knows]->b:person }`）
  此前直接报错，现支持
- 单测：`test_parse_exists_with_where_clause`、
  `test_parse_exists_with_return_expr`、`test_parse_exists_bare_pattern`
  锁定行为（`graphdb-query` lib 1430 通过）

### 已改善：binder 错误信息

`binder_impl.rs` 的 EXISTS/IN 绑定错误从笼统
"binding not yet implemented" 改为指明当前限制与替代路径。

### 仍未实施（后续任务）

> 正式实现方案见 `docs/plan/query_exists_in_subquery_impl.md`：
> P0 PatternApply 双侧键重构、P1 binder 放行 + WHERE 合取位置转换、
> P2 相关子查询逐行重执行、P3 表达式级兜底。

- **binder → PatternApply 转换**：核查确认 `PatternApplyNode` 目前
  **仅测试代码构造**，规划层无任何生产者（`create_pattern_apply` 工厂
  无调用方）——「binder 产生 PatternApply 的路径」从未建成，属
  独立功能开发而非验证
- **运行时兜底 `execute_subquery`**：流式上下文（`BorrowedRowContext`
  等）无存储/计划执行访问，需先建立子计划执行器注入机制
- 转换的约束：`PatternApply` 执行器将右输入**物化一次**（`apply_operator.rs`），
  且 `key_expressions` 对左右行用同一表达式求值——**相关子查询**（逐行
  重执行）需要 `Apply`（correlated_columns）或全新逐行执行机制，属
  设计决策点
