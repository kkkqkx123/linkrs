# Fulltext/Vector Query 集成分析与重构方案

## 1. 现有架构概览

### 1.1 Fulltext 集成路径

```
Query String
    │
    ▼
[Parser] ─── fulltext_parser.rs ───▶ AST (8 Stmt variants)
    │
    ▼
[Planner] ─── FulltextSearchPlanner ───▶ Plan Nodes (8 nodes)
    │
    ▼
[Compiler] ───▶ Operator Specs (FulltextSpec)
    │
    ▼
[Executor] ─── FulltextOperator::next() ───▶ FulltextIndexManager
    │
    ▼
[TantivySearchEngine] ───▶ BM25 results
```

**关键组件位置：**

| 组件 | 文件路径 |
|------|----------|
| AST 定义 | `crates/graphdb-query/src/parser/ast/fulltext.rs` |
| 解析器 | `crates/graphdb-query/src/parser/parsing/fulltext_parser.rs` |
| 规划器 | `crates/graphdb-query/src/planning/fulltext_planner.rs` |
| Plan 节点 | `crates/graphdb-query/src/planning/plan/core/nodes/search/fulltext.rs` |
| Spec 定义 | `crates/graphdb-query/src/executor/streaming/operators/spec.rs` |
| Operator | `crates/graphdb-query/src/executor/streaming/operators/fulltext_operator.rs` |
| 表达式函数 | `crates/graphdb-query/src/executor/expression/functions/fulltext.rs` |
| 引擎管理 | `crates/graphdb-fulltext/src/manager.rs` |
| 后端引擎 | `crates/graphdb-fulltext/src/engine.rs` |

**ExecutionContext 持有方式：**

```rust
// crates/graphdb-query/src/executor/base/execution_context.rs
pub struct ExecutionContext {
    #[cfg(feature = "fulltext")]
    pub search_engine: Option<Arc<dyn FulltextSearchEngine>>,  // trait object
    #[cfg(feature = "fulltext")]
    pub fulltext_manager: Option<Arc<FulltextIndexManager>>,   // 具体类型
}
```

**Pipeline 注入：**

```rust
// crates/graphdb-query/src/pipeline.rs
pub struct QueryPipelineManager<S: QueryStorage + 'static> {
    #[cfg(feature = "fulltext")]
    pub(crate) fulltext_manager: Option<Arc<FulltextIndexManager>>,
}
```

### 1.2 Vector 集成路径

```
Query String
    │
    ▼
[Parser] ─── vector_parser ───▶ AST (5 Stmt variants)
    │
    ▼
[Planner] ─── VectorSearchPlanner ───▶ Plan Nodes (5 nodes)
    │
    ▼
[Compiler] ───▶ Operator Specs (VectorSpec)
    │
    ▼
[Executor] ─── VectorOperator::next() ───▶ VectorSyncCoordinator
    │
    ▼
[VectorBackend] ───▶ LocalVectorEngine / QdrantClient
```

**关键组件位置：**

| 组件 | 文件路径 |
|------|----------|
| AST 定义 | `crates/graphdb-query/src/parser/ast/vector.rs` |
| 解析器 | `crates/graphdb-query/src/parser/parsing/vector_parser.rs` |
| 规划器 | `crates/graphdb-query/src/planning/vector_planner.rs` |
| Plan 节点 | `crates/graphdb-query/src/planning/plan/core/nodes/search/vector.rs` |
| Spec 定义 | `crates/graphdb-query/src/executor/streaming/operators/spec.rs` |
| Operator | `crates/graphdb-query/src/executor/streaming/operators/vector_operator.rs` |
| 同步协调器 | `crates/graphdb-sync/src/vector_sync.rs` |
| 本地引擎 | `crates/vector-search/src/engine.rs` |
| 远程客户端 | `crates/vector-client/src/client.rs` |

**ExecutionContext 持有方式：**

```rust
// crates/graphdb-query/src/executor/base/execution_context.rs
pub struct ExecutionContext {
    #[cfg(feature = "vector")]
    pub vector_coordinator: Option<Arc<VectorSyncCoordinator>>,  // 具体类型
}
```

**Pipeline 注入：**

```rust
// crates/graphdb-query/src/pipeline.rs
pub struct QueryPipelineManager<S: QueryStorage + 'static> {
    #[cfg(feature = "vector")]
    pub(crate) vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
}
```

---

## 2. 框架层集成模式对比

