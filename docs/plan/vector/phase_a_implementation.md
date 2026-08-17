# 内置向量引擎 Phase A 具体修改方案：Tier 0 精确扫描 + 存储 + 集成

> 状态：实施方案（2026-08-17）。
>
> 上游设计：
> - [vector_local_engine_plan.md](../vector_local_engine_plan.md)（总方案，本方案执行其 §6 Phase A 全部条目）
> - [vector_local_engine_pgvector_analysis.md](../vector_local_engine_pgvector_analysis.md)（pgvector 效仿分析，本文标注出处引用）
>
> 参照实现：`ref/pgvector`（v0.8.6）。本文所有 pgvector 出处均为
> 分析文档 §1 的 13 个效仿点在本阶段的落点。
>
> 说明：本文是**改动级**实施方案（文件、格式、接口、feature、测试、顺序），
> 与总方案的决策级内容不重复。涉及现有代码位置均标注 `路径:行号`。

## 0. 结论摘要

| 项 | 决策 |
|----|------|
| 目标 | 打通「存储 → SIMD 核 → 过滤 → WAL → graphdb-sync 本地路径 → 配置/feature」全链路，默认 feature 即可运行，不依赖外部 Qdrant |
| 新 crate | `crates/vector-search`（叶子 crate，不依赖任何 graphdb crate 与 tonic/prost/reqwest） |
| 存储 | 每 collection 目录 5 文件：meta/vectors/keys/payloads/wal；`vectors.bin` 分段 mmap + ArcSwap 快照；tombstone 位图；20% 阈值压缩 |
| WAL | 每 collection 追加式 `wal.bin`；提交时「先整批 append+fsync，后应用内存」；回放幂等（upsert 覆盖 / delete 空操作） |
| 距离核 | L2/Cosine/Dot 三个 AVX2 核 + naive 对照；cosine 单循环累加（对齐 `vector.c:650` `VectorCosineSimilarity`） |
| 分数语义 | 输出 Qdrant 兼容 score（cosine=相似度、dot=内积、euclid=1/(1+d)），保证下游 threshold 语义不变 |
| 并发 | 每 collection 一把 `parking_lot::RwLock`（写串行）+ `ArcSwap` 读快照（mmap 读无锁）；本地路径纯同步，async 仅 coordinator 表面 |
| 类型归属 | `SearchQuery`/`VectorFilter`/`VectorPoint`/`CollectionConfig`/`DistanceMetric` 等全部迁入 vector-search；vector-client `pub use` 转发 |
| feature | 根 `vector`（默认，本地引擎，无重依赖）+ `vector-qdrant`（外部 Qdrant 适配）；原 `qdrant` feature 迁移 |
| embedding | 留 vector-client（qdrant 路径专属，`vector-qdrant` 门控），本地路径不接线（phase A 已知边界） |

## 1. 范围界定

### 1.1 本阶段做

- `crates/vector-search` 全量：mmap 存储、tombstone 压缩、WAL 追加/回放、AVX2
  距离核、`VectorFilter` 后过滤、`LocalVectorEngine` 引擎接口；
- graphdb-sync 本地路径：`VectorSyncCoordinator` 提交时同步应用 + WAL；
- graphdb-config / config.toml / 根 feature 改造；类型引用全局切换；
- 单元 + 集成 + 删除/压缩/崩溃恢复测试；`benches/vector_scan_bench.rs`。

### 1.2 本阶段明确不做（留给 Phase B/C）

- Tier 1 IVFFlat（k-means、list、probe、漂移重建）；
- qdrant 路径的 feature 收尾与错误映射统一（Phase C）；
- embedding 迁移与本地路径接线；
- payload 字段索引（`create_payload_index` 族）；
- Manhattan 距离、量化、HNSW。

## 2. 新 crate：crates/vector-search

### 2.1 Cargo.toml

```toml
[package]
name = "vector-search"
version.workspace = true
edition.workspace = true

[lib]
name = "vector_search"
path = "src/lib.rs"

[dependencies]
serde.workspace = true
serde_json.workspace = true
postcard.workspace = true
thiserror.workspace = true
parking_lot.workspace = true
rayon.workspace = true
memmap2.workspace = true
bitvec.workspace = true
arc-swap.workspace = true
tracing.workspace = true
chrono.workspace = true

[dev-dependencies]
tempfile.workspace = true
rand.workspace = true

[[bench]]
name = "vector_scan_bench"
harness = false
```

依赖全部取自 workspace 既有项（AGENTS.md 依赖列表），不新增任何 graphdb
crate 依赖、不引入 tonic/prost/reqwest。

### 2.2 模块布局

```
crates/vector-search/
├── Cargo.toml
├── src/
│   ├── lib.rs                 # pub mod 声明 + 顶层 re-export
│   ├── types.rs               # = 现有 vector-client/src/types*.rs 迁入（§3）
│   ├── error.rs               # VectorSearchError
│   ├── distance/
│   │   ├── mod.rs             # DistanceMetric 分发、score↔distance 换算
│   │   ├── naive.rs           # 朴素参考实现（对照基线）
│   │   └── avx2.rs            # #[target_feature(enable="avx2")] 显式核
│   ├── filter.rs              # VectorFilter 对 payload 的求值器
│   ├── storage/
│   │   ├── mod.rs             # CollectionStore（per-collection 装配 + 对外 ops）
│   │   ├── meta.rs            # Meta + meta.bin
│   │   ├── vectors.rs         # vectors.bin 分段 mmap
│   │   ├── keys.rs            # keys.bin（slot→PointId + blob）
│   │   ├── payloads.rs        # payloads.bin（slot rec + tombstone 位 + blob）
│   │   ├── wal.rs             # wal.bin 追加 + 回放（幂等）
│   │   └── compaction.rs      # tombstone 压缩（临时文件 + rename 原子替换）
│   └── engine.rs              # LocalVectorEngine（collection 注册表 + apply_txn）
├── benches/
│   └── vector_scan_bench.rs
└── tests/
    ├── storage_test.rs        # 槽位/压缩/崩溃恢复
    ├── search_test.rs         # 距离/过滤/阈值/分页
    └── recovery_test.rs       # WAL 回放幂等
```

