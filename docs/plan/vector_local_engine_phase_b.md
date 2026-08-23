# 内置向量引擎 Phase B 设计评审与实施方案（Tier 1 IVFFlat）

> 状态：实施方案（2026-08-23）。
>
> 前置：`docs/plan/vector_local_engine_plan.md`（总方案）；Phase A 已合入
> （`crates/vector-search`：mmap 存储、tombstone 压缩、WAL 回放、SIMD 距离核、
> 过滤后处理、`benches/vector_scan_bench.rs` 基线）。
>
> 本文回答两个问题：Phase B 的四项设计是否合理（合理，但有 8 处缺口需先定案，
> 见 §1）；Phase B 的完整代码修改方案（见 §2）。

---

## 1. Phase B 设计合理性分析

原 Phase B 清单：

- 采样 k-means 聚类、list 分配、probe 搜索；
- 漂移监测与重建调度（10% 规则）；
- 压缩与重建的并发安全（读写锁切换）；
- bench：Tier 0 vs Tier 1（延迟/recall/构建时间），据此决定默认层级。

### 1.1 合理的部分

| 设计点 | 评价 |
|--------|------|
| IVFFlat 作为 Tier 1 | **正确**。与 Phase A 存储模型天然契合：向量已是稠密 slot 行主序，聚类只需存质心 + slot→list 归属；删除复用 tombstone 位图（搜索期跳过即可，无需改 list 结构）；质心距离复用现有 SIMD 距离核，分数语义与 Tier 0 完全一致 |
| 漂移重建（10% 规则） | **方向正确**，pgvector 有同款教训；但"漂移"本身需要量化定义（见 §1.2-G6） |
| bench 决定默认层级 | **正确**，避免拍脑袋选默认值；且 `SearchQuery.nprobe` 字段已在 Phase A 类型中预留（types.rs:773） |
| 读写锁切换保证压缩/重建并发安全 | **必要且可行**。Phase A 已用 `ArcSwap` 发布不可变快照（vectors/tombstones 同款模式），索引发布可完全复用该模式 |

### 1.2 设计缺口（实现前必须定案）

以下问题原方案未回答，本方案 §2 给出对应决策：

| # | 缺口 | 风险 | 本方案决策（详见 §2） |
|---|------|------|----------------------|
| G1 | **Tier 升级触发条件未定义**。"数据量增长后"不是可实现的条件：谁在何时判定、依据什么、自动还是手动 | 无法实现调度；或每次提交都检查造成开销 | 显式 promotion 规则：`live_count ≥ min_build_points` 且配置允许时，由维护线程异步建索引；提交路径只做一次计数器自增 |
| G2 | **IVF 状态的持久化与崩溃恢复未定义**。总方案 §3.2 只说质心进 `meta.bin`，但 slot→list 归属、WAL 回放后新增点如何处理均未说明 | 崩溃后索引状态不一致 | 新增独立 `index.bin`（派生结构，可随时丢弃重建）；打开时校验失败即丢弃并回退 Tier 0；WAL 回放产生的点走 pending 集合 |
| G3 | **构建期间的新插入会漏检**。后台建索引期间到达的 upsert 不在任何 list 里，probe 搜索将漏掉它们 | 静默丢结果（最严重的一类 bug） | 引入 `pending` 无归属集合：搜索时除 probe 命中的 list 外，恒定线性扫描 pending；发布时合并 |
| G4 | **逐插入更新索引结构的代价被低估**。若整个索引用 `ArcSwap` COW，每次插入需克隆全部 list（10 万点约 400KB memcpy/次） | 写放大严重 | list 粒度加锁：每个 list 一把 `parking_lot::RwLock<Vec<u32>>`（插入写锁一个 list，搜索读锁 nprobe 个 list），质心不可变无需加锁 |
| G5 | **压缩使 slot 全部重编号，索引必然失效**。Phase A 的 compact 会把存活 slot 压到 `0..live_count`（storage.rs:458），旧 slot→list 全部作废；且压缩与重建并发进行会产生撕裂的 slot 视图 | 数据错乱或索引指向错误 slot | 压缩完成后立即清除已发布索引（回退 Tier 0，零成本）并入队重建任务；压缩与重建用同一把 `maintenance` 互斥锁串行化（锁序：maintenance → inner.write，杜绝死锁） |
| G6 | **"10% 漂移"没有量化定义**。是质心移动距离？点换簇比例？recall 采样？ | 无法实现、无法测试 | 定义 `drift_ratio = 采样点中「当前最近质心 ≠ 所属质心」的比例`；每 collection 累计 `drift_check_interval` 次 upsert 后由维护线程采样计算（最多 `sample_limit` 个点），超阈值触发重建 |
| G7 | **probe + 过滤的结果不足语义未定义**。nprobe 个 list 扫完 + payload 后过滤可能凑不满 limit | 结果不稳定、用户困惑 | 第一版明确为近似语义并文档化；提供一次受控补救：过滤后不足 limit 且仍有未探测 list 时 nprobe 翻倍重试一次；`nprobe = lists` 即退化为精确 |
| G8 | **k-means 细节缺失**：训练集大小、迭代次数、空簇、随机性 | 构建时间不可控；测试不可复现 | 步长采样上限 `sample_limit`（默认 65_536）、`max_iter ≤ 10`、空簇以距其质心最远点重播、固定种子（collection 名哈希）保证可复现 |

