# IndexManifest 问题修改方案

基于 `index_manifest_analysis.md` 中识别的 5 个问题，以下逐一给出具体修改方案。

---

## 3.1 retired manifest 无限累积

### 现状
`ManifestCatalog.retired: Mutex<Vec<RetiredManifest>>` 只增不减，长查询或 cursor 泄漏时磁盘不释放。

### 方案

**核心思路：age-based 强制回收 + 可观测性**

#### 修改 1：`RetiredManifest` 增加时间戳

```rust
// manifest.rs
use std::time::{Duration, Instant};

#[derive(Debug)]
struct RetiredManifest {
    manifest: Arc<IndexManifest>,
    retired_at: Instant,
}
```

#### 修改 2：`ManifestCatalog` 增加配置与告警

```rust
pub struct ManifestCatalog {
    active: RwLock<Arc<IndexManifest>>,
    retired: Mutex<Vec<RetiredManifest>>,
    published: AtomicU64,
    reclaimed_files: AtomicU64,
    max_retired: usize,                      // 新增：背压阈值
    max_hold_duration: Duration,              // 新增：强制回收超时
}
```

#### 修改 3：`new()` 增加参数

```rust
pub fn new_with_options(
    manifest: IndexManifest,
    max_retired: usize,
    max_hold_duration: Duration,
) -> Result<Self, String> {
    manifest.validate()?;
    Ok(Self {
        active: RwLock::new(Arc::new(manifest)),
        retired: Mutex::new(Vec::new()),
        published: AtomicU64::new(0),
        reclaimed_files: AtomicU64::new(0),
        max_retired,
        max_hold_duration,
    })
}
```

#### 修改 4：`publish()` 增加背压检查

```rust
pub fn publish(&self, manifest: IndexManifest) -> Result<ManifestHandle, String> {
    // ... existing validation ...

    let mut retired = self.retired.lock();
    if retired.len() >= self.max_retired {
        // 检查是否有超时的可强制回收
        let now = Instant::now();
        let force_reclaim: Vec<_> = retired.iter()
            .filter(|e| now.duration_since(e.retired_at) > self.max_hold_duration)
            .map(|e| (*e.manifest).clone())
            .collect();

        if !force_reclaim.is_empty() {
            log::warn!(
                "Force-reclaiming {} retired manifests exceeded {:?} hold limit",
                force_reclaim.len(),
                self.max_hold_duration
            );
            retired.retain(|e| now.duration_since(e.retired_at) <= self.max_hold_duration);
            self.reclaimed_files.fetch_add(
                force_reclaim.iter().map(|m| m.shards.len() as u64).sum(),
                Ordering::Relaxed,
            );
        }
    }

    // ... existing publish logic ...
}
```

#### 修改 5：`stats()` 增加可观测字段

```rust
pub struct ManifestCatalogStats {
    pub active_epoch: ManifestEpoch,
    pub active_generation: IndexGeneration,
    pub active_readers: u64,
    pub retired_generations: u64,
    pub published_manifests: u64,
    pub reclaimed_files: u64,
    pub oldest_retired_age_secs: u64,  // 新增：最老 retired 的驻留时间
}
```

#### 修改 6：`ManifestHandle` 增加 lease 语义（可选）

cursor 在创建 handle 时可绑定一个最大 lease 时长。catalog 在 force-reclaim 时跳过有 active lease 的 manifest，仅对过期 lease 强制回收。

```rust
pub struct ManifestHandle {
    inner: Arc<IndexManifest>,
    lease_deadline: Option<Instant>,
}
```

#### 影响范围

| 文件 | 改动 |
|------|------|
| `crates/graphdb-storage/src/storage/index/manifest.rs` | 核心改造 |
| `crates/graphdb-storage/src/storage/index/index_data_manager.rs` | `register_native_index` 传入 `max_retired` 参数 |

#### 测试

```rust
#[test]
fn force_reclaim_expired_retired_manifests() {
    let catalog = ManifestCatalog::new_with_options(
        manifest(1, vec![shard(0, None, None)]),
        2,
        Duration::from_millis(50),
    ).expect("catalog should be valid");
    let old_reader = catalog.acquire();
    catalog.publish(manifest(2, vec![shard(0, None, None)])).unwrap();
    
    std::thread::sleep(Duration::from_millis(100));
    
    // 即使 handle 未 drop，超时也强制回收
    assert!(!catalog.take_reclaimable_files().is_empty());
}
```

---

## 3.2 Arc::strong_count 脆弱性

### 现状
任何临时 `Arc::clone` 忘记 drop 就会阻止回收，且无法定位泄漏点。

### 方案

**核心思路：仅在 debug/assert build 中启用强引用追踪**

#### 修改 1：`InstrumentedArc` 包装器

