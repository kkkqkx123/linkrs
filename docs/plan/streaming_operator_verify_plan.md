# Streaming Executor 实现验证与补全计划

## 背景

旧 `Executor<S>` trait 体系已被删除，`StreamingExecutor` enum 体系已建立。
`temp/executor/` 目录保存了旧代码快照，用于对照验证和逻辑补全。

本文档分析：
1. 逐个模块对比 streaming 实现与旧实现的差异
2. 评估 79-variant enum 架构 vs 旧 struct+generic 架构的优劣
3. 提出改进建议
4. 给出分阶段验证/补全计划

---

## 一、模块对照分析

### 1.1 数据访问（旧 data_access/ → streaming operators/access + sources）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `GetVerticesExecutor` | 调用 `storage.get_vertex()` / `scan_vertices()` | `GetVertices` 返回 `None`（静默跳过） | **严重缺失**：无存储调用，数据静默丢失 |
| `GetEdgesExecutor` | 调用 `storage.get_edge()` / `scan_edges_by_type()` | `GetEdges` 返回 `None` | **严重缺失** |
| `GetNeighborsExecutor` | 调用 `storage.get_node_edges()` + `get_vertex()` 聚合邻居 | `GetNeighbors` 返回 `None` | **严重缺失** |
| `IndexScanExecutor` | 按索引类型(UNIQUE/PREFIX/RANGE/FULL)扫描 + 过滤 | `IndexScan` 返回 `None` | **严重缺失** |
| `ScanVerticesExecutor` | 调用 `storage.scan_vertices()` + tag/vertex 过滤 | `ScanVertices` 有 buffer 遍历逻辑 | **半完成**：buffer 需要从存储层预加载 |
| `ScanEdgesExecutor` | 调用 `scan_edges_by_type()` / `scan_all_edges()` + 过滤 | `ScanEdges` 有 buffer 遍历逻辑 | **半完成** |

**结论**：访问类 operator（GetVertices/GetEdges/GetNeighbors/IndexScan/EdgeIndexScan）目前全部返回 `None`，依赖 planner 优化掉它们。如果 planner 失效，数据静默丢失。需要存储层访问能力。

### 1.2 数据修改（旧 data_modification/ → streaming operators/data_modification）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `InsertExecutor` | 调用 `storage.insert_vertex()` / `insert_edge()`，支持 `if_not_exists` | 仅计数 row，无存储写入 | **严重缺失** |
| `UpdateExecutor` | `get_vertex()` → 条件求值 → `update_vertex()`，支持 upsert | 仅计数 row | **严重缺失** |
| `DeleteExecutor` | 级联删除边+点，条件过滤，tag/index 删除 | 仅计数 row | **严重缺失** |
| `TagOpsExecutor` | 调用 `add_tag()` / `drop_tag()` | 无对应实现 | **缺失** |
| `IndexOpsExecutor` | 调用 `create_tag_index()` / `drop_tag_index()` | 无对应实现 | **缺失** |

**结论**：所有数据修改 operator 目前只计数，不实际写入。需要接入 `StorageWriter` 接口。

### 1.3 图遍历（旧 graph_operations/graph_traversal/ → streaming operators/graph_traversal）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `ExpandExecutor` | 调用 `get_node_edges()` + 按 direction/edge_type 过滤 | 仅添加 metadata 列（edge_type, direction），无边查询 | **严重缺失** |
| `ExpandAllExecutor` | 同上，但递归遍历所有深度 | 同 Expand，pass-through | **严重缺失** |
| `TraverseExecutor` | 递归扩展，visited set，depth tracking | visited set 维护了但无边查询 | **严重缺失** |
| `ShortestPathExecutor` | 委托给 BFS/Dijkstra/A* 算法实现 | 仅添加 metadata 列 | **严重缺失** |
| `AllPathsExecutor` | 双向 BFS 算法，NPath 路径共享 | 有缓冲但无边查询 | **严重缺失** |
| `BiExpand/BiTraverse` | 双边扩展，用于双向搜索 | visited set 维护了但无边查询 | **严重缺失** |

**结论**：所有图遍历 operator 保留了状态机框架（open/next/stop/close）和 visited set 追踪，但缺少核心的 `storage.get_node_edges()` 调用。需要接入存储层边查询。

`algorithms/` 模块已成功从 `graph_operations/graph_traversal/algorithms/` 迁移到 `executor/algorithms/`，内容完整。