另：总方案估时 2~3 天偏紧（G3/G4/G5 的并发处理是主要工作量），修正为 **3~4 天**。

### 1.3 结论

Phase B 方向合理、与 Phase A 架构一致，可以按 §2 实施；但 G1~G8 必须先按本方案
定案，否则会在实现中途被迫返工（尤其 G3/G5 两处并发正确性问题）。

---

## 2. 完整代码修改方案

### 2.0 总体结构

```
crates/vector-search/src/
  index.rs            新增  IvfIndex：list 分配 / probe 搜索 / pending / 漂移
  index/
    mod.rs            新增  模块声明
    kmeans.rs         新增  采样 k-means 训练
    persist.rs        新增  index.bin 读写与校验
src/storage.rs        修改  接线：发布/失效索引、upsert 分配、search 分层
src/engine.rs         修改  维护线程：promotion / 漂移检测 / 重建调度
src/types.rs          修改  IvfConfig、CollectionConfig/CollectionInfo 扩展
crates/graphdb-config 修改  [vector.local] 增加 ivf 配置段
crates/graphdb-sync   修改  创建 collection 时透传 IvfConfig
benches/              修改  Tier0 vs Tier1 对比组
tests/                新增  ivf 集成测试
```

不引入任何新依赖（`std::sync::mpsc` + `parking_lot` + `rayon` 已够用）。

---

### 2.1 `src/types.rs` 扩展

```rust
/// Tier 1 IVFFlat configuration. All thresholds are evaluated per collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfConfig {
    /// Number of clusters; `None` = auto (`sqrt(live)` clamped to [1, 4096]).
    pub lists: Option<u32>,
    /// Minimum live points before an index is built.
    pub min_build_points: u64,
    /// Training sample cap and drift-check sample cap.
    pub sample_limit: usize,
    /// k-means iteration cap.
    pub kmeans_max_iter: u32,
    /// Drift ratio above which a rebuild is scheduled.
    pub drift_threshold: f64,
    /// Upserts accumulated before the next drift check.
    pub drift_check_interval: u64,
    /// Default nprobe when the query does not set one.
    pub default_nprobe: usize,
    /// Whether automatic promotion to Tier 1 is allowed.
    pub auto_promotion: bool,
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            lists: None,
            min_build_points: 100_000,
            sample_limit: 65_536,
            kmeans_max_iter: 10,
            drift_threshold: 0.10,
            drift_check_interval: 25_000,
            default_nprobe: 8,
            // Off until the Phase B bench justifies turning it on (§2.7).
            auto_promotion: false,
        }
    }
}
```

