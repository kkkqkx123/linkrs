# 向量客户端 Crate 架构分析

> 分析日期: 2026-04-06
> 目的: 分析是否需要创建独立的向量客户端crate

---

## 目录

- [1. 现有架构分析](#1-现有架构分析)
- [2. BM25 Crate架构](#2-bm25-crate架构)
- [3. Inversearch Crate架构](#3-inversearch-crate架构)
- [4. 向量客户端需求分析](#4-向量客户端需求分析)
- [5. 架构方案对比](#5-架构方案对比)
- [6. 推荐方案](#6-推荐方案)

---

## 1. 现有架构分析

### 1.1 项目Crate结构

```
crates/
├── bm25/              # BM25全文检索服务
│   ├── src/
│   │   ├── api/       # API层（embedded/server）
│   │   ├── storage/   # 存储抽象层
│   │   ├── config/    # 配置管理
│   │   └── error.rs   # 错误定义
│   └── Cargo.toml
│
├── inversearch/       # 倒排索引搜索服务
│   ├── src/
│   │   ├── api/       # API层
│   │   ├── storage/   # 存储抽象层
│   │   ├── search/    # 搜索核心
│   │   ├── index/     # 索引管理
│   │   └── config/    # 配置管理
│   └── Cargo.toml
│
└── graphdb/           # 主项目（计划中）
    └── src/
        ├── vector/    # 向量模块
        └── ...
```

### 1.2 共同架构模式

| 层级 | 职责 | BM25 | Inversearch |
|------|------|------|-------------|
| API层 | 对外接口 | embedded/server | embedded/server |
| 存储层 | 存储抽象 | StorageInterface trait | StorageInterface trait |
| 配置层 | 配置管理 | Bm25Config | Config |
| 错误层 | 错误定义 | Bm25Error | InversearchError |

---

## 2. BM25 Crate架构

### 2.1 模块结构

```
bm25/
├── api/
│   ├── core/          # 核心API
│   │   ├── index.rs   # 索引管理
│   │   ├── search.rs  # 搜索接口
│   │   ├── document.rs # 文档操作
│   │   └── batch.rs   # 批量操作
│   ├── embedded/      # 嵌入式API
│   └── server/        # 服务API（gRPC）
│
├── storage/
│   ├── common/
│   │   ├── trait.rs   # StorageInterface trait
│   │   └── types.rs   # 存储类型
│   ├── tantivy.rs     # Tantivy实现
│   ├── redis.rs       # Redis实现
│   └── factory.rs     # 存储工厂
│
├── config/
│   ├── mod.rs
│   ├── builder.rs
│   └── loader.rs
│
└── error.rs
```

### 2.2 核心Trait

```rust
#[async_trait]
pub trait StorageInterface: Send + Sync {
    async fn init(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn commit_stats(&mut self, term: &str, tf: f32, df: u64) -> Result<()>;
    async fn commit_batch(&mut self, stats: &Bm25Stats) -> Result<()>;
    async fn get_stats(&self, term: &str) -> Result<Option<Bm25Stats>>;
    async fn get_df(&self, term: &str) -> Result<Option<u64>>;
    async fn get_tf(&self, term: &str, doc_id: &str) -> Result<Option<f32>>;
    async fn clear(&mut self) -> Result<()>;
    async fn delete_doc_stats(&mut self, doc_id: &str) -> Result<()>;
    async fn info(&self) -> Result<StorageInfo>;
    async fn health_check(&self) -> Result<bool>;
}
```

### 2.3 Feature Flags

```toml
[features]
default = ["embedded", "storage-tantivy"]
embedded = []
service = ["tonic", "prost", "tokio/full", ...]
storage-tantivy = ["async-trait"]
storage-redis = ["redis", "async-trait", "bb8"]
```

---

## 3. Inversearch Crate架构

### 3.1 模块结构

```
inversearch/
├── api/
│   ├── core/          # 核心类型
│   ├── embedded/      # 嵌入式API
│   └── server/        # 服务API
│
├── storage/
│   ├── common/
│   │   ├── trait.rs   # StorageInterface trait
│   │   ├── types.rs
│   │   └── config.rs
│   ├── file.rs        # 文件存储
│   ├── memory.rs      # 内存存储
│   ├── redis.rs       # Redis存储
│   ├── wal.rs         # WAL存储
│   └── cold_warm_cache/ # 冷热缓存
│
├── search/            # 搜索核心
├── index/             # 索引管理
├── resolver/          # 结果解析
├── highlight/         # 高亮功能
├── tokenizer/         # 分词器
└── config/
```

### 3.2 核心Trait

```rust
#[async_trait]
pub trait StorageInterface: Send + Sync {
    async fn mount(&self, index: &Index) -> Result<()>;
    async fn open(&self) -> Result<()>;
    async fn close(&self) -> Result<()>;
    async fn destroy(&self) -> Result<()>;
    async fn commit(&self, index: &Index, replace: bool, append: bool) -> Result<()>;
    async fn get(&self, key: &str, ctx: Option<&str>, limit: usize, offset: usize, resolve: bool, enrich: bool) -> Result<SearchResults>;
    async fn enrich(&self, ids: &[DocId]) -> Result<EnrichedSearchResults>;
    async fn has(&self, id: DocId) -> Result<bool>;
    async fn remove(&self, ids: &[DocId]) -> Result<()>;
    async fn clear(&self) -> Result<()>;
    async fn info(&self) -> Result<StorageInfo>;
}
```

---

## 4. 向量客户端需求分析

### 4.1 功能需求

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 向量存储 | 插入/更新/删除向量点 | 高 |
| 相似度搜索 | 向量相似度检索 | 高 |
| Payload管理 | 元数据存储和过滤 | 高 |
| 集合管理 | 创建/删除/查询集合 | 高 |
| 批量操作 | 批量插入/搜索 | 中 |
| 健康检查 | 连接状态监控 | 中 |

### 4.2 接口需求

```rust
#[async_trait]
pub trait VectorEngine: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    
    // 集合管理
    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<()>;
    async fn collection_exists(&self, name: &str) -> Result<bool>;
    
    // 向量操作
    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()>;
    async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<()>;
    async fn delete(&self, collection: &str, point_id: &str) -> Result<()>;
    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<()>;
    
    // 搜索
    async fn search(&self, collection: &str, query: VectorQuery) -> Result<Vec<SearchResult>>;
    async fn search_batch(&self, collection: &str, queries: Vec<VectorQuery>) -> Result<Vec<Vec<SearchResult>>>;
    
    // 元数据
    async fn count(&self, collection: &str) -> Result<u64>;
    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>>;
    
    // 健康检查
    async fn health_check(&self) -> Result<HealthStatus>;
}
```

### 4.3 与现有架构的对比

| 特性 | BM25/Inversearch | 向量客户端 |
|------|------------------|-----------|
| 存储引擎 | 本地文件/Redis | 远程服务(Qdrant) |
| 数据模型 | 文档/词项 | 向量点 |
| 搜索方式 | 关键词匹配 | 向量相似度 |
| 索引类型 | 倒排索引 | HNSW |
| 独立服务 | 可选 | 必需(Qdrant) |

---

## 5. 架构方案对比

### 方案A: 集成到主项目

```
graphdb/
└── src/
    └── vector/
        ├── mod.rs
        ├── engine.rs      # VectorEngine trait
        ├── qdrant.rs      # Qdrant适配器
        ├── manager.rs     # 索引管理
        ├── coordinator.rs # 协调器
        └── config.rs
```

**优点**:
- 简单直接，减少crate数量
- 与主项目紧密集成
- 共享配置和错误处理

**缺点**:
- 增加主项目复杂度
- 不便于独立测试
- 难以复用

### 方案B: 独立Crate

```
crates/
└── vector-client/
    ├── src/
    │   ├── lib.rs
    │   ├── api/
    │   │   ├── core/      # 核心类型
    │   │   └── embedded/  # 嵌入式API
    │   ├── engine/
    │   │   ├── mod.rs     # VectorEngine trait
    │   │   ├── qdrant.rs  # Qdrant实现
    │   │   └── mock.rs    # Mock实现(测试)
    │   ├── config/
    │   └── error.rs
    └── Cargo.toml
```

**优点**:
- 遵循现有架构模式
- 独立测试和发布
- 可复用性强
- 清晰的模块边界

**缺点**:
- 需要额外维护
- 增加依赖管理复杂度

### 方案C: 混合方案

```
crates/
└── vector-client/     # 轻量级客户端crate
    ├── src/
    │   ├── lib.rs
    │   ├── engine.rs  # VectorEngine trait + Qdrant实现
    │   ├── types.rs   # 核心类型
    │   ├── config.rs
    │   └── error.rs
    └── Cargo.toml

graphdb/
└── src/
    └── vector/
        ├── mod.rs
        ├── manager.rs     # 索引管理
        ├── coordinator.rs # 协调器
        └── sync.rs        # 同步逻辑
```

**优点**:
- 客户端逻辑独立
- 业务逻辑在主项目
- 平衡复杂度和复用性

**缺点**:
- 需要定义清晰的边界

---

## 6. 推荐方案

### 6.1 推荐采用方案B（独立Crate）

理由：
1. **架构一致性**: 与BM25、Inversearch保持一致
2. **可测试性**: 独立测试，支持Mock实现
3. **可扩展性**: 未来可支持其他向量引擎（Milvus、Weaviate）
4. **关注点分离**: 客户端逻辑与业务逻辑分离

### 6.2 推荐的Crate结构

```
crates/vector-client/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Crate入口
│   │
│   ├── api/                # API层
│   │   ├── mod.rs
│   │   ├── core/           # 核心API
│   │   │   ├── mod.rs
│   │   │   ├── collection.rs  # 集合管理
│   │   │   ├── point.rs       # 点操作
│   │   │   ├── search.rs      # 搜索接口
│   │   │   └── payload.rs     # Payload管理
│   │   │
│   │   └── embedded/       # 嵌入式API
│   │       ├── mod.rs
│   │       └── client.rs
│   │
│   ├── engine/             # 引擎层
│   │   ├── mod.rs          # VectorEngine trait
│   │   ├── qdrant.rs       # Qdrant实现
│   │   └── mock.rs         # Mock实现(测试)
│   │
│   ├── types/              # 类型定义
│   │   ├── mod.rs
│   │   ├── point.rs        # VectorPoint等
│   │   ├── search.rs       # SearchResult等
│   │   ├── config.rs       # CollectionConfig等
│   │   └── filter.rs       # 过滤条件
│   │
│   ├── config/             # 配置管理
│   │   ├── mod.rs
│   │   ├── client.rs       # 客户端配置
│   │   └── collection.rs   # 集合配置
│   │
│   └── error.rs            # 错误定义
│
└── tests/
    ├── integration/
    └── mock_tests.rs
```

### 6.3 Cargo.toml设计

```toml
[package]
name = "vector-client"
version = "0.1.0"
edition = "2021"

[lib]
name = "vector_client"
path = "src/lib.rs"

[features]
default = ["qdrant"]
qdrant = ["qdrant-client"]
mock = []  # Mock实现，用于测试

[dependencies]
# 核心依赖
async-trait = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"

# Qdrant客户端
qdrant-client = { version = "1.7", optional = true }

# 工具
tracing = "0.1"

[dev-dependencies]
tokio-test = "0.4"
```

### 6.4 核心接口设计

```rust
// src/engine/mod.rs
use async_trait::async_trait;
use crate::types::*;
use crate::error::Result;

#[async_trait]
pub trait VectorEngine: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    
    async fn health_check(&self) -> Result<HealthStatus>;
    
    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<()>;
    async fn collection_exists(&self, name: &str) -> Result<bool>;
    async fn collection_info(&self, name: &str) -> Result<CollectionInfo>;
    
    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()>;
    async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<UpsertResult>;
    
    async fn delete(&self, collection: &str, point_id: &str) -> Result<()>;
    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult>;
    async fn delete_by_filter(&self, collection: &str, filter: VectorFilter) -> Result<DeleteResult>;
    
    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>>;
    async fn search_batch(&self, collection: &str, queries: Vec<SearchQuery>) -> Result<Vec<Vec<SearchResult>>>;
    
    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>>;
    async fn count(&self, collection: &str) -> Result<u64>;
    
    async fn set_payload(&self, collection: &str, point_ids: Vec<&str>, payload: Payload) -> Result<()>;
    async fn delete_payload(&self, collection: &str, point_ids: Vec<&str>, keys: Vec<&str>) -> Result<()>;
}
```

### 6.5 与主项目的集成

```rust
// graphdb/src/vector/mod.rs
pub use vector_client::{
    VectorEngine, VectorPoint, SearchResult, CollectionConfig, VectorFilter,
};

mod manager;
mod coordinator;
mod sync;

pub use manager::VectorIndexManager;
pub use coordinator::VectorCoordinator;
```

---

## 实施计划

| 阶段 | 内容 | 时间 |
|------|------|------|
| Phase 1 | 创建vector-client crate骨架 | 1天 |
| Phase 2 | 实现VectorEngine trait和Qdrant适配器 | 2天 |
| Phase 3 | 实现核心API和类型定义 | 1天 |
| Phase 4 | 添加Mock实现和测试 | 1天 |
| Phase 5 | 集成到主项目 | 2天 |

---

## 总结

**推荐创建独立的 `vector-client` crate**，理由如下：

1. 与现有BM25、Inversearch架构保持一致
2. 支持独立测试和Mock实现
3. 便于未来扩展其他向量引擎
4. 清晰的模块边界和职责分离
5. 可复用性强，便于其他项目使用
