# Expand / 图遍历操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的图遍历操作（Expand / Traverse / AppendVertices 等），
包括其语义、逻辑层节点、物理算子、算法流程和优化策略。
本文档与 2026-11-01 代码现状对齐。

在关系数据库中 `MATCH (a)-[r]->(b)` 需要两次 Scan 加一次 Join；在图数据库中它被编译为
**专用遍历算子**，直接沿 CSR 邻接表读取邻接边，这是 Join 文档中
"JoinToExpand / JoinToAppendVertices 优化"的落点。

---

## 一、逻辑层遍历节点

**文件路径**: `crates/linkrs-query/src/planning/plan/core/nodes/traversal/traversal_node.rs`
（各节点实现位于同名子目录：`expand.rs` / `expand_all.rs` / `traverse.rs` / `bi_traverse.rs` / `append_vertices.rs`）

| 节点 | 说明 | 输入形态 |
|------|------|----------|
| `ExpandNode` | 单步扩展：从输入行中的顶点出发，读取邻接边产生 (src, edge, dst) 行 | 单输入 |
| `ExpandAllNode` | 变长/全路径扩展：带 `step_limit`、`path_semantic`、`count_only` 等控制参数 | 单输入 |
| `TraverseNode` | 多步遍历（min_depth/max_depth 区间），可携带 `VisitedSet` 去重 | 单输入 |
| `BiExpandNode` / `BiTraverseNode` | 双向扩展/遍历 | 单输入 |
| `AppendVerticesNode` | 批量取回顶点属性（`space_id` + tag + `TagProp` 投影），常挂在边遍历之后补齐目标点属性 | 单输入 |
| `GetVerticesNode` / `GetNeighborsNode` | 直接取点 / 取邻居 | 单输入 |
| `MultiShortestPathNode` / `BFSShortestNode` | 最短路径类算法节点 | 多输入 |

路径算法的公共定义见 `traversal/path_algorithms.rs`。

---

## 二、物理层 GraphSpec 与执行算子

### 2.1 GraphSpec

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/graph.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::GraphSpec`）

### 2.2 GraphOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/graph_operator.rs`

执行器入口为 `GraphOperator`（结构同 JoinOperator：`kind` + 运行时上下文 + 输出布局 + 配置），
状态枚举 `GraphOperatorKind` 包含 7 种：

| 变体 | 说明 | 关键字段 |
|------|------|----------|
| `Expand` | 单步扩展 | `edge_types`、`direction: EdgeDirection`、`filter_expr`、`dst_tag` |
| `ExpandAll` | 变长/全路径扩展 | `src_vids`、`step_limit`/`step_limits`、`count_only`、`emit_raw_ids`、`lightweight_source`、`path_semantic`、`closed_loop`、`skip_rows`、`include_empty_paths`、边/点必需属性裁剪 |
| `Traverse` / `TraverseAll` | 多步遍历（深度区间） | `min_depth`、`max_depth`、`visited: VisitedSet`、`path_semantic` |
| `BiExpand` / `BiTraverse` | 双向扩展/遍历 | 同上基础字段 |
| `Subgraph` | 以顶点为中心取 N 跳子图 | `steps` |

辅助结构：

- `ExpandCtx`（`graph_operator.rs:30`）：单次扩展的上下文，含 `edge_ttl_active()` TTL 边存活判断；
- `VisitedSet`（`crates/linkrs-query/src/executor/streaming/operators/visited_set.rs`）：遍历去重集合；
- `EdgeDirection`：出边 / 入边 / 双向；
- `PathSemantic`：路径语义（如 trail/walk 级别的去重约定）。

### 2.3 执行流程

```
1. bind_runtime / from_spec：从 GraphSpec 实例化，注入存储句柄（Arc<RwLock<dyn QueryStorage>>）
2. 逐行读取上游输出（通常是顶点扫描结果）
3. 对每行的源顶点，按 direction 沿 CSR 邻接表读取邻接边
4. 应用 edge_types 过滤 + filter_expr 边过滤 + TTL 存活判断
5. 产出 (src, edge, dst) 行（ExpandAll/Traverse 再按 step_limit/visited 做多跳推进与去重）
6. 后接 AppendVertices 批量取回目标点属性（若需要）
```

---

## 三、从查询到执行的完整流程

```
1. 解析 / 验证
   ↓
2. 规划阶段：MATCH 模式按边粒度生成 Expand/Traverse 链，
   目标点属性通过 AppendVertices 补齐
   ↓
3. 优化阶段（启发式）
   - JoinToExpand（heuristic/join_optimization/join_to_expand.rs）：
     将图连接 Join 改写为 Expand 遍历
   - JoinToAppendVertices（.../join_to_append_vertices.rs）：
     将 Join 改写为 AppendVertices
   - EliminateAppendVertices（heuristic/elimination/eliminate_append_vertices.rs）：
     Project 不含 PathBuild 时消除 AppendVertices
   - PushProjectDownAppendVertices（heuristic/projection_pushdown/...）：
     投影下推到 AppendVertices，只取所需属性
   ↓
4. 物理规划：逻辑遍历节点 → GraphSpec → GraphOperator
```

---

## 四、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **逻辑遍历节点** | `crates/linkrs-query/src/planning/plan/core/nodes/traversal/traversal_node.rs`（+ 子目录 `expand.rs` / `expand_all.rs` / `traverse.rs` / `bi_traverse.rs` / `append_vertices.rs`） |
| **路径算法节点** | `.../plan/core/nodes/traversal/path_algorithms.rs` |
| **物理 GraphSpec** | `.../executor/streaming/operators/spec/graph.rs` |
| **Graph 执行算子** | `.../executor/streaming/operators/graph_operator.rs` |
| **去重集合** | `.../executor/streaming/operators/visited_set.rs` |
| **Join→Expand 改写** | `.../optimizer/heuristic/join_optimization/join_to_expand.rs` |
| **Join→AppendVertices 改写** | `.../optimizer/heuristic/join_optimization/join_to_append_vertices.rs` |
| **AppendVertices 消除** | `.../optimizer/heuristic/elimination/eliminate_append_vertices.rs` |
| **投影下推** | `.../optimizer/heuristic/projection_pushdown/push_project_down_append_vertices.rs` |

---

## 五、总结

1. **遍历优先**：图模式匹配编译为沿 CSR 的专用遍历算子，而非通用 Join；
2. **算子族完整**：单步/变长/多跳/双向/子图 7 种物理变体，覆盖 GO 语句与 Cypher 风格 MATCH；
3. **路径语义显式**：`PathSemantic`、`VisitedSet`、`closed_loop`、`include_empty_paths` 等参数
   将路径去重与空路径语义显式化，避免隐式约定；
4. **属性最小化**：AppendVertices 只取投影所需的点属性（投影下推 + 属性裁剪字段）；
5. **与 Join 互转**：优化器在 Join 与 Expand/AppendVertices 之间双向改写，选择更廉价的执行形态。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
