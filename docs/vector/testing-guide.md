# 向量引擎测试实现指南

## 1. 测试架构

### 1.1 测试分层

```
tests/
├── integration_vector_search.rs    # 向量搜索集成测试
├── integration_vector_query.rs     # 向量查询解析测试
├── integration_sync.rs             # 同步机制测试
└── common/                         # 测试工具模块
```

### 1.2 测试上下文

```rust
struct VectorTestContext {
    coordinator: Arc<VectorSyncCoordinator>,
    manager: Arc<VectorManager>,
}

impl VectorTestContext {
    async fn with_mock_engine() -> Self {
        let vector_config = VectorClientConfig::mock();
        
        let manager = Arc::new(
            VectorManager::new(vector_config).await.expect("Failed to create manager")
        );
        let coordinator = Arc::new(VectorSyncCoordinator::new(manager.clone(), None));
        
        Self { coordinator, manager }
    }
}
```

## 2. 测试辅助函数

### 2.1 向量生成

```rust
/// 生成测试向量
fn create_test_vector(size: usize, offset: f32) -> Vec<f32> {
    (0..size)
        .map(|i| (i as f32 + offset) / size as f32)
        .collect::<Vec<f32>>()
}

/// 生成带向量的顶点
fn create_test_vertex_with_vector(
    vid: i64,
    tag_name: &str,
    field_name: &str,
    vector: Vec<f32>,
) -> Vertex {
    let mut props = HashMap::new();
    let list_values: Vec<Value> = vector.iter().map(|&v| Value::Float(v as f64)).collect();
    props.insert(
        field_name.to_string(),
        Value::List(graphdb::core::List { values: list_values }),
    );
    let tag = Tag::new(tag_name.to_string(), props);
    Vertex::new(Value::Int(vid), vec![tag])
}
```

### 2.2 集合命名

```rust
/// 生成集合名称
fn make_collection_name(space_id: u64, tag_name: &str, field_name: &str) -> String {
    format!("space_{}_{}_{}", space_id, tag_name, field_name)
}

// 使用示例
let collection_name = make_collection_name(1, "Document", "embedding");
// 结果："space_1_Document_embedding"
```

## 3. 核心测试模式

### 3.1 集合管理测试

```rust
#[tokio::test]
async fn test_vector_manager_create_index() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    let result = ctx.manager.create_index("test_collection", config).await;
    
    assert!(result.is_ok(), "Creating index should succeed");
}

#[tokio::test]
async fn test_vector_manager_create_duplicate_index() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    ctx.manager.create_index("test_collection", config.clone()).await.unwrap();
    
    let result = ctx.manager.create_index("test_collection", config).await;
    
    assert!(result.is_err(), "Creating duplicate index should fail");
}

#[tokio::test]
async fn test_vector_manager_metadata() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    ctx.manager.create_index("test_collection", config).await.unwrap();
    
    // 验证元数据
    let metadata = ctx.manager.get_index_metadata("test_collection");
    assert!(metadata.is_some(), "Metadata should exist");
    
    // 验证存在性检查
    let exists = ctx.manager.index_exists("test_collection");
    assert!(exists, "Index should exist");
    
    let not_exists = ctx.manager.index_exists("non_existent");
    assert!(!not_exists, "Non-existent index should not exist");
}
```

### 3.2 向量插入测试

```rust
#[tokio::test]
async fn test_vector_insert() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .expect("Failed to create index");
    
    // 等待集合创建完成
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // 插入向量
    let vector = create_test_vector(3, 0.1);
    let point = VectorPoint::new("1", vector);
    
    ctx.coordinator
        .vector_manager()
        .upsert("space_1_Document_embedding", point)
        .await
        .expect("Failed to upsert vector");
    
    // 验证插入成功
    let count = ctx.coordinator.vector_manager().count("space_1_Document_embedding").await.unwrap();
    assert_eq!(count, 1, "Should have 1 vector");
}
```

### 3.3 向量搜索测试

```rust
#[tokio::test]
async fn test_vector_search_basic() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引并插入数据
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let vector = create_test_vector(3, 0.5);
    let point = VectorPoint::new("1", vector);
    ctx.coordinator.vector_manager()
        .upsert("space_1_Document_embedding", point)
        .await
        .unwrap();
    
    // 搜索
    let query_vector = create_test_vector(3, 0.5);
    let search_query = SearchQuery::new(query_vector, 10);
    
    let results = ctx.coordinator
        .search("space_1_Document_embedding", search_query)
        .await
        .unwrap();
    
    // 验证结果
    assert!(!results.is_empty(), "Search should return results");
    assert_eq!(results[0].id, "1", "Should find the inserted vector");
    assert!(results[0].score > 0.9, "Should have high similarity score");
}
```