### 2.3 错误类型（error.rs）

```rust
#[derive(Debug, thiserror::Error)]
pub enum VectorSearchError {
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    #[error("collection already exists: {0}")]
    CollectionAlreadyExists(String),
    #[error("invalid collection name: {0}")]
    InvalidCollectionName(String),
    #[error("invalid vector dimension: expected {expected}, got {actual}")]
    InvalidVectorDimension { expected: usize, actual: usize },
    #[error("invalid point id: {0}")]
    InvalidPointId(String),
    #[error("non-finite vector element at index {0}")]
    NonFiniteElement(usize),
    #[error("metric not supported by local engine: {0:?}")]
    UnsupportedMetric(DistanceMetric),
    #[error("filter error: {0}")]
    Filter(String),
    #[error("corrupt data: {0}")]
    CorruptData(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] postcard::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, VectorSearchError>;
```

graphdb-sync 内新增 `From<VectorSearchError> for VectorCoordinatorError` 映射
（qdrant 路径的 `From<VectorClientError>` 映射保留，`graphdb-sync/src/sync/vector_error.rs`）。

## 3. 类型迁移（实施第一步，先动类型后动逻辑）

### 3.1 迁移清单

以下类型整体从 `crates/vector-client/src/` 迁入 `vector-search/src/types.rs`
（保持原有字段与 derive，补 `Serialize/Deserialize` 以支持 meta.bin/WAL postcard
持久化，需要则拆分到 `types/` 子目录，当前单文件即可）：

| 源文件 | 类型 |
|--------|------|
| `types.rs` | `Payload`、`PointId`、`CollectionName` |
| `types/config.rs` | `DistanceMetric`、`HnswConfig`、`IndexType`、`CompressionRatio`、`QuantizationType`、`QuantizationConfig`、`CollectionConfig`、`CollectionInfo`、`CollectionStatus`、`PayloadSchemaType`、`HealthStatus` |
| `types/filter.rs` | `PayloadValue`、`GeoPoint`、`GeoRadius`、`GeoBoundingBox`、`ValuesCountCondition`、`VectorFilter`、`MinShouldCondition`、`FilterCondition`、`ConditionType`、`RangeCondition`、`PayloadSelector` |
| `types/point.rs` | `VectorPoint`、`VectorPoints`、`UpsertResult`、`UpsertStatus`、`DeleteResult` |
| `types/search.rs` | `SearchMode`、`SearchQuery`、`SearchResult`、`SearchResults`、`BatchSearchQuery` |

> `DistanceMetric` 现含 `Manhattan`（vector-client 自身标注 qdrant 不支持）。
> 本地引擎 Phase A 支持 L2/Cosine/Dot，Manhattan 返回 `UnsupportedMetric`
> （保留枚举变体以兼容 qdrant 路径，不删）。

### 3.2 vector-client 转发

`crates/vector-client/src/types.rs` 整体删除，改 lib 级转发（`lib.rs`）：

```rust
pub use vector_search::types;
pub use types::*;      // 原有 `pub use types::*` 语义不变
```

`vector-client/Cargo.toml` 增加 `vector-search = { path = "../vector-search" }`
（非 optional，无条件）。crate 内所有 `use crate::types::*` / `super::*`
引用路径保持可用，engine/manager/api 无需改类型路径。

**注意事项**：`types.rs` 里的 `DistanceMetric` 实现引用了 `DistanceMetric`
的 `is_supported_by_qdrant`/`requires_custom_implementation` 帮助方法
（`types/config.rs:12-20`）——这两个方法属 qdrant 语义，迁入 vector-search
后保留（无害的纯查询），或标注 `#[doc(hidden)]`。

### 3.3 下游引用切换（文件级清单）

| 文件 | 改动 |
|------|------|
| `crates/graphdb-query/src/query/planning/vector_planner.rs:24` | `use vector_client::types::{...}` → `use vector_search::types::{...}` |
| `crates/graphdb-query/src/query/planning/plan/core/nodes/search/data_access.rs:11-12` | `pub use vector_client::types::VectorFilter` → `vector_search::types::VectorFilter` |
| `crates/graphdb-query/src/query/planning/plan/logical/logical_nodes/search.rs:58` | 同上（test 内 import） |
| `crates/graphdb-query/src/query/executor/streaming/operators/vector_operator.rs:227-233` | `vector_client::DistanceMetric` → `vector_search::DistanceMetric` |
| `crates/graphdb-api/src/api/core/vector_api.rs:8-12` | `vector_client::manager::IndexMetadata` / `types::PointId` / `CollectionConfig` 等 → vector-search；`VectorManager` 换为 `VectorBackend`（§8.1） |
| `crates/graphdb-server/src/server/graph_service.rs:33`、`startup.rs:13-15` | `vector_client::VectorManager` / `EmbeddingService` → 后端枚举/门控（§10） |
| `crates/graphdb-server/src/server/http/handlers/vector.rs:12` | `vector_client::{DistanceMetric, VectorFilter}` → vector-search |
| `crates/graphdb-sync/src/sync/vector_sync.rs:16-19` | `vector_client::{...}` 拆为：类型→vector-search；`VectorManager`/`EmbeddingService`→`#[cfg(feature="vector-qdrant")]`（§8） |
| `config.toml:105-141` | `[vector]` 表重定义（§9.3） |
| `crates/graphdb-config/src/config.rs:74-75,111-114,425-441` | `VectorClientConfig` → 新 `VectorConfig`（§9.2） |