### 1.4 Join 操作（旧 relational_algebra/join/ → streaming operators/binary）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `InnerJoinExecutor` | Hash Join：build hash 表 + probe + 拼接行 | `InnerJoin`/`HashJoin` 已实现类似逻辑 | **基本正确**，但缺少 hash 表优化（当前是 nested loop） |
| `LeftJoinExecutor` | Hash Join + 保留未匹配左行 + NULL 填充 | `LeftJoin` 已实现 | **基本正确** |
| `CrossJoinExecutor` | 笛卡尔积 | `CrossJoin`/`NestedLoopJoin` 已实现 | **基本正确** |
| `SemiJoinExecutor` | 返回左行中在右表有匹配的行 | `SemiJoin` 已实现 | **基本正确** |
| `FullOuterJoinExecutor` | 三阶段：build → probe → emit unmatched right | 已初步实现 | **需验证** |
| `RightJoinExecutor` | 与 LeftJoin 对称 | 已初步实现 | **需验证** |

**结论**：Join 操作是目前 streaming 中实现最完整的模块。主要缺失是 hash 表优化（当前都是 nested loop）。

### 1.5 Set 操作（旧 relational_algebra/set_operations/ → streaming operators/set_ops）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `UnionExecutor` | 合并 + HashSet 去重 | `Union` 已实现 | **基本正确** |
| `UnionAllExecutor` | 合并，不去重 | `UnionAll` 已实现 | **基本正确** |
| `IntersectExecutor` | 两阶段 buffering + 求交 | `Intersect` 已实现 | **基本正确** |
| `MinusExecutor` | 左表减右表 | `Minus`/`Except` 已实现 | **基本正确** |

### 1.6 DDL/Management（旧 admin/ → streaming operators/management）

| 旧实现 | 旧行为 | Streaming 状态 | 问题 |
|--------|--------|---------------|------|
| `SpaceManage` (create/drop/alter/desc/clear/switch) | 调用 `storage.create_space()` / `drop_space()` 等 | 返回伪结果 `{action, name, "executed"}` | **严重缺失** |
| `TagManage` (create/alter/drop/desc) | 调用 `storage.create_tag()` / `alter_tag()` 等 | 同上 | **严重缺失** |
| `EdgeManage` (create/alter/drop/desc) | 调用 `storage.create_edge()` / `alter_edge()` 等 | 同上 | **严重缺失** |
| `IndexManage` | 调用索引 DDL | 同上 | **严重缺失** |
| `UserManage` | 调用用户 DDL | 同上 | **严重缺失** |

**结论**：所有 DDL operator 目前只返回伪结果，不执行任何实际操作。需要接入 StorageClient 接口。

### 1.7 表达式求值（expression/ → 保持不变）

表达式系统未改动，功能完整。无需验证。

### 1.8 Explain/Profile（explain/）

| 组件 | 状态 | 问题 |
|------|------|------|
| `ExplainExecutor` (PlanOnly 模式) | 正常工作 | 格式化和 plan 生成正常 |
| `ExplainExecutor` (Analyze 模式) | `execute_with_instrumentation()` 返回 `Empty` | **严重缺失**：不实际执行 |
| `ProfileExecutor` | 同上，`execute_profiled()` 返回 `Empty` | **严重缺失** |

### 1.9 其他模块

| 旧模块 | 状态 |
|--------|------|
| `control_flow/` | 已删除，streaming 中有对应 `control_flow.rs`（pass-through） |
| `result_processing/` | 已删除，功能分散到 streaming relational operators |
| `utils/` | 已删除，`object_pool`/`recursion_detector`/`tag_filter` 不再使用 |

---

## 二、架构评估：79-variant Enum vs 旧 Struct+Generic

### 2.1 旧架构特点

```rust
// 每个 operator 一个 struct，通过 Executor<S> trait 统一
pub struct GetVerticesExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
    vertex_ids: Option<Vec<Value>>,
    // ...
}

impl<S: StorageReader> Executor<S> for GetVerticesExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // 直接调用 storage 方法
        let vertex = self.base.get_storage().read().get_vertex(...)?;
        // ...
    }
}

// 工厂模式创建
let executor = GraphTraversalExecutorFactory::create_expand_executor(params);
```

**优点**：
- 类型安全，每个 operator 有精确的字段类型
- 存储访问通过泛型 `S` 直接传递，编译期保证
- 每个 operator 独立文件，易于测试
- 可扩展：新增 operator = 新增 struct

**缺点**：
- 大量重复的 trait 实现模板代码（每个 struct 都 impl Executor<S>）
- `Executor<S>` 的 `execute()` 返回 `ExecutionResult`（整个 DataSet），不支持 streaming pull
- 泛型 `S: StorageClient` 爆炸：所有 operator 都绑定具体存储实现
- 工厂模式复杂，类型推导困难
- `dyn` 使用受限，难以实现 operator 的动态组合

### 2.2 新架构特点

```rust
// 79 个 variant 的 enum
pub enum StreamingExecutor {
    ScanVertices { partition_id, buffer, current_index },
    Filter { input, predicate, opened },
    Expand { input, edge_type, direction, filter_expr, opened },
    // ... 76 more
}

// 函数分发
impl StreamingExecutor {
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Filter { .. } => operators::single_input::next_filter(self),
            Self::Expand { .. } => operators::graph_traversal::next_expand(self),
            // ...
        }
    }
}

// DataChunk 流式 pull
let chunk = executor.next()?;
```