### 2.1 相同点

| 方面 | Fulltext | Vector | 一致性 |
|------|----------|--------|--------|
| Planner 注册 | `PlannerEnum::FulltextSearch` | `PlannerEnum::VectorSearch` | ✅ 相同 |
| Plan 节点基类 | `PlanNode + ZeroInputNode` | `PlanNode + ZeroInputNode` | ✅ 相同 |
| Metadata 预解析 | `.with_metadata()` builder | `.with_metadata()` builder | ✅ 相同 |
| Logical/Physical 转换 | `convert_logical_to_physical` | `convert_logical_to_physical` | ✅ 相同 |
| Spec 构建 | 从 PlanNode 生成 Spec | 从 PlanNode 生成 Spec | ✅ 相同 |
| Output | `DataChunk` rows | `DataChunk` rows | ✅ 相同 |

### 2.2 差异点

| 方面 | Fulltext | Vector | 影响 |
|------|----------|--------|------|
| **Feature gate** | `fulltext` | `vector` | 命名不一致 |
| **ExecutionContext 字段数** | 2 个（`search_engine` + `fulltext_manager`） | 1 个（`vector_coordinator`） | 抽象层次不对称 |
| **MetadataContext 注册** | ❌ 未注册 | ✅ 注册并预解析 | Fulltext 缺乏早期错误检测 |
| **后端抽象** | 单后端（Tantivy） | 双后端（Local + Qdrant） | Vector 更灵活 |
| **一致性模型** | 无 | Read-Your-Writes (outbox + LSN) | 混合查询语义不一致 |
| **DDL 命令数** | 5 个（CREATE/DROP/ALTER/SHOW/DESCRIBE） | 2 个（CREATE/DROP） | Fulltext 管理能力更完整 |
| **Plan 节点数** | 8 个 | 5 个 | Fulltext 更复杂 |

---

## 3. 现有设计评估

### 3.1 优点

1. **架构一致性**：两者遵循相同的 `Parser → Planner → PlanNode → Spec → Operator` 管道，框架层代码高度对称
2. **Feature gate 隔离**：可选依赖正确隔离，不影响核心功能
3. **静态分发**：通过 `PlannerEnum`/`PlanNodeEnum` 枚举实现静态分发，符合项目消除动态分发的目标
4. **类型安全**：所有类型在编译时确定，无运行时类型转换开销

### 3.2 问题

| 问题 | 严重程度 | 说明 |
|------|----------|------|
| **抽象层次不一致** | 中 | Fulltext 在 ExecutionContext 中使用 trait object (`dyn FulltextSearchEngine`)，Vector 使用具体类型 (`VectorSyncCoordinator`)。两者在框架层的抽象模式不同 |
| **元数据解析策略不同** | 中 | Fulltext 的索引元数据不在 `MetadataContext` 中注册，导致索引存在性检查延迟到执行时。Vector 注册了，支持早期错误检测 |
| **缺乏统一搜索抽象** | 低 | 两者没有共享的 `SearchEngine` trait，无法多态处理。虽然符合消除 dyn 的目标，但增加了框架扩展成本 |
| **一致性模型差异** | 低 | Vector 有 RYW 一致性支持，Fulltext 没有。在混合查询场景下可能产生语义不一致 |
| **VectorQueryExpr 未完全接线** | 低 | `VectorQueryExpr::Text` 和 `Parameter` 类型在 spec builder 中被拒绝，尚未实现端到端 |

---

## 4. 修改方案

### 4.1 方案一：统一 SearchProvider trait（推荐）

**目标**：引入轻量级 trait 统一 DDL 管理接口，减少 `pipeline.rs` 和 `execution_context.rs` 中的重复代码。

**步骤：**

1. 在 `graphdb-query` 中定义 `SearchProvider` trait：

```rust
// crates/graphdb-query/src/executor/traits.rs (新增)
pub trait SearchProvider: Send + Sync + Debug + 'static {
    fn name(&self) -> &str;
    fn provider_type(&self) -> SearchProviderType;
}

pub enum SearchProviderType {
    Fulltext,
    Vector,
}
```

2. 让 `FulltextIndexManager` 和 `VectorSyncCoordinator` 实现该 trait：

```rust
// crates/graphdb-fulltext/src/manager.rs
impl SearchProvider for FulltextIndexManager { ... }

// crates/graphdb-sync/src/vector_sync.rs
impl SearchProvider for VectorSyncCoordinator { ... }
```

