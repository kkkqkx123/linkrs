# MVCC Garbage Collection 研究报告

## 1. 概述

本文档研究了主流数据库系统中 MVCC (Multi-Version Concurrency Control) 的垃圾回收实现方式，并基于研究结果提出 GraphDB 索引模块的优化方案。

## 2. 主流数据库 MVCC 实现分析

### 2.1 PostgreSQL

#### 核心机制

PostgreSQL 使用 MVCC 来实现事务隔离，每个 UPDATE 或 DELETE 操作不会立即删除旧版本数据，而是创建新版本或标记删除。

```
┌─────────────────────────────────────────────────────────────┐
│                    PostgreSQL MVCC 架构                      │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────┐    ┌─────────┐    ┌─────────┐                  │
│  │ Tuple 1 │───▶│ Tuple 2 │───▶│ Tuple 3 │  (同一行的版本链) │
│  │ xmin=10 │    │ xmin=20 │    │ xmin=30 │                  │
│  │ xmax=20 │    │ xmax=30 │    │ xmax=∞  │                  │
│  └─────────┘    └─────────┘    └─────────┘                  │
│                                                              │
│  VACUUM 进程负责清理对任何事务都不可见的旧版本                  │
└─────────────────────────────────────────────────────────────┘
```

#### VACUUM 机制

```sql
-- 手动触发
VACUUM (VERBOSE, ANALYZE) my_table;

-- 自动触发 (autovacuum)
-- 根据表的活动级别自动调整
```

**关键特性**：

| 特性 | 描述 |
|------|------|
| **非阻塞** | 普通 VACUUM 不获取排他锁，可与读写操作并行 |
| **增量处理** | 分批处理，避免长时间阻塞 |
| **空间复用** | 清理的空间保留在表中供后续使用 |
| **自动调度** | autovacuum 根据活动级别自动触发 |
| **并行支持** | 支持并行 VACUUM 利用多核 CPU |

**VACUUM 选项**：

```sql
-- 完全清理（需要排他锁，会阻塞）
VACUUM FULL my_table;

-- 并行清理
VACUUM (PARALLEL 4) my_table;

-- 跳过无法立即锁定的表
VACUUM (SKIP_LOCKED) my_table;

-- 控制索引清理
VACUUM (INDEX_CLEANUP AUTO) my_table;
```

### 2.2 TiKV

#### 核心机制

TiKV 使用基于时间戳的 MVCC，每个 key 可以有多个版本，通过时间戳区分。

```
┌─────────────────────────────────────────────────────────────┐
│                    TiKV MVCC 架构                            │
├─────────────────────────────────────────────────────────────┤
│  Key 编码: key + timestamp (降序)                            │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ user_key: "user_123"                                  │   │
│  ├──────────────────────────────────────────────────────┤   │
│  │ versions:                                             │   │
│  │   ts=100 → value="Alice", type=PUT                    │   │
│  │   ts=50  → value="Bob", type=PUT                      │   │
│  │   ts=30  → type=DELETE (tombstone)                    │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│  API V2 支持:                                                │
│  - TTL (Time To Live)                                       │
│  - Tombstone 标记                                           │
│  - 多版本共存                                                │
└─────────────────────────────────────────────────────────────┘
```

**数据结构**：

```rust
pub struct RawValue {
    pub user_value: Vec<u8>,
    pub expire_ts: Option<u64>,    // TTL 过期时间
    pub is_delete: bool,           // Tombstone 标记
}

// API V2 编码格式
// Value = [user_value][expire_ts (optional)][meta_flags]
```

**GC 机制**：

```bash
# tikv-ctl 命令行工具
tikv-ctl --data-dir /path/to/tikv/data compact -r 123 --cf default,write --bottommost force

# MVCC 数据检查
tikv-ctl --data-dir /path/to/tikv/data mvcc -k 'zuser_123'
```

**GC 策略**：