**优点**：
- **流式 pull 模型**：`next() -> Option<DataChunk>`，支持 pipeline 执行，内存可控
- **无泛型**：enum 不需要 `S: StorageClient`，减少编译复杂度
- **统一接口**：所有 operator 通过 4 个方法（open/next/stop/close）统一
- **数据格式统一**：DataChunk 作为唯一数据交换格式
- **函数分派**：每个操作有独立函数，无需 trait 实现

**缺点**：
- **79 个 match arm × 4 个方法 = 316 行纯路由**：容易遗漏或配错
- **类型不安全**：enum variant 的字段是匿名 struct，编译器不检查匹配完整性
- **存储访问不直接**：当前无泛型 `S`，存储需要额外机制（ExecutionContext 或全局变量）
- **enum 膨胀**：修改 1 个 variant 需要修改所有 4 个方法的匹配臂
- **不支持外部扩展**：enum 的变体是封闭的，无法在 crate 外添加 operator

### 2.3 评估结论

**新架构更适合 streaming pull 执行模型**，但当前 enum 设计过于扁平化。

旧架构在类型安全和存储集成方面更好，新架构在流式执行和可组合性方面更好。

核心问题：**新架构放弃了泛型 `S`，导致存储访问能力丢失**。这是当前所有数据访问/修改/DDL operator 无法工作的根本原因。

---

## 三、改进建议

### 3.1 架构改进：Trait Object 替代 79-variant Enum

建议从单一的 `StreamingExecutor` enum 改为 **trait-based 组合 + 有限的 enum 分发**：

```rust
/// 核心 operator trait（pull-based）
pub trait StreamingOperator: Send {
    fn open(&mut self) -> Result<(), QueryError>;
    fn next(&mut self) -> Result<Option<DataChunk>, QueryError>;
    fn stop(&mut self) -> Result<(), QueryError>;
    fn close(&mut self) -> Result<(), QueryError>;
}

/// 带存储访问的 operator（需要访问 storage 的 operator 实现此 trait）
pub trait StorageOperator<S: StorageClient>: StreamingOperator {
    fn set_storage(&mut self, storage: Arc<RwLock<S>>);
}

/// Pipeline 节点（编译期已知类型）
pub enum PipelineNode<S: StorageClient> {
    Source(Box<dyn StreamingOperator>),
    Transform(Box<dyn StreamingOperator>),
    Sink(Box<dyn StreamingOperator>),
    // 管理类 operator 需要 storage
    Management(Box<dyn StorageOperator<S>>),
}
```

**优势**：
- 类型安全 + 开放扩展（外部 crate 可实现 `StreamingOperator`）
- enum 从 79 变 4 个，match 代码减少 90%
- 存储访问通过 `StorageOperator` 子 trait 隔离，不影响纯数据 operator
- 向后兼容：现有函数可以直接包装为 `StreamingOperator` impl

### 3.2 存储访问改进

当前 streaming operator 无法访问存储层。建议两种方案：

**方案 A**：在 `ExecutionContext` 中提供存储访问方法
```rust
// execution_context.rs
impl ExecutionContext {
    pub fn get_storage_client(&self) -> Option<&Arc<dyn StorageClient>> { ... }
}
```

**方案 B**：存储层通过 trait bound 传递
```rust
impl<S: StorageClient> StreamingExecutor {
    pub fn next_with_storage(&mut self, storage: &S) -> Result<Option<DataChunk>> { ... }
}
```

推荐方案 A，不需要改动 enum 定义。

### 3.3 Operator 分组优化

建议将当前 79 个 variant 按优先级分组：

**P0 - 核心查询路径**（已实现，需验证正确性）：
ScanVertices, ScanEdges, Filter, Project, Limit, Distinct, Aggregate, Sort, GroupBy, WindowFunction, HashJoin, Union, UnionAll, Intersect, Except

**P1 - 近期需要存储集成**：
GetVertices, GetEdges, GetNeighbors, IndexScan, EdgeIndexScan, InsertVertices, InsertEdges, UpdateVertices, UpdateEdges, DeleteVertices, DeleteEdges, Expand, ExpandAll, Traverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath, AppendVertices

**P2 - DDL/Management**（需要 StorageClient）：
SpaceManage, TagManage, EdgeManage, IndexManage, UserManage, FulltextManage, VectorManage

**P3 - 低优先级**：
FulltextSearch, FulltextLookup, MatchFulltext, VectorSearch, VectorLookup, TopN, Materialize, Remove, DataCollect, Unwind, Apply, PatternApply, RollUpApply, Loop, Select, BeginTransaction, Commit, Rollback

