# Crate Export Visibility Optimization

## 背景

分析 `crates/bm25` (bm25-service) 与 `crates/inversearch` (inversearch-service) 两个 crate 的导出结构，缩减非必要的 `pub` 导出，将仅在 crate 内部使用的模块和项改为 `pub(crate)`。

当前唯一的真实外部使用者是根 crate (`graphdb`)，仅使用以下入口：
- **bm25-service**: `Bm25Index` + `IndexManagerConfig`
- **inversearch-service**: `EmbeddedIndex` + `EmbeddedConfig`

两个 crate 的 `lib.rs` 存在大量宽泛的 re-export（`pub use module::*` 或逐项 re-export），将大量内部实现细节暴露为公共 API，增加了维护负担和误用风险。

---

## 1. crates/bm25 (bm25-service)

### 1.1 外部真实依赖

| 路径 | 用途 |
|------|------|
| `bm25_service::api::embedded::Bm25Index` | graphdb 的 `Bm25SearchEngine` 包装 |
| `bm25_service::config::IndexManagerConfig` | graphdb 的 config 层引用 |

### 1.2 当前 lib.rs 导出问题

```rust
// lib.rs (当前)
pub mod api;                    // 对外开放所有 api 子模块
pub mod config;                 // 对外开放 config 子模块
pub mod error;                  // 对外开放 error（但 error 内部类型均已在 lib 层 re-export）
pub mod storage;                // 对外开放整个 storage 实现
pub mod tokenizer;              // 对外开放整个 tokenizer

pub use api::core;              // 将 core 模块整体提升至 crate 根路径
pub use api::core::{...};       // 再次逐项 re-export core 内所有项
pub use api::embedded::{...};   // 正确，仅导出外部需要的项
pub use error::{...};           // 正确
pub use config::{...};          // 正确
pub use storage::{...};         // 过度导出：storage 内部实现不应暴露
```

### 1.3 修改方案

#### 1.3.1 模块级可见性调整

| 模块 | 当前 | 改为 | 原因 |
|------|------|------|------|
| `api::core` | `pub` (子模块全部 `pub`) | `pub(crate)` (子模块项改为 `pub(crate)`) | 仅被 `api/embedded.rs` 和 `config/mod.rs` 内部使用 |
| `storage` | `pub` | `pub(crate)` | 所有 storage 实现仅内部使用，外部只需通过 `Bm25Index` 操作 |
| `tokenizer` | `pub` | `pub(crate)` | 仅内部使用 |
| `config::loader` | `pub` | `pub(crate)` | 仅内部使用 |
| `config::validator` | `pub` | `pub(crate)` | 仅内部使用 |
| `config::builder` | `pub(crate)` 已有 | — | 维持 |
| `error` | `pub` | `pub(crate)` | 所有 error 类型已在 `lib.rs` re-export，无需暴露模块路径 |
| `api::embedded` | `pub` | 维持 `pub` | 外部通过此路径使用 `Bm25Index` |

#### 1.3.2 子模块项可见性调整 (`api::core/*`)

`api/core/` 下的各项函数和类型仅在 `api/embedded.rs` 内部调用，无需对外暴露：

| 文件 | 当前导出 | 改为 | 原因 |
|------|----------|------|------|
| `api/core/mod.rs` | `pub mod batch;` 等 + 大量 `pub use` | 模块改为 `pub(crate)`, `pub use` 改为 `pub(crate) use` | 所有 re-export 仅内部使用 |
| `api/core/batch.rs` | `pub fn batch_*` | `pub(crate)` | 仅被 `api/embedded.rs` 调用 |
| `api/core/delete.rs` | `pub fn *` | `pub(crate)` | 同上 |
| `api/core/document.rs` | `pub fn *` | `pub(crate)` | 同上 |
| `api/core/index.rs` | `pub struct IndexManager` 等 | `pub(crate)` | 除 `IndexManagerConfig` 外均不需要对外暴露。`IndexManagerConfig` 需要保留 `pub` 因为通过 `config` 模块 re-export 给外部 |
| `api/core/persistence.rs` | `pub *` | `pub(crate)` | 仅内部使用 |
| `api/core/schema.rs` | `pub struct IndexSchema` | `pub(crate)` | 仅内部使用 |
| `api/core/search.rs` | `pub fn search` 等 | `pub(crate)` | 仅内部使用 |
| `api/core/stats.rs` | `pub *` | `pub(crate)` | 仅内部使用 |

