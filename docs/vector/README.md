# 向量引擎文档索引

## 文档概览

本目录包含 GraphDB 向量引擎的完整设计和实现文档，涵盖了从架构设计、实现指南到测试策略的各个方面。

## 文档列表

### 1. [向量引擎设计文档](vector-engine-design.md)

**内容概要：**
- 向量引擎的整体架构设计
- 核心接口定义（VectorEngine Trait）
- 数据结构设计（CollectionConfig, VectorPoint, SearchQuery 等）
- MockEngine 实现详解
- 生产引擎（Qdrant）实现指南
- 性能优化建议
- 错误处理机制
- 监控和日志

**适用场景：**
- 了解向量引擎的整体设计
- 学习如何设计和实现向量引擎
- 参考接口定义和数据结构
- 性能优化和错误处理指导

**关键章节：**
1. 分层架构设计
2. VectorEngine Trait 定义
3. 核心数据结构
4. MockEngine 实现（含余弦相似度算法）
5. QdrantEngine 实现指南
6. 测试策略
7. 性能优化建议

### 2. [测试实现指南](testing-guide.md)

**内容概要：**
- 测试架构和分层
- 测试上下文和辅助函数
- 核心测试模式（集合管理、CRUD、搜索、批量操作等）
- 并发测试实现
- 错误处理测试
- 性能测试示例
- 测试最佳实践
- 常见问题解答

**适用场景：**
- 编写向量引擎测试
- 学习测试模式和技巧
- 解决测试中遇到的问题
- 提高测试覆盖率

**关键章节：**
1. 测试上下文（VectorTestContext）
2. 辅助函数（向量生成、集合命名）
3. 核心测试模式
4. 并发测试实现
5. 性能测试示例
6. 测试最佳实践

### 3. [实现检查清单](implementation-checklist.md)

**内容概要：**
- 完整的接口实现清单
- 数据结构实现清单
- 算法实现清单
- 并发控制清单
- 错误处理清单
- 性能优化清单
- 测试覆盖清单
- 文档清单
- 生产就绪清单
- MockEngine 和 QdrantEngine 特定检查项

**适用场景：**
- 实现新的向量引擎
- 验证实现完整性
- 代码审查参考
- 项目进度跟踪

**关键章节：**
1. 核心接口实现清单
2. 数据结构实现清单
3. 算法实现清单
4. 测试覆盖清单
5. MockEngine 特定检查项
6. QdrantEngine 特定检查项

## 快速开始指南

### 对于新手

1. **第一步**：阅读 [向量引擎设计文档](vector-engine-design.md) 的 1-3 章，了解整体架构
2. **第二步**：阅读 MockEngine 实现部分（第 4 章），理解参考实现
3. **第三步**：阅读 [测试实现指南](testing-guide.md)，学习如何测试
4. **第四步**：参考 [实现检查清单](implementation-checklist.md) 验证理解

### 对于实现者

1. **规划阶段**：使用 [实现检查清单](implementation-checklist.md) 规划工作
2. **设计阶段**：参考 [向量引擎设计文档](vector-engine-design.md) 的接口定义
3. **实现阶段**：对照检查清单逐项实现
4. **测试阶段**：使用 [测试实现指南](testing-guide.md) 编写测试

### 对于审查者

1. **代码审查**：使用 [实现检查清单](implementation-checklist.md) 验证完整性
2. **测试审查**：参考 [测试实现指南](testing-guide.md) 验证测试覆盖
3. **文档审查**：检查 [向量引擎设计文档](vector-engine-design.md) 的准确性

## 核心概念速查

### VectorEngine Trait

```rust
#[async_trait]
pub trait VectorEngine: Send + Sync + std::fmt::Debug {
    // 基本信息
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    async fn health_check(&self) -> Result<HealthStatus>;

    // 集合管理
    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<()>;
    async fn collection_exists(&self, name: &str) -> Result<bool>;
    async fn collection_info(&self, name: &str) -> Result<CollectionInfo>;

    // 向量操作
    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult>;
    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>>;
    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult>;
    
    // ... 更多方法
}
```

### 核心数据结构

```rust
// 集合配置
pub struct CollectionConfig {
    pub vector_size: usize,
    pub distance: DistanceMetric,
    pub hnsw: Option<HnswConfig>,
    pub quantization: Option<QuantizationConfig>,
}

// 向量点
pub struct VectorPoint {
    pub id: PointId,
    pub vector: Vec<f32>,
    pub payload: Option<Payload>,
}

// 搜索查询
pub struct SearchQuery {
    pub vector: Vec<f32>,
    pub limit: usize,
    pub offset: Option<usize>,
    pub score_threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
}

// 搜索结果
pub struct SearchResult {
    pub id: PointId,
    pub score: f32,
    pub payload: Option<Payload>,
    pub vector: Option<Vec<f32>>,
}
```

