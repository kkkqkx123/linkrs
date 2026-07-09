# Executor 目录清理与结构调整计划

## 背景

`ExecutorEnum` 删除后，遗留了大量旧 `Executor<S>` trait 的实现文件，同时 `StreamingExecutor`
体系已建立但迁移未完全收尾。本文分析当前代码结构问题，提出分阶段清理方案。

---

## 一、当前结构问题

### 1.1 双执行器体系共存

```
旧 Executor<S> trait（dead code）     StreamingExecutor（生产路径）
├── admin/                            ├── operators/management (DDL, stub)
├── data_access/                      ├── operators/access (stub/error)
├── data_modification/                ├── operators/data_modification (stub)
├── graph_operations/                 ├── operators/graph_traversal (stub)
│   └── graph_traversal/
│       └── algorithms/               └── operators/graph_traversal (stub)
├── relational_algebra/
│   ├── join/
│   ├── selection/
│   └── set_operations/
└── result_processing/
    └── transformations/
```

生产路径：`query_pipeline_manager.execute_plan()` → `StreamingQueryExecutor::from_plan_node()`
→ `StreamingExecutorBuilder::from_plan_node()` → `StreamingExecutor`

**旧 `Executor<S>` 的 impl 均不在生产路径中被调用**，但 streaming 对于这些操作的实现
多数为 pass-through stub（仅 delegate 给 input），尚无实际逻辑。

### 1.2 模块组织问题

| 问题 | 说明 |
|------|------|
| `control_flow/` 空壳 | 只剩注释，streaming 已取代 |
| `graph_operations/` 4 层嵌套 | `executor/graph_operations/graph_traversal/algorithms/` |
| 7 个缺失模块声明 | `expand`、`expand_all`、`shortest_path`、`traverse`、`materialize`、`filter`、`UnwindExecutor` |
| `fulltext_index/` 嵌套过深 | `admin/index/fulltext_index/` |
| 旧 re-export 过多 | `executor/mod.rs` re-export 数十个旧类型 |

### 1.3 StreamingExecutor 实现状态

| 类别 | Builder 支持 | 实现质量 |
|------|-------------|---------|
| ScanVertices/ScanEdges | ✅ | 有 buffer 遍历逻辑 |
| Filter/Project/Limit/Distinct | ✅ | 有完整表达式求值/列裁剪/行计数 |
| Aggregate/Sort/GroupBy/WindowFunction | ✅ | 有完整缓冲+排序+聚合逻辑 |
| HashJoin/InnerJoin/LeftJoin/CrossJoin | ✅ | 有 HashJoin/NestedLoop 实现 |
| Union/UnionAll/Intersect/Except | ✅ | 有完整集合操作逻辑 |
| TopN/Dedup/Assign/Materialize/Unwind | ❌ builder 不支持 | 但 operator 有实现 |
| Expand/Traverse/BFS/ShortestPath/AllPaths | ❌ builder 不支持 | pass-through stub |
| SpaceManage/TagManage/EdgeManage/IndexManage/UserManage | ❌ builder 不支持 | pass-through stub |
| InsertVertices/InsertEdges/Update/Delete | ❌ builder 不支持 | pass-through stub（只计数，不写存储） |
| GetVertices/GetEdges/GetNeighbors/IndexScan | ❌ builder 不支持 | 返回 "requires storage integration" 错误 |
| FulltextSearch/VectorSearch | ❌ builder 不支持 | pass-through stub |
| BeginTransaction/Commit/Rollback | ❌ builder 不支持 | pass-through stub |

---

## 二、分阶段清理方案

### Phase 1：安全清理（可立即执行，不影响功能）

**目标**：清除编译错误和无意义文件，收窄清理范围。

1. 删除 `control_flow/` 空模块（只剩 `mod.rs` 注释）
2. 删除 7 个缺失模块的 `pub mod` 声明：
   - `graph_traversal/{expand,expand_all,shortest_path,traverse}.rs`
   - `graph_operations/materialize.rs`
   - `selection/filter.rs`
   - `UnwindExecutor` re-export
3. 删除 `explain/instrumented_executor.rs`（已完成）
4. 删除 `utils/object_pool.rs`（已完成）
5. 删除 `base/manage_executor_enums.rs`（已完成）
6. 删除 `ExecutorEnum` stub + 相关 `pub mod` 和 re-export（已完成）
7. 清理 `executor/mod.rs` 中的已删除类型 re-export（已完成）