```rust
// manifest.rs 或新建的 arc_instrument.rs
use std::sync::atomic::{AtomicUsize, Ordering};

static ARC_CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);
static ARC_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct InstrumentedArc<T> {
    inner: Arc<T>,
    id: usize,  // 唯一标识，关联 clone/drop 事件
}

impl<T> InstrumentedArc<T> {
    fn new(value: T) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        Self {
            inner: Arc::new(value),
            id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
        }
    }

    fn clone_ref(&self) -> Self {
        #[cfg(debug_assertions)]
        {
            ARC_CLONE_COUNT.fetch_add(1, Ordering::Relaxed);
            log::trace!(
                "Arc clone: id={}, strong_count_after={}, type={}",
                self.id,
                Arc::strong_count(&self.inner) + 1,
                std::any::type_name::<T>(),
            );
        }
        Self {
            inner: Arc::clone(&self.inner),
            id: self.id,
        }
    }
}
```

#### 修改 2：`ManifestCatalog` 中替换部分 `Arc` 操作

不需要替换所有 `Arc::clone`——仅在 `publish()` 和 `acquire()` 路径改用 `InstrumentedArc`，加上 `#[cfg(debug_assertions)]` 条件编译。

#### 修改 3：定期健康检查线程（可选）

```rust
// 在 IndexDataManagerImpl 中新增方法
pub fn diagnostics_check(&self) -> Vec<LeakReport> {
    let catalogs = self.manifest_catalogs.read();
    let mut reports = Vec::new();
    for (identity, catalog) in catalogs.iter() {
        let stats = catalog.stats();
        if stats.retired_generations > 10 {
            reports.push(LeakReport {
                identity: *identity,
                retired_count: stats.retired_generations,
                oldest_age_secs: stats.oldest_retired_age_secs,
            });
        }
    }
    reports
}
```

#### 影响范围

| 文件 | 改动 |
|------|------|
| `crates/graphdb-storage/src/storage/index/manifest.rs` | `acquire()` / `publish()` 使用 `InstrumentedArc` |

#### 性能考量

`#[cfg(debug_assertions)]` 确保 release build 零开销。线程安全的计数器仅统计 clone/drop 次数，不做回溯，开销可控。

---

## 3.3 Epoch/Generation 语义重叠

### 现状
`IndexGeneration` 和 `ManifestEpoch` 都是 `u64` newtype，值常相同。

### 方案

**核心思路：合并字段，保留单一语义**

#### 修改 1：`IndexManifest` 结构体

```rust
// 删除 epoch 字段，仅保留 generation
pub struct IndexManifest {
    pub format_version: u16,
    pub space_id: u64,
    pub index_id: u64,
    pub generation: IndexGeneration,
    // pub epoch: ManifestEpoch,  // 删除
    pub shards: Vec<IndexShard>,
}
```

#### 修改 2：校验逻辑

```rust
// publish() 中只检查 generation，去掉 epoch 检查
if manifest.generation <= active.generation {
    return Err("Index generation cannot move backwards".to_string());
}
```

#### 修改 3：保留 `ManifestEpoch` type alias 以兼容外部代码

```rust
// sync_protocol.rs 中新增 type alias 供外部引用
pub type ManifestEpoch = IndexGeneration;
```

或者直接删除 `ManifestEpoch`，全局替换为 `IndexGeneration`。

#### 影响范围

| 文件 | 改动 |
|------|------|
| `crates/graphdb-storage/src/storage/index/manifest.rs` | `IndexManifest` 字段、`validate()`、`new()` |
| `crates/graphdb-storage/src/storage/index/index_data_manager.rs` | 构造 manifest 的调用点 |
| `crates/graphdb-core/src/core/types/sync_protocol.rs` | 删除或 type alias |
| `crates/graphdb-storage/src/storage/index/manifest.rs` tests | 所有构造 manifest 的测试 |

#### 风险评估

- `IndexManifest` 是 serde 序列化的，`epoch` 字段删除会导致与旧 JSON 不兼容
- 需要 bump `MANIFEST_FORMAT_VERSION` 并添加 migrations 或在 `validate()` 中兼容旧版本

#### 建议

由于项目处于开发阶段、无 backward compatibility 约束，可以直接删除 `epoch` 字段、版本号 +1。

---

## 3.4 无 checksum

### 现状
`IndexShard` 记录 `checkpoint_file` 路径，不存校验和。

### 方案

**核心思路：在 manifest 中存储文件级 checksum，load 时校验**