### 距离度量

```rust
pub enum DistanceMetric {
    Cosine,    // 余弦相似度：cos(θ) = (a·b) / (||a|| * ||b||)
    Euclid,    // 欧几里得距离：√(Σ(aᵢ - bᵢ)²)
    Dot,       // 点积：Σ(aᵢ * bᵢ)
}
```

### 相似度计算（余弦）

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}
```

## 架构决策记录

### 为什么使用 Trait 抽象？

**决策**：使用 `VectorEngine` Trait 抽象底层引擎实现

**原因**：
1. 支持多种后端（Mock、Qdrant、其他）
2. 便于测试（可以使用 MockEngine）
3. 符合依赖倒置原则
4. 易于扩展新引擎

**参考**：[向量引擎设计文档 - 架构设计](vector-engine-design.md#2-架构设计)

### 为什么选择异步接口？

**决策**：所有公共方法使用 `async`

**原因**：
1. 向量操作可能涉及 IO（网络、磁盘）
2. 支持高并发场景
3. 符合 Rust 现代异步编程实践
4. 便于与 Tokio 运行时集成

**参考**：[向量引擎设计文档 - 核心接口](vector-engine-design.md#3-核心接口定义)

### 为什么 MockEngine 使用暴力搜索？

**决策**：MockEngine 使用 O(n) 的暴力搜索而非 HNSW

**原因**：
1. 实现简单，便于理解和维护
2. 适合测试场景（数据量小）
3. 保证结果准确性
4. 生产环境使用 QdrantEngine（支持 HNSW）

**参考**：[向量引擎设计文档 - MockEngine 实现](vector-engine-design.md#4-mockengine-实现)

### 为什么使用 Arc<RwLock<>>？

**决策**：使用 `Arc<RwLock<HashMap>>` 管理集合

**原因**：
1. `Arc` 支持多线程共享
2. `RwLock` 支持读写分离（提高并发性能）
3. `HashMap` 提供 O(1) 的查找性能
4. 实现简单，适合测试和开发

**参考**：[向量引擎设计文档 - MockEngine 数据结构](vector-engine-design.md#41-数据结构)

## 常见问题 (FAQ)

### Q: 如何选择使用 MockEngine 还是 QdrantEngine？

**A:** 
- **开发/测试阶段**：使用 MockEngine（简单、快速、无依赖）
- **生产环境**：使用 QdrantEngine（高性能、持久化、功能完整）
- **原型验证**：使用 MockEngine（快速验证接口设计）

### Q: 向量引擎支持哪些距离度量？

**A:** 目前支持三种：
- **Cosine**：余弦相似度，适合文本、图像等高维向量
- **Euclid**：欧几里得距离，适合空间向量
- **Dot**：点积，适合归一化向量

### Q: 如何优化搜索性能？

**A:** 
1. 使用 HNSW 索引（QdrantEngine 支持）
2. 使用量化压缩减少内存
3. 批量搜索并行执行
4. 缓存热点结果

详见：[性能优化建议](vector-engine-design.md#7-性能优化建议)

### Q: MockEngine 支持过滤搜索吗？

**A:** MockEngine 的过滤功能尚未完全实现。对于过滤测试：
- 使用 `#[ignore]` 标记相关测试
- 或者使用 QdrantEngine 进行集成测试

### Q: 如何调试测试失败？

**A:** 
```bash
# 使用 nocapture 查看输出
cargo test test_name -- --nocapture

# 查看特定测试的详细输出
RUST_LOG=debug cargo test test_name -- --nocapture
```

### Q: 向量引擎的并发性能如何？

**A:** 
- MockEngine 使用 `RwLock`，支持并发读写
- 读操作不阻塞，写操作互斥
- 适合测试场景，不适合高并发生产环境
- 生产环境使用 QdrantEngine

