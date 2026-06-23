# 存储层改进方案

## 背景

inversearch 的存储层（`crates/inversearch/src/storage/`）共约 3456 行代码，分布在 20 个文件中，提供三个后端：`MemoryStorage`、`FileStorage`、`ColdWarmCacheManager`。此外，graphdb 自身也有独立的存储层（`src/storage/`）用于图和属性的持久化。

本文档分析 inversearch 存储层的现有问题，以及 graphdb 和 inversearch 存储是否应共用同一基础设施。

## 当前存储架构

```
StorageInterface (trait)
    ├── MemoryStorage         — 内存 HashMap，无持久化
    ├── FileStorage           — 单文件 postcard 序列化，仅 close() 时写盘
    └── ColdWarmCacheManager  — 三級缓存：Hot(WAL+DashMap) → Warm(mmap) → Cold(disk)
```

### 关键数据流

```
StorageBase (内存结构)
├── data: HashMap<String, Vec<DocId>>            // term -> doc_id 列表
├── context_data: HashMap<String, HashMap<String, Vec<DocId>>>  // 上下文索引
└── documents: HashMap<DocId, String>            // 文档内容
```

## 现有问题

### P0 — 数据安全性

| 问题 | 文件位置 | 说明 |
|------|----------|------|
| **FileStorage.commit() 不写磁盘** | `storage/file.rs:68-79` | commit() 仅更新内存 HashMap，close() 时才序列化到文件。两者之间崩溃 → 全部数据丢失 |
| **无事务支持** | 全局 | 没有 begin/commit/rollback 模式。批量操作中途失败留下脏数据 |
| **WAL 存全量快照** | `cold_warm_cache/manager.rs` | 每次 WAL entry 包含完整 IndexData。大索引下 WAL 膨胀快，恢复慢 |
| **无加载完整性校验** | 全局 | 加载文件时不检查校验和或版本兼容性 |

### P1 — 架构与性能

| 问题 | 说明 |
|------|------|
| **KeystoreMap 哈希桶仅 256** | 简单 CRC 哈希，`hash % 256`，大数据集下冲突率极高 |
| **两个独立的压缩模块** | `storage/common/compression.rs` 和 `serialize/compression.rs` 都包装 zstd，API 不统一 |
| **LRU 淘汰 O(n log n)** | ColdWarmCache 的 evict 每次对所有 entry 按访问时间排序 |
| **commit() O(n) 扫描** | ColdWarmCacheManager.commit() 遍历所有 caches 判断 insert vs update |
| **FileStorage 无法通过工厂创建** | `StorageFactory` 只支持 MemoryStorage 和 ColdWarmCache |

### P2 — 实现缺陷

| 问题 | 说明 |
|------|------|
| **IndexSnapshot 是 stub** | `persistence.rs` 的 map_entries/ctx_entries 始终为空 |
| **BackgroundTaskManager 命名与行为不符** | 清理任务实际调用了 create_checkpoint |
| **has() O(n * m)** | `StorageBase::has()` 遍历所有 term 向量和 context 映射 |

## 改进建议

### 立即修复 (P0)

**1. FileStorage 增加 WAL 或即时写入**

```rust
// 建议方案：增量 WAL + 最终合并
async fn commit(&self, index: &Index, ...) {
    // 写入增量 WAL entry（仅记录变更，非全量）
    self.wal.append(WalEntry::from_changes(changes)).await;
    // 更新内存状态
    self.cache.apply(index);
}

async fn close(&self) {
    // 合并 WAL + cache → 原子写主文件
    let data = self.merge().await;
    atomic_write(&self.path, &data).await;
    // 清理已合并的 WAL
    self.wal.clear().await;
}
```

这使 `FileStorage` 获得崩溃恢复能力，代码量增加约 150 行。

**2. 增加事务接口**

```rust
pub trait Transactional {
    async fn begin_tx(&mut self) -> Result<TxId>;
    async fn commit_tx(&mut self, tx: TxId) -> Result<()>;
    async fn rollback_tx(&mut self, tx: TxId) -> Result<()>;
}
```

批量操作在事务内执行，失败时回滚。单机场景下用内存快照 + WAL 即可实现。

### 中期优化 (P1)

**3. 合并两个压缩模块**

将 `storage/common/compression.rs` 和 `serialize/compression.rs` 的功能整合到 `compress/` 模块下，统一 API：

```rust
pub enum CompressionAlgo {
    Zstd { level: i32 },
    Lz4,
    None,
}
```

