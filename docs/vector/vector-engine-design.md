# 向量引擎设计文档

## 1. 概述

本文档描述了 GraphDB 向量搜索引擎的架构设计、接口规范和实现指南。向量引擎为图数据库提供了高效的向量相似度搜索能力，支持 ANN（Approximate Nearest Neighbor）搜索、过滤、批量操作等功能。

## 2. 架构设计

### 2.1 分层架构

```
┌─────────────────────────────────────────────────────────┐
│              GraphDB 应用层                              │
│  - 向量查询解析器                                        │
│  - 向量同步协调器 (VectorSyncCoordinator)               │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│              向量管理层 (VectorManager)                  │
│  - 索引生命周期管理                                      │
│  - 多索引协调                                            │
│  - 健康检查                                              │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│           引擎抽象层 (VectorEngine Trait)                │
│  - 定义标准接口规范                                      │
│  - 解耦具体实现                                          │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│              具体引擎实现                                │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │ MockEngine  │  │ QdrantEngine │  │ 其他引擎...  │  │
│  │ (测试/开发)  │  │  (生产)       │  │              │  │
│  └─────────────┘  └──────────────┘  └──────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### 2.2 设计原则

1. **接口隔离**：通过 `VectorEngine` Trait 定义标准接口，支持多种后端实现
2. **单一职责**：VectorManager 负责索引管理，VectorEngine 负责底层操作
3. **依赖倒置**：高层模块依赖抽象接口，不依赖具体实现
4. **开闭原则**：对扩展开放，对修改封闭，易于添加新引擎

## 3. 核心接口定义

### 3.1 VectorEngine Trait

```rust
#[async_trait]
pub trait VectorEngine: Send + Sync + std::fmt::Debug {
    // ========== 基本信息 ==========
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    async fn health_check(&self) -> Result<HealthStatus>;

    // ========== 集合管理 ==========
    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<()>;
    async fn collection_exists(&self, name: &str) -> Result<bool>;
    async fn collection_info(&self, name: &str) -> Result<CollectionInfo>;

    // ========== 向量写入 ==========
    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult>;
    async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<UpsertResult>;

    // ========== 向量删除 ==========
    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult>;
    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult>;
    async fn delete_by_filter(&self, collection: &str, filter: VectorFilter) -> Result<DeleteResult>;

    // ========== 向量搜索 ==========
    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>>;
    async fn search_batch(&self, collection: &str, queries: Vec<SearchQuery>) -> Result<Vec<Vec<SearchResult>>>;

    // ========== 向量检索 ==========
    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>>;
    async fn get_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<Vec<Option<VectorPoint>>>;
    async fn count(&self, collection: &str) -> Result<u64>;

    // ========== Payload 管理 ==========
    async fn set_payload(&self, collection: &str, point_ids: Vec<&str>, payload: Payload) -> Result<()>;
    async fn delete_payload(&self, collection: &str, point_ids: Vec<&str>, keys: Vec<&str>) -> Result<()>;

    // ========== 高级功能 ==========
    async fn scroll(&self, collection: &str, limit: usize, offset: Option<&str>,
                    with_payload: Option<bool>, with_vector: Option<bool>)
                    -> Result<(Vec<VectorPoint>, Option<String>)>;
    async fn create_payload_index(&self, collection: &str, field: &str,
                                  schema: PayloadSchemaType) -> Result<()>;
    async fn delete_payload_index(&self, collection: &str, field: &str) -> Result<()>;
    async fn list_payload_indexes(&self, collection: &str)
                                  -> Result<Vec<(String, PayloadSchemaType)>>;
}
```

### 3.2 核心数据结构

#### 3.2.1 集合配置 (CollectionConfig)

```rust
pub struct CollectionConfig {
    pub vector_size: usize,           // 向量维度
    pub distance: DistanceMetric,     // 距离度量方式
    pub hnsw: Option<HnswConfig>,     // HNSW 索引配置
    pub quantization: Option<QuantizationConfig>, // 量化配置
}

pub enum DistanceMetric {
    Cosine,    // 余弦相似度
    Euclid,    // 欧几里得距离
    Dot,       // 点积
}
```

#### 3.2.2 向量点 (VectorPoint)

```rust
pub struct VectorPoint {
    pub id: PointId,                  // 点 ID
    pub vector: Vec<f32>,             // 向量数据
    pub payload: Option<Payload>,     // 附加数据
}

