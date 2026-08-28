# Fulltext 外部引擎扩展设计

> 状态：设计文档（2026-08-28）
> 前置文档：
> - `docs/plan/fulltext_vector_architecture_refactor.md`（Phase 2: Fulltext 后端抽象）
> - `crates/graphdb-fulltext/src/engine.rs`（`FulltextSearchEngine` trait 定义）

---

## 1. 背景

当前项目仅支持 Tantivy 作为全文搜索引擎。随着业务需求演进，可能需要接入外部全文搜索引擎（如 Elasticsearch、Meilisearch、Typesense 等）以获得：
- 分布式全文检索能力
- 更丰富的查询语法（聚合、高亮、同义词）
- 已有的搜索基础设施复用

本设计文档基于 Phase 2 已完成的 `FulltextSearchEngine` trait，给出外部引擎接入的初步设计。

---

## 2. 已有基础设施

### 2.1 `FulltextSearchEngine` trait

Phase 2 在 `crates/graphdb-fulltext/src/engine.rs` 中定义了后端无关的搜索引擎 trait：

```rust
#[async_trait]
pub trait FulltextSearchEngine: Send + Sync + std::fmt::Debug + 'static {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError>;
    async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError>;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError>;
    async fn delete(&self, doc_id: &str) -> Result<(), SearchError>;
    async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError>;
    async fn commit(&self) -> Result<(), SearchError>;
    async fn commit_with_payload(&self, payload: String) -> Result<(), SearchError>;
    fn commit_payload(&self) -> Result<Option<String>, SearchError>;
    async fn rollback(&self) -> Result<(), SearchError>;
    async fn stats(&self) -> Result<IndexStats, SearchError>;
    fn consistency_state(&self) -> ConsistencyState;
    fn mark_inconsistent(&self);
    fn mark_consistent(&self);
    async fn clear(&self) -> Result<(), SearchError>;
    async fn close(&self) -> Result<(), SearchError>;
}
```

`TantivySearchEngine` 已实现此 trait。`MetricsSearchEngine` 装饰器已改为接受 `Arc<dyn FulltextSearchEngine>`。

### 2.2 `FulltextIndexManager`

`FulltextIndexManager` 当前直接持有 `DashMap<IndexKey, Arc<TantivySearchEngine>>`。外部引擎扩展需要将其改为 `DashMap<IndexKey, Arc<dyn FulltextSearchEngine>>`。

---

## 3. 架构设计

### 3.1 Engine Factory 模式

引入 `FulltextEngineFactory` trait，由 `FulltextIndexManager` 在创建索引时调用：

```rust
pub trait FulltextEngineFactory: Send + Sync + 'static {
    /// Create a new search engine instance for the given index path.
    fn create_engine(&self, index_path: &Path) -> Result<Arc<dyn FulltextSearchEngine>, SearchError>;
}
```

默认实现（Tantivy）：

```rust
pub struct TantivyEngineFactory {
    config: TantivyConfig,
}

impl FulltextEngineFactory for TantivyEngineFactory {
    fn create_engine(&self, index_path: &Path) -> Result<Arc<dyn FulltextSearchEngine>, SearchError> {
        let engine = TantivySearchEngine::open_or_create(index_path, self.config.clone())?;
        Ok(Arc::new(engine))
    }
}
```

Elasticsearch 实现（未来）：

```rust
pub struct ElasticsearchEngineFactory {
    config: ElasticsearchConfig,
}

impl FulltextEngineFactory for ElasticsearchEngineFactory {
    fn create_engine(&self, index_path: &Path) -> Result<Arc<dyn FulltextSearchEngine>, SearchError> {
        let engine = ElasticsearchEngine::new(self.config.clone())?;
        Ok(Arc::new(engine))
    }
}
```

### 3.2 Manager 改造

```rust
pub struct FulltextIndexManager {
    engines: DashMap<IndexKey, Arc<dyn FulltextSearchEngine>>,
    factory: Arc<dyn FulltextEngineFactory>,
    // ... 其他字段不变
}
```

`create_index` 方法改为通过 factory 创建引擎：

```rust
pub async fn create_index(...) -> Result<String, SearchError> {
    // ... 验证逻辑不变
    let engine = self.factory.create_engine(&storage_path.join(&index_id))?;
    self.engines.insert(key.clone(), engine);
    // ...
}
```

### 3.3 Feature Flag 扩展