**验收**：`cargo build --features vector-qdrant`（旧 qdrant 全功能）与
`cargo build --features vector`（本地）两套均通过；`cargo test -p vector-client`
在转发后全部保持通过。

## 4. 存储设计

对齐 pgvector 两个存储思想（分析 §2.5）：定长稠密行主序 f32（`vector.h`
`Vector` 的 `4*dim+8` 定长），元数据与数据分离（`IvfflatMetaPageData`）。

### 4.1 目录与文件

```
<data_dir>/vector/<collection>/
├── meta.bin        # postcard(Meta)，定长头部信息
├── vectors.bin     # 稠密行主序 f32，分段（segment）增长
├── keys.bin        # 头部 + KeyRec[capacity] + blob 区（utf8 point id）
├── payloads.bin    # 头部 + SlotRec[capacity] + blob 区（postcard(Payload)）
└── wal.bin         # 追加式事务日志（§4.6）
```

### 4.2 内存结构与并发

```rust
struct CollectionStore {
    meta: Meta,
    vectors: arc_swap::ArcSwap<Vec<Arc<Mmap>>>,  // 分段 mmap，读快照无锁
    keys: arc_swap::ArcSwap<KeysView>,           // { mmap, recs, blob } 只读快照
    payloads: arc_swap::ArcSwap<PayloadsView>,   // 同上
    reverse: parking_lot::RwLock<HashMap<PointId, u32>>,  // PointId→slot
    tombstones: arc_swap::ArcSwap<bitvec::BitVec>,         // slot 删除镜像
    wal: std::fs::File,                          // 追加句柄（写锁内使用）
}
```

- 写路径（upsert/delete/compact）持 `parking_lot::RwLock` 写锁，串行化；
- 读路径（search/scan）先持读锁快照 `ArcSwap` 引用，**释放锁后无锁访问**
  （mmap 对象不可变，压缩/增长只是替换 Arc 内容，旧读者持有旧 Arc 依旧有效）——
  满足总方案 §3.7「mmap 读路径无锁（slot 位图/压缩期间用读写锁切换）」；
- `reverse` 只服务于 `get`/`delete`/幂等 upsert（点按 id 复用槽位），搜索热路径不触碰。

### 4.3 meta.bin

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    format_version: u32,       // = 1
    collection: String,
    vector_size: usize,        // 维度
    distance: DistanceMetric,
    segment_slots: u32,        // 每段槽位数，默认 8192
    slot_capacity: u64,        // 已分配槽位
    next_slot: u64,            // 高水位（含 tombstone）
    live_count: u64,           // 存活点
    tombstone_count: u64,
    last_applied_txn: u64,     // WAL 水位（回放优化/一致性校验，§4.6）
    created_at: i64,
}
```

打开时校验 `format_version` 与 `vector_size>=1`（对齐 pgvector `CheckDim`，
`vector.c:94-106`）；`vectors.bin` 文件长度 = `capacity*dim*4`，不符报
`CorruptData`。

### 4.4 vectors.bin（分段 mmap）

- 文件按 `segment_slots*dim*4` 字节整段追加；槽位 `slot` 位于
  `segment = slot / segment_slots`，段内偏移 `(slot % segment_slots)*dim*4`；
- 每次增长在写锁内：`File::set_len` → 新建覆盖新段区域的 `Mmap` → push 进
  `Vec<Arc<Mmap>>` → ArcSwap 换入（旧段 mmap 不变，读者无感）；
- 扫描用 `(score, slot)` 配对，跳过 tombstone 位图置位的槽。

### 4.5 keys.bin / payloads.bin（槽位目录 + blob 区）

两文件同构，头部定长、记录数组定长、blob 区追加：

```
[u8;4] magic          # "VKEY" / "VPLD"
[u32]  version        # 1
[u64]  rec_capacity   # 与 slot_capacity 一致
[u64]  blob_len
KeyRec[rec_capacity]  # { off: u32, len: u32 } 8B/条
SlotRec[rec_capacity] # { off: u32, len: u32, flags: u8, pad: [u8;3] } 12B/条
blob 区               # 追加式：key=utf8；payload=postcard(Payload)
```

- `SlotRec.flags` bit0 = tombstone；内存中镜像一份 `BitVec` 供快速扫描；
- upsert 新点：分配 `slot = next_slot++`（超容量先 `grow`），写 blob、写记录；
- upsert 已有 id：查 `reverse` 复用槽位，覆盖写（新 blob 追加，旧 blob 留给压缩回收）；
- 打开时重建 `reverse`：遍历存活 `KeyRec`（对齐分析 §3.1「无页内空洞」，
  物理删除统一走压缩）。

### 4.6 WAL wal.bin（追加式，提交协议，回放）

对齐分析 §2.7「向量写与业务事务经 WAL 原子落盘」：每 collection 目录内
追加式日志，op 携带 txn id，回放幂等。

**记录格式**：

```rust
// wal.bin 每条： [u32 len][postcard(WalTxn)]
#[derive(Serialize, Deserialize)]
struct WalTxn {
    txn_id: u64,
    ops: Vec<WalRecord>,
}