| 策略 | 描述 |
|------|------|
| **Compaction 触发** | 在 RocksDB compaction 过程中清理旧版本 |
| **TTL 过期清理** | 根据 expire_ts 清理过期数据 |
| **Tombstone 清理** | 在 compaction 时清理已删除的标记 |
| **安全时间戳** | 只清理早于 min_snapshot_ts 的版本 |

### 2.3 FoundationDB

#### 核心机制

FoundationDB 使用全局版本号进行 MVCC 管理，所有数据都带有版本信息。

```
┌─────────────────────────────────────────────────────────────┐
│                  FoundationDB MVCC 架构                      │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  StorageServer 结构:                                         │
│  ┌────────────────────────────────────────────────────┐     │
│  │ VersionedData versionedData;  // 版本化数据         │     │
│  │ map<Version, MutationLog> mutationLog;  // 变更日志 │     │
│  │                                                    │     │
│  │ Version version;           // 当前版本              │     │
│  │ Version durableVersion;    // 已持久化版本          │     │
│  │ Version oldestVersion;     // 最小可读版本          │     │
│  │ Version desiredOldestVersion; // GC 目标版本        │     │
│  └────────────────────────────────────────────────────┘     │
│                                                              │
│  版本管理:                                                   │
│  oldestVersion ≤ durableVersion ≤ version                   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**版本管理**：

```cpp
struct StorageServer {
    // MVCC data
    VersionedData versionedData;
    std::map<Version, Standalone<VerUpdateRef>> mutationLog;
    
    // Version tracking
    NotifiedVersion version;              // 当前最新版本
    NotifiedVersion durableVersion;       // 已持久化版本
    NotifiedVersion oldestVersion;        // 最小可读版本 (GC 边界)
    NotifiedVersion desiredOldestVersion; // 目标 GC 版本
};
```

**GC 策略**：

| 机制 | 描述 |
|------|------|
| **版本窗口** | oldestVersion 到 version 之间的版本需要保留 |
| **异步清理** | 后台进程异步清理旧版本 |
| **速率控制** | RateKeeper 控制 GC 速率，避免影响正常操作 |

### 2.4 RocksDB

#### 核心机制

RocksDB 作为存储引擎，提供了多种 MVCC 支持：

```
┌─────────────────────────────────────────────────────────────┐
│                    RocksDB MVCC 支持                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  1. Snapshot 机制:                                           │
│     ┌─────────────────────────────────────────────┐         │
│     │ Snapshot 1 (seq=100) → 看到 seq ≤ 100 的数据 │         │
│     │ Snapshot 2 (seq=200) → 看到 seq ≤ 200 的数据 │         │
│     └─────────────────────────────────────────────┘         │
│                                                              │
│  2. BlobDB GC:                                               │
│     - enable_garbage_collection = true                      │
│     - 在 compaction 时清理无效 blob                          │
│     - garbage_collection_cutoff 控制清理范围                 │
│                                                              │
│  3. Timestamp API:                                           │
│     - 基于时间戳的版本管理                                    │
│     - full_history_ts_low 控制历史版本保留                   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Snapshot 释放触发 Compaction**：

```c++
// 当 snapshot 释放时，自动触发 compaction 清理过期数据
// 不再需要多个 tombstone 才触发
```

**BlobDB GC**：

```
BlobDBOptions:
  enable_garbage_collection: true
  garbage_collection_cutoff: 0.5  // 清理最老的 50% 文件

GC 过程:
  1. 识别最老的 N 个 blob 文件
  2. 重定位有效的 blob 到新文件
  3. 删除不再被引用的旧文件
```

## 3. 关键设计模式总结

### 3.1 GC 触发时机

| 数据库 | 触发方式 | 特点 |
|--------|----------|------|
| PostgreSQL | 自动 (autovacuum) + 手动 | 根据活动级别自适应 |
| TiKV | Compaction 触发 | 与存储引擎集成 |
| FoundationDB | 后台异步 | 速率受控 |
| RocksDB | Snapshot 释放 + Compaction | 事件驱动 |

### 3.2 并发控制