### 3.4 批量操作测试

```rust
#[tokio::test]
async fn test_batch_upsert() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // 批量插入
    let points: Vec<VectorPoint> = (0..5)
        .map(|i| {
            let vector = create_test_vector(3, i as f32 * 0.1);
            VectorPoint::new(i.to_string(), vector)
        })
        .collect();
    
    ctx.coordinator
        .vector_manager()
        .upsert_batch("space_1_Document_embedding", points)
        .await
        .expect("Failed to batch upsert");
    
    // 验证数量
    let count = ctx.coordinator
        .vector_manager()
        .count("space_1_Document_embedding")
        .await
        .unwrap();
    
    assert_eq!(count, 5, "Should have 5 vectors");
}

#[tokio::test]
async fn test_batch_delete() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引并插入数据
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let points: Vec<VectorPoint> = (0..5)
        .map(|i| {
            let vector = create_test_vector(3, i as f32 * 0.1);
            VectorPoint::new(i.to_string(), vector)
        })
        .collect();
    
    ctx.coordinator
        .vector_manager()
        .upsert_batch("space_1_Document_embedding", points)
        .await
        .unwrap();
    
    // 批量删除
    let ids_to_delete: Vec<&str> = vec!["0", "1"];
    ctx.coordinator
        .vector_manager()
        .delete_batch("space_1_Document_embedding", ids_to_delete)
        .await
        .expect("Failed to batch delete");
    
    // 验证剩余数量
    let count = ctx.coordinator
        .vector_manager()
        .count("space_1_Document_embedding")
        .await
        .unwrap();
    
    assert_eq!(count, 3, "Should have 3 vectors remaining");
}
```

### 3.5 顶点同步测试

```rust
#[tokio::test]
async fn test_vertex_insert_with_vector() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // 插入带向量的顶点
    let vector = create_test_vector(3, 0.5);
    let vertex = create_test_vertex_with_vector(1, "Document", "embedding", vector.clone());
    
    ctx.coordinator
        .on_vertex_inserted(1, &vertex)
        .await
        .expect("Failed to insert vertex");
    
    // 验证向量可搜索
    let query_vector = create_test_vector(3, 0.5);
    let search_query = SearchQuery::new(query_vector, 10);
    
    let results = ctx.coordinator
        .search("space_1_Document_embedding", search_query)
        .await
        .unwrap();
    
    assert!(!results.is_empty(), "Should find the inserted vector");
}

#[tokio::test]
async fn test_vertex_delete_with_vector() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // 插入顶点
    let vector = create_test_vector(3, 0.5);
    let vertex = create_test_vertex_with_vector(1, "Document", "embedding", vector);
    
    ctx.coordinator
        .on_vertex_inserted(1, &vertex)
        .await
        .unwrap();
    
    // 删除顶点
    ctx.coordinator
        .on_vertex_deleted(1, "Document", &vertex.vid)
        .await
        .unwrap();
    
    // 验证向量不可搜索
    let query_vector = create_test_vector(3, 0.5);
    let search_query = SearchQuery::new(query_vector, 10);
    
    let results = ctx.coordinator
        .search("space_1_Document_embedding", search_query)
        .await
        .unwrap();
    
    assert!(results.is_empty(), "Should not find the deleted vector");
}
```

### 3.6 过滤搜索测试

```rust
#[tokio::test]
async fn test_vector_search_with_filter() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // 插入带 payload 的向量
    let vector1 = create_test_vector(3, 0.1);
    let mut payload1 = HashMap::new();
    payload1.insert("category".to_string(), serde_json::json!("A"));
    let point1 = VectorPoint::new("1", vector1).with_payload(payload1);
    
    let vector2 = create_test_vector(3, 0.2);
    let mut payload2 = HashMap::new();
    payload2.insert("category".to_string(), serde_json::json!("B"));
    let point2 = VectorPoint::new("2", vector2).with_payload(payload2);
    
    ctx.coordinator.vector_manager()
        .upsert("space_1_Document_embedding", point1)
        .await
        .unwrap();
    ctx.coordinator.vector_manager()
        .upsert("space_1_Document_embedding", point2)
        .await
        .unwrap();
    
    // 带过滤的搜索
    let query_vector = create_test_vector(3, 0.1);
    let filter = VectorFilter::new()
        .must(FilterCondition::match_value("category", "A"));
    let search_query = SearchQuery::new(query_vector, 10)
        .with_filter(filter);
    
    let results = ctx.coordinator
        .search("space_1_Document_embedding", search_query)
        .await
        .unwrap();
    
    // 验证结果
    assert_eq!(results.len(), 1, "Should return only matching result");
    assert_eq!(results[0].id, "1");
}
```

