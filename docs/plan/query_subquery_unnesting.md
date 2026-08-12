# 子查询反嵌套增强方案

## 1. 现状分析

### 现有实现

- CBO 路径：`optimizer/cost_based/subquery_unnesting.rs`（563 行）
  - `SubqueryUnnestingOptimizer`：阈值 `max_subquery_rows=1000`、`max_complexity=50`
  - `should_unnest()` 五步判定：确定性 → 复杂度 → 简单子查询形状 → 行数 → 代价对比
  - `unnest()` 将 `PatternApplyNode` 转换为 `InnerJoinNode`，`replace_all_variables`
    替换占位变量生成 hash/probe keys
- 接入点：`OptimizerEngine::unnest_subqueries()`（`optimizer/engine.rs:861`）在
  `optimize_logical` 与 `optimize_plan_nodes` 两条路径的 Phase 1 调用
- 启发式侧：`heuristic/batch.rs:104` 的 `OptimizationBatch::Decorrelation` 批次
  存在但**无任何规则挂载**（`assign_rule_to_batch` 中无规则映射），为空批次

### 明确缺陷

| 缺陷 | 位置 | 影响 |
|------|------|------|
| 左输入行数硬编码 `left_rows = 100.0` | subquery_unnesting.rs:270, 278 | 代价对比失真，决策不可靠 |
| Filter 行数固定按 30% 折减 | subquery_unnesting.rs:254 | 选择性估计粗糙 |
| 仅接受 ScanVertices + 等值 Filter + Project 链 | `is_simple_subquery` | 覆盖面窄 |
| 无 EXISTS / IN / NOT IN 支持 | - | 文档建议的核心场景缺失 |
| 启发式 Decorrelation 批次为空 | batch.rs | 优化与代价阶段割裂 |

## 2. 方案设计

### 2.1 用真实统计估计替换硬编码

将 `should_unnest()` 中的硬编码替换为已有统计基础设施：

- 左输入行数：调用 `cost_based/row_estimates.rs` 的 `estimate_node_output_rows`
  对 PatternApply 左输入做估计，而非常量 `100.0`
- 子查询行数：Filter 的 30% 固定折减改为走 `SelectivityEstimator::estimate_from_expression`
  （`cost/selectivity.rs`，已支持 EWMA 修正），`estimate_subquery_rows` 保留为兜底

```rust
// 替换前（subquery_unnesting.rs:270）
let left_rows = 100.0;
// 替换后
let left_rows = estimate_node_output_rows(left_input, stats, cost_calc)
    .unwrap_or(100.0) as f64;
```

### 2.2 扩展可反嵌套子查询形状

`is_simple_subquery` 目前只接受单表扫描 + 等值过滤 + Project。扩展支持：

1. `ScanEdges` / `IndexScan`（命中索引的过滤）
2. 带 `Limit` 的子查询（反嵌套后转 InnerJoin + 保留 LIMIT 语义需谨慎，仅限
   无相关性的 Limit）
3. 基础聚合子查询（`COUNT`/`EXISTS` 语义）：反嵌套为 HashAggregate 侧输出，
   或保持 Apply 但决策改为 `KeepPatternApply` 并给出原因

### 2.3 EXISTS / IN / NOT IN 子查询支持

现状：`Expression::Exists` 在 `expression_evaluator.rs:301` 已通过
`context.execute_subquery` 逐行执行（NestedLoop 语义）。

目标：转换为 SemiJoin / AntiJoin 计划节点，避免逐行执行。

```rust
// 计划节点侧（planning/plan/core/nodes/）
pub struct SemiJoinNode { ... }   // 新增
pub struct AntiJoinNode { ... }   // 新增
```

- 新增计划节点枚举变体 + `PhysicalPlanBuilder` / materializer 支持
- 执行算子：已有 `streaming/operators/join/cross_semi_join.rs` 可作为基础，
  扩展为带 probe/build 两侧的 Hash SemiJoin / Hash AntiJoin
- 转换规则：`Exists { body }` 若 body 为简单子查询 → SemiJoin；
  `NOT Exists` → AntiJoin；`expr IN (subquery)` → SemiJoin（等值连接）
- 启发式批次接线：在 `heuristic/batch.rs` 的 `assign_rule_to_batch` 中挂载
  轻量判定规则（确定性 + 形状检查），重判定留给 CBO 阶段

### 2.4 决策可观测性

- 将 `UnnestDecision` 的 reasons（`UnnestReason` / `KeepReason`）通过 CBO notes
  输出到 EXPLAIN（已有机制），便于验证
- 执行反馈：`QueryExecutionFeedback` 中记录 Apply 与 Join 的实际行数对比，
  供 `FeedbackDrivenSelectivity` 修正后续决策

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 替换硬编码行数估计，接入真实 stats | `subquery_unnesting.rs`, `row_estimates.rs` |
| 2 | 扩展 `is_simple_subquery` 支持 ScanEdges/IndexScan | `subquery_unnesting.rs` |
| 3 | 新增 SemiJoin/AntiJoin 计划节点 + 执行算子 | `planning/plan/core/nodes/`, `streaming/operators/join/` |
| 4 | EXISTS/IN/NOT IN 转换规则 + 启发式批次接线 | `optimizer/engine.rs`, `heuristic/batch.rs` |
| 5 | EXPLAIN 输出 + 反馈回路验证 | `executor/explain/`, `stats/feedback/` |

## 4. 验证方法

- 正确性：对每条转换规则新增单元测试（输入计划 → 输出计划断言 + 执行结果对比
  Apply 与 Join 两种计划结果一致）
- 回归：`cargo test -p graphdb-query` 全量
- 性能：benchmarks 中增加相关子查询用例，对比反嵌套前后执行时间与行数
- EXPLAIN ANALYZE：人工核验 estimated vs actual rows

## 5. 预期收益

- 相关子查询从逐行嵌套循环改为哈希连接，典型场景 10~100 倍行处理性能提升
- 消除硬编码估计，决策准确率随统计反馈提升
- EXISTS/IN/NOT IN 语义获得优化路径（文档 4.2.8 项落地）

## 6. 风险与回退

- **风险**：SemiJoin/AntiJoin 新算子引入正确性 bug。缓解：现有
  `cross_semi_join.rs` 语义可对照；转换保持"保守优先"（复杂度/确定性不满足即
  `KeepPatternApply`）
- **回退**：`OptimizerEngine` 增加开关 `enable_subquery_unnesting`（默认 true），
  关闭即回到纯 Apply 执行