pub type Payload = HashMap<String, serde_json::Value>;
pub type PointId = String;
```

#### 3.2.3 搜索查询 (SearchQuery)

```rust
pub struct SearchQuery {
    pub vector: Vec<f32>,             // 查询向量
    pub limit: usize,                 // 返回结果数量
    pub offset: Option<usize>,        // 偏移量
    pub score_threshold: Option<f32>, // 分数阈值
    pub filter: Option<VectorFilter>, // 过滤条件
    pub with_payload: Option<bool>,   // 是否返回 payload
    pub with_vector: Option<bool>,    // 是否返回向量
}
```

#### 3.2.4 搜索结果 (SearchResult)

```rust
pub struct SearchResult {
    pub id: PointId,                  // 点 ID
    pub score: f32,                   // 相似度分数
    pub payload: Option<Payload>,     // 附加数据
    pub vector: Option<Vec<f32>>,     // 向量数据
}
```

#### 3.2.5 过滤条件 (VectorFilter)

```rust
pub struct VectorFilter {
    pub must: Option<Vec<FilterCondition>>,      // 必须满足
    pub must_not: Option<Vec<FilterCondition>>,  // 必须不满足
    pub should: Option<Vec<FilterCondition>>,    // 应该满足
    pub min_should: Option<MinShouldCondition>,  // 最少满足数量
}

pub struct FilterCondition {
    pub field: String,
    pub condition: ConditionType,
}

pub enum ConditionType {
    Match { value: String },
    MatchAny { values: Vec<String> },
    Range(RangeCondition),
    IsEmpty,
    IsNull,
    HasId { ids: Vec<String> },
    Nested { filter: Box<VectorFilter> },
    Payload { key: String, value: PayloadValue },
    GeoRadius(GeoRadius),
    GeoBoundingBox(GeoBoundingBox),
    ValuesCount(ValuesCountCondition),
    Contains { value: String },
}
```

## 4. MockEngine 实现

### 4.1 设计目标

MockEngine 是一个内存级的向量引擎实现，主要用于：

- 单元测试和集成测试
- 开发和调试
- 功能验证和原型设计

### 4.2 数据结构

```rust
pub struct MockEngine {
    collections: Arc<RwLock<HashMap<String, (CollectionConfig, CollectionStore)>>>,
    healthy: Arc<RwLock<bool>>,
}

type CollectionStore = HashMap<String, VectorPoint>;
```

**设计要点：**

- 使用 `Arc<RwLock<>>` 实现线程安全的并发访问
- 支持多个集合的独立管理
- 可动态控制健康状态（用于故障测试）

### 4.3 核心算法实现

#### 4.3.1 余弦相似度计算

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // 计算点积
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

    // 计算范数
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    // 处理零向量边界情况
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    // 返回余弦相似度
    dot / (norm_a * norm_b)
}
```

**实现要点：**

- 使用迭代器高效计算
- 处理零向量边界情况
- 返回归一化的相似度分数（-1 到 1）

#### 4.3.2 搜索算法

```rust
async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    // 1. 获取集合（使用读锁，不阻塞写入）
    let collections = self.collections.read().await;
    let (_, store) = collections.get(collection)
        .ok_or_else(|| VectorClientError::CollectionNotFound(...))?;

    // 2. 计算所有向量的相似度（暴力搜索）
    let mut results: Vec<SearchResult> = store.values()
        .map(|point| {
            let score = Self::cosine_similarity(&query.vector, &point.vector);
            SearchResult {
                id: point.id.clone(),
                score,
                payload: if query.with_payload.unwrap_or(true) {
                    point.payload.clone()
                } else { None },
                vector: if query.with_vector.unwrap_or(false) {
                    Some(point.vector.clone())
                } else { None },
            }
        })
        .collect();

    // 3. 按分数降序排序
    results.sort_by(|a, b| b.score.partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal));

    // 4. 应用分数阈值过滤
    if let Some(threshold) = query.score_threshold {
        results.retain(|r| r.score >= threshold);
    }

    // 5. 应用过滤条件（TODO: 需要实现）
    if let Some(_filter) = query.filter {
        // 过滤逻辑待实现
    }

    // 6. 分页处理
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit;
    results = results.into_iter().skip(offset).take(limit).collect();

    Ok(results)
}
```

**实现要点：**

- 使用读锁避免阻塞写入操作
- 暴力搜索适合小规模数据
- 支持可选字段返回
- 完整的排序和分页逻辑

### 4.4 并发控制