```toml
[features]
fulltext-search = ["dep:tantivy"]           # 默认 Tantivy
fulltext-elasticsearch = ["dep:reqwest"]     # Elasticsearch 客户端
fulltext-meilisearch = ["dep:reqwest"]       # Meilisearch 客户端
```

### 3.4 配置扩展

在 `graphdb-config` 中扩展 `FulltextConfig`：

```rust
pub struct FulltextConfig {
    pub default_engine: FulltextEngineType,
    pub tantivy: TantivyConfig,
    pub elasticsearch: Option<ElasticsearchConfig>,  // 新增
    pub meilisearch: Option<MeilisearchConfig>,      // 新增
    // ...
}

pub struct ElasticsearchConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub index_prefix: String,
    pub batch_size: usize,
    pub timeout_ms: u64,
}
```

---

## 4. 各引擎差异处理

### 4.1 Commit Payload 协调

Tantivy 支持 `commit_with_payload`，用于 sync 层的 consistency fence。外部引擎通常不支持此机制。

**策略**：`commit_with_payload` 默认实现忽略 payload，sync 层在检测到非 Tantivy 引擎时降级为不使用 payload fence（依赖 SQLite outbox 的 frontier 推进）。

### 4.2 Consistency State

Tantivy 有本地 `consistency_state`（Consistent/Inconsistent/Rebuilding）。外部引擎的 consistency 由服务端管理。

**策略**：外部引擎的 `consistency_state()` 始终返回 `Consistent`（假设服务端可用），`mark_inconsistent/mark_consistent` 作为空操作。

### 4.3 Stats

外部引擎的 doc_count 和 index_size 需要通过 API 查询。

**策略**：`stats()` 实现调用外部 API 获取实时数据。对于高频查询场景，可加缓存（类似 Tantivy 的 5 秒 TTL）。

---

## 5. 同步层影响

### 5.1 Outbox 投递

`SyncCoordinator` 的 fulltext 投递路径不感知引擎类型——它通过 `FulltextIndexManager` 间接操作。引擎差异被 Manager 和 Engine trait 屏蔽。

### 5.2 Consistency Fence

当前 sync 层在 commit 时调用 `engine.commit_with_payload(lsn_string)` 作为 consistency fence。外部引擎不支持此机制时：

- 方案 A：在 `FulltextSearchEngine` trait 中增加 `supports_payload_fence() -> bool` 方法
- 方案 B：在 sync 层检测 `commit_payload()` 返回 `None` 时降级

建议方案 A，显式声明能力。

---

## 6. 实施路径

### Phase 1: Engine Factory 抽象（1-2 周）

1. 定义 `FulltextEngineFactory` trait
2. 实现 `TantivyEngineFactory`
3. 改造 `FulltextIndexManager` 使用 factory + `Arc<dyn FulltextSearchEngine>`
4. 添加 `supports_payload_fence()` 到 trait

### Phase 2: Elasticsearch 引擎（2-3 周）

1. 创建 `graphdb-fulltext-elasticsearch` crate（或 feature-gated 模块）
2. 实现 `ElasticsearchEngine: FulltextSearchEngine`
3. 实现 `ElasticsearchEngineFactory`
4. 配置管理和 HTTP 客户端
5. 集成测试

### Phase 3: Meilisearch 引擎（1-2 周）

类似 Phase 2，实现 Meilisearch 后端。

---

## 7. 测试策略

| 测试类型 | 覆盖范围 | 位置 |
|----------|----------|------|
| 单元测试 | Engine trait 实现的正确性 | 各引擎 crate |
| 集成测试 | Manager + Factory + Engine 端到端 | `tests/fulltext_e2e.rs` |
| 对比测试 | Tantivy vs 外部引擎的查询一致性 | `tests/fulltext_comparison.rs` |
| Sync 测试 | Outbox 投递 + consistency fence 降级 | `tests/sync/fulltext_sync.rs` |

---

## 8. 风险与对策

| 风险 | 对策 |
|------|------|
| 外部引擎 latency 高于 Tantivy | 配置 timeout + 降级策略 |
| commit_with_payload 降级影响一致性 | 文档化降级行为，sync 层 frontier 推进兜底 |
| Feature flag 膨胀 | 每个外部引擎独立 feature，按需启用 |
| API 兼容性 | Engine trait 保持最小接口，扩展能力通过 `supports_*` 方法声明 |