- `CollectionConfig` 增加 `pub ivf_config: Option<IvfConfig>` 及构造方法
  `with_ivf(mut self, cfg: IvfConfig)`（项目无向后兼容负担，直接加字段）。
- `CollectionInfo` 增加 `pub index: Option<IndexInfo>`：

```rust
pub struct IndexInfo {
    pub tier: u8,           // 0 = exact, 1 = ivfflat
    pub lists: u32,
    pub nprobe_default: usize,
    pub built_at_live_count: u64,
    pub last_drift_ratio: Option<f64>,
}
```

- `SearchQuery.nprobe` 已存在（types.rs:773），无需改动。

---

### 2.2 新增 `src/index/kmeans.rs`（对应 G8）

```rust
pub(crate) struct KmeansOptions {
    pub k: u32,
    pub dim: usize,
    pub max_iter: u32,
    pub seed: u64,
}

pub(crate) struct KmeansResult {
    pub centroids: Vec<Vec<f32>>,   // k x dim
}

pub(crate) fn choose_list_count(live: u64) -> u32 {
    ((live as f64).sqrt().round() as u32).clamp(1, 4096)
}

/// Deterministic sampled k-means over stride-sampled vectors.
pub(crate) fn train(
    metric: DistanceMetric,
    sample: &[&[f32]],
    opts: &KmeansOptions,
) -> Result<KmeansResult>
```

实现要点：

1. **确定性**：`StdRng::seed_from_u64(opts.seed)`，种子由
   `hash(collection name) ^ live_count` 派生；同数据集重训结果一致（可测试性）。
2. **初始化**：k-means++（在采样子集上，代价 O(k·n) 一次性，可接受）。
3. **迭代**：分配步 rayon 并行（每样本调 `crate::distance::distance`）；更新步累
   加求和求均值。质心平均移动量 `< 1e-3` 提前收敛退出。
4. **空簇处理**：以「距其所属质心最远的样本」重新播种该簇。
5. **采样由调用方完成**（storage.rs 提供 stride 采样的 slot 列表 + mmap 视图），
   本模块只吃 `&[&[f32]]`，保持纯函数便于单测。

单测（文件内 `#[cfg(test)]`）：3 个高斯团合成数据 → 训练 → 断言每个采样点到
最近质心的簇内距离远小于跨簇距离；断言两次训练质心逐元素一致（确定性）。

---

### 2.3 新增 `src/index/persist.rs`（对应 G2）

```rust
const INDEX_MAGIC: [u8; 4] = *b"VIVF";
const INDEX_VERSION: u16 = 1;

#[derive(Serialize, Deserialize)]
pub(crate) struct PersistedIvf {
    pub lists: u32,
    pub dim: usize,
    pub distance: DistanceMetric,
    pub built_at_live_count: u64,
    pub centroids: Vec<Vec<f32>>,
    /// slot -> list; entries for tombstoned slots are ignored on load.
    pub slot_list: Vec<u32>,
}

pub(crate) fn save(path: &Path, data: &PersistedIvf) -> Result<()>;   // temp + fsync + rename
pub(crate) fn load(path: &Path) -> Result<Option<PersistedIvf>>;      // None = absent/invalid
```

要点：

- 写入走 temp 文件 + rename（与 compaction.rs 的原子替换风格一致）。
- `load` 校验：magic/version、`slot_list.len() == meta.next_slot`、
  `centroids.len() == lists`、每个质心长度 == dim、metric 与 meta 一致。
  **任何不符一律返回 `Ok(None)` 并删除损坏的 index.bin**——索引是派生结构，
  宁可回退 Tier 0 也不阻塞打开。
- 不持久化质心到 `meta.bin`（修正总方案 §3.2 的说法），保持 Meta 结构稳定，
  避免 postcard 格式连带 bump FORMAT_VERSION。

---

### 2.4 新增 `src/index.rs` / `src/index/mod.rs`：IvfIndex（对应 G3/G4/G6）

