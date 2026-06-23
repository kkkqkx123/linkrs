# Qdrant 迁移总结

## 1. 迁移概述

成功将 Vector Client 从 MockEngine 迁移到使用真实 Qdrant 服务。

### 主要变更

1. **移除 MockEngine**
   - 删除 `src/engine/mock.rs` 文件
   - 移除 `mock` 特性
   - 更新所有相关配置和引用

2. **QdrantEngine 配置**
   - 添加 `qdrant_local` 配置方法
   - 支持自定义 HTTP 和 gRPC 端口
   - 默认端口：HTTP 6333, gRPC 6334

3. **测试框架**
   - 创建 Qdrant 基准测试 `benches/qdrant_benchmark.rs`
   - 7 个基准测试场景
   - 集成测试待完善（需要处理 Qdrant UUID 格式）

## 2. 文件变更清单

### 删除的文件

- `crates/vector-client/src/engine/mock.rs`
- `benches/vector_benchmark.rs`

### 修改的文件

- `crates/vector-client/src/engine/mod.rs` - 移除 MockEngine 导出
- `crates/vector-client/src/config/client.rs` - 移除 Mock 配置，添加 qdrant_local 方法
- `crates/vector-client/src/manager/mod.rs` - 移除 MockEngine 初始化逻辑
- `crates/vector-client/src/api/embedded/client.rs` - 移除 mock 相关 API
- `crates/vector-client/src/lib.rs` - 移除 MockEngine 导出
- `crates/vector-client/Cargo.toml` - 移除 mock 特性
- `tests/integration_vector_search.rs` - 更新为使用 QdrantEngine

### 新增的文件

- `docs/benchmark/qdrant-benchmark-design.md` - 设计文档
- `benches/qdrant_benchmark.rs` - Qdrant 基准测试

## 3. 配置变更

### EngineType

```rust
// 之前
pub enum EngineType {
    Qdrant,
    Mock,
}

// 现在
pub enum EngineType {
    Qdrant,
}
```

### VectorClientConfig

```rust
// 之前
VectorClientConfig::mock()

// 现在
VectorClientConfig::qdrant_local("localhost", 6334, 6333)
```

### ConnectionConfig

```rust
// 新增字段
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,          // gRPC 端口
    pub use_tls: bool,
    pub api_key: Option<String>,
    pub connect_timeout_secs: u64,
    pub http_port: Option<u16>,  // 新增
}
```

## 4. 基准测试

### 测试场景

1. **基础搜索性能**
   - `search_100_vectors_128d`
   - `search_1000_vectors_128d`

2. **不同维度测试**
   - 128d, 256d, 512d

3. **不同距离度量**
   - Cosine, Euclidean, Dot Product

4. **批量操作**
   - 批量 upsert (10, 100, 1000)

5. **过滤搜索**
   - Payload 过滤性能

6. **并发操作**
   - 1, 4, 8 并发度

### 运行基准测试

```shell
cd crates/vector-client
cargo bench --bench qdrant_benchmark
```

**前提条件**：

- Qdrant 服务运行在 localhost:6333 (HTTP) 和 localhost:6334 (gRPC)

## 5. 集成测试

### 当前状态

原有集成测试文件 `tests/integration_vector_search.rs` 已更新为使用 QdrantEngine，但需要注意：

1. **Point ID 格式**：Qdrant 要求 UUID 格式或数字 ID
2. **测试隔离**：每个测试使用独立的 collection
3. **资源清理**：测试后需要删除 collection

### 测试用例

- VectorManager 基础操作
- VectorSyncCoordinator 集成
- 向量搜索功能
- 批量操作
- 健康检查

## 6. 编译验证

### 编译成功

```shell
cd crates/vector-client
cargo build --lib
cargo build --bench qdrant_benchmark
```

结果：✅ 编译成功，无错误

### 基准测试编译

```shell
cargo bench --bench qdrant_benchmark --no-run
```

结果：✅ 编译成功，可执行

### 基准测试运行

```shell
cargo bench --bench qdrant_benchmark -- search_100_vectors_128d
```

结果：✅ 运行成功

```
qdrant_search/search_100_vectors_128d
                        time:   [283.00 µs 288.23 µs 294.82 µs]
                        thrpt:  [3.3920 Kelem/s 3.4694 Kelem/s 3.5336 Kelem/s]
```

### 测试运行

```shell
cargo test --test integration_vector_qdrant
```

注意：集成测试需要处理 Qdrant 的 Point ID 格式要求

## 7. 使用说明

### 启动 Qdrant

```powershell
# Windows
start-qdrant
```

验证服务：

- HTTP: http://localhost:6333
- gRPC: localhost:6334

### 创建 VectorClient

```rust
use vector_client::{VectorClient, VectorClientConfig};

let config = VectorClientConfig::qdrant_local("localhost", 6334, 6333);
let client = VectorClient::new(config).await?;
```

### 运行基准测试

```shell
# 运行所有基准测试
cargo bench --bench qdrant_benchmark

# 运行特定基准测试
cargo bench --bench qdrant_benchmark -- --bench search_1000_vectors_128d
```

## 8. 性能优化建议

### Qdrant 配置

```yaml
# config.yaml
storage:
  storage_path: ./qdrant_storage

performance:
  max_search_threads: 4

service:
  grpc_port: 6334
  http_port: 6333
```

### 客户端优化

1. **连接复用**：避免重复创建 QdrantEngine
2. **批量操作**：使用 upsert_batch 代替多次 upsert
3. **合理超时**：根据数据规模设置合适的超时时间
4. **索引优化**：为常用过滤字段创建 payload 索引

## 9. 注意事项

### Point ID 格式

Qdrant 支持以下 ID 格式：

- UUID: `550e8400-e29b-41d4-a716-446655440000`
- 数字：`1234567890`

不支持的格式：

- 短字符串：`p1`, `point_1`
- 非 UUID 格式的字符串

### 测试隔离

每个测试应该：

1. 使用唯一的 collection 名称
2. 测试完成后清理资源
3. 避免测试间相互依赖

### 错误处理

常见错误及解决方案：

1. **ConnectionFailed**
   - 检查 Qdrant 服务是否运行
   - 验证端口配置是否正确

2. **Invalid UUID format**
   - 使用 UUID 或数字作为 Point ID
   - 或使用 `qdrant_client::qdrant::PointId` 的 from_num 方法

3. **Collection not found**
   - 确保先创建 collection
   - 检查 collection 名称拼写

## 10. 后续工作

### 待完善

1. **集成测试**：创建完整的 Qdrant 集成测试套件
2. **性能调优**：根据基准测试结果优化配置
3. **文档补充**：添加更多使用示例和最佳实践

### 建议

1. **CI/CD 集成**：在 CI 中运行 Qdrant Docker 容器进行测试
2. **性能监控**：建立性能基线，持续监控性能变化
3. **错误处理**：增强错误处理和重试机制

## 11. 参考文档

- [Qdrant 官方文档](https://qdrant.tech/documentation/)
- [qdrant-client Rust SDK](https://github.com/qdrant/qdrant-client)
- [设计文档](docs/benchmark/qdrant-benchmark-design.md)
- [原有 Vector 文档](docs/vector/README.md)