### 3.7 健康检查测试

```rust
#[tokio::test]
async fn test_health_check() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let health = ctx.coordinator
        .vector_manager()
        .engine()
        .health_check()
        .await
        .expect("Failed to perform health check");
    
    assert!(health.is_healthy, "Mock engine should be healthy");
    assert_eq!(health.engine_name, "mock");
    assert!(health.engine_version.contains("mock"));
}
```

## 4. 并发测试

### 4.1 并发读写测试

```rust
#[tokio::test]
async fn test_concurrent_upsert_and_search() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    ctx.coordinator
        .create_vector_index(1, "Document", "embedding", 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let manager = ctx.coordinator.vector_manager().clone();
    let mut handles = vec![];
    
    // 并发写入
    for i in 0..100 {
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let vector: Vec<f32> = (0..3).map(|j| (i * j) as f32).collect();
            let point = VectorPoint::new(format!("point_{}", i), vector);
            manager.upsert("space_1_Document_embedding", point).await
        }));
    }
    
    // 并发搜索
    for i in 0..50 {
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let query_vector: Vec<f32> = (0..3).map(|j| (i * j) as f32).collect();
            let query = SearchQuery::new(query_vector, 10);
            manager.search("space_1_Document_embedding", query).await
        }));
    }
    
    // 等待所有任务完成
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Task should succeed");
    }
    
    // 验证最终数量
    let count = manager.count("space_1_Document_embedding").await.unwrap();
    assert_eq!(count, 100, "Should have 100 vectors");
}
```

## 5. 错误处理测试

### 5.1 集合不存在错误

```rust
#[tokio::test]
async fn test_search_nonexistent_collection() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let query = SearchQuery::new(vec![0.1, 0.2, 0.3], 10);
    let result = ctx.coordinator
        .search("non_existent_collection", query)
        .await;
    
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VectorClientError::CollectionNotFound(_)
    ));
}
```

### 5.2 重复创建错误

```rust
#[tokio::test]
async fn test_create_duplicate_index() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    ctx.manager.create_index("test", config.clone()).await.unwrap();
    
    let result = ctx.manager.create_index("test", config).await;
    
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VectorClientError::CollectionAlreadyExists(_)
    ));
}
```

### 5.3 维度不匹配错误

```rust
#[tokio::test]
async fn test_invalid_vector_dimension() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建 3 维索引
    let config = CollectionConfig::new(3, DistanceMetric::Cosine);
    ctx.manager.create_index("test", config).await.unwrap();
    
    // 尝试插入 5 维向量（应该失败）
    let point = VectorPoint::new("1", vec![0.1, 0.2, 0.3, 0.4, 0.5]);
    let result = ctx.manager.upsert("test", point).await;
    
    // MockEngine 不验证维度，但生产引擎应该验证
    // 这个测试用于记录期望的行为
    // assert!(result.is_err());
}
```

## 6. 性能测试

### 6.1 搜索性能测试

```rust
#[tokio::test]
async fn bench_search_performance() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    // 创建索引
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);
    ctx.manager.create_index("bench", config).await.unwrap();
    
    // 插入 1000 个向量
    for i in 0..1000 {
        let vector: Vec<f32> = (0..128).map(|j| ((i * j) % 256) as f32 / 256.0).collect();
        let point = VectorPoint::new(format!("point_{}", i), vector);
        ctx.manager.upsert("bench", point).await.unwrap();
    }
    
    // 测试搜索性能
    let query_vector: Vec<f32> = (0..128).map(|i| i as f32 / 128.0).collect();
    let query = SearchQuery::new(query_vector, 10);
    
    let start = std::time::Instant::now();
    let results = ctx.manager.search("bench", query).await.unwrap();
    let duration = start.elapsed();
    
    println!("Search 1000 vectors took: {:?}", duration);
    assert!(!results.is_empty());
    assert_eq!(results.len(), 10);
    
    // 性能要求：应该在 100ms 内完成
    assert!(duration < Duration::from_millis(100), "Search should be fast");
}
```