```rust
pub(crate) struct IvfIndex {
    dim: usize,
    metric: DistanceMetric,
    config: IvfConfig,
    /// Immutable after construction; no lock needed.
    centroids: Vec<Vec<f32>>,
    /// Per-list mutable membership. Insert takes one write lock; search takes
    /// read locks on probed lists only (G4).
    lists: Vec<parking_lot::RwLock<Vec<u32>>>,
    /// slot -> list, grown under this lock; u32::MAX = unknown/pending.
    slot_list: parking_lot::RwLock<Vec<u32>>,
    /// Slots inserted while no index was published (first build window and
    /// WAL-replayed points). Scanned linearly by every probe search so they
    /// are never missed (G3). Drained into lists on publish.
    pending: parking_lot::RwLock<Vec<u32>>,
    pub built_at_live_count: u64,
    upserts_since_check: AtomicU64,
}

impl IvfIndex {
    /// Build from a snapshot of live slots (called off the store lock).
    pub(crate) fn build(
        config: &IvfConfig,
        dim: usize,
        metric: DistanceMetric,
        collection: &str,
        slots: &[u32],                    // live slot ids at snapshot time
        vectors: &[Arc<Mmap>],
        segment_slots: u32,
    ) -> Result<Self>;

    /// Assign one freshly inserted slot to its nearest centroid.
    pub(crate) fn assign_slot(&self, slot: u32, vector: &[f32]);

    /// Register a slot inserted while the index was not yet published.
    pub(crate) fn mark_pending(&self, slot: u32);

    /// Move pending slots into their nearest lists (publish path).
    pub(crate) fn drain_pending(&self);

    /// Probe search: top-nprobe centroid lists + pending, tombstone-aware.
    /// Returns candidate (score, slot) pairs; filtering/threshold/top-K stay
    /// in storage.rs so both tiers share identical semantics.
    pub(crate) fn probe_candidates(
        &self,
        query: &SearchQuery,
        nprobe: usize,
        tombstones: &BitVec,
        vectors: &[Arc<Mmap>],
        segment_slots: u32,
    ) -> Result<Vec<(f32, u32)>>;

    /// Fraction of sampled points whose nearest centroid differs from their
    /// assigned list (G6). Called from the maintenance worker only.
    pub(crate) fn drift_ratio(
        &self,
        sample_slots: &[u32],
        tombstones: &BitVec,
        vectors: &[Arc<Mmap>],
        segment_slots: u32,
    ) -> f64;

    pub(crate) fn note_upsert(&self);          // bump drift counter
    pub(crate) fn should_check_drift(&self, interval: u64) -> bool;
}
```

关键语义：

- **probe 搜索**：先算 query 到全部质心的距离（O(lists·dim)，SIMD 核复用），
  取最小 nprobe 个 list；对每个命中 list 读锁克隆槽位表后立即释放，再并行算距
  离；pending 集合无条件全扫（通常很小）。tombstone 位图过滤与 Tier 0 相同。
- **删除不做 list 移除**：搜索期 tombstone 跳过已覆盖正确性；物理摘除留给压缩
  （压缩必然触发重建）。这样插入路径只碰一把 list 锁。
- **slot_list 容量增长**：`assign_slot` 时若 `slot >= slot_list.len()`，在
  `slot_list` 写锁下 resize 到当前 capacity，填 `u32::MAX`。
- **over-fetch**：候选数按 `limit + offset` 的 heap 截断已在 storage.rs 完成；
  无 filter 时 probe 结果直接进入同一管线。

---

### 2.5 修改 `src/storage.rs`：接线（对应 G1/G2/G5）

#### 2.5.1 结构体新增字段

```rust
pub struct CollectionStore {
    // ...existing fields...
    /// Published IVF index; None = Tier 0 exact scan. Swapped atomically.
    ivf: ArcSwap<Option<Arc<IvfIndex>>>,
    /// Serializes compaction vs index build/rebuild (G5). Lock order:
    /// maintenance -> inner.write. Never taken while holding inner.write.
    maintenance: parking_lot::Mutex<()>,
}
```