#[derive(Serialize, Deserialize)]
enum WalRecord {
    Upsert { point: VectorPoint },
    Delete { point_id: String },
    DeleteBatch { point_ids: Vec<String> },
    Compact,          // 检查点标记（压缩后写入，回放时忽略数据但推进水位）
    DropCollection,   // 预留（drop 时整目录删除，可不记）
}
```

**提交协议**（`LocalVectorEngine::apply_txn`，graphdb-sync 本地路径入口）：

```
1) 对涉及的各 collection，按 collection 名字典序依次：
     序列化 WalTxn{txn_id, ops} → 追加 wal.bin → flush + fsync
   （确定性顺序保证崩溃时各文件推进方向一致）
2) 全部 fsync 成功后，应用到内存 + mmap（槽位分配/覆盖写/位图）
3) 更新 meta.last_applied_txn（下一步 fsync）
```

**回放**：启动打开 collection 时按序读 `wal.bin`，逐 `WalTxn` 应用——
upsert 幂等（按 id 覆盖槽位，重复应用不产生新槽位）、delete 幂等（缺失点
空操作）、`Compact` 只推水位。回放结束校验 `meta.live_count` 与槽位一致性。

**崩溃恢复语义**（与图事务的边界，见 §14.1）：
- 崩溃于「graph 提交后、vector WAL 未写」→ graph WAL 重放会重跑事务 →
  coordinator 幂等再应用；
- 崩溃于「WAL 已写、内存未应用」→ 启动回放补齐；
- 幂等键 = `PointId`（upsert 覆盖）+ 空操作 delete，无需去重集合。

### 4.7 删除与压缩

- 删除 = 置 tombstone 位图 + `reverse` 移除；`live_count--`，`tombstone_count++`
  （对齐分析 §2.2「行删除 + 后台清理」，IVFFlat `ivfvacuum.c` 的清理职责在此
  简化为整库压缩）；
- 压缩触发：`tombstone_count / next_slot > 0.20`（20% 阈值，分析 §4.2 #5），
  写锁内执行：
  1. 遍历存活槽位，写 `vectors_tmp.bin`/`keys_tmp.bin`/`payloads_tmp.bin`
     （新槽位序 0..live_count）；
  2. 三文件分别 fsync → rename 原子替换（对齐 graphdb-storage
     `persistence.rs` 的 temp+rename+dir fsync 模式）；
  3. 重建 mmap 快照、`reverse`、`BitVec`，重写 `meta.bin`；
  4. wal.bin 追加 `Compact` 检查点后**截断**（重开空文件）→ fsync。
- 压缩期间持有写锁阻塞搜索，单机场景可接受（总方案 §3.4 语义：无图结构
  损坏问题、无需 REINDEX）。

## 5. SIMD 距离核

### 5.1 语义（对齐 pgvector 距离函数 + Qdrant score）

| 度量 | 内部排序距离（越小越近） | 输出 score（Qdrant 兼容） | pgvector 出处 |
|------|--------------------------|---------------------------|---------------|
| L2 (Euclid) | `Σ(a-b)²`（平方距离，省 sqrt） | `1/(1+sqrt(d²))` | `vector.c:560` `VectorL2SquaredDistance` |
| Dot | `-Σ(a·b)` | `Σ(a·b)`（内积） | `vector.c:608` `VectorInnerProduct` |
| Cosine | `1 - Σ(a·b)/sqrt(Σa²·Σb²)`（夹取[-1,1]） | 相似度 `Σ(a·b)/sqrt(...)` | `vector.c:650` `VectorCosineSimilarity` |

cosine 必须**单循环**同时累加点积与两个范数（`vector.c:656-662`），避免两次遍历。
`score_threshold` 一律在输出 score 上按下限过滤，保证与 qdrant 路径语义一致。

输入校验：upsert 时拒绝 NaN/Inf 元素（对齐 pgvector `CheckElement`，
`vector.c:111-121`），返回 `NonFiniteElement`。

### 5.2 实现

- `distance/naive.rs`：逐元素标量循环，作为**对照基线**（不要主动优化，
  保证可读性与确定性）；
- `distance/avx2.rs`：
  - `#[cfg(target_arch = "x86_64")]` + 运行时 `is_x86_feature_detected!("avx2")`
    检测；不满足则回落 naive（.cargo/config.toml 的 `x86-64-v3` 使主流路径
    恒命中 avx2）；
  - 每核处理 8 个 f32/YMM（`_mm256_mul_ps`/`_mm256_fmadd_ps`/`_mm256_add_ps`），
    尾数标量；cosine 三个累加器各一条 YMM；
  - 距离核输入为裸指针 + dim（`&[f32]` 切片即可），按 32B 对齐的段内偏移
    访问 `vectors.bin`（`slot*dim` 保证对齐到 4B，f32 索引天然满足；avx2
    宽松对齐加载亦可）；
- `distance/mod.rs`：`pub fn distance(metric, a, b) -> f32`（内部距离）与
  `pub fn to_score(metric, dist) -> f32`；`pub unsafe fn distance_avx2(...)`
  仅测试/引擎内部使用。

### 5.3 正确性对照