#### 修改 1：`IndexShard` 增加 checksum 字段

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexShard {
    pub shard_id: u32,
    pub lower: Option<Vec<u8>>,
    pub upper: Option<Vec<u8>>,
    pub checkpoint_file: PathBuf,
    pub checksum: Option<String>,  // 新增：SHA-256 hex，None 表示无校验
}
```

#### 修改 2：`store()` 流程中计算 checksum

```rust
pub fn store(&self, path: &Path) -> StorageResult<()> {
    self.validate().map_err(StorageError::db_error)?;
    // ... 写入目录 ...

    // 序列化前为每个 shard 填充 checksum
    let mut manifest_to_write = self.clone();
    for shard in &mut manifest_to_write.shards {
        if let Some(computed) = shard.compute_checksum()? {
            shard.checksum = Some(computed);
        }
    }

    let bytes = serde_json::to_vec(&manifest_to_write)...;
    // ... atomic write ...
}
```

#### 修改 3：`load()` 流程中校验

```rust
pub fn load(path: &Path) -> StorageResult<Self> {
    let bytes = std::fs::read(path)?;
    let manifest: Self = serde_json::from_slice(&bytes)?;
    manifest.validate().map_err(StorageError::db_error)?;

    // 校验每个 shard 的 checkpoint 文件
    for shard in &manifest.shards {
        if let Some(ref expected) = shard.checksum {
            let actual = compute_file_checksum(&shard.checkpoint_file)?;
            if actual != *expected {
                return Err(StorageError::db_error(format!(
                    "Checksum mismatch for shard {}: expected {}, got {}",
                    shard.shard_id, expected, actual
                )));
            }
        }
    }
    Ok(manifest)
}
```

#### 修改 4：checksum 工具函数

```rust
fn compute_file_checksum(path: &Path) -> StorageResult<String> {
    use sha2::{Sha256, Digest};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

impl IndexShard {
    pub fn compute_checksum(&self) -> StorageResult<Option<String>> {
        if self.checkpoint_file.as_os_str().is_empty() {
            return Ok(None);
        }
        Ok(Some(compute_file_checksum(&self.checkpoint_file)?))
    }
}
```

#### 影响范围

| 文件 | 改动 |
|------|------|
| `crates/graphdb-storage/src/storage/index/manifest.rs` | `IndexShard`、`store()`、`load()` |
| `crates/graphdb-storage/Cargo.toml` | 新增 `sha2` 依赖 |
| `MANIFEST_FORMAT_VERSION` | bump 到 3 |

#### 性能影响

`store()` 时为每个 shard 多一次顺序读（计算 SHA-256），对 checkpoint 文件通常大小（MB~GB 级）可接受。`load()` 的校验读在 debug build 启用，release 可 gate 掉或仅在首次加载时启用。

---

## 3.5 JSON 序列化

### 现状
使用 `serde_json` 序列化 manifest，人类可读但不紧凑。

### 方案

**核心思路：切换到 `postcard`（`serde` + `COBS` 定长帧）保持 serde 兼容、减小体积**

#### 修改 1：`store()` / `load()` 切换

```rust
pub fn store(&self, path: &Path) -> StorageResult<()> {
    self.validate().map_err(StorageError::db_error)?;
    // ... 临时目录创建 ...

    let bytes = postcard::to_stdvec(self).map_err(|error| {
        StorageError::db_error(format!("Serialize index manifest: {error}"))
    })?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn load(path: &Path) -> StorageResult<Self> {
    let bytes = std::fs::read(path)?;
    let manifest: Self = postcard::from_bytes(&bytes)
        .map_err(|error| StorageError::db_error(format!("Read index manifest: {error}")))?;
    manifest.validate().map_err(StorageError::db_error)?;
    Ok(manifest)
}
```

#### 修改 2：`GenerationBuildState` 同步切换

```rust
// index_data_manager.rs 中 save_build_state / load_build_state 同步切换
fn save_build_state(index_root: &Path, state: &GenerationBuildState) -> StorageResult<()> {
    // ...
    let bytes = postcard::to_stdvec(state)...;
    // ...
}
```

#### 修改 3：保留调试可读性——新增 `manifest_dump` CLI 子命令

```rust
// graphdb-cli 中新增
fn dump_manifest(path: &Path) -> Result<()> {
    let manifest = IndexManifest::load(path)?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}
```

#### 影响范围

| 文件 | 改动 |
|------|------|
| `crates/graphdb-storage/src/storage/index/manifest.rs` | `store()`、`load()` |
| `crates/graphdb-storage/src/storage/index/index_data_manager.rs` | `save_build_state()`、`load_build_state()` |
| `crates/graphdb-storage/Cargo.toml` | 新增 `postcard` 依赖 |
| `MANIFEST_FORMAT_VERSION` | bump 到 3 |

#### 风险评估

- 二进制格式下无法直接 `cat` 查看 manifest（CLI 工具补足）
- `postcard` 默认要求固定大小——`Vec<u8>` 字段在 `postcard` 中支持，但需确认 `PathBuf` 序列化行为。备选方案：`bincode`。

#### 建议

该改动优先级最低。在 manifest 文件较小（通常 <10KB）的场景下，JSON 完全够用。仅在 manifest 元数据膨胀或频繁 load 时切换。

---

## 实施优先级

| 优先级 | 问题 | 理由 |
|--------|------|------|
| P0 | 3.1 retired 累积 | 唯一可能在正常运行中触发磁盘耗尽的问题 |
| P1 | 3.3 epoch 合并 | 开发期无兼容负担，未来改动成本递增 |
| P1 | 3.4 checksum | 改动小、能防止静默数据损坏 |
| P2 | 3.2 Arc 追踪 | 调试基础设施，非紧急 |
| P3 | 3.5 序列化 | 当前阶段 JSON 足够 |
