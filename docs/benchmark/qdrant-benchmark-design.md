# Qdrant 基准测试设计文档

## 1. 概述

本文档描述了使用真实 Qdrant 服务进行基准测试的设计方案。目标是替换原有的 MockEngine，使用本地 Qdrant 服务（HTTP 端口 6333，RPC 端口 6334）进行真实性能测试和集成测试。

## 2. 架构设计

### 2.1 测试架构

```
┌─────────────────────────────────────────────────────────┐
│                   基准测试框架                           │
│                   (Criterion.rs)                        │
└────────────────┬────────────────────────────────────────┘
                 │
                 │ 异步调用
                 ▼
┌─────────────────────────────────────────────────────────┐
│                   QdrantEngine                          │
│              (实际 Qdrant 客户端)                        │
└────────────────┬────────────────────────────────────────┘
                 │
                 │ RPC (gRPC)
                 │ 端口：6334
                 ▼
┌─────────────────────────────────────────────────────────┐
│              Qdrant 服务 (本地运行)                      │
│              HTTP: 6333, RPC: 6334                      │
└─────────────────────────────────────────────────────────┘
```

### 2.2 关键设计决策

1. **使用真实 Qdrant 服务**：移除 MockEngine，直接使用本地 Qdrant 实例
2. **测量业务逻辑**：排除连接建立开销，专注于向量操作性能
3. **预热机制**：在正式测试前发送预热请求，避免冷启动影响
4. **测试隔离**：每个测试使用独立的 collection，测试后清理

## 3. 配置要求

### 3.1 Qdrant 服务配置

```yaml
# Qdrant 配置（已启动）
服务状态：运行中 (PID: 4740)
HTTP 端口：6333
RPC 端口：6334
版本：1.17.x
```

### 3.2 连接配置

```rust
use vector_client::{VectorClientConfig, ConnectionConfig};

let config = VectorClientConfig::new(
    ConnectionConfig::qdrant_local("localhost", 6334, 6333)
);
```

## 4. 基准测试设计

### 4.1 测试场景

#### 4.1.1 基础向量搜索性能

- **测试目标**：测量不同规模下的向量搜索性能
- **向量维度**：128d, 256d, 512d, 768d
- **数据规模**：100, 1000, 10000 个向量
- **测量指标**：
  - 平均搜索延迟
  - P95 延迟
  - 吞吐量（queries/s）

#### 4.1.2 不同距离度量性能

- **测试目标**：比较不同距离度量的性能差异
- **距离度量**：Cosine, Euclidean, Dot Product
- **固定参数**：1000 个向量，128 维
- **测量指标**：搜索延迟对比

#### 4.1.3 批量操作性能

- **测试目标**：测量批量 upsert 性能
- **批量大小**：10, 100, 1000
- **测量指标**：
  - 批量写入吞吐量
  - 单次操作平均延迟

#### 4.1.4 过滤搜索性能

- **测试目标**：测量带过滤条件的搜索性能
- **过滤类型**：Payload 匹配过滤
- **数据规模**：1000 个向量（50% 匹配过滤）
- **测量指标**：过滤搜索 vs 无过滤搜索的性能差异

#### 4.1.5 并发操作性能

- **测试目标**：测量并发读写性能
- **并发度**：1, 4, 8, 16 个并发任务
- **操作类型**：混合读写（80% 读，20% 写）
- **测量指标**：吞吐量、延迟分布

### 4.2 基准测试实现规范

参考 `benches/real_api_bench.rs` 的设计模式：

```rust
use criterion::{
    criterion_group, criterion_main,
    BenchmarkId, Criterion, Throughput,
};
use std::time::Duration;

fn bench_qdrant_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("qdrant_search");

    // 关键配置：只测量业务逻辑，排除连接开销
    group.measurement_time(Duration::from_secs(30));  // 长时间采样
    group.sample_size(50);  // 减少样本数，避免过度调用
    group.throughput(Throughput::Elements(1));

    // 创建 Qdrant 引擎（一次创建，重复使用）
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = rt.block_on(async {
        let config = VectorClientConfig::qdrant_local("localhost", 6334, 6333);
        QdrantEngine::new(config).await.unwrap()
    });

    // 预热：发送几个预热请求（不测量）
    rt.block_on(async {
        setup_collection(&engine, "bench", 128, 1000).await;
        for _ in 0..3 {
            let query_vector = create_test_vector(128, 0.5);
            let query = SearchQuery::new(query_vector, 10);
            let _ = engine.search("bench", query).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // 正式基准测试
    group.bench_function("search_1000_vectors_128d", |b| {
        b.to_async(&rt).iter(|| async {
            let query_vector = create_test_vector(128, 0.5);
            let query = SearchQuery::new(query_vector, 10);

            let results = engine.search("bench", query).await.unwrap();
            black_box(results)
        })
    });

    group.finish();
}
```