```rust
// 写入操作使用写锁
async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult> {
    let mut collections = self.collections.write().await;  // 写锁
    let (_, store) = collections.get_mut(collection)?;
    store.insert(point.id.clone(), point);
    Ok(...)
}

// 读取操作使用读锁
async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    let collections = self.collections.read().await;  // 读锁
    let (_, store) = collections.get(collection)?;
    // ... 搜索逻辑
}
```

**设计要点：**

- 读写分离，提高并发性能
- 使用 `tokio::sync::RwLock` 支持异步环境
- 避免死锁：先读后写，锁粒度适中

## 5. 生产引擎实现指南 (Qdrant)

### 5.1 依赖配置

```toml
[dependencies]
qdrant-client = "1.x"
tonic = "0.x"
prost = "0.x"
```

### 5.2 引擎结构

```rust
pub struct QdrantEngine {
    client: Arc<QdrantClient>,
    config: QdrantConfig,
}

pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
}
```

### 5.3 关键实现

#### 5.3.1 连接管理

```rust
impl QdrantEngine {
    pub async fn new(config: QdrantConfig) -> Result<Self> {
        let mut client_config = QdrantClientConfig::from_url(&config.url);

        if let Some(api_key) = &config.api_key {
            client_config.set_api_key(api_key);
        }

        let client = QdrantClient::from_config(client_config)
            .await
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            client: Arc::new(client),
            config,
        })
    }

    pub async fn with_url(url: &str) -> Result<Self> {
        let config = QdrantConfig {
            url: url.to_string(),
            api_key: None,
            timeout: Duration::from_secs(30),
        };
        Self::new(config).await
    }
}
```

#### 5.3.2 集合创建

```rust
async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()> {
    // 转换距离度量
    let distance = match config.distance {
        DistanceMetric::Cosine => Distance::Cosine,
        DistanceMetric::Euclid => Distance::Euclid,
        DistanceMetric::Dot => Distance::Dot,
    };

    // 构建向量参数
    let vector_params = VectorParamsBuilder::new(
        config.vector_size as u64,
        distance
    );

    // 可选：配置 HNSW
    if let Some(hnsw) = config.hnsw {
        vector_params.hnsw_config(HnswConfigDiff {
            m: Some(hnsw.m as u64),
            ef_construct: Some(hnsw.ef_construct as u64),
            ..Default::default()
        });
    }

    // 创建集合
    self.client.create_collection(
        CreateCollectionBuilder::new(name)
            .vectors_config(vector_params)
    ).await?;

    Ok(())
}
```

#### 5.3.3 搜索实现

```rust
async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    // 构建搜索点
    let mut search_points = SearchPointsBuilder::new(
        collection,
        query.vector,
        query.limit as u64
    )
    .with_payload(query.with_payload.unwrap_or(true))
    .with_vector(query.with_vector.unwrap_or(false));

    // 添加过滤条件
    if let Some(filter) = query.filter {
        let qdrant_filter = self.filter_to_qdrant(filter);
        search_points.filter(qdrant_filter);
    }

    // 添加分数阈值
    if let Some(threshold) = query.score_threshold {
        search_points.score_threshold(threshold);
    }

    // 添加偏移量
    if let Some(offset) = query.offset {
        search_points.offset(offset as u64);
    }

    // 执行搜索
    let result = self.client.search_points(search_points).await?;

    // 转换结果
    let results = result.result
        .into_iter()
        .map(self.from_qdrant_result)
        .collect();

    Ok(results)
}

fn filter_to_qdrant(&self, filter: VectorFilter) -> qdrant_client::qdrant::Filter {
    // 实现过滤条件转换逻辑
    // ...
}

fn from_qdrant_result(&self, point: ScoredPoint) -> SearchResult {
    SearchResult {
        id: point.id.to_string(),
        score: point.score,
        payload: Some(point.payload),
        vector: None,  // 根据需要提取
    }
}
```

## 6. 测试策略

### 6.1 测试分层

```
┌─────────────────────────────────────┐
│     集成测试 (Integration Tests)     │
│  - 端到端功能验证                    │
│  - 多组件协作测试                    │
└─────────────────────────────────────┘
              ↓
┌─────────────────────────────────────┐
│      单元测试 (Unit Tests)           │
│  - 单个函数/方法测试                  │
│  - Mock 引擎测试                     │
└─────────────────────────────────────┘
              ↓
┌─────────────────────────────────────┐
│      性能测试 (Benchmarks)           │
│  - 搜索性能测试                      │
│  - 并发性能测试                      │
└─────────────────────────────────────┘
```