#### 2.5.2 打开/创建路径

- `create`：无变化（Tier 0 起步）；`CollectionConfig.ivf_config` 存入内存
  （不落 meta.bin，随 engine 配置传递）。
- `open`：WAL 回放完成后尝试 `persist::load(dir/index.bin)`：
  - 有效 → 构造 `IvfIndex`（质心/slot_list 直接装载），WAL 回放新增的点已在
    `reverse` 里但没有归属 → 全部进 `pending`；
  - 无效/缺失 → 保持 `None`（Tier 0）。

#### 2.5.3 插入路径（apply_upsert_locked 尾部追加）

```rust
let ivf = self.ivf.load();
match &*ivf {
    Some(index) => {
        index.assign_slot(slot as u32, &point.vector);
        index.note_upsert();
    }
    None => self.pending_slot(slot as u32),   // see below
}
```

`pending_slot`：若存在未发布的构建中状态则记入 pending；否则为纯 Tier 0
collection，不记任何东西（避免无谓内存）。实现上用一个
`building: parking_lot::Mutex<bool>` 标记区分两种 None。

#### 2.5.4 搜索路径分层

```rust
pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
    let ivf = self.ivf.load();
    if let Some(index) = &*ivf {
        let nprobe = query.nprobe.unwrap_or(index.default_nprobe())
            .min(index.list_count());
        let mut candidates =
            index.probe_candidates(query, nprobe, &self.tombstones.load(), ...)?;
        // Filter/threshold/top-K/assembly identical to Tier 0: extract the
        // existing steps 2..5 of search() into finish_candidates().
        return self.finish_candidates(candidates, query, /*exact_fallback*/ true);
    }
    drop(ivf);
    // ...existing Tier 0 full scan, then finish_candidates(..., false)
}
```

重构要求：把现 `search()` 的第 2~5 步（payload 过滤、score_threshold、top-K
heap、结果组装）抽成私有方法 `finish_candidates`，两条路径共用——保证分数、
过滤、offset/limit 语义完全一致（评审关注点）。

G7 补救逻辑放在 `finish_candidates` 的调用侧：带 filter 且结果数 < limit 且
`nprobe < lists` 时，以 `min(nprobe*2, lists)` 重探一次，仅一次。

#### 2.5.5 压缩路径（G5）

`compact()` 改造：

```rust
pub fn compact(&self) -> Result<u64> {
    let _guard = self.maintenance.lock();     // BEFORE inner.write
    let mut inner = self.inner.write();
    // ...existing body unchanged...
    inner.meta.save(&self.dir)?;

    // Invalidate Tier 1: slot numbering changed wholesale.
    self.ivf.store(Arc::new(None));
    let _ = std::fs::remove_file(self.dir.join("index.bin"));
    // WAL checkpoint + truncate as today
    Ok(live_count)
}
```

engine 层在 compact 返回后按需入队重建（见 §2.6）。
注意 `delete()`/`apply_txn()` 内部的 `threshold_met -> compact()` 内联调用同样
经过新的加锁入口，不会破坏锁序（它们此时不持有 inner.write——现有代码正是
先释放写锁再 compact，storage.rs:218/251）。

---

### 2.6 修改 `src/engine.rs`：维护线程与调度（对应 G1/G6）

```rust
enum MaintenanceJob {
    Build(String),      // build or rebuild Tier 1 for a collection
    Shutdown,
}

pub struct LocalVectorEngine {
    // ...existing fields...
    jobs: std::sync::mpsc::Sender<MaintenanceJob>,
}
```

- **worker 线程**：`open()` 时 `std::thread::Builder::new()
  .name("vector-maintenance").spawn(...)`；循环
  `recv_timeout(Duration::from_secs(30))`：
  - 收到 `Build(name)` → 执行 `build_collection_index(store)`（§2.6.1）；
  - 每 30s 空转周期 → 对每个 collection 做漂移检查（§2.6.2）；
  - `Shutdown`（Drop 中发送并 join，测试里显式控制）。
