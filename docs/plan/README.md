# 查询模块改进方案（docs/plan）

基于 `docs/analysis/linkrs_vs_nebula_query_analysis.md` 的对比分析与代码现状核查，
本文档目录收录查询模块剩余改进方向的落地方案。

## 优先级总览

| 优先级 | 方向 | 文档 | 状态 |
|--------|------|------|------|
| 高 | 子查询反嵌套增强 | [query_subquery_unnesting.md](query_subquery_unnesting.md) | 待实施 |
| 高 | 类型化属性裁剪（Property Pruning） | [query_property_pruning.md](query_property_pruning.md) | 待实施 |
| 高 | 表达式求值器运行时上下文表达式 | [query_expression_runtime_context.md](query_expression_runtime_context.md) | 待实施 |
| 中 | 解析器 Clause 级错误恢复 | [query_parser_clause_recovery.md](query_parser_clause_recovery.md) | 待实施 |
| 中 | MVCC 快照注入 QueryContext | [query_transaction_snapshot.md](query_transaction_snapshot.md) | 待实施 |
| 中 | 分区规划启用前置条件 | [query_partitioning_enablement.md](query_partitioning_enablement.md) | 待实施 |

## 背景

分析文档（2026-08-06）提出的改进项中，以下内容已随代码演进落地，**无需重复投入**：

- 启发式优化规则已扩充至 48 条（`optimizer/heuristic/rule_enum.rs`），涵盖
  CombineFilter / CollapseProject / EliminateNoop / 过滤下推 / Limit 下推等
- 执行反馈闭环已完整接线（`stats/feedback/`）：EWMA 选择性修正 → 优化器前置应用 →
  执行后回写，EXPLAIN ANALYZE 可见
- EXPLAIN / PROFILE 已支持 table / dot / tree / json 四种输出格式
- 子查询反嵌套已有 CBO 路径（`cost_based/subquery_unnesting.rs`）
- 分区规划器已有保守实现（`optimizer/partitioning.rs`），缺启用条件

## 已完成清理项（2026-08-11）

- 移除 `NodeExecutionStats::cache_hit_rate()` 与 `GlobalExecutionStats.cache_hit_rate`
  死代码（metrics 迁移遗留，全仓库无调用方）
- 清理 `graphdb-query` 代码文件中全部中文/中英混杂注释（26 个文件，含
  `stats/feedback/`、`cost/selectivity.rs` 等），替换为英文，符合 AGENTS.md 代码语言约定
- 修复 `create_planner.rs` 测试断言中的损坏字符串（"The多边al" → "The multi-edge"）
- `user_parser.rs` unicode 测试数据由中文替换为日文假名（保留 unicode 测试意图）

注：`crates/` 其他 crate 及 `tests/` 中的中文均为**测试数据**（jieba 分词器、
向量检索、全文搜索的中文输入是必要测试用例），属合理使用，未改动。

## 实施顺序建议

1. **阶段一**：低风险增量 —— 解析器 Clause 恢复（`query_parser_clause_recovery.md`）
2. **阶段二**：优化器增强 —— 子查询反嵌套 + 属性裁剪（高优先级两篇）
3. **阶段三**：执行层 —— 表达式运行时上下文、事务快照
4. **阶段四**：分区规划启用（依赖存储层提供单调版本，需跨 crate 协调）

每篇方案文档包含：现状分析、方案设计（引用具体文件/函数）、实施步骤、
验证方法、预期收益、风险与回退。