#### 1.3.3 lib.rs re-export 精简

移除以下不再需要的 lib 层 re-export：

```rust
// 移除整行
pub use api::core;    // 不需要将 core 模块暴露为公共路径

// 保留以下（外部确实需要）：
pub use api::embedded::{Bm25Index, SearchResult, SearchResultWithHighlights};
pub use config::IndexManagerConfigBuilder;
pub use config::{Bm25Config, FieldWeights, SearchConfig};
```

---

## 2. crates/inversearch (inversearch-service)

### 2.1 外部真实依赖

| 路径 | 用途 |
|------|------|
| `inversearch_service::api::embedded::EmbeddedIndex` | graphdb 的 `InversearchEngine` 包装 |
| `inversearch_service::config::EmbeddedConfig` | graphdb 的 config 层引用 |

### 2.2 当前 lib.rs 导出问题

`lib.rs` 对几乎所有内部模块都做了 `pub use` 将内部项提升到 crate 根路径（共约 200+ 个 re-export），而外部（graphdb）仅使用了其中 2 个入口。此外 `api::core` 模块整体是 `lib.rs` 的镜像，属于冗余导出层。

### 2.3 集成测试现状

`crates/inversearch/tests/` 下存在 **45 个文件 / ~134KB 集成测试代码**，全部通过 `inversearch_service::*` 公共路径访问 crate。这些测试覆盖了 15+ 个模块：

| 模块 | 测试导入示例 | 被测试引用 |
|------|------------|-----------|
| `search` | `inversearch_service::search::search` | 是 |
| `index` | `inversearch_service::Index`, `inversearch_service::index::IndexOptions` | 是 |
| `resolver` | `inversearch_service::resolver::{exclusion, intersect_and, ...}` | 是 |
| `highlight` | `inversearch_service::highlight::highlight_single_document` | 是 |
| `intersect` | `inversearch_service::intersect::SuggestionEngine` | 是 |
| `document` | `inversearch_service::document::{Document, DocumentConfig}` | 是 |
| `r#type` | `inversearch_service::r#type::IntermediateSearchResults` | 是 |
| `storage` | `inversearch_service::storage::common::*` | 是 |
| `error` | `inversearch_service::error::{InversearchError, IndexError, ...}` | 是 |
| `config` | `inversearch_service::config::{Config, CacheConfig, ...}` | 是 |
| `encoder` | `inversearch_service::encoder::Encoder` | 是 |
| `async_` | — | 否 |
| `charset` | — | 否 |
| `common` | — | 否 |
| `compress` | — | 否 |
| `keystore` | — | 否 |
| `serialize` | — | 否 |
| `tokenizer` | — | 否 |
| `api::core` | — | 否 |

关键观察：**所有集成测试以外部的视角行使公共 API**，它们通过 `inversearch_service::*` 路径访问，而非 `crate::*`。这正是集成测试的本来目的。

### 2.4 可见性调整策略选择

针对集成测试，文档调研了两种方案并推荐混合策略（方案 C）：

#### 方案 A：集成测试转单元测试（不推荐）

将 `tests/*.rs` 迁移为 `src/**/tests.rs` 内的 `#[cfg(test)] mod tests`。

| 问题 | 说明 |
|------|------|
| **测试性质变质** | 集成测试的价值在于**以外部的视角验证公共 API**。转为单元测试后可访问 `pub(crate)` 内部项，反而降低了测试约束力，失去"公共 API 契约测试"的意义 |
| **迁移成本高** | 45 个文件 / 134KB 代码需全部重构路径 `inversearch_service::` → `crate::` |
| **代码组织退化** | 测试代码混入 `src/` 生产代码目录 |

#### 方案 B：保留模块 `pub`，仅收缩 lib 层 re-export（可用，但有优化空间）