## 5. 集成测试设计

### 5.1 测试组织

```
tests/
├── integration_vector_qdrant.rs    # Qdrant 真实服务集成测试
└── common/
    └── qdrant_test_context.rs      # Qdrant 测试上下文
```

### 5.2 测试上下文

```rust
struct QdrantTestContext {
    engine: Arc<QdrantEngine>,
    collection_name: String,
}

impl QdrantTestContext {
    async fn new(test_name: &str) -> Self {
        let config = VectorClientConfig::qdrant_local("localhost", 6334, 6333);
        let engine = Arc::new(QdrantEngine::new(config).await.unwrap());

        // 使用测试名作为 collection 名，确保隔离
        let collection_name = format!("test_{}", test_name);

        // 创建 collection
        let config = CollectionConfig::new(128, DistanceMetric::Cosine);
        engine.create_collection(&collection_name, config).await.unwrap();

        Self {
            engine,
            collection_name,
        }
    }

    async fn cleanup(self) {
        // 删除 collection
        self.engine.delete_collection(&self.collection_name).await.unwrap();
    }
}
```

### 5.3 测试用例

1. **基础 CRUD 操作**
   - 创建/删除 collection
   - 向量 upsert/get/delete
   - 批量操作

2. **搜索功能**
   - 基础搜索
   - 过滤搜索
   - 分页搜索
   - 分数阈值

3. **Payload 操作**
   - 设置/删除 payload
   - Payload 过滤

4. **索引操作**
   - 创建字段索引
   - 删除字段索引

## 6. 性能优化建议

### 6.1 测试优化

1. **连接池复用**：避免重复创建连接
2. **Collection 复用**：多个测试共享 collection（注意数据隔离）
3. **异步批量操作**：使用批量 API 减少网络往返
4. **合理超时设置**：避免测试挂起

### 6.2 Qdrant 配置优化

```yaml
# 建议的 Qdrant 配置优化
performance:
  max_search_threads: 4
  cpu_budget_threads: 4

storage:
  performance:
    max_io_threads: 4
```

## 7. 实施步骤

### 7.1 第一阶段：移除 MockEngine

1. 删除 `src/engine/mock.rs`
2. 更新 `src/engine/mod.rs`，移除 MockEngine 导出
3. 更新依赖配置

### 7.2 第二阶段：创建 Qdrant 基准测试

1. 创建 `benches/qdrant_benchmark.rs`
2. 实现 5 个测试场景
3. 配置 Criterion 参数

### 7.3 第三阶段：更新集成测试

1. 创建 `tests/integration_vector_qdrant.rs`
2. 实现测试上下文
3. 迁移现有测试用例

### 7.4 第四阶段：文档和清理

1. 更新测试文档
2. 清理未使用的依赖
3. 运行完整测试套件

## 8. 注意事项

### 8.1 测试环境要求

- Qdrant 服务必须运行在 localhost:6333 (HTTP) 和 localhost:6334 (RPC)
- 测试前确保 Qdrant 服务已启动
- 测试后清理测试数据

### 8.2 测试隔离

- 每个测试使用独立的 collection
- 使用 `#[tokio::test]` 标记异步测试
- 测试失败时确保资源清理

### 8.3 性能测试最佳实践

- 避免在性能测试中打印日志
- 使用 `black_box` 防止编译器优化
- 多次运行取平均值
- 记录系统配置和环境信息

## 9. 参考文档

- [Qdrant 官方文档](https://qdrant.tech/documentation/)
- [Criterion.rs 基准测试指南](https://bheisler.github.io/criterion.rs/book/)
- [qdrant-client Rust SDK](https://github.com/qdrant/qdrant-client)
- 原有设计文档：`docs/vector/vector-engine-design.md`

## 10. 验收标准

- [ ] MockEngine 完全移除
- [ ] QdrantEngine 基准测试运行正常
- [ ] 所有集成测试通过
- [ ] 性能数据可重复且稳定
- [ ] 文档完整且准确
