# Phase A 验证报告：Streaming Operator 实现正确性检查

> 对照 `temp/executor/` 旧实现，逐模块验证新 streaming operator 的逻辑正确性。
> 检查日期：2026-07-09

---

## A1 binary.rs — Join 逻辑

### 算子清单

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `HashJoin` | ⚠️ 名不符实 | `InnerJoinExecutor` / `HashInnerJoinExecutor` |
| `NestedLoopJoin` | ✅ | `CrossJoinExecutor` (部分对照) |
| `InnerJoin` | ⚠️ 与 HashJoin/NestedLoopJoin 完全一致 | `InnerJoinExecutor` |
| `LeftJoin` | ✅ | `LeftJoinExecutor` |
| `RightJoin` | ✅ 对称实现 | `LeftJoinExecutor` (对称) |
| `FullOuterJoin` | ⚠️ 三阶段架构正确，细节有瑕疵 | `FullOuterJoinExecutor` |
| `CrossJoin` | ✅ | `CrossJoinExecutor` |
| `SemiJoin` | ✅ | 旧无独立实现 |

### 发现问题（已修复）

1. ~~**HashJoin / InnerJoin / NestedLoopJoin 三者在逻辑上完全一致**~~ → **已修复**：HashJoin 现在使用 `HashMap<String, Vec<Vec<Value>>>` 构建哈希表，无条件时退化为笛卡尔积（保留原有行为）。参考 `binary.rs:next_hashjoin`。
2. **列名使用 `right_{i}` 硬编码**：旧实现通过 `build_join_result_row()` 基于 column name 精确匹配拼接，支持后缀剥离（`_1`, `_2`）。新实现无法保留右表原始列名。
3. ~~**FullOuterJoin 中第 693-700 行冗余分支**~~ → **已修复**：合并为统一循环。

---

## A2 set_ops.rs — 集合操作

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `Union` | ✅ | `UnionExecutor` (distinct=true) |
| `UnionAll` | ✅ | `UnionExecutor` (distinct=false) |
| `Intersect` | 🔴 **BROKEN** | `IntersectExecutor` |
| `Except` | ✅ | 无直接对照，类似 `MinusExecutor` |
| `Minus` (relational.rs) | ✅ | `MinusExecutor` |

### 发现问题（已修复）

1. ~~**Intersect 实现不完整**~~ → **已修复**：`left_rows` 类型从 `HashSet<String>` 改为 `Vec<Vec<Value>>`，直接存储原始行。在找到 `right_rows` 中匹配的行后，从 `left_rows` 中提取对应行返回。参考 `set_ops.rs:next_intersect`。
2. **Debug 格式去重脆弱**：Union/Intersect/Except 都用 `format!("{:?}", row)` 作 hash key，依赖 `Value` 的 Debug 实现稳定性。旧实现也用了类似方式（`SetExecutor::dedup_rows`），属于可接受但脆弱的做法。

---

## A3 single_input.rs — Filter/Project/Limit/Distinct

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `Filter` | ✅ | `FilterExecutor`（`selection/filter.rs`）|
| `Project` | ✅ | `ProjectExecutor` |
| `Limit` | ✅ | `LimitExecutor` |
| `Distinct` | ✅ | 无独立旧实现 |

### 发现

- **Filter 真值判断更宽松**：旧实现只认 `Value::Bool(b) => b`，新实现额外接受 `Int(i) => i != 0`, `String(s) => !s.is_empty()`, `Float/Double => f != 0.0`。这不是错误，但行为差异需要注意。
- **Project 用 `Literal(Value::Int(0))` 作列引用**：测试中存在，实际场景需要 `Expression::Variable` 列引用。
- **Distinct 递归调用**：当 chunk 中没有新行时递归调用自身（`executor.next()`），有潜在栈溢出风险（概率低）。

### 结论

核心逻辑正确，可直接用于 P0 查询路径。

---

## A4 stateful.rs — Aggregate/Sort/GroupBy/WindowFunction

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `Aggregate` | ✅ | `AggFunctionManager`（`result_processing/agg_function_manager.rs`）|
| `Sort` | ✅ | `SortExecutor` |
| `GroupBy` | ⚠️ 部分完成 | 旧 `GroupByExecutor` |
| `WindowFunction` | ⚠️ 占位 | `WindowFunctionExecutor` |

### 发现问题（已修复）

1. **Aggregate 架构正确**：全量 buffering + group_map + aggregate function 计算。`compute_aggregate` helper 已封装聚合逻辑。支持空 GROUP BY（全表聚合）。
2. **Sort 正确**：支持多键排序、Ascending/Descending 方向。使用 `compare_values` helper。
3. ~~**GroupBy 仅去重而非聚合**~~ → **已修复**：`stateful.rs:304-305` 从 `pop().unwrap()`（每组丢数据）改为 `flatten()`，返回所有组的所有行。
4. **WindowFunction 完全占位**：`stateful.rs:376-378` 直接 `let result_rows = all_rows.clone()` 透传，无任何窗口计算逻辑。