- **去重**：`in_flight: Mutex<HashSet<String>>`，同一 collection 同时最多一个
  构建任务；入队前查重。

#### 2.6.1 构建/重建流程（不在 store 锁内执行重活）

```text
1. guard   = store.maintenance.lock()          // excludes compaction (G5)
2. snapshot: meta(next_slot, dim, metric, segment_slots),
             tombstones = store.tombstones.load(),
             vsnap = store.vectors.snapshot()
3. live slots = 0..next_slot where !tombstone
   if live < min_build_points { return }
4. stride-sample min(live, sample_limit) slots -> training set
5. kmeans::train(...)                        // heavy, off-lock
6. index = IvfIndex::from_centroids(...)     // empty lists
7. publish:
     store.inner.write()                     // brief
       drain pending into lists (nearest-centroid assignment per slot)
       store.ivf.store(Arc::new(Some(Arc::new(index))))
8. persist::save(index.bin)                  // best-effort; failure = log warn
9. release guard
```

正确性论证（对应评审 G3/G5）：

- 构建期间的新 upsert：步骤 7 前 `ivf == None && building == true` → 进
  pending；步骤 7 一把写锁内 drain + 发布，之后新 upsert 直接 assign。无漏检窗口。
- 构建期间的 delete：只打 tombstone，不动 list；搜索期跳过。无影响。
- 构建期间不可能发生 compact（maintenance 互斥），slot 编号稳定。
- 步骤 7~8 之间崩溃：index.bin 缺失或落后于内存态，重启后 load 校验
  `slot_list.len() == next_slot` 失败 → 回退 Tier 0，等待再次 promotion。安全。

手动 API（CLI/测试/运维用）：

```rust
impl LocalVectorEngine {
    pub fn build_index(&self, collection: &str) -> Result<()>;   // synchronous build
    pub fn drop_index(&self, collection: &str) -> Result<()>;    // back to Tier 0
}
```

#### 2.6.2 漂移检查（G6）

```text
if !index.should_check_drift(interval) { continue }
sample = stride-sample min(live, sample_limit) live slots
ratio  = index.drift_ratio(sample, ...)
store.record_drift(ratio)                    // exposed via CollectionInfo
if ratio > config.drift_threshold { enqueue Build(name) }
```

提交路径零开销：`note_upsert()` 只是一次 `AtomicU64::fetch_add`。

---

### 2.7 配置与上层接线

#### graphdb-config（config.rs）

```toml
[vector.local]
data_dir = "..."

[vector.local.ivf]
auto_promotion = false      # 默认关闭，待 Phase B bench 结论后再改默认值
lists = 0                   # 0 = auto(sqrt(n))
min_build_points = 100000
default_nprobe = 8
drift_threshold = 0.10
drift_check_interval = 25000
```

`LocalVectorConfig` 增加 `pub ivf: Option<IvfSettings>`（serde default），提供
`IvfSettings::to_ivf_config() -> IvfConfig`。graphdb-config 已依赖
vector-search 所需类型？——注意：config crate 当前通过 vector-client 间接引用
类型；为守住 DAG，`IvfSettings` 在 config 内独立定义（结构重复但解耦），或在
graphdb-sync builder 处完成映射。**推荐后者**：config 只存原始数值，builder.rs
组装 `CollectionConfig` 时映射为 `vector_search::IvfConfig`。

#### graphdb-sync（sync/builder.rs、manager.rs）

- `builder.rs:191` 处创建 `VectorBackend::Local(engine)` 时，把
  `config.vector.local.ivf` 映射结果暂存；
- 创建 collection 的路径（`manager.rs:791` / `backend.create_index`）将
  `CollectionConfig::new(dim, metric).with_ivf(ivf_cfg)` 传入，使 engine 能感知
  promotion 配置。

#### root feature / Cargo.toml

无需改动：vector-search 是 vector 必选依赖，无新第三方依赖。

---

### 2.8 bench 扩展（`benches/vector_scan_bench.rs` 或新增 `benches/ivf_bench.rs`）

