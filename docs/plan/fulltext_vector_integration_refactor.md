# Fulltext vs Vector 集成差异修复计划

## 问题清单

### P0 - 高优先级

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| 1 | Fulltext Operator 静默降级 | `crates/graphdb-query/src/executor/streaming/operators/fulltext_operator.rs:468-473` | 当 `fulltext_manager` 为 `None` 时，`FulltextSearch/Lookup/Match` 静默降级到输入行，返回意外结果而非显式错误 |
| 2 | `validate_metric` 重复实现 | `crates/graphdb-api/src/api_core/vector_api.rs:27-41` + `crates/graphdb-sync/src/vector_sync.rs:30-46` | 相同逻辑两处维护，DRY 违反，增加漂移风险 |

### P1 - 中优先级

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| 3 | Fulltext 无专用 API 结构体 | `crates/graphdb-api/src/api_core.rs` | Vector 有 `VectorApi`，Fulltext 无对应抽象，用户体验不对称 |
| 4 | Fulltext 无 Wire DTOs | `crates/graphdb-wire/` | Vector 有专用 DTO，Fulltext 仅有标准查询行，限制未来扩展 |
| 5 | Fulltext 无专用 HTTP 端点 | `crates/graphdb-server/src/http/handlers/` | Vector 有 REST 管理端点，Fulltext 仅通过 SQL 访问 |

### P2 - 低优先级

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| 6 | Feature Gate 耦合 | `crates/graphdb-fulltext/Cargo.toml:26` | `vector = ["graphdb-sync/vector"]` 不必要地将 fulltext 与 vector feature 绑定 |
| 7 | 指标采集策略不一致 | vector: `vector_metrics.rs` (轮询) vs fulltext: `metrics.rs` (内联) | 两种策略各有优劣，统一更清晰 |

---

## 分阶段修改方案

### Phase 1: 修复 Fulltext Operator 静默降级

**目标**: 将静默降级改为显式错误，与 Vector 行为一致

**修改文件**:
- `crates/graphdb-query/src/executor/streaming/operators/fulltext_operator.rs`

**具体改动**:

```rust
// FulltextSearch 变体 (line 468-473)
// BEFORE:
// No manager configured: fall through to the input.
if let Some(mut chunk) = input.advance()? {
    chunk.materialize_selection_by("Fulltext");
    return Ok(Some(chunk));
}
Ok(None)

// AFTER:
let _ = input;
Err(QueryError::execution(
    "FULLTEXT SEARCH cannot execute: no fulltext manager is configured",
))
```

对 `FulltextLookup` 和 `MatchFulltext` 变体做相同修改。

**验证**: 运行现有 fulltext 测试，确认无回归

---

### Phase 2: 提取共享 `validate_metric`

**目标**: 消除重复验证逻辑，统一返回类型

**修改文件**:
- 新建: `crates/graphdb-core/src/vector_validation.rs` (或在 `crates/vector-search/src/types.rs` 中添加)
- 修改: `crates/graphdb-api/src/api_core/vector_api.rs` (删除本地 `validate_metric`)
- 修改: `crates/graphdb-sync/src/vector_sync.rs` (删除本地 `validate_metric`)

**具体改动**:

```rust
// crates/vector-search/src/types.rs (或 graphdb-core)
pub fn validate_distance_metric(distance: DistanceMetric) -> Result<(), String> {
    if matches!(
        distance,
        DistanceMetric::Cosine
            | DistanceMetric::Euclid
            | DistanceMetric::Dot
            | DistanceMetric::Manhattan
    ) {
        Ok(())
    } else {
        Err(format!(
            "distance metric {distance:?} is not supported; supported metrics: Cosine, Euclid, Dot, Manhattan"
        ))
    }
}
```

```rust
// crates/graphdb-api/src/api_core/vector_api.rs
use vector_search::validate_distance_metric;

fn validate_metric(distance: DistanceMetric) -> CoreResult<()> {
    validate_distance_metric(distance).map_err(CoreError::VectorError)
}
```

```rust
// crates/graphdb-sync/src/vector_sync.rs
use vector_search::validate_distance_metric;

fn validate_metric(distance: DistanceMetric) -> VectorCoordinatorResult<()> {
    validate_distance_metric(distance).map_err(|e| {
        VectorCoordinatorError::Vector(VectorError::ConfigError(e))
    })
}
```

**验证**: 编译通过，运行 `cargo test --lib`

---

### Phase 3: 移除 Feature Gate 耦合