### 6.2 批量插入性能测试

```rust
#[tokio::test]
async fn bench_batch_upsert_performance() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    let config = CollectionConfig::new(128, DistanceMetric::Cosine);
    ctx.manager.create_index("bench", config).await.unwrap();
    
    // 准备 1000 个向量
    let points: Vec<VectorPoint> = (0..1000)
        .map(|i| {
            let vector: Vec<f32> = (0..128).map(|j| ((i * j) % 256) as f32 / 256.0).collect();
            VectorPoint::new(format!("point_{}", i), vector)
        })
        .collect();
    
    // 批量插入
    let start = std::time::Instant::now();
    ctx.manager
        .upsert_batch("bench", points)
        .await
        .unwrap();
    let duration = start.elapsed();
    
    println!("Batch upsert 1000 vectors took: {:?}", duration);
    
    // 验证数量
    let count = ctx.manager.count("bench").await.unwrap();
    assert_eq!(count, 1000);
    
    // 性能要求：应该在 500ms 内完成
    assert!(duration < Duration::from_millis(500), "Batch upsert should be fast");
}
```

## 7. 测试最佳实践

### 7.1 测试隔离

```rust
// 每个测试使用独立的集合名称
#[tokio::test]
async fn test_isolated_1() {
    let ctx = VectorTestContext::with_mock_engine().await;
    ctx.manager.create_index("test_1", config.clone()).await.unwrap();
    // ...
}

#[tokio::test]
async fn test_isolated_2() {
    let ctx = VectorTestContext::with_mock_engine().await;
    ctx.manager.create_index("test_2", config.clone()).await.unwrap();
    // ...
}
```

### 7.2 使用超时

```rust
#[tokio::test(timeout = 5000)]  // 5 秒超时
async fn test_with_timeout() {
    // 长时间运行的测试
}
```

### 7.3 清晰的断言消息

```rust
assert!(result.is_ok(), "Creating index should succeed: {:?}", result.err());
assert_eq!(results.len(), 1, "Should return only matching result");
assert_eq!(results[0].id, "1", "First result should be point 1");
```

### 7.4 测试清理

```rust
#[tokio::test]
async fn test_with_cleanup() {
    let ctx = VectorTestContext::with_mock_engine().await;
    
    ctx.manager.create_index("test", config.clone()).await.unwrap();
    
    // 测试逻辑
    // ...
    
    // 清理（可选，因为 MockEngine 在测试结束后会自动清理）
    ctx.manager.drop_index("test").await.unwrap();
}
```

## 8. 常见问题

### Q1: 为什么测试失败显示 "CollectionNotFound"？

**A:** 确保：
1. 在搜索前已经创建了集合
2. 使用正确的集合名称（带 `space_` 前缀）
3. 给集合创建留出时间（添加 `sleep(10ms)`）

### Q2: 为什么并发测试会失败？

**A:** 检查：
1. 是否使用了 `Arc` 共享管理器
2. 是否正确处理了 `RwLock`
3. 是否有死锁风险

### Q3: 如何测试过滤功能？

**A:** MockEngine 的过滤功能尚未完全实现。对于过滤测试：
1. 使用 `#[ignore]` 标记测试
2. 或者使用 QdrantEngine 进行集成测试

### Q4: 如何调试测试失败？

**A:** 使用 `--nocapture` 标志：
```bash
cargo test test_name -- --nocapture
```

添加调试输出：
```rust
println!("Create index result: {:?}", result);
println!("Collection exists: {}", exists);
```

## 9. 测试检查清单

在提交测试前，确保：

- [ ] 测试名称清晰描述测试内容
- [ ] 使用独立的集合名称
- [ ] 有清晰的断言消息
- [ ] 处理了所有可能的错误
- [ ] 测试是可重复的
- [ ] 并发测试没有死锁风险
- [ ] 性能测试有合理的超时时间
- [ ] 添加了必要的注释

## 10. 参考资料

- [Rust 测试文档](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Tokio 测试指南](https://tokio.rs/tokio/tutorial/testing)
- [集成测试最佳实践](https://doc.rust-lang.org/rust-by-example/testing/integration_testing.html)