### Phase 2：补充 Streaming 实现（前提条件，使后续删除不破坏功能）

**目标**：为所有旧模块对应的 streaming operator 补充真正实现，并扩展 builder。

#### 2a. 补充 Graph Traversal 实现

需要让 `streaming/executor/operators/graph_traversal.rs` 中的 11 个 operator
（Expand, ExpandAll, Traverse, TraverseAll, AppendVertices, BiExpand, BiTraverse,
ShortestPath, BFSShortest, AllPaths, MultiShortestPath）具备实际路径搜索/遍历逻辑，
而非 pass-through。

#### 2b. 补充 DDL/Management 实现

需要让 `streaming/executor/operators/management.rs` 中的 7 个 operator
（SpaceManage, TagManage, EdgeManage, IndexManage, UserManage,
FulltextManage, VectorManage）具备实际 DDL 执行逻辑。

需要引入存储层连接，复用旧 admin executor 中的业务逻辑。

#### 2c. 补充 Data Modification 实现

需要让 `streaming/executor/operators/data_modification.rs` 中的 8 个 operator
（InsertVertices, InsertEdges, UpdateVertices, UpdateEdges, DeleteVertices,
DeleteEdges, PipeDeleteVertices, PipeDeleteEdges）具备实际写入逻辑。

#### 2d. 补充 Data Access 实现

需要让 `streaming/executor/operators/access.rs` 中的 8 个 operator
（Start, GetVertices, GetEdges, GetNeighbors, IndexScan, EdgeIndexScan,
Argument, Sample）不再返回 error。

Sign 实际需要存储层集成。

#### 2e. 扩展 StreamingExecutorBuilder

为 `from_plan_node()` 增加对新 PlanNodeEnum 变体的匹配分支：
- SpaceManage/TagManage/EdgeManage/IndexManage/UserManage
- Expand/ExpandAll/Traverse/TraverseAll
- InsertVertices/InsertEdges/UpdateVertices/UpdateEdges/DeleteVertices/DeleteEdges
- GetVertices/GetEdges/GetNeighbors
- FulltextSearch/FulltextLookup/MatchFulltext/VectorSearch/VectorLookup

### Phase 3：删除旧模块 + 结构调整

**目标**：删除旧 executor 代码，重组目录结构。

3a. 删除 `admin/`（DDL 旧实现）
3b. 删除 `data_access/`（数据读取旧实现）
3c. 删除 `data_modification/`（数据写入旧实现）
3d. 删除 `graph_operations/`（图遍历旧实现），将 `algorithms/` 提至 `executor/` 下
3e. 删除 `relational_algebra/`（join/filter/set ops 旧实现）
3f. 删除 `result_processing/`（聚合/转换旧实现）
3g. 清理 `executor/mod.rs` 中所有旧类型的 re-export

### Phase 4：后续优化

4a. `admin/fulltext_index/` 提级到 `admin/fulltext_index/`（与 space/tag/edge 平级）
4b. 清理 build 时的 unreachable_pattern 警告（streaming executor.rs 的 `_ =>` 兜底臂）
4c. 修复 explain 路径：将 `execute_with_instrumentation`/`execute_profiled` 接入 streaming
4d. 重写 `executor/mod.rs` 注释，去掉对旧体系的引用

---

## 三、依赖关系

```
Phase 1（安全删除）
    ↓
Phase 2（补充实现）—— 最多工作量
    ├── 2a Graph Traversal
    ├── 2b DDL/Management
    ├── 2c Data Modification
    ├── 2d Data Access
    └── 2e Builder 扩展
    ↓
Phase 3（删除旧模块）
    ↓
Phase 4（后续优化，无严格依赖）
```

## 四、风险

- Phase 2 是 gate：在 streaming 实现完整前删除旧代码会破坏功能
- 旧 `admin/` executor 中有大量 test 代码（`tests.rs`），清理时需迁移
- `explain_executor.rs` 和 `profile_executor.rs` 仍用旧 `BaseExecutor`，需单独改造
- 需确认 Builder 的 PlanNodeEnum 覆盖完整后，再执行 Phase 3