详见：[并发测试](testing-guide.md#4-并发测试)

### Q: 如何添加新的向量引擎实现？

**A:** 
1. 实现 `VectorEngine` Trait
2. 提供引擎配置结构
3. 实现所有必需方法
4. 编写单元测试和集成测试
5. 更新文档

详见：[实现检查清单](implementation-checklist.md)

## 代码示例

### 创建和使用向量索引

```rust
use graphdb::vector::{VectorManager, VectorClientConfig};
use graphdb::vector_client::{CollectionConfig, DistanceMetric, VectorPoint, SearchQuery};

#[tokio::main]
async fn main() {
    // 1. 创建管理器
    let config = VectorClientConfig::mock();
    let manager = VectorManager::new(config).await.unwrap();
    
    // 2. 创建索引
    let collection_config = CollectionConfig::new(128, DistanceMetric::Cosine);
    manager.create_index("my_index", collection_config).await.unwrap();
    
    // 3. 插入向量
    let vector: Vec<f32> = (0..128).map(|i| i as f32 / 128.0).collect();
    let point = VectorPoint::new("point_1", vector);
    manager.upsert("my_index", point).await.unwrap();
    
    // 4. 搜索
    let query_vector: Vec<f32> = (0..128).map(|i| i as f32 / 128.0).collect();
    let query = SearchQuery::new(query_vector, 10);
    let results = manager.search("my_index", query).await.unwrap();
    
    println!("Found {} results", results.len());
    for result in results {
        println!("ID: {}, Score: {}", result.id, result.score);
    }
}
```

### 批量操作

```rust
// 批量插入
let points: Vec<VectorPoint> = (0..100)
    .map(|i| {
        let vector: Vec<f32> = (0..128).map(|j| ((i * j) % 256) as f32 / 256.0).collect();
        VectorPoint::new(format!("point_{}", i), vector)
    })
    .collect();

manager.upsert_batch("my_index", points).await.unwrap();

// 批量搜索
let queries: Vec<SearchQuery> = (0..10)
    .map(|i| {
        let vector: Vec<f32> = (0..128).map(|j| ((i * j) % 256) as f32 / 256.0).collect();
        SearchQuery::new(vector, 5)
    })
    .collect();

let all_results = manager.search_batch("my_index", queries).await.unwrap();
```

### 使用过滤条件

```rust
use graphdb::vector_client::{VectorFilter, FilterCondition};

let mut payload = HashMap::new();
payload.insert("category".to_string(), serde_json::json!("A"));
let point = VectorPoint::new("1", vector).with_payload(payload);

// 带过滤的搜索
let filter = VectorFilter::new()
    .must(FilterCondition::match_value("category", "A"));
let query = SearchQuery::new(query_vector, 10)
    .with_filter(filter);

let results = manager.search("my_index", query).await.unwrap();
```

## 性能基准

### MockEngine 性能

| 操作 | 数据量 | 延迟 | 吞吐量 |
|------|--------|------|--------|
| 搜索 | 100 向量 | < 1ms | - |
| 搜索 | 1000 向量 | < 10ms | - |
| 搜索 | 10000 向量 | < 100ms | - |
| 批量插入 | 1000 向量 | < 50ms | > 20k 向量/s |
| 并发搜索 | 100 并发 | < 100ms | - |

**注意**：MockEngine 使用暴力搜索，性能随数据量线性下降。生产环境请使用 QdrantEngine。

### QdrantEngine 性能（参考）

| 操作 | 数据量 | 延迟 | 吞吐量 |
|------|--------|------|--------|
| 搜索 (HNSW) | 1M 向量 | < 10ms | > 1000 QPS |
| 搜索 (HNSW) | 10M 向量 | < 50ms | > 500 QPS |
| 批量插入 | 1000 向量 | < 100ms | > 10k 向量/s |

**注意**：实际性能取决于硬件配置和参数设置。

## 相关资源

### 内部资源
- [Cargo.toml](../../Cargo.toml) - 项目依赖
- [src/vector/mod.rs](../../src/vector/mod.rs) - 向量模块入口
- [crates/vector-client/](../../crates/vector-client/) - 向量客户端 crate

### 外部资源
- [Qdrant 官方文档](https://qdrant.tech/documentation/)
- [HNSW 算法论文](https://arxiv.org/abs/1603.09320)
- [向量相似度搜索指南](https://www.pinecone.io/learn/vector-similarity-search/)
- [Rust Async 编程](https://rust-lang.github.io/async-book/)
- [Tokio 文档](https://tokio.rs/)

## 更新日志

### 2026-04-10
- 创建文档索引
- 完成向量引擎设计文档
- 完成测试实现指南
- 完成实现检查清单

### 未来计划
- [ ] 添加 QdrantEngine 实现文档
- [ ] 添加性能调优指南
- [ ] 添加故障排查手册
- [ ] 添加最佳实践案例

## 贡献指南

欢迎贡献文档！请遵循以下步骤：

1. Fork 项目
2. 创建特性分支
3. 修改文档
4. 运行 `cargo fmt` 和 `cargo clippy`
5. 提交 PR

文档风格指南：
- 使用中文编写文档
- 代码注释使用英文
- 提供代码示例
- 包含性能数据
- 引用外部资源

---

**维护者**：GraphDB Team  
**最后更新**：2026-04-10  
**版本**：1.0.0
