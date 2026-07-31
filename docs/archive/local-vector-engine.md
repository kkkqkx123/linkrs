# Local Vector Engine (嵌入式向量引擎)

当部署场景不需要 Qdrant 时，可在 `vector-client` 中新增 `LocalEngine` 实现 `VectorEngine` trait。

## 设计

```
vector-client/src/
├── engine/
│   ├── mod.rs          ← 新增 LocalEngine
│   ├── local.rs        ← 实现
│   └── ...
```

### 搜索后端

使用 `arroy 0.6.4`（HNSW + LMDB 持久化，Meilisearch 维护的纯 Rust 库）。

```rust
pub struct LocalEngine {
    // 按 collection 名隔离的 arroy 数据库实例
    databases: HashMap<String, ArroyContext>,
    // root 持久化目录
    persistence_path: PathBuf,
}

struct ArroyContext {
    env: heed::Env,
    db: Database<Euclidean>,
    writer: Writer<Euclidean>,
    reader: Reader<Euclidean>,
}
```

### 接口实现

`VectorEngine` trait ~20 个方法中，核心需要实现：

| 方法 | 实现方式 |
|------|---------|
| `create_collection` | 创建 LMDB env + arroy Database |
| `delete_collection` | 删除 LMDB 目录 |
| `upsert` | `writer.add_item` + `writer.builder.build` |
| `upsert_batch` | 循环调用 add_item + 批量 build |
| `delete` | `writer.del_item` + build |
| `search` | `reader.nns().by_item` 或 `by_vector` |

辅助方法（collection_exists, count, health_check 等）直接查询 metadata。

### 限制

- 不支持 payload 过滤（Qdrant 的 pre-filter 功能）
- 线程安全的写操作需要 LMDB 事务序列化
- 不支持分布式

### Feature gate

```toml
# Cargo.toml
[features]
local-engine = ["dep:arroy", "dep:heed"]
```

与 `qdrant-*` features 互斥可选项，通过配置选择 `vector_engine = "local"`。

## 工作量估算

- 核心搜索路径（search, upsert, delete）：3 天
- 辅助方法（collection mgmt, health, batch）：2 天
- 测试 + 文档：1 天

总计约 6 天。