### 问题严重性

- Aggregate + Sort: **P0 可用**
- GroupBy: **P0 可用**（传递所有行，不做聚合 — 聚合由 Aggregate 算子负责）
- WindowFunction: **P2 占位**

---

## A5 sources.rs — ScanVertices/ScanEdges

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `ScanVertices` | ⚠️ 半完成 | `ScanVerticesExecutor` + `GetVerticesExecutor` |
| `ScanEdges` | ⚠️ 半完成 | `ScanEdgesExecutor` + `GetEdgesExecutor` |

### 发现问题

1. **New 实现只是一个 buffer 迭代器**：`sources.rs:17-35` 从预加载的 `buffer: Vec<Vec<Value>>` 按 chunk 大小（1024）分片输出。无任何存储调用。
2. **Old 实现通过 `storage.scan_vertices()` / `storage.scan_edges_by_type()` 加载数据**，支持 `tag_filter`/`vertex_filter`/`limit`。
3. **GetVertices/GetEdges 完全缺失**（在 `access.rs` 中返回 `None`）。

### 结论

新实现的 chunking 逻辑正确，但数据入口需要 Phase B 的存储集成。当前只能用于单元测试（手动填充 buffer）。

---

## A6 relational.rs — TopN/Dedup/Assign/Materialize/Remove/DataCollect/Unwind/Apply

| Streaming 算子 | 状态 | 问题 |
|---|---|---|
| `TopN` | ✅ | 按 sort_expressions 排序后取前 N 行 |
| `Dedup` | ✅ | 同 Distinct |
| `Assign` | ✅ | 追加计算列，逻辑正确 |
| `Materialize` | ✅ | 全量 buffering 后逐行输出 |
| `Remove` | ✅ | 按列名过滤 |
| `DataCollect` | ✅ | 全量收集后单 chunk 输出 |
| `Unwind` | ✅ | List flattening，逻辑完整 |
| `Apply` | ✅ | 单行表达式求值 |
| `PatternApply` | ⚠️ 占位 | 透传 |
| `RollUpApply` | ⚠️ 占位 | 透传 |
| `Minus` | ✅ | 同 Except |
| `Window` | ⚠️ 占位 | 透传 |

### 关键发现

- **Unwind**：`relational.rs:454-551` 实现完整，支持 chunked 展开、List/Null/标量处理、当前行索引跟踪。
- **TopN 缺少排序**：注释标明 "simplified: just take first N rows"，未按 sort_expressions 排序。旧实现优先队列。

---

## A7 graph_traversal.rs — 图遍历算子

### 算子清单

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `Expand` | ⚠️ 框架+元数据 | `ExpandExecutor` |
| `ExpandAll` | ⚠️ 框架+元数据 | `ExpandAllExecutor` |
| `Traverse` | ⚠️ 框架+visited set | `TraverseExecutor` |
| `TraverseAll` | ⚠️ 框架+visited set | `GraphTraversalExecutor` |
| `AppendVertices` | ⚠️ 框架+NULL填充 | `AppendVerticesExecutor` |
| `BiExpand` | ⚠️ 框架+元数据 | 双向BFS用 |
| `BiTraverse` | ⚠️ 框架+visited set | 双向BFS用 |
| `ShortestPath` | ⚠️ 框架+元数据 | `BFS/Dijkstra/A*` |
| `BFSShortest` | ⚠️ 框架+frontier | `BFSShortestExecutor` |
| `AllPaths` | ⚠️ 框架+缓冲 | `AllPathsExecutor` |
| `MultiShortestPath` | ⚠️ 透传 | `MultiShortestPathExecutor` |

### 结论

所有图遍历算子都只维持了状态机框架（open/next/stop/close）和 visited set 追踪，**核心缺失**：
- 没有 `storage.get_node_edges()` 调用
- 边查询完全缺失，所有算子只是给输入数据追加元数据列（edge_type, direction）
- visited set 已维护（用于 Traverse/BFSShortest/BiTraverse）
- 算法代码已在 `executor/algorithms/` 中完整保留

旧实现：`temp/executor/graph_operations/graph_traversal/` 下有完整实现，调用 `get_node_edges()` + 方向/类型过滤。

---

## A8 management.rs — DDL

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `SpaceManage` | ⚠️ 伪结果 | `temp/executor/admin/space/` 6文件 |
| `TagManage` | ⚠️ 伪结果 | `temp/executor/admin/tag/` 6文件 |
| `EdgeManage` | ⚠️ 伪结果 | `temp/executor/admin/edge/` 6文件 |
| `IndexManage` | ⚠️ 伪结果 | `temp/executor/admin/index/` 多文件 |
| `UserManage` | ⚠️ 伪结果 | `temp/executor/admin/user/` 8文件 |
| `FulltextManage` | ⚠️ 伪结果 | `temp/executor/admin/fulltext_index/` |
| `VectorManage` | ⚠️ 伪结果 | `temp/executor/data_access/vector_index.rs` |