| 数据库 | 方式 | 优点 |
|--------|------|------|
| PostgreSQL | 非阻塞 VACUUM | 不影响读写 |
| TiKV | 分 Column Family | 读写分离 |
| FoundationDB | 版本窗口 | 确定性 |
| RocksDB | Snapshot + Compaction | 无锁读取 |

### 3.3 空间管理

| 数据库 | 策略 |
|--------|------|
| PostgreSQL | 空间复用，不归还 OS (除非 VACUUM FULL) |
| TiKV | Compaction 时归还空间 |
| FoundationDB | 版本化存储，定期清理 |
| RocksDB | Compaction 时清理，支持 TTL |

## 4. GraphDB 当前实现分析

### 4.1 当前设计

```rust
pub struct IndexEntry {
    pub created_ts: Timestamp,
    pub deleted_ts: Option<Timestamp>,
}

pub fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<usize, StorageError> {
    // 获取写锁，阻塞所有操作
    let mut forward_index = self.forward_index.write();
    
    // 收集所有需要删除的 key
    let keys_to_remove: Vec<IndexKey> = forward_index
        .iter()
        .filter(|(_, entry)| {
            entry.deleted_ts.map_or(false, |deleted_ts| deleted_ts < safe_ts)
        })
        .map(|(key, _)| key.clone())
        .collect();
    
    // 删除
    for key in &keys_to_remove {
        forward_index.remove(key);
    }
    
    Ok(keys_to_remove.len())
}
```

### 4.2 问题分析

| 问题 | 影响 | 严重程度 |
|------|------|----------|
| 全局写锁 | GC 期间阻塞所有索引操作 | 高 |
| 一次性清理 | 大量 tombstone 时耗时过长 | 高 |
| 内存压力 | 所有数据在内存中 | 中 |
| 无增量支持 | 无法分批处理 | 中 |
| 无自动触发 | 需要外部调用 | 中 |

## 5. 优化方案

### 5.1 方案概述

基于研究，提出以下优化方案：

```
┌─────────────────────────────────────────────────────────────┐
│                    优化方案架构                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              IndexGcManager (新增)                   │    │
│  │  - 后台 GC 任务调度                                  │    │
│  │  - 增量 GC 执行                                      │    │
│  │  - 与 VersionManager 集成                            │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                   │
│                          ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │         ShardedIndexManager (优化)                   │    │
│  │  - 分片减少锁竞争                                    │    │
│  │  - 支持并行 GC                                       │    │
│  │  - 细粒度锁控制                                      │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                   │
│                          ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │         Incremental GC (优化)                        │    │
│  │  - 分批处理                                          │    │
│  │  - 可中断/可恢复                                     │    │
│  │  - 速率限制                                          │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 具体实现

#### 5.2.1 分片索引管理器

```rust
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

pub struct ShardedVertexIndexManager {
    shards: [RwLock<BTreeMap<IndexKey, IndexEntry>>; SHARD_COUNT],
    version_manager: Arc<VersionManager>,
}

impl ShardedVertexIndexManager {
    fn shard_for_key(&self, key: &IndexKey) -> usize {
        let hash = fxhash::hash(key);
        hash & SHARD_MASK
    }

    pub fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError> {
        let mut total_removed = 0usize;
        
        // 可以并行处理各个分片
        for shard in &self.shards {
            let mut shard = shard.write();
            let before = shard.len();
            shard.retain(|_, entry| {
                entry.deleted_ts.map_or(true, |ts| ts >= safe_ts)
            });
            total_removed += before - shard.len();
        }
        
        Ok(GcStats { entries_removed: total_removed })
    }

    pub fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError> {
        let mut total_removed = 0usize;
        let mut processed = 0usize;
        
        for shard in &self.shards {
            if processed >= batch_size {
                break;
            }
            
            let mut shard = shard.write();
            let mut keys_to_remove = Vec::new();
            
            for (key, entry) in shard.iter() {
                if processed >= batch_size {
                    break;
                }
                if entry.deleted_ts.map_or(false, |ts| ts < safe_ts) {
                    keys_to_remove.push(key.clone());
                    processed += 1;
                }
            }
            
            for key in &keys_to_remove {
                shard.remove(key);
            }
            total_removed += keys_to_remove.len();
        }
        
        Ok(GcStats { entries_removed: total_removed })
    }
}
```

#### 5.2.2 GC 任务管理器

```rust
pub struct IndexGcConfig {
    pub batch_size: usize,
    pub interval_ms: u64,
    pub min_interval_between_gc_ms: u64,
}