- `#[cfg(test)]`：naive vs avx2 在随机向量（含 dim=1、dim=128、dim=1025 等非
  8 倍数、零向量）上逐点 `assert!((a-b).abs()<1e-5)`；cosine 零范数 → 距离 1、
  score 0（对齐 `vector-engine-design.md` §4.3.1 零向量边界）；
- 总方案 §8「bench 内嵌断言」落为上述单元测试 + bench 内以 env 开关跑一轮
  一致性抽检。

## 6. 搜索管线（Tier 0 精确扫描）

对齐 pgvector 默认精确扫描（README L197 "exact nearest neighbor search…
perfect recall"）+ 近似索引后过滤语义（README L450），在本阶段是唯一路径。

```
search(collection, &SearchQuery):
 1) 快照 mmap/位图（读锁内取 ArcSwap，锁后无锁）
 2) rayon 并行：按 slot 区间切块，跳过 tombstone，
    每块产出局部 Vec<(score, slot)>（SIMD 核 + to_score）
 3) 合并后 filter 求值（§6.2）→ 淘汰不命中候选
 4) score_threshold 下限过滤
 5) 对 (score, slot) 降序做堆选 topK（K = offset + limit），再排序
 6) 组装 SearchResult{ id, score, payload/vector 按 with_* 决定 }，取 [offset, offset+limit)
```

- 并行度：`rayon::current_num_threads()`，切片粒度 ~4096 槽，或按分段；
- 候选全量打分（后过滤必须全扫），堆选只在排序段收敛；
- `SearchQuery.nprobe`/`search_mode` 字段 Phase A 忽略（Tier 1 预留）；
- 大 collection 返回前裁剪 `with_payload=false`/`with_vector=false` 时不读 blob。

## 7. 引擎接口（LocalVectorEngine）

`engine.rs`，纯同步实现（总方案 §3.7「内置路径为同步实现」）：

```rust
pub struct LocalVectorEngine {
    root_dir: PathBuf,                      // <data_dir>/vector
    collections: parking_lot::RwLock<HashMap<String, Arc<CollectionStore>>>,
}

impl LocalVectorEngine {
    pub fn open(root_dir: impl AsRef<Path>) -> Result<Self>;   // 扫描目录 + WAL 回放
    pub fn create_collection(&self, name: &str, config: &CollectionConfig) -> Result<()>;
    pub fn delete_collection(&self, name: &str) -> Result<()>;
    pub fn collection_exists(&self, name: &str) -> bool;
    pub fn collection_info(&self, name: &str) -> Result<CollectionInfo>;

    pub fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()>;
    pub fn upsert_batch(&self, collection: &str, points: &[VectorPoint]) -> Result<()>;
    pub fn delete(&self, collection: &str, point_id: &str) -> Result<()>;
    pub fn delete_batch(&self, collection: &str, point_ids: &[String]) -> Result<()>;
    pub fn delete_by_filter(&self, collection: &str, filter: &VectorFilter) -> Result<u64>;

    pub fn search(&self, collection: &str, query: &SearchQuery) -> Result<Vec<SearchResult>>;
    pub fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>>;
    pub fn count(&self, collection: &str) -> Result<u64>;

    /// 事务批量提交：整批 WAL append+fsync 后统一应用（§4.6 提交协议）
    pub fn apply_txn(&self, txn_id: u64, ops: Vec<TxnOp>) -> Result<()>;
}

pub enum TxnOp {
    Upsert { collection: String, point: VectorPoint },
    Delete { collection: String, point_id: String },
}
```

- 集合名/维度校验、`UnsupportedMetric`、`CorruptData` 错误均在此层产出；
- `delete_by_filter`：全量过滤求值出 id 集合，转 tombstone（Phase A 不做
  过滤索引）；供图库顶点级删除（`vector_sync.rs:536-749` 的
  `on_vertex_deleted`）复用；
- `open()` 幂等：重复 open 同一目录只回放增量（水位 `last_applied_txn`）。

## 8. graphdb-sync 集成

### 8.1 VectorBackend 后端枚举

`crates/graphdb-sync/src/sync/` 新增 `backend.rs`（或并入 `vector_sync.rs`），
具体类型（不引入 `Arc<dyn>`，对齐 AGENTS.md 偏好）：

```rust
#[derive(Clone)]
pub enum VectorBackend {
    Local(Arc<vector_search::LocalVectorEngine>),
    #[cfg(feature = "vector-qdrant")]
    Qdrant(Arc<vector_client::VectorManager>),
}
```

`VectorBackend` 提供异步外壳方法（内部 local 同步直调 / qdrant 委托 manager）：
`create_collection`、`delete_collection`、`collection_exists`、
`collection_info`、`upsert`/`upsert_batch`、`delete`/`delete_batch`、
`delete_by_filter`、`search`、`get`、`count`、`health_check`。
`health_check` 对 Local 恒返回 healthy（本地无远端可查）。

### 8.2 coordinator 改造（vector_sync.rs）