---

## 四、分阶段验证/补全计划

### Phase A：验证核心实现正确性（对照 temp/）

| 步骤 | 文件 | 验证内容 | 参考旧代码 |
|------|------|---------|-----------|
| A1 | `binary.rs` | Join 逻辑：确保 HashJoin/InnerJoin/LeftJoin/CrossJoin/SemiJoin 结果正确 | `temp/.../relational_algebra/join/inner_join.rs` |
| A2 | `set_ops.rs` | 集合操作：Union/UnionAll/Intersect/Except/Minus 正确性 | `temp/.../set_operations/` |
| A3 | `single_input.rs` | Filter/Project/Limit/Distinct 表达式求值正确性 | `temp/.../selection/` |
| A4 | `stateful.rs` | Aggregate/Sort/GroupBy/WindowFunction 聚合/排序逻辑 | `temp/.../result_processing/agg_function_manager.rs` |
| A5 | `sources.rs` | ScanVertices/ScanEdges buffer 遍历逻辑 | `temp/.../data_access/vertex.rs`, `edge.rs` |
| A6 | `relational.rs` | TopN/Dedup/Assign/Materialize/Remove/DataCollect/Minut 正确性 | `temp/.../transformations/` |
| A7 | `graph_traversal.rs` | Traverse visited set、AllPaths 缓冲逻辑 | `temp/.../graph_traversal/` |
| A8 | `management.rs` | DDL 返回结果格式 | `temp/.../admin/` |
| A9 | `data_modification.rs` | 计数逻辑 | `temp/.../data_modification/` |
| A10 | `control_flow.rs` | Loop/Select/Transaction pass-through | `temp/.../control_flow/` |

### Phase B：补全存储集成

| 步骤 | 文件 | 需要补全的功能 | 参考旧代码 |
|------|------|--------------|-----------|
| B1 | `access.rs` | GetVertices: 实现 `storage.get_vertex()` / `scan_vertices()` 调用 | `temp/.../data_access/vertex.rs` |
| B2 | `access.rs` | GetEdges: 实现 `storage.get_edge()` / `scan_edges_by_type()` 调用 | `temp/.../data_access/edge.rs` |
| B3 | `access.rs` | GetNeighbors: 实现 `storage.get_node_edges()` + `get_vertex()` | `temp/.../data_access/neighbor.rs` |
| B4 | `access.rs` | IndexScan: 实现索引扫描逻辑 | `temp/.../data_access/index.rs`, `search.rs` |
| B5 | `data_modification.rs` | InsertVertices/InsertEdges: 调用 `storage.insert_vertex/edge()` | `temp/.../data_modification/insert.rs` |
| B6 | `data_modification.rs` | UpdateVertices/UpdateEdges: 调用 `storage.update_vertex/edge()` | `temp/.../data_modification/update.rs` |
| B7 | `data_modification.rs` | DeleteVertices/DeleteEdges: 级联删除 | `temp/.../data_modification/delete.rs` |
| B8 | `graph_traversal.rs` | Expand/Traverse: 调用 `storage.get_node_edges()` | `temp/.../data_access/neighbor.rs`, `temp/.../graph_traversal/` |
| B9 | `management.rs` | 所有 DDL operator 调用 storage DDL 方法 | `temp/.../admin/` 下所有文件 |

### Phase C：补全缺失算子

| 步骤 | 算子 | 说明 |
|------|------|------|
| C1 | `Unwind` | **已完成**：基于 col_index 的 list flattening |
| C2 | `RightJoin`/`FullOuterJoin` | **已完成**：与 LeftJoin 对称的实现 |
| C3 | Explain Analyze / Profile | TODO：接入 StreamingQueryExecutor |
| C4 | FulltextSearch / VectorSearch | 搜索算子，需要全文索引/向量索引集成 |

### Phase D：测试与集成

| 步骤 | 内容 |
|------|------|
| D1 | 为 Phase A 所有算子编写单元测试（对照旧 tests.rs） |
| D2 | 为 Phase B 存储集成编写集成测试 |
| D3 | 验证 query_pipeline_manager 到 streaming 的全链路 |
| D4 | 清理 builder.rs 中 `_ => {}` 兜底分支 |

---

## 五、风险与依赖

- **根本依赖**：存储集成是 Phase B 的 gate，在完成前所有访问/修改/管理/图遍历 operator 都无法真正工作
- **类型安全**：`StreamingOperator` trait 方案（3.1）可以在 Phase C 之后作为独立重构进行，不影响当前代码
- **兼容性**：`executor/mod.rs` 已清理 re-export，不影响其他 crate 的编译
- **测试**：旧 admin/ 目录有 6 个 `tests.rs`（共 1500+ 行测试），需要迁移到新的 streaming 测试框架