**4. KeystoreMap 哈希改进**

- 桶数改为可配置（默认 1024 或 4096）
- 哈希算法升级为 `ahash`（已存在于依赖中）或 `fxhash`

**5. LRU 淘汰改用真正的 LRU 数据结构**

当前 `cold_warm_cache/manager.rs` 的 evict 遍历全部 entry 排序 → 改为使用 `linked-hash-map`（已在 inversearch 依赖中）或 `lru` crate。

### 远期 (P2)

**6. WAL 增量格式**

当前 WAL 存全量 IndexData → 改为存 `Vec<(String, Vec<DocId>)>`（变更的 term 列表），回放时 merge 而非 replace。

**7. PersistenceManager IndexSnapshot 完整实现**

将 `map_entries` 和 `ctx_entries` 从空 Vec 改为实际序列化全量索引。

## 存储是否应由 graphdb 统一管理

### 问题：inversearch 的存储和 graphdb 的存储（src/storage/）能否共用？

经过分析，**不建议强行统一**，理由如下：

| 维度 | graphdb 存储 | inversearch 存储 |
|------|------------|-----------------|
| 数据模型 | 图结构（vertex + edge + property） | 倒排索引（term → doc_id list） |
| 访问模式 | KV + 图遍历 | term-based search + scoring |
| 事务需求 | ACID（图事务） | 无（最终一致可接受） |
| 性能要求 | 低延迟点查 | 高吞吐批量索引 |
| 持久化格式 | 自定义 page-based 存储 | postcard 序列化 HashMap |

两个存储层的访问模式和性能需求有本质差异。硬性统一会导致：

- **抽象泄漏**：图存储的事务隔离语义对搜索存储无意义
- **性能折中**：倒排索引要求批量顺序写，图存储要求随机点查，两者优化方向矛盾
- **依赖膨胀**：如果熔铸为一个通用 crate，需要同时满足两个场景，特征数量翻倍

### 推荐的演进方向

**保持独立，但提取工具层为通用 crate**：

```
crates/
├── graphdb-storage-core/     ← 新增：图 + 搜索引公用的基础设施
│   ├── io (atomic_write, mmap, file_utils)
│   ├── compression (zstd, lz4 统一包装)
│   ├── wal (通用 WAL 实现)
│   └── error (io / checksum / version)
├── inversearch/               当前 storage/ 模块保留
└── tantivy/                   使用 tantivy 自有的 directory 模块
```

**提取原则**：只提取与领域无关的**基础设施**（原子写、压缩、WAL 框架、错误类型），不放业务逻辑。

### 具体提取范围

| 组件 | 当前在 | 建议 |
|------|--------|------|
| `atomic_write` | `storage/common/io.rs` + 多处重复实现 | 提取到 `graphdb-storage-core` |
| `compress/decompress` | `storage/common/compression.rs` + `serialize/compression.rs` | 提取到 `graphdb-storage-core` |
| WAL 框架 | `cold_warm_cache/manager.rs`（内嵌实现） | 提取为通用 trait，让搜索和图存储各自实现 |
| Checksum 验证 | 不存在 | 在 `graphdb-storage-core` 中新增 |
| 错误类型 | 各层各自定义 | `IoError` / `CorruptionError` / `VersionMismatch` 统一 |

### 何时提取

建议在以下条件满足后提取：

1. `src/storage/` 和 `crates/inversearch/src/storage/` 都出现了第三处需要 `atomic_write` 或 `compress` 的场景（当前是两处）
2. 项目需要支持不同的存储后端（如 RocksDB、SQLite），此时通用 I/O 层才有实质意义
3. 目前**不急于提取**——两套存储独立运作良好，过早抽象反而增加理解成本

## 相关文件

- `crates/inversearch/src/storage/common/trait.rs` — StorageInterface trait
- `crates/inversearch/src/storage/file.rs` — FileStorage 实现
- `crates/inversearch/src/storage/common/io.rs` — 原子写 / 文件 I/O
- `crates/inversearch/src/storage/common/compression.rs` — 压缩包装
- `crates/inversearch/src/storage/cold_warm_cache/manager.rs` — 三級缓存 + WAL
- `crates/inversearch/src/keystore/mod.rs` — KeystoreMap 哈希桶实现
- `crates/inversearch/src/serialize/compression.rs` — 序列化层的压缩
- `crates/inversearch/src/storage/persistence.rs` — 备份/恢复/导出
- `src/storage/` — graphdb 自身的图存储