| 位置 | 现状 | 改动 |
|------|------|------|
| `vector_sync.rs:16-19` import | `vector_client::{EmbeddingService, FilterCondition, SearchQuery, SearchResult, VectorFilter, VectorManager, VectorPoint}` | 类型→`vector_search`；`EmbeddingService`/`VectorManager`→`#[cfg(feature="vector-qdrant")]` |
| `VectorSyncCoordinator` 字段 | `vector_manager: Arc<VectorManager>` + `embedding_service: Option<Arc<EmbeddingService>>` | → `backend: VectorBackend`；`embedding_service` 字段整体 `#[cfg(feature="vector-qdrant")]` |
| `new()`/`with_transaction_buffer()`（375-403） | 收 `VectorManager` | 收 `VectorBackend`；`is_disabled_engine()` 判断改为后端类型（Local 恒 Active；无后端时为 Disabled） |
| `create_vector_index`（430-513） | 经 manager 建物理 collection + payload index | `backend.create_collection`（Local 无需建 group_id 字段索引）；逻辑索引注册逻辑不变 |
| `commit_transaction`（783-808） | peek→`on_vector_change_batch`→take | Local 分支：`buffer.peek_updates` → 转 `TxnOp` → `backend.apply_txn(txn_id, ops)` → 成功才 `take_updates`；qdrant 分支保持原样 |
| `on_vector_change_batch`（830-913） | manager 分组 upsert/delete | 仅 qdrant 分支使用（`apply_vector_mutation` 等 outbox 路径），Local 分支改走 `apply_txn`，此函数保留 |
| `search` 族（916-1173） | manager.search | `backend.search`；group_id 注入逻辑不变 |
| `embed_text`（1027-1040） | embedding_service 字段 | `#[cfg(feature="vector-qdrant")]`；Local 仅此方法门控，返回 `EmbeddingError` |

> 总方案 §3.6「`commit_transaction` 现有接口不变，实现替换」：签名
> `async fn commit_transaction(&self, txn_id) -> Result<()>` 保持，内部按后端分支。

`sync.rs:16-17` 门控 `pub mod vector_sync` 由 `#[cfg(feature="qdrant")]`
改为 `#[cfg(feature="vector")]`；`sync.rs:45-46,55-59` 的 qdrant 专属导出
（`VectorReceiver` 等）改 `#[cfg(feature="vector-qdrant")]`。

### 8.3 提交时序（Local）

```
图事务 commit
 └─ SyncManager → coordinator.commit_transaction(txn_id)
     ├─ Local：peek_updates → TxnOp[] → LocalVectorEngine.apply_txn
     │           （各 collection WAL 顺序 append+fsync → 应用内存 → 更新水位）
     ├─ 成功 → buffer.take_updates
     └─ 失败 → 返回错误（不取缓冲，可重试/图事务失败），无 DLQ/outbox/熔断
```

## 9. 配置与 feature

### 9.1 根 Cargo.toml

```toml
[workspace.members]   # 追加
"crates/vector-search",

[features]
default = ["server", "vector"]            # 原 default=["server"]
vector = ["graphdb-api/vector", "graphdb-server/vector", "graphdb-config/vector",
          "graphdb-sync/vector", "graphdb-query/vector"]
vector-qdrant = ["vector", "graphdb-api/vector-qdrant", "graphdb-server/vector-qdrant",
                 "graphdb-config/vector-qdrant", "graphdb-sync/vector-qdrant",
                 "graphdb-query/vector-qdrant"]
# 删除原 qdrant feature（graphdb-search 内 `qdrant=["graphdb-sync/qdrant"]`
# 未使用项一并清理，或留空别名到 vector-qdrant）
```

根 `vector-client` 依赖（`Cargo.toml:175`）改 optional，仅 `vector-qdrant`
启用：`vector-client = { path = "crates/vector-client", optional = true }`；
检查根 `src/lib.rs`/`src/main.rs` 是否有直接引用，若有同步门控。

### 9.2 graphdb-config

`crates/graphdb-config/src/config.rs`：

```rust
#[cfg(feature = "vector")]
#[serde(default)]
pub vector: VectorConfig,          // 原 111-114 行 `vector: VectorClientConfig`

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct VectorConfig {
    pub enabled: bool,                       // default true
    pub engine: VectorEngineKind,            // default Local
    #[serde(default)]
    pub local: LocalVectorConfig,            // { data_dir: PathBuf }
    #[cfg(feature = "vector-qdrant")]
    #[serde(default)]
    pub qdrant: vector_client::VectorClientConfig,  // 原 [vector] 表内容迁至此
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
pub enum VectorEngineKind { #[default] Local, Qdrant }
```

- 访问器改造：`is_vector_enabled()`（425-435）→ `enabled && (Local || Qdrant.enabled)`；
  新增 `is_local_vector()` / `vector_config()` 返回 `&VectorConfig`；
- `resolve_relative_paths()`（200-257）追加解析 `vector.local.data_dir`
  （相对 config 基准目录），默认 `<common.database.data_dir>/vector`
  （查看 `CommonConfig.database` 现有字段确认 data_dir 名，默认
  `graphdb-storage/src/storage/engine/paths.rs` 的 `data/`）；
- `Cargo.toml`：feature `qdrant=["dep:vector-client"]` 拆为
  `vector=[]` + `vector-qdrant=["vector","dep:vector-client"]`；去掉
  依赖上 `features=["qdrant-grpc"]` 的死约束（vector-client 自身 default）。

### 9.3 config.toml（105-141 行替换）

```toml
[vector]
enabled = true
engine = "local"          # "local" | "qdrant"

[vector.local]
# data_dir 默认 <数据库 data_dir>/vector
# data_dir = "data/vector"

[vector.qdrant]           # 原 [vector] 表整体迁入（enabled/engine 除外）
enabled = true
[vector.qdrant.connection]
host = "localhost"
port = 6334
http_port = 6333
use_tls = false
# api_key = "your-api-key"
connect_timeout_secs = 5
[vector.qdrant.timeout]
request_timeout_secs = 30
search_timeout_secs = 10
upsert_timeout_secs = 30
# [vector.qdrant.embedding] ... 原注释块迁移
```

