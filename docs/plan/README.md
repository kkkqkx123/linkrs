# 查询模块改进方案（docs/plan）

基于 `docs/analysis/linkrs_vs_nebula_query_analysis.md` 的对比分析与代码现状核查，
本文档目录收录查询模块剩余改进方向的落地方案。

## 优先级总览

| 优先级 | 方向 | 文档 | 状态 |
|--------|------|------|------|
| 高 | 子查询反嵌套增强 | [query_subquery_unnesting.md](query_subquery_unnesting.md) | 已实施（2026-08-12，遗留见文末） |
| 高 | 类型化属性裁剪（Property Pruning） | [query_property_pruning.md](query_property_pruning.md) | 已实施（2026-08-12，遗留见文末） |
| 高 | 表达式求值器运行时上下文表达式 | [query_expression_runtime_context.md](query_expression_runtime_context.md) | 已实施（2026-08-12，遗留见文末） |
| 中 | 解析器 Clause 级错误恢复 | [query_parser_clause_recovery.md](query_parser_clause_recovery.md) | 已实施 |
| 中 | MVCC 快照注入 QueryContext | [query_transaction_snapshot.md](query_transaction_snapshot.md) | 已实施（2026-08-12，遗留验证见文末） |
| 中 | 分区规划启用前置条件 | [query_partitioning_enablement.md](query_partitioning_enablement.md) | 待实施 |

## 背景

分析文档（2026-08-06）提出的改进项中，以下内容已随代码演进落地，**无需重复投入**：

- 启发式优化规则已扩充至 53 条（`optimizer/heuristic/rule_enum.rs`），涵盖
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

## 已完成实施项（2026-08-12）

阶段二剩余任务落地：

- 子查询反嵌套：
  - 启发式 `Decorrelation` 批次挂载轻量规则 `UnnestSimplePatternApplyRule`
    （形状 + 确定性门控，`heuristic/decorrelation.rs`），CBO 重判定不变
  - `QueryExecutionFeedback` 记录 `apply_rows` / `join_rows`（Apply vs SemiJoin
    实际行数），执行后回写并在反馈循环中以 debug 日志呈现
- 类型化属性裁剪：
  - 新增 `RequiredPropertyAnalyzer`（`optimizer/analysis/required_properties.rs`）：
    自顶向下需求传播，仅收集 `Property { object: Variable, .. }` 引用，
    裸引用/不透明对象/函数参数标记 full-value 并阻止裁剪（sticky）
  - 启用 `PushProjectDownGetVerticesRule` / `PushProjectDownGetNeighborsRule`
    （保留 Project 节点语义，仅收窄 `projected_properties`），规则计数 48 → 51
  - EXPLAIN 增加 GetVertices/GetNeighbors 的 `projected` 列展示

### 2026-08-12 增补（剩余问题全部落地）

- **GetEdges 属性裁剪四层打通**：
  - `GetEdgesNode.projected_properties` 字段 + `PushProjectDownGetEdgesRule`
    （规则计数 51 → 53，含 `PushProjectDownAppendVerticesRule`）
  - `SourceSpec::GetEdges` 投影字段 + flat 列布局 + EXPLAIN `projected` 展示
  - 执行双路径：点查 `get_edge_projected`（graph_storage 覆盖实现在
    `edge_record_to_edge` 前过滤属性，避免整 HashMap 构建）；扫描分支逐边裁剪
  - 存储层 `StorageReader::get_edge_projected`（默认实现 + 覆盖实现 + 单测）
- **AppendVertices 物理执行**：
  - `UnarySpec::AppendVertices` 重定义为存储型（entity 表达式 + prop 列表），
    `UnaryOperator::AppendVertices` 逐行点查追加 flat/full 列
  - conversion.rs 从 unsupported 改为 `push_unary_op`；metadata 布局配套
  - `JoinToAppendVerticesRule` 补全节点（vertex_props / src_expression /
    绑定变量）；`PushProjectDownAppendVerticesRule` 收窄 `vertex_props`