新增组（沿用 DIM=128、SEED 固定）：

| 组 | 内容 |
|----|------|
| `ivf_build_time` | N ∈ {100k, 1M}：`build_index` 耗时（含采样+kmeans+发布） |
| `ivf_latency_vs_nprobe` | N=100k/1M，nprobe ∈ {1, 4, 16, 64, all}：查询 p50（criterion 均值即可） |
| `recall_vs_nprobe` | 同上参数：对固定 100 条查询计算 recall@10（以 Tier 0 精确结果为 ground truth，内嵌断言 recall@nprobe=all == 1.0） |
| `tier_crossover` | 输出 Tier0 与 Tier1(nprobe=默认) 的延迟交叉点表格（打印，供决策） |
| `ivf_upsert_overhead` | 有/无索引时的 apply_txn 吞吐对比（验证 G4 的锁粒度设计） |

决策规则写入 bench 注释：若 Tier1(default_nprobe) 在 1M 点延迟 < Tier0 的 1/3
且 recall@10 ≥ 0.98，则将 `auto_promotion` 默认改为 true。

---

### 2.9 测试计划

单元测试（各文件内）：

- `kmeans.rs`：合成三簇数据的划分质量；两次训练结果逐元素一致。
- `persist.rs`：roundtrip；magic/version/长度/metric 各自损坏 → `Ok(None)`。
- `index.rs`：assign/probe/drain_pending/drift_ratio 的定向小用例（dim=4）。
- `storage.rs` 增补：upsert 在有索引时进入 list、pending 在无发布时累积。

集成测试（新增 `crates/vector-search/tests/ivf_test.rs`）：

1. `promote_search_rebuild_roundtrip`：小阈值配置（min_build_points=100 等）
   → 插入聚类数据 → 手动 `build_index` → probe 结果 ⊇ 簇中心近邻、
   recall@10 ≥ 0.9 → 删除部分点 → `compact()` → 索引失效回退 Tier 0 →
   再 `build_index` → 结果仍正确。
2. `build_window_no_missing_results`：构建期间并发插入的点必须出现在后续
   probe 搜索中（G3 回归测试）。
3. `corrupt_index_bin_falls_back_to_tier0`：截断 index.bin → open 成功、
   count/search 正确（G2 回归测试）。
4. `drift_triggers_rebuild`：插入偏移数据使 drift_ratio > 阈值 → 维护线程
   触发重建（轮询 CollectionInfo 观察 built_at_live_count 变化）。
5. `concurrent_search_during_rebuild`：后台线程持续 search，主线程反复
   build/drop，无 panic、结果始终非空（压力冒烟）。
6. `filtered_probe_retry_once`：强选择性 filter 下 probe 不足 limit 时翻倍
   重试一次的行为（G7）。

验证命令：

```shell
cargo test -p vector-search
cargo test --lib
cargo bench -p vector-search -- ivf
cargo fmt && cargo clippy -p vector-search
```

---

### 2.10 文档同步

- `docs/vector/vector-engine-design.md`、`docs/vector/implementation-checklist.md`
  增加 Phase B 章节：Tier 语义、近似搜索声明（G7）、index.bin 格式、配置项。
- 总方案 `vector_local_engine_plan.md` §3.2 修订一行：质心与归属存于
  `index.bin`（派生结构），不入 `meta.bin`；§6 Phase B 清单替换为本文件的
  决策链接。

### 2.11 实施顺序与工作量

| 步骤 | 内容 | 估时 |
|------|------|------|
| 1 | types.rs 扩展 + kmeans.rs + 单测 | 0.5 天 |
| 2 | index.rs（IvfIndex）+ persist.rs + 单测 | 1 天 |
| 3 | storage.rs 接线（finish_candidates 抽取、compact 失效、锁序改造） | 1 天 |
| 4 | engine.rs 维护线程 + 手动 API + graphdb-config/sync 接线 | 0.5 天 |
| 5 | 集成测试 + bench + 文档 | 1 天 |
| 合计 | | **3~4 天** |
