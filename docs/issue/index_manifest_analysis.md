# IndexManifest 设计分析

## 1. 整体架构

IndexManifest 是原生索引（native index）的核心元数据结构，采用**不可变 generation 模式**：

```
┌─────────────────────────────────────────────────────┐
│  IndexManifest (不可变路由表)                         │
│  ┌─────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │space_id │  │index_id  │  │generation + epoch  │  │
│  │(u64)    │  │(u64)     │  │(各 u64 newtype)    │  │
│  └─────────┘  └──────────┘  └────────────────────┘  │
│  ┌──────────────────────────────────────────────┐    │
│  │shards: Vec<IndexShard>                       │    │
│  │  [(-∞, "m"), ("m", +∞)]  半开区间、连续     │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────┐
│  ManifestCatalog (生命周期管理)                       │
│  ┌──────────────────┐  ┌─────────────────────────┐  │
│  │active: RwLock    │  │retired: Mutex<Vec<...>> │  │
│  │  Arc<Manifest>   │  │  等待回收的旧 generation  │  │
│  └──────────────────┘  └─────────────────────────┘  │
│  ┌──────────────────┐  ┌─────────────────────────┐  │
│  │published: Atomic │  │reclaimed_files: Atomic  │  │
│  └──────────────────┘  └─────────────────────────┘  │
└─────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────┐
│  GenerationBuildState (崩溃安全重建状态机)            │
│  Building → CatchingUp → Publishing → Active        │
│  (snapshot_ts, start_lsn, barrier_lsn)              │
└─────────────────────────────────────────────────────┘
```

### 核心类型

| 类型 | 位置 | 作用 |
|------|------|------|
| `IndexManifest` | `storage/index/manifest.rs` | 单个不可变 generation 的路由表 |
| `IndexShard` | 同上 | 半开 key 范围 `[lower, upper)` + checkpoint 文件路径 |
| `ManifestCatalog` | 同上 | 管理 active/retired manifest，epoch 发布，文件回收 |
| `ManifestHandle` | 同上 | cursor 持有的 Arc pin，阻止回收 |
| `GenerationBuildState` | 同上 | 持久化重建阶段，崩溃后可恢复 |
| `GenerationState` | 同上 | 状态机枚举：Building/CatchingUp/Publishing/Active/Failed/Cancelled |
| `IndexIdentity` | `index_data_manager.rs` | `(space_id, index_id)` 复合标识 |
| `IndexGeneration` | `core/types/sync_protocol.rs` | u64 newtype，数据 generation |
| `ManifestEpoch` | 同上 | u64 newtype，发布 epoch |

## 2. 设计优点

| 方面 | 说明 |
|------|------|
| **不可变 manifest** | 发布后不可修改，Arc 共享无数据竞争，读路径无锁 |
| **epoch 单调递增** | `publish()` 校验 `manifest.epoch > active.epoch`，防止乱序发布 |
| **Handle 防回收** | `ManifestHandle(Arc<IndexManifest>)` 利用 `Arc::strong_count==1` 判断是否可回收，避免 use-after-free |
| **崩溃安全** | `GenerationBuildState` 持久化每个阶段到磁盘，崩溃后可从断点恢复或回滚 |
| **校验完备** | shard 连续、半开区间、首末 shard 无界、重复 shard_id 检测 |
| **原子发布** | `RwLock` + `mem::replace` 实现无锁读、原子写 |
| **写时复制** | 新 generation 构建时不影响 active generation 的读写 |

## 3. 潜在问题

### 3.1 retired manifest 无限累积

**严重度：中**

长查询持有 `ManifestHandle` 时，`retired` Vec 只增不减。无背压、无告警机制。

```rust
// ManifestCatalog::take_reclaimable_manifests
retired.retain(|entry| {
    if Arc::strong_count(&entry.manifest) == 1 {
        manifests.push((*entry.manifest).clone());
        false  // 移除
    } else {
        true   // 保留 — 只要有一个 handle 未 drop，就永远保留
    }
});
```

**影响**：如果 cursor 泄漏或查询时间极长，旧 generation 的 checkpoint 文件永远不会被删除，磁盘空间持续增长。

**建议**：当前阶段可接受。未来可加：
- 查询超时机制
- retired 数量告警
- 强制回收的 debug 接口

### 3.2 Arc::strong_count 脆弱性

**严重度：中**

任何未及时 drop 的 Arc clone 都会阻止回收，且难以排查。

```rust
// 任何地方的临时 clone 都会导致 strong_count > 1
let temp = Arc::clone(&manifest);  // 忘记 drop → 泄漏
```

**建议**：当前阶段可接受。未来可加 `strong_count` 调试日志。

### 3.3 Epoch/Generation 语义重叠

**严重度：低**

两者都是 `u64` newtype，实际使用中值常相同（测试里都传 `epoch`）。

```rust
// 测试中的典型用法
IndexManifest::new(
    1, 1,
    IndexGeneration::new(epoch),
    ManifestEpoch::new(epoch),  // 值相同
    shards,
)
```

**影响**：认知负担，但不影响正确性。

**建议**：如果未来不需要区分，可合并为单一 `u64` 字段。

### 3.4 无 checksum

**严重度：低**

manifest 引用 checkpoint 文件但不存校验和，依赖文件系统完整性。

**建议**：单节点 DB 可接受。分布式场景需要加 checksum。

### 3.5 JSON 序列化

**严重度：低**

使用 serde_json 序列化 manifest，人类可读但不够紧凑。

**建议**：单节点 DB 可接受。性能敏感场景可换二进制格式。

## 4. 文件结构

```
crates/graphdb-storage/src/storage/index/
├── manifest.rs              # IndexManifest + ManifestCatalog + GenerationBuildState
├── index_data_manager.rs    # IndexDataManagerImpl — 运行时管理
├── vertex_index_manager.rs  # VertexIndexCursor — tag 索引读
├── edge_index_manager.rs    # EdgeIndexCursor — edge 索引读
├── shard_runtime.rs         # IndexRuntime + GenerationRuntime — 内存中索引数据
├── generic_index_manager.rs # GenericIndexManager — BTreeMap 存储 + flush/load
├── key_codec.rs             # 键编码
└── manifest.rs              # 元数据持久化
```

## 5. 结论

设计**合理**，核心模式（不可变 manifest + epoch 发布 + Arc handle 回收）是数据库系统的成熟实践。对于单节点图数据库的定位，当前实现没有阻塞性问题。

唯一值得关注的是 **retired manifest 累积**——如果未来有长事务或流式查询，需要加查询超时或 handle 泄漏检测。当前阶段不需要改。