### 6.2 测试覆盖要点

#### 6.2.1 集合管理测试

```rust
#[tokio::test]
async fn test_create_collection() {
    let engine = MockEngine::new();
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);

    assert!(engine.create_collection("test", config).await.is_ok());
    assert!(engine.collection_exists("test").await.unwrap());
}

#[tokio::test]
async fn test_create_duplicate_collection() {
    let engine = MockEngine::new();
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);

    engine.create_collection("test", config.clone()).await.unwrap();

    // 重复创建应该失败
    assert!(engine.create_collection("test", config).await.is_err());
}
```

#### 6.2.2 CRUD 操作测试

```rust
#[tokio::test]
async fn test_upsert_and_search() {
    let engine = MockEngine::new();
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    engine.create_collection("test", config).await.unwrap();

    // 插入向量
    let point = VectorPoint::new("1", vec![0.1, 0.2, 0.3]);
    engine.upsert("test", point).await.unwrap();

    // 搜索
    let query = SearchQuery::new(vec![0.1, 0.2, 0.3], 10);
    let results = engine.search("test", query).await.unwrap();

    assert!(!results.is_empty());
    assert_eq!(results[0].id, "1");
}
```

#### 6.2.3 并发测试

```rust
#[tokio::test]
async fn test_concurrent_upsert_and_search() {
    let engine = Arc::new(MockEngine::new());
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);
    engine.create_collection("test", config).await.unwrap();

    let mut handles = vec![];

    // 并发写入
    for i in 0..100 {
        let engine = engine.clone();
        handles.push(tokio::spawn(async move {
            let point = VectorPoint::new(
                format!("point_{}", i),
                vec![0.1; 128]
            );
            engine.upsert("test", point).await
        }));
    }

    // 并发搜索
    for i in 0..100 {
        let engine = engine.clone();
        handles.push(tokio::spawn(async move {
            let query = SearchQuery::new(vec![0.1; 128], 10);
            engine.search("test", query).await
        }));
    }

    // 等待所有任务完成
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
}
```

### 6.3 性能测试

```rust
#[bench]
fn bench_search_1000_vectors(b: &mut Bencher) {
    let engine = MockEngine::new();
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);
    engine.create_collection("bench", config).await.unwrap();

    // 插入 1000 个向量
    for i in 0..1000 {
        let point = VectorPoint::new(
            format!("point_{}", i),
            (0..128).map(|j| (i * j) as f32).collect()
        );
        engine.upsert("bench", point).await.unwrap();
    }

    let query = SearchQuery::new(vec![0.5; 128], 10);

    b.iter(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(engine.search("bench", query.clone())).unwrap()
    });
}
```

## 7. 性能优化建议

### 7.1 索引优化

#### 7.1.1 HNSW 索引

```rust
pub struct HnswConfig {
    pub m: usize,                    // 每个节点的最大连接数 (默认 16)
    pub ef_construct: usize,         // 构建时的搜索深度 (默认 100)
    pub full_scan_threshold: Option<usize>,
    pub max_indexing_threads: Option<usize>,
    pub on_disk: Option<bool>,
    pub payload_m: Option<usize>,
}
```

**优化建议：**

- `m` 越大，搜索越准确，但内存占用越高
- `ef_construct` 越大，索引质量越高，但构建时间越长
- 对于高维向量（>512），建议使用较大的 `m` (32-64)

#### 7.1.2 量化压缩

```rust
pub struct QuantizationConfig {
    pub enabled: bool,
    pub quant_type: Option<QuantizationType>,
}

pub enum QuantizationType {
    Scalar { quantile: Option<f32>, always_ram: Option<bool> },
    Product { compression: CompressionRatio, always_ram: Option<bool> },
    Binary { always_ram: Option<bool> },
}
```

**优化建议：**

- Scalar 量化：4-8 倍压缩，精度损失小
- Product 量化：8-64 倍压缩，适合高维向量
- Binary 量化：32 倍压缩，仅适用于特定场景

### 7.2 批量操作优化

```rust
// 批量 upsert 减少网络往返
async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<UpsertResult> {
    // 单次网络请求处理多个点
    // 内部可以并行处理
}

// 批量搜索并行执行
async fn search_batch(&self, collection: &str, queries: Vec<SearchQuery>) -> Result<Vec<Vec<SearchResult>>> {
    let futures = queries.into_iter()
        .map(|q| self.search(collection, q));

    futures::future::try_join_all(futures).await
}
```