- **Apply/Join 反馈闭环**：
  - `FeedbackDrivenFactor` + `CardinalityFeedbackManager`：按算子形状键
    `{space}:{Type}:{discriminator}` 修正行数；`estimate_node_output_rows_corrected`
    供 CBO 消费（unnest / topn），写回计划保持原始估计
  - `execution_loops` 实报（profile `advance_count`）
  - `DecisionFeedbackStore` + `DecorrelationAdvice`：按空间记录 Apply/SemiJoin
    实测行数与耗时，`should_unnest` 实证优先（`Empirical` / `EmpiricalKeep`），
    成本模型固定系数替换为实测 `apply_cost_per_row`
- 验证：`graphdb-query` lib 1403 通过、`graphdb-storage` lib 721 通过、
  e2e 67 通过、clippy 零警告

### 2026-08-12 增补（阶段三剩余全部落地）

**表达式求值器运行时上下文表达式**（`query_expression_runtime_context.md`）：

- `ExpressionContext` trait 新增 `evaluate_label` / `evaluate_list_comprehension` /
  `evaluate_label_tag_property` / `evaluate_predicate` / `evaluate_reduce` /
  `evaluate_path_build` 六个运行时上下文方法，通用默认实现基于
  `get_variable`/`set_variable` 完整求值（ListComprehension/Predicate/Reduce
  零上下文即可用），`evaluate_label` 返回精确错误信息（原笼统
  "require runtime context"）
- `expression_evaluator.rs` 六个硬编码 `type_error` 分支改调 trait 方法
- binder 使用路径核查 + 修复：ListComprehension/Predicate 的局部迭代变量
  绑定（`inner_scope_with_variable` / `local_variable`，Predicate 三元参数
  防护），WITH/RETURN/顶点投影路径打通
- `chunk/eval.rs` 逐行回退路径复用 trait 方法（无需新增列式代码）
- 测试：`test_return_list_comprehension`（RETURN + WHERE 过滤 + 映射）、
  `test_return_list_comprehension_in_vertex_projection`（MATCH 投影中）
  端到端通过；`graphdb-query` lib 1410 → 1415

**MVCC 快照注入 QueryContext**（`query_transaction_snapshot.md`）：

- `QueryContext`/`QueryContextBuilder` 增加 `snapshot_ts` 字段与
  `with_snapshot_ts` / `snapshot_ts()`（方案步骤 1）
- `prepared.rs::snapshot_ts_for_request`：显式事务语句从
  `operation_context.read_timestamp` 派生快照注入 QueryContext；
  auto-commit 保持 None（方案步骤 3，执行器透传经 StorageOperationContext
  已完成，步骤 2/4/5 无需改动，差异见方案文档第 7 节）
- SQL `BEGIN [TRANSACTION] [READ ONLY | READ WRITE]`：lexer 关键字 +
  parser + `GraphService` 接线 → `begin_read_transaction`（只读快照事务）
- 验证：`graphdb-query` lib 1415、`graphdb-transaction` lib 224、
  `graphdb-api` lib 通过；clippy 零新增警告（既有 10 条测试警告未动）

### 遗留验证（资源受限环境未跑 integration）

- `cargo test -p graphdb-query --test '*'` 全量回归
- `BEGIN READ ONLY` 会话级端到端（同一事务两语句读同一快照、
  只读事务内 DML 拒绝）

## 实施顺序建议

1. **阶段一**：低风险增量 —— 解析器 Clause 恢复（`query_parser_clause_recovery.md`）✅
2. **阶段二**：优化器增强 —— 子查询反嵌套 + 属性裁剪（高优先级两篇）✅
3. **阶段三**：执行层 —— 表达式运行时上下文、事务快照 ✅（2026-08-12）
4. **阶段四**：分区规划启用（依赖存储层提供单调版本，需跨 crate 协调）

每篇方案文档包含：现状分析、方案设计（引用具体文件/函数）、实施步骤、
验证方法、预期收益、风险与回退。