**目标**: fulltext crate 不再感知 vector feature

**修改文件**:
- `crates/graphdb-fulltext/Cargo.toml`

**具体改动**:

```toml
# BEFORE
[features]
fulltext = ["dep:tantivy"]
jieba = ["dep:jieba-rs", "dep:tantivy-tokenizer-api"]
vector = ["graphdb-sync/vector"]

[dev-dependencies]
graphdb-sync = { path = "../graphdb-sync", features = ["fulltext", "vector"] }

# AFTER
[features]
fulltext = ["dep:tantivy"]
jieba = ["dep:jieba-rs", "dep:tantivy-tokenizer-api"]

[dev-dependencies]
graphdb-sync = { path = "../graphdb-sync", features = ["fulltext"] }
```

**验证**: `cargo check --workspace`，确认无编译错误

---

### Phase 4: 添加 FulltextApi 结构体

**目标**: 与 `VectorApi` 对称，为未来 HTTP 端点做准备

**修改文件**:
- 新建: `crates/graphdb-api/src/api_core/fulltext_api.rs`
- 修改: `crates/graphdb-api/src/api_core.rs` (注册模块)

**具体改动**:

```rust
// crates/graphdb-api/src/api_core/fulltext_api.rs
use graphdb_fulltext::manager::FulltextIndexManager;
use std::sync::Arc;

pub struct FulltextApi {
    manager: Arc<FulltextIndexManager>,
}

impl FulltextApi {
    pub fn new(manager: Arc<FulltextIndexManager>) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> &Arc<FulltextIndexManager> {
        &self.manager
    }

    pub async fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String> {
        self.manager
            .create_index(space_id, tag_name, field_name, None)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String> {
        self.manager
            .drop_index(space_id, tag_name, field_name)
            .await
            .map_err(|e| e.to_string())
    }

    pub fn list_indexes(&self) -> Vec<graphdb_fulltext::IndexMetadata> {
        self.manager.list_indexes()
    }

    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<graphdb_fulltext::SearchResult>, String> {
        self.manager
            .search(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| e.to_string())
    }
}
```

```rust
// crates/graphdb-api/src/api_core.rs
#[cfg(feature = "fulltext")]
pub mod fulltext_api;
#[cfg(feature = "fulltext")]
pub use fulltext_api::FulltextApi;
```

**验证**: `cargo check -p graphdb-api`

---

### Phase 5 (可选): 添加 Fulltext Wire DTOs

**目标**: 为未来 HTTP 端点提供结构化类型

**修改文件**:
- 新建: `crates/graphdb-wire/src/fulltext.rs`
- 修改: `crates/graphdb-wire/src/lib.rs`

**具体改动**:

```rust
// crates/graphdb-wire/src/fulltext.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchRequest {
    pub index_name: String,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchResponse {
    pub results: Vec<FulltextSearchResult>,
    pub total_hits: usize,
    pub took_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchResult {
    pub doc_id: String,
    pub score: f32,
    pub highlights: Option<Vec<HighlightResult>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightResult {
    pub field: String,
    pub fragments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextIndexInfo {
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub status: String,
    pub doc_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFulltextIndexRequest {
    pub index_name: String,
    pub schema_name: String,
    pub fields: Vec<FulltextFieldDef>,
    pub if_not_exists: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextFieldDef {
    pub field_name: String,
    pub analyzer: Option<String>,
    pub boost: Option<f32>,
}
```

```rust
// crates/graphdb-wire/src/lib.rs
#[cfg(feature = "fulltext")]
pub mod fulltext;
```

**验证**: `cargo check -p graphdb-wire`

---

## 依赖关系

```
Phase 1 (Operator 修复)  ─┐
                          ├─> Phase 4 (FulltextApi) ─> Phase 5 (Wire DTOs)
Phase 2 (validate_metric) ─┤
                          │
Phase 3 (Feature Gate)   ─┘
```

Phase 1-3 可并行执行，Phase 4 依赖 Phase 1 完成，Phase 5 依赖 Phase 4 完成。

---

## 回滚策略

每个 Phase 独立提交，回滚粒度为单个 Phase。关键检查点:
- Phase 1: 运行 `cargo test -p graphdb-query --lib`
- Phase 2: 运行 `cargo test --workspace`
- Phase 3: 运行 `cargo check --workspace`
- Phase 4: 运行 `cargo test -p graphdb-api`
- Phase 5: 运行 `cargo test -p graphdb-wire`