### 7.3 缓存策略

```rust
// 缓存热点向量
struct VectorCache {
    cache: Arc<moka::future::Cache<PointId, VectorPoint>>,
}

impl VectorCache {
    async fn get(&self, id: &PointId) -> Option<VectorPoint> {
        self.cache.get(id).await
    }

    async fn insert(&self, point: VectorPoint) {
        self.cache.insert(point.id.clone(), point).await;
    }
}
```

### 7.4 异步处理

```rust
// 使用异步 IO
async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    // 非阻塞 IO 操作
    self.client.search_points(query).await?;
    // ...
}

// 支持流式返回
async fn search_stream(&self, collection: &str, query: SearchQuery)
    -> Result<impl Stream<Item = SearchResult>> {
    // 逐步返回结果，减少内存占用
}
```

## 8. 错误处理

### 8.1 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum VectorClientError {
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Collection already exists: {0}")]
    CollectionAlreadyExists(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidVectorDimension { expected: usize, actual: usize },

    #[error("Filter error: {0}")]
    FilterError(String),

    #[error("Index error: {0}")]
    IndexError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}
```

### 8.2 错误处理最佳实践

```rust
// 1. 使用 Result 包装所有可能失败的操作
async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult> {
    // 验证向量维度
    let config = self.get_collection_config(collection).await?;
    if point.vector.len() != config.vector_size {
        return Err(VectorClientError::InvalidVectorDimension {
            expected: config.vector_size,
            actual: point.vector.len(),
        });
    }

    // 执行 upsert
    // ...
}

// 2. 提供有意义的错误信息
match result {
    Ok(_) => Ok(()),
    Err(e) => Err(VectorClientError::InternalError(
        format!("Failed to upsert point: {}", e)
    )),
}

// 3. 使用 ? 操作符简化错误传播
async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    let collections = self.collections.read().await?;  // ? 自动传播错误
    let (_, store) = collections.get(collection)
        .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
    // ...
}
```

## 9. 监控和日志

### 9.1 日志记录

```rust
use tracing::{debug, info, warn, error};

async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    debug!("Searching in collection '{}' with limit {}", collection, query.limit);

    let start = std::time::Instant::now();

    // 搜索逻辑
    let results = /* ... */;

    let duration = start.elapsed();
    info!(
        "Search completed in {:?}: {} results",
        duration,
        results.len()
    );

    Ok(results)
}
```

### 9.2 指标监控

```rust
// 使用 metrics 或 prometheus 库
lazy_static! {
    static ref SEARCH_DURATION: HistogramVec = register_histogram_vec!(
        "vector_search_duration_seconds",
        "Vector search duration in seconds",
        &["collection"]
    ).unwrap();

    static ref SEARCH_TOTAL: CounterVec = register_counter_vec!(
        "vector_search_total",
        "Total number of vector searches",
        &["collection"]
    ).unwrap();
}

async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
    let start = std::time::Instant::now();
    SEARCH_TOTAL.with_label_values(&[collection]).inc();

    let results = /* ... */;

    let duration = start.elapsed();
    SEARCH_DURATION
        .with_label_values(&[collection])
        .observe(duration.as_secs_f64());

    Ok(results)
}
```

## 10. 最佳实践总结

### 10.1 设计原则

1. **接口优先**：先定义清晰的接口，再实现具体功能
2. **错误处理**：使用 Result 类型，提供有意义的错误信息
3. **并发安全**：使用合适的锁机制，避免数据竞争
4. **性能优先**：批量操作、索引优化、缓存策略

### 10.2 实现建议

1. **从 MockEngine 开始**：先实现简单的内存版本，验证接口设计
2. **逐步优化**：根据性能测试结果，逐步添加索引和缓存
3. **完整测试**：单元测试 + 集成测试 + 性能测试
4. **文档完善**：为所有公共 API 提供文档和示例

### 10.3 生产部署

1. **健康检查**：实现完整的健康检查机制
2. **监控告警**：集成监控和告警系统
3. **容错处理**：实现重试、降级、熔断机制
4. **性能调优**：根据实际负载调整参数

## 11. 参考资料

- [Qdrant 官方文档](https://qdrant.tech/documentation/)
- [HNSW 算法论文](https://arxiv.org/abs/1603.09320)
- [向量相似度搜索指南](https://www.pinecone.io/learn/vector-similarity-search/)
- [Rust Async 编程](https://rust-lang.github.io/async-book/)