impl Default for IndexGcConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            interval_ms: 1000,
            min_interval_between_gc_ms: 100,
        }
    }
}

pub struct IndexGcManager {
    index_manager: InMemoryIndexDataManager,
    version_manager: Arc<VersionManager>,
    config: IndexGcConfig,
    last_gc_ts: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
}

impl IndexGcManager {
    pub fn new(
        index_manager: InMemoryIndexDataManager,
        version_manager: Arc<VersionManager>,
        config: IndexGcConfig,
    ) -> Self {
        Self {
            index_manager,
            version_manager,
            config,
            last_gc_ts: Arc::new(AtomicU32::new(0)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run_gc_pass(&self) -> Result<GcStats, StorageError> {
        let safe_ts = self.version_manager.get_safe_gc_timestamp();
        
        let stats = self.index_manager.gc_tombstones_incremental(
            safe_ts,
            self.config.batch_size,
        )?;
        
        self.last_gc_ts.store(safe_ts, Ordering::Release);
        Ok(stats)
    }

    pub fn start_background_gc(&self) -> JoinHandle<()> {
        let running = self.running.clone();
        let config = self.config.clone();
        let manager = self.clone();
        
        running.store(true, Ordering::Release);
        
        std::thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                if let Ok(stats) = manager.run_gc_pass() {
                    if stats.total_removed() > 0 {
                        tracing::debug!(
                            entries_removed = stats.total_removed(),
                            "GC pass completed"
                        );
                    }
                }
                std::thread::sleep(Duration::from_millis(config.interval_ms));
            }
        })
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }
}
```

#### 5.2.3 VersionManager 扩展

```rust
impl VersionManager {
    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        let min_read_ts = self.min_active_read_ts.load(Ordering::Acquire);
        let min_txn_ts = self.min_active_txn_ts.load(Ordering::Acquire);
        
        min_read_ts.min(min_txn_ts)
    }

    pub fn register_gc_callback(&self, callback: Box<dyn Fn(Timestamp) + Send + Sync>) {
        let mut callbacks = self.gc_callbacks.write();
        callbacks.push(callback);
    }
}
```

### 5.3 性能对比

| 指标 | 当前实现 | 优化后 |
|------|----------|--------|
| GC 阻塞时间 | 全局阻塞 | 分片级别阻塞 |
| 并发支持 | 无 | 分片并行 |
| 内存效率 | 一次性处理 | 增量处理 |
| 自动化 | 手动触发 | 后台自动 |
| 可扩展性 | 差 | 好 |

## 6. 实施计划

### 6.1 阶段一：基础优化 (P0)

- [x] 实现 `gc_tombstones` 基础方法
- [ ] 添加 `gc_tombstones_incremental` 增量方法
- [ ] 添加 `GcStats` 统计结构

### 6.2 阶段二：分片优化 (P1)

- [ ] 实现 `ShardedVertexIndexManager`
- [ ] 实现 `ShardedEdgeIndexManager`
- [ ] 更新 `InMemoryIndexDataManager` 使用分片管理器

### 6.3 阶段三：自动化 (P1)

- [ ] 实现 `IndexGcManager`
- [ ] 集成 `VersionManager`
- [ ] 添加后台 GC 线程

### 6.4 阶段四：监控与调优 (P2)

- [ ] 添加 GC 统计指标
- [ ] 添加配置选项
- [ ] 性能测试与调优

## 7. 参考

- PostgreSQL Documentation: https://www.postgresql.org/docs/current/mvcc.html
- TiKV Documentation: https://tikv.org/docs/
- FoundationDB Documentation: https://apple.github.io/foundationdb/
- RocksDB Documentation: https://rocksdb.org/