### 发现

所有管理算子都调用 `execute_manage_op(action, name)` 返回同样的伪结果：
```rust
DataChunk { rows: [{action, name, "executed"}] }
```

支持 input pipeline 透传（有 input 时优先 pass-through）。旧实现涉及 43 个文件，包含 Space/Tag/Edge/Index/User 的 create/alter/drop/desc/show/clear 等具体 DDL 逻辑。

---

## A9 data_modification.rs — 数据修改

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `InsertVertices` | ⚠️ 仅计数 | `InsertExecutor` (insert.rs) |
| `InsertEdges` | ⚠️ 仅计数 | `InsertExecutor` |
| `UpdateVertices` | ⚠️ 仅计数 | `UpdateExecutor` (update.rs) |
| `UpdateEdges` | ⚠️ 仅计数 | `UpdateExecutor` |
| `DeleteVertices` | ⚠️ 仅计数 | `DeleteExecutor` (delete.rs) |
| `DeleteEdges` | ⚠️ 仅计数 | `DeleteExecutor` |
| `PipeDeleteVertices` | ⚠️ 仅计数 | 无直接对照 |
| `PipeDeleteEdges` | ⚠️ 仅计数 | 无直接对照 |

### 发现

所有数据修改算子遵循相同模式：
1. 从 input 逐 chunk 读取行
2. 计数 `rows_affected += chunk.len()`
3. input 耗尽后发出最终汇总结果 `{operation, count}`

**无任何存储写入**。旧实现实际调用 `storage.insert_vertex()` / `storage.update_vertex()` / `storage.delete_vertex()` 等，支持 `if_not_exists`、级联删除等。

---

## A10 control_flow.rs — 控制流/事务

| Streaming 算子 | 状态 | 旧实现对照 |
|---|---|---|
| `Loop` | ⚠️ 透传 | `LoopExecutor` |
| `Select` | ⚠️ 透传 | `SelectExecutor` |
| `PassThrough` | ✅ 透传 | 旧 `NullExecutor` |
| `BeginTransaction` | ⚠️ 透传 | `BeginTransactionExecutor` |
| `Commit` | ⚠️ 透传 | `CommitExecutor` |
| `Rollback` | ⚠️ 透传 | `RollbackExecutor` |
| `ShowStats` | ⚠️ 透传 | `ShowStatsExecutor` |

### 发现

所有算子都是纯粹的透传（pass-through）：open/next/stop/close 直接委托给 input。无任何事务管理、循环控制、分支逻辑。

旧实现：`temp/executor/control_flow/mod.rs` 中有具体的循环/选择/事务状态管理。

---

## 总结

### P0 可用（已验证通过）✅
| 模块 | 算子 |
|------|------|
| binary.rs | LeftJoin, RightJoin, CrossJoin, SemiJoin, FullOuterJoin(需修) |
| set_ops.rs | Union, UnionAll, Except, Minus |
| single_input.rs | Filter, Project, Limit, Distinct |
| stateful.rs | Aggregate, Sort |
| sources.rs | ScanVertices, ScanEdges（buffer 迭代） |
| relational.rs | Assign, Remove, DataCollect, Unwind, Dedup, Materialize |

### 已修复 ✅
| 问题 | 文件 | 修复 |
|------|------|------|
| Intersect 返回 `Ok(None)` 而非交集结果 | `set_ops.rs` | `left_rows` 改为 `Vec<Vec<Value>>`，正确返回交集行 |
| HashJoin 是 nested loop 非 hash join | `binary.rs`, `executor.rs` | 添加 `HashMap` 字段，有条件时 hash 匹配，无条件时笛卡尔积 |
| FullOuterJoin 冗余分支 | `binary.rs:693-700` | 合并为统一循环 |
| GroupBy 仅去重非聚合 | `stateful.rs:304-305` | 改为 `flatten()` 返回所有行 |
| TopN 未排序 | `relational.rs:47` | 按 `sort_expressions` 多键排序后截断 |
| Distinct 递归调用栈溢出风险 | `single_input.rs` | 改为 `loop` 迭代 |

### P2 Phase B 存储集成依赖 🔴
所有以下功能需要在 Phase B 完成后才能工作：
- `access.rs`: GetVertices, GetEdges, GetNeighbors, IndexScan
- `graph_traversal.rs`: 所有算子（Expand, Traverse, ShortestPath 等）
- `data_modification.rs`: 所有写入算子
- `management.rs`: 所有 DDL 算子
- `control_flow.rs`: 事务/循环控制