保持内部模块为 `pub`，但不在 `lib.rs` 中 re-export。集成测试通过完整路径访问。

优点：零迁移成本，集成测试不受影响。
缺点：仍然暴露了模块路径给外部——但对于内部 crate（非发布到 crates.io 的公共库），这实际是可接受的。

#### 方案 C（推荐）：混合策略

根据是否被集成测试使用，将模块分为三类处理：

| 类别 | 处理方式 | 覆盖模块 |
|------|----------|----------|
| **未被集成测试引用** | 降为 `pub(crate)`，移除 lib.rs re-export | `async_`, `charset`, `common`, `compress`, `keystore`, `serialize`, `tokenizer`, `api::core` |
| **被集成测试引用** | 保留 `pub`，移除 lib.rs re-export | `search`, `index`, `resolver`, `highlight`, `intersect`, `document`, `r#type`, `storage`, `error`, `config`, `encoder` |
| **外部消费者入口** | 保留 `pub`，保留 lib.rs re-export | `api::embedded`, `config::EmbeddedConfig` |

**这样做的收益**：

1. **lib.rs re-export 从 ~200 项精简到 ~10 项**，外部新手看到 lib.rs 就知道"这才是推荐入口"
2. **45 个集成测试无需任何修改**，通过完整路径继续工作
3. **8 个纯内部模块彻底隐藏**，减少误用风险
4. **被集成测试引用的模块虽保持 `pub` 但不再 re-export**——`pub` 模块路径是一种"软可见性"标记，在内部 crate 中是可接受的折中

### 2.5 模块级可见性调整明细

#### 2.5.1 可直接降为 `pub(crate)` 的模块（无测试依赖）

| 模块 | 当前 | 改为 | 原因 |
|------|------|------|------|
| `api::core` | `pub` | `pub(crate)` | 该模块仅为 lib.rs 的镜像，外部无直接使用。模块内所有 re-export 改为 `pub(crate)` |
| `charset` | `pub` | `pub(crate)` | 仅内部使用 |
| `common` | `pub` | `pub(crate)` | 仅内部使用 |
| `compress` | `pub` | `pub(crate)` | 仅内部使用 |
| `keystore` | `pub` | `pub(crate)` | 仅内部使用 |
| `serialize` | `pub` | `pub(crate)` | 仅内部使用 |
| `tokenizer` | `pub` | `pub(crate)` | 仅内部使用 |
| `async_` | `pub` | `pub(crate)` | 仅内部使用 |

#### 2.5.2 保留 `pub` 但移除 re-export 的模块（被集成测试使用）

| 模块 | 当前 lib.rs re-export | 操作 | 说明 |
|------|----------------------|------|------|
| `search` | `pub use search::{multi_field_search, search, CacheKeyGenerator, ...}` | 移除 | 集成测试通过 `inversearch_service::search::search` 访问 |
| `index` | `pub use index::{Register, ScoreFn, ...}` | 移除 | 集成测试通过 `inversearch_service::Index`、`inversearch_service::index::IndexOptions` 访问 |
| `resolver` | `pub use resolver::{combine_search_results, exclusion, ...}` | 移除 | 集成测试通过 `inversearch_service::resolver::...` 访问 |
| `highlight` | `pub use highlight::{highlight_document, ...}` | 移除 | 集成测试通过 `inversearch_service::highlight::...` 访问 |
| `intersect` | `pub use intersect::SuggestionEngine` | 移除 | 集成测试通过 `inversearch_service::intersect::SuggestionEngine` 访问 |
| `document` | `pub use document::{parse_tree, Batch, Document, ...}` | 移除 | 集成测试通过 `inversearch_service::document::{Document, DocumentConfig}` 访问 |
| `r#type` | `pub use r#type::{ContextOptions, EncoderOptions, ...}` | 移除 | 集成测试通过 `inversearch_service::r#type::IntermediateSearchResults` 访问 |
| `storage` | `pub use storage::{StorageInterface, MemoryStorage, StorageManager, ...}` | 移除 | 集成测试通过 `inversearch_service::storage::...` 访问 |
| `error` | `pub use error::{CacheError, InversearchError, ...}` | 移除 | 集成测试通过 `inversearch_service::error::...` 访问 |
| `config` | `pub use config::{Config, EmbeddedConfig, ...}` | 精简 | 保留 `pub use config::EmbeddedConfig`（被 graphdb 使用），移除其他 config re-export |
| `encoder` | `pub use encoder::Encoder` | 移除 | 集成测试通过 `inversearch_service::encoder::Encoder` 访问 |