> 迁移时删除无效的 `[vector.retry]` 表（现被 serde 静默忽略，非 `VectorClientConfig`
> 字段，见 config.toml:133-141）。

### 9.4 各 crate feature（文件级）

| crate | 原 | 新 |
|-------|-----|-----|
| graphdb-sync `Cargo.toml:7,23` | `qdrant=["dep:vector-client"]` | `vector=["dep:vector-search"]`；`vector-qdrant=["vector","dep:vector-client"]` |
| graphdb-query `Cargo.toml:29-32` | `qdrant=["dep:vector-client","graphdb-sync/qdrant"]` | `vector=["dep:vector-search","graphdb-sync/vector"]`；`vector-qdrant=["vector"]` |
| graphdb-api `Cargo.toml:20,23-28` | `qdrant=[...]` | `vector=["dep:vector-search","graphdb-config/vector","graphdb-sync/vector","graphdb-query/vector"]`；`vector-qdrant=["vector","dep:vector-client","graphdb-config/vector-qdrant","graphdb-sync/vector-qdrant","graphdb-query/vector-qdrant"]` |
| graphdb-server `Cargo.toml:50,54-59` | `qdrant=[...]` | 同 graphdb-api 模式 |
| graphdb-config | `qdrant` | `vector` + `vector-qdrant`（§9.2） |

所有 `#[cfg(feature = "qdrant")]` 代码门控改为 `#[cfg(feature = "vector")]`
（本地类型/后端）或 `#[cfg(feature = "vector-qdrant")]`（qdrant 专属）：
`graphdb-query/planning.rs:22`、`data_access.rs:6-16`、`vector_operator.rs`、
`graphdb-api/api.rs:18-19`、`api/core.rs:13-14,26-27`、
`graphdb-server/router.rs:223-271` 等。

## 10. server / api 接线

### 10.1 graphdb-api VectorApi（vector_api.rs）

```rust
pub struct VectorApi {
    backend: VectorBackend,                    // 原 vector_manager: Arc<VectorManager>
    coordinator: Option<Arc<VectorSyncCoordinator>>,
}
```

- `create_index`（60-89）：Local 时 `backend.create_collection`（HNSW/量化
  配置本地忽略，仅取 `vector_size`/`distance`）+ coordinator 逻辑索引注册；
  qdrant 保留原 HNSW 配置下发；
- `get_index_info`/`list_indexes`（128-147）：统一从 coordinator 逻辑索引
  元数据读取（补 `coordinator.index_info(...)` 访问器），替代
  `VectorManager.indexes` DashMap 依赖；qdrant 路径物理信息仍可经
  `backend.collection_info`；
- `insert_vector(_batch)`/`delete_vector(_batch)`/`search_with_options`/
  `get_vector`/`count`（149-295）：改走 `backend`。

### 10.2 graphdb-server

- `startup.rs:84-104`：构建 `VectorBackend`：
  - Local → `LocalVectorEngine::open(config.vector_config().local.data_dir)`；
  - Qdrant（`vector-qdrant`）→ `VectorManager::new(config.vector.qdrant)`；
- `startup.rs:136-250`：coordinator 构造改传 `VectorBackend`；EmbeddingService
  构造移入 `#[cfg(feature="vector-qdrant")]`；
- `graph_service.rs:33,130-143,214-263`：`with_shared_vector_manager` →
  `with_shared_vector_backend`，`VectorApi::new(backend)`；
- `handlers/vector.rs` 逻辑不变（类型 import 已切 vector-search）。

### 10.3 graphdb-query

仅类型 import 切换（§3.3），无行为改动；`vector_operator.rs` 经
`crate::sync::VectorSyncCoordinator` 透传，不需感知后端差异。

### 10.4 根 crate

确认 `src/lib.rs`/`src/main.rs` 对 `vector-client` 无直接引用后，根依赖改
optional（§9.1）；`src/lib.rs` 的 `pub use` re-export 若含 vector 类型则改从
`vector-search` 转发。

## 11. 测试计划

### 11.1 vector-search 单元测试（随模块内 `#[cfg(test)]`）

| 模块 | 用例 |
|------|------|
| `distance` | naive vs avx2 逐点一致（随机、dim 非 8 倍数、零向量、cosine 零范数边界）；L2/Dot/Cosine 已知值；`to_score` 单调性 |
| `storage` | 槽位分配/复用/增长（跨 segment）；keys/payloads 往返；tombstone 位图读写 |
| `filter` | 全部 `ConditionType` 变体求值（Match/MatchAny/Range/IsEmpty/IsNull/HasId/Nested/GeoRadius/GeoBoundingBox/ValuesCount/Contains）+ must/must_not/should/min_should 组合 |
| `wal` | 追加→回放往返；双写（重复 txn）幂等；`Compact` 截断后回放 |
| `compaction` | 20% 阈值触发；压缩后槽位重排、reverse 重建、搜索正确；压缩期间读写锁切换 |
| `engine` | 建库/删库/冲突；维度校验；NaN/Inf 拒绝；Manhattan 拒绝；`apply_txn` 批量 |

### 11.2 vector-search 集成测试（tests/）

- `recovery_test.rs`：构造 WAL 已写但内存未应用的状态（测试钩子：apply 前
  panic/中断）→ 重新 `open` → 数据一致；
- `search_test.rs`：过滤 + score_threshold + offset/limit 组合；with_payload/
  with_vector 裁剪。

### 11.3 graphdb-sync 集成（`crates/graphdb-sync/tests/`，feature `vector`）