3. 在 `ExecutionContext` 中使用 trait object：

```rust
pub struct ExecutionContext {
    pub search_providers: Vec<Arc<dyn SearchProvider>>,
    // 保留具体类型的字段用于直接访问
    #[cfg(feature = "fulltext")]
    pub fulltext_manager: Option<Arc<FulltextIndexManager>>,
    #[cfg(feature = "vector")]
    pub vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
}
```

**优点**：
- 保持向后兼容，不破坏现有代码
- 提供统一的 provider 枚举和管理接口
- 便于未来添加新的搜索类型

**缺点**：
- 引入了新的 trait 和动态分发（仅用于 provider 列表）
- 需要修改多个 crate

### 4.2 方案二：统一 MetadataContext 注册

**目标**：让 Fulltext 索引也在 planning 阶段注册到 `MetadataContext`，实现早期错误检测。

**步骤：**

1. 在 `compiler.rs` 的 `build_metadata_context()` 中添加 Fulltext 索引注册：

```rust
// crates/graphdb-query/src/pipeline/compiler.rs
#[cfg(feature = "fulltext")]
if let Some(manager) = &self.fulltext_manager {
    for meta in manager.list_indexes() {
        metadata_context.register_index(
            meta.space_id,
            meta.tag_name,
            meta.field_name,
            IndexType::Fulltext,
        );
    }
}
```

2. 在 `metadata_context.rs` 中添加 `IndexType::Fulltext` 变体：

```rust
pub enum IndexType {
    Property,
    Vector,
    Fulltext,  // 新增
}
```

3. 在 `FulltextSearchPlanner` 中利用预解析的元数据：

```rust
fn transform_with_metadata(&self, stmt: &Stmt, ctx: &MetadataContext) -> Result<PlanNodeEnum> {
    // 利用 ctx 获取 space_id, tag_name, field_name
    // 提前验证索引存在性
}
```

**优点**：
- 实现早期错误检测，提高用户体验
- 与 Vector 的元数据解析策略保持一致
- 不改变现有 API

**缺点**：
- 需要修改 Fulltext 的 planner 实现
- 可能引入额外的 metadata 查询开销

### 4.3 方案三：统一 Feature Gate 命名

**目标**：统一 feature gate 命名规范。

**步骤：**

1. 在 `graphdb-query/Cargo.toml` 中统一命名：

```toml
[features]
default = []
fulltext = ["graphdb-fulltext/fulltext", "graphdb-sync/fulltext"]
vector = ["dep:vector-search", "graphdb-sync/vector"]
```

2. 修改所有 `#[cfg(feature = "fulltext")]` 为 `#[cfg(feature = "fulltext")]`

**优点**：
- 命名一致性更好
- 更符合 Rust 生态的 feature gate 命名惯例

**缺点**：
- 需要修改多处代码
- 可能破坏现有用户配置

---

## 5. 实施优先级建议

| 优先级 | 方案 | 原因 |
|--------|------|------|
| P1 | 方案二（MetadataContext 注册） | 改进用户体验，风险低 |
| P2 | 方案一（SearchProvider trait） | 提升框架扩展性，需要更多测试 |
| P3 | 方案三（Feature Gate 重命名） | 低优先级，可延后 |

**注意**：根据项目文档 `AGENTS.md`，当前项目处于开发阶段，无需特别考虑向后兼容性。因此可以更积极地进行架构优化。

---

## 6. 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `crates/graphdb-query/src/executor/traits.rs` | 新增 | SearchProvider trait 定义 |
| `crates/graphdb-query/src/executor/base/execution_context.rs` | 修改 | 添加 search_providers 字段 |
| `crates/graphdb-query/src/pipeline.rs` | 修改 | 统一 provider 注入逻辑 |
| `crates/graphdb-query/src/pipeline/compiler.rs` | 修改 | 添加 Fulltext 索引注册 |
| `crates/graphdb-query/src/planning/plan/core/nodes/search/metadata_context.rs` | 修改 | 添加 IndexType::Fulltext |
| `crates/graphdb-fulltext/src/manager.rs` | 修改 | 实现 SearchProvider trait |
| `crates/graphdb-sync/src/vector_sync.rs` | 修改 | 实现 SearchProvider trait |
| `crates/graphdb-query/src/planning/fulltext_planner.rs` | 修改 | 利用预解析元数据 |