#### 2.5.3 保留 `pub` + 保留 re-export 的入口

```rust
// lib.rs 精简后保留的 re-export
pub use api::embedded::{
    EmbeddedBatch, EmbeddedBatchOperation, EmbeddedBatchResult,
    EmbeddedIndex, EmbeddedIndexBuilder, EmbeddedIndexStats,
    EmbeddedSearchResult,
};
pub use config::EmbeddedConfig;
```

#### 2.5.4 api::core 模块的完整处理

`api/core/mod.rs` 是 `lib.rs` 的完全镜像，是冗余导出层，直接降为 `pub(crate)`：

- 模块声明: `pub mod core` → `pub(crate) mod core`（在 `api/mod.rs` 中修改）
- 内部所有 `pub use crate::xxx::...` → `pub(crate) use crate::xxx::...`

---

## 3. 实施步骤

### Phase 1: bm25-service（无集成测试，可直接执行）

1. `storage/` 模块改为 `pub(crate)`，内部项改为 `pub(crate)`
2. `tokenizer/` 模块改为 `pub(crate)`
3. `api/core/` → 模块及内部所有项改为 `pub(crate)`
4. `config/loader.rs` `config/validator.rs` 改为 `pub(crate)`
5. `error/` 模块改为 `pub(crate)`
6. 精简 `lib.rs` re-export，移除 `pub use api::core;`
7. 运行 `cargo clippy --all-targets` 验证

### Phase 2: inversearch-service（按混合策略执行）

1. **内部模块收紧**：将 `async_`, `charset`, `common`, `compress`, `keystore`, `serialize`, `tokenizer` 模块改为 `pub(crate)`
2. **api::core 降级**：模块及内部所有 re-export 改为 `pub(crate)`
3. **入口保留**：`api::embedded` 和 `config` 模块保留 `pub`
4. **lib.rs 精简化**：
   - 移除除 `pub use api::embedded::*` 和 `pub use config::EmbeddedConfig` 之外的所有 re-export
   - 移除不再需要的 `pub mod` 声明（已降为 `pub(crate)` 的模块）
5. 运行 `cargo clippy --all-targets` 验证

### Phase 3: 验证

- `cargo clippy --all-targets --all-features`（无 warning）
- `cargo test --lib`（所有单元测试通过）
- `cargo test --test '*'`（所有集成测试通过——关键验证点）
- 确认 graphdb 主 crate 可正常编译（`cargo check`）

---

## 4. 预期收益

| 指标 | bm25 | inversearch |
|------|------|-------------|
| lib.rs 行数 | 30 → ~15 | 108 → ~15 |
| 公共模块数 | 5 → 2 | 22 → 12（8 个隐藏 + 10 个保留不 re-export）|
| 公共 API 项数 | ~50 → ~10 | ~200 → ~14 |
| 集成测试影响 | 无（不存在） | 无（无需修改） |
| 减少误用风险 | 中 | 高 |

### 与纯方案 A/B 的对比

| 维度 | 方案 A（全转单元测试） | 方案 B（仅缩 re-export） | 方案 C 混合策略（推荐） |
|------|----------------------|------------------------|----------------------|
| 集成测试改动 | 45 文件全改 | 无 | **无** |
| 测试性质 | 变质为内部测试 | 保持外部视角 | **保持外部视角** |
| 纯内部模块隐藏 | 完整隐藏 | 仍暴露 | **完整隐藏** |
| 实施风险 | 高（134KB 测试需迁移） | 极低 | **低** |
| 未来可维护性 | 中等 | 低（模块路径仍对外） | **高** |