- coordinator 本地路径全链路：`create_vector_index` → 顶点插入 → 事务提交 →
  `search_with_options`（group_id 注入正确）→ 顶点删除（tombstone）→ 压缩后
  再搜索；
- `commit_transaction` 失败语义：apply_txn 报错 → 缓冲保留 → 重试成功。

### 11.4 崩溃恢复（e2e）

- 写入一批 txn 后 kill（drop 引擎不 drop 目录）→ 重启 `open` → 断言与
  崩溃前提交结果一致；
- 图事务与向量 WAL 的交错恢复：图 WAL 重放驱动 coordinator 幂等应用。

### 11.5 回归

- 现有 qdrant 路径：`cargo test --features vector-qdrant` 全绿（vector-client
  转发后自身测试通过）；
- 根 crate：`cargo test --features vector`（默认 feature 无 qdrant 依赖）通过。

## 12. bench 基线（benches/vector_scan_bench.rs）

criterion 基准，先于优化建立基线（总方案 §3.3「用数据决定何时需要 Tier 1」）：

| 基准 | 变量 |
|------|------|
| `scan_latency` | 向量数 1e4 / 1e5 / 1e6，dim=128，三种度量 |
| `scan_throughput` | 同上，bytes/s |
| `simd_vs_naive` | 同输入下 avx2 vs naive 延迟比（不设硬阈值，输出报表） |
| `filter_selectivity` | 命中率 100%/50%/10%/1% 对端到端延迟影响 |
| `upsert_wal` | 单点/批量的 WAL append + 应用吞吐（与图事务吞吐对照） |

- 数据：随机单位方向（cosine/dot）或高斯（L2），伪随机种子固定；
- 运行 `cargo bench -p vector-search`；结果记入
  `docs/vector/` 新基准文档或追加现有 testing-guide（Phase B 决策输入）。

## 13. 实施顺序与验收（PR 级分解）

| # | 工作项 | 主要内容 | 验收 |
|---|--------|----------|------|
| W1 | 类型迁移 | 建 vector-search（types+error+Cargo）；vector-client 转发；全仓 import 切换 | `cargo build --features vector-qdrant` + `cargo test -p vector-client` 全绿 |
| W2 | 存储 | meta/vectors/keys/payloads mmap + 内存装配 + `CollectionStore` | storage 单测绿；`open`/`grow`/往返通过 |
| W3 | WAL + 压缩 | wal 追加/回放/截断；tombstone；20% 压缩 | wal/compaction 单测 + recovery 集成绿 |
| W4 | 距离核 + 过滤 + 搜索 | naive/avx2 核、`to_score`、filter 求值器、Tier 0 管线 | distance/filter/search 单测绿 |
| W5 | 引擎接口 | `LocalVectorEngine` 全 ops + `apply_txn` | engine 单测绿 |
| W6 | sync 集成 | `VectorBackend` 枚举；coordinator 本地分支（commit 同步应用）；feature `vector` | sync 集成测试绿；`vector-qdrant` 编译回归 |
| W7 | 配置接线 | 根/各 crate feature；VectorConfig；config.toml；server/api 后端构造 | 默认 `cargo build`（无 qdrant 依赖）通过；local 启动 e2e 通过 |
| W8 | 测试 + bench | 崩溃恢复 e2e；bench 基线跑通并记录 | recovery e2e 绿；bench 报表入库 |

估时：W1 0.5 天、W2 1 天、W3 1 天、W4 1 天、W5 0.5 天、W6 1 天、
W7 0.5 天、W8 0.5 天，合计约 **6 天**（总方案 Phase A 估 4~5 天上浮，
含类型迁移与接线）。

## 14. 风险与边界

### 14.1 跨 collection 事务原子性

wal.bin 为每 collection 独立文件（总方案 §3.2 布局），单个事务跨多个
collection 时无法单文件原子。缓解：各 collection 按字典序追加保证推进顺序
确定性；回放幂等；graph WAL 重放兜底收敛。Phase A 接受该边界并在
`docs/vector/` 文档标注；如后续实测不可接受，可评估根级全局 wal.bin
（偏离总方案布局，需单独立项）。

### 14.2 读无锁的范围

mmap 快照（ArcSwap）实现搜索路径无锁；`reverse` 哈希表（get/delete 用）
持读锁。压缩/增长期间写锁阻塞搜索（单机可接受，见 §4.7）。

### 14.3 embedding 本地路径不可用

按总方案 §4.3「建议留 vector-client」，本地路径（默认 feature）不包含
embedding；`embed_text` 返回错误。若 Phase B/C 需要本地 embedding，再评估
将 `EmbeddingService` 迁出 vector-client（无 qdrant 依赖的纯 HTTP 调用）。

### 14.4 Manhattan / payload 索引 / 高级接口

本地引擎 Phase A 不支持：Manhattan（保留枚举）、`create_payload_index` 族
（返回 `NotSupported`）、`scroll`/`search_batch`（coordinator 面组装，不属
引擎职责）。均在文档标注、Phase C 收尾。

### 14.5 SIMD 正确性

avx2 与 naive 双实现 + 逐点断言（§5.3），杜绝自动向量化回归（总方案 §8
第一行风险项）。

### 14.6 大索引启动加载

mmap 惰性映射 + 分段加载；WAL 回放按 txn 组顺序执行、upsert/delete 幂等，
回放耗时与未压缩体积成正比，随压缩检查点截断收敛（总方案 §8「大索引启动
加载时间」风险）。