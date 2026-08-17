# 内置向量引擎设计方案（自研 pgvector 风格）

> 状态：设计方案（2026-08-17）。
>
> 前置/关联：
> - `docs/archive/local-vector-engine.md`（旧 arroy 方案，被本方案取代）
> - `docs/vector/vector-engine-design.md`（现有 VectorEngine trait 设计）
> - `docs/vector/implementation-checklist.md`（实现检查清单）
>
> 本方案回答三个问题：是否需要内置向量引擎（需要）；是否引入 hnsw-rs/hnswlib
> （否，价值有限）；内置路径是否绕过 vector-client（是，直接集成并大幅简化
> 中间层）。

## 0. 结论摘要

| 项 | 决策 |
|----|------|
| 引入 hnsw-rs / hnswlib / arroy | **否**。hnsw-rs 无任何删除接口；hnswlib-rs 仅 tombstone 删除（内存不回收、recall 下降、需全量重建）；三者均为内存存储 + 全量快照持久化，与项目 mmap/WAL 模式冲突 |
| 实现方式 | **自研**，取 pgvector 设计思想（索引是存储引擎上的二级结构、删除走正常行删除路径、无索引时精确扫描），结合项目特点调整 |
| 索引层级 | Tier 0 精确扫描（SIMD flat scan，默认）→ Tier 1 IVFFlat（数据量增长后）→ HNSW 仅在有明确基准需求时追加 |
| 中间层 | **内置路径绕过 vector-client**，直接集成进 graphdb-sync；vector-client 降级为可选的 Qdrant 外部适配（feature 门控） |
| 新 crate | `crates/vector-search`（叶子 crate，仿 graphdb-search 定位，不依赖任何 graphdb crate） |
| 同步简化 | 内置路径删除 retry/DLQ/circuit breaker/outbox 机制；事务缓冲改为"提交时本地 WAL 回放"，向量写与图事务原子一致 |
| 类型归属 | `SearchQuery`/`VectorFilter`/`CollectionConfig` 等类型迁至 vector-search，vector-client 改为 `pub use` 转发（保持 qdrant 路径兼容） |

## 1. 背景与动机

1. **定位矛盾**：项目定位"lightweight single-node、聚焦本地部署"，当前向量功能
   强依赖外部 Qdrant 服务，违背定位。
2. **不可测试**：现有测试全部使用 `VectorClientConfig::disabled()`，无外部
   Qdrant 时向量功能完全无法验证，CI 亦无法覆盖。
3. **生态不对称**：全文检索已有内置 BM25（graphdb-search），向量是唯一缺失的
   内置索引类型。
4. **删除语义**：向量挂接在图顶点/边上，图数据库的边增删是核心高频操作。
   HNSW 类索引对删除的脆弱性（见 §2）使其不适合作为图库向量索引的主载体。

### 1.1 现状链路

```
查询 → vector_operator（graphdb-query）
     → VectorSyncCoordinator（graphdb-sync，事务缓冲/补偿）
     → VectorManager（vector-client，索引生命周期）
     → VectorEngine trait → QdrantGrpcEngine → 网络 → Qdrant
```

同步侧为支撑外部服务可靠性，引入了 outbox / dead_letter_queue / circuit_breaker /
retry / sqlite_outbox 等机制（crates/graphdb-sync/src/sync/）。这些机制的存在
**仅因为 Qdrant 是远程、易失败的**——内置路径下全部不再需要。

## 2. 现成库评估

| 维度 | hnsw-rs | hnswlib-rs(wilsonzlin) | arroy | 自研 |
|------|---------|------------------------|-------|------|
| 删除 | **无删除接口**（README/API 均无） | tombstone，NodeId 不复用、内存不回收 | 删除需走 LMDB 事务，图拓扑不变 | tombstone + 定期压缩，平凡 |
| 持久化 | 全量 bincode dump/reload | 全量 bincode save/load | LMDB 增量（较好） | mmap 文件 + 追加式 WAL |
| 过滤 | FilterT 搜索期过滤（好） | 仅 ID/谓词过滤 | 无 | post-filter（Tier 1 可 per-list 预过滤） |
| 事务 | 无 | 无 | 有 | 并入图事务 WAL |
| 依赖 | rand0.9/edition2024 等重树 | 较新生态小 | heed/LMDB 重依赖 | 无新增重依赖 |
| 周边工作量 | 过滤/payload/持久化/并发仍需自写 | 同左 | 同左 | 全部自有 |

**结论**：HNSW 图本身只占向量引擎工作量的约 1/5，围绕索引的周边（存储、过滤、
删除、持久化、事务、并发）无论如何都要自己实现；而删库最核心的删除语义恰好是
这些库最薄弱处。引入价值有限。

## 3. 目标设计

### 3.1 crate 布局与依赖

```
crates/vector-search（新，叶子 crate）
  ├─ 不依赖任何 graphdb crate（仿 vector-client 现状）
  ├─ 依赖：memmap2、postcard、serde_json、rayon、parking_lot、tracing（均已在 workspace）
  └─ 提供：向量索引存储、SIMD 距离核、过滤、持久化、WAL 回放

graphdb-sync ──内置路径──→ vector-search（直接调用，无中间层）
graphdb-sync ──qdrant 路径──→ vector-client（保留，feature 门控）

graphdb-query / graphdb-api / graphdb-server：类型改从 vector-search 引入
vector-client：pub use vector-search 的类型 + 保留 Qdrant 引擎
```

### 3.2 数据模型与存储布局

每个 collection 一个目录 `<data_dir>/vector/<collection>/`：

| 文件 | 格式 | 说明 |
|------|------|------|
| `meta.bin` | postcard | 维度、距离度量、索引层级配置、Tier 1 聚类中心 |
| `vectors.bin` | mmap 稠密行主序 f32 数组 | 定长槽位（slot = 内部稠密 u32 序号），分 segment 增长 |
| `payloads.bin` | mmap 容器 | slot → postcard(Payload)；含删除位图（tombstone bitmap） |
| `keys.bin` | mmap | slot → PointId（u64/String）双向映射 |
| `wal.bin` | 追加式 | 向量操作日志（txn id + op），崩溃恢复用 |

- PointId → 内部 slot 映射为稠密 u32，热路径避免哈希；
- slot 删除 = 位图标记，触发阈值（如 20% tombstone）后压缩（memcpy 存活行）。
- 对齐项目现有存储风格（graphdb-storage 的 mmap 容器），不引入新存储范式。

### 3.3 索引层级

| Tier | 算法 | 适用规模（128 维估） | 删除 | 备注 |
|------|------|---------------------|------|------|
| 0 | 精确扫描（默认） | ≤ 10^5~10^6 点 | 平凡 | AVX2 距离核 + rayon 并行；精确、无 recall 问题 |
| 1 | IVFFlat | 10^6+ 点 | 平凡（从 list 移除） | k-means 采样训练；漂移超阈值（约 10%）重建，构建比 HNSW 快 4~32 倍 |
| 2 | HNSW（预留） | 需要时追加 | tombstone + 定期重建 | 仅当基准显示 Tier 1 延迟不达标 |

- **Tier 0 是默认与基线**：项目已有 `-C target-cpu=x86-64-v3`（AVX2），本地
  单机场景 10 万级向量毫秒级响应，精确结果，与 pgvector 无索引 seq scan 同思路。
- Tier 1 的 k-means 在采样子集上训练（pgvector 同款思路），列表分配随插入
  逐个进行，无需整库重扫。
- 建 bench 基线（`benches/vector_scan_bench.rs`）先行，用数据决定何时需要 Tier 1/2。

### 3.4 删除与压缩

- 删除 = tombstone 位图标记 + 从 Tier 1 list 中移除（O(1)）；
- 压缩（compaction）：tombstone 比例超阈值或定时触发，遍历存活 slot 重写
  vectors/payloads/keys，并触发 Tier 1 重建；
- 无图结构损坏问题（区别于 HNSW），无需 REINDEX 语义。

### 3.5 过滤

- Tier 0：候选集过滤（对 top-K 超采后按 `VectorFilter` 后过滤，Qdrant 同款
  post-filter 语义）；`score_threshold`/`offset`/`limit` 同样后处理；
- Tier 1：可做 per-list 预过滤（按需，第一版不做）；
- `VectorFilter` 类型完整保留（must/must_not/should/min_should + 各条件变体），
  从 vector-client 迁入 vector-search。

### 3.6 持久化与事务（关键简化）

目标：向量写与图事务提交原子一致。

```
现状：事务缓冲 → 提交后异步补偿（网络、重试、DLQ…）
目标：事务缓冲 → 提交时同步执行：
      1) 追加 wal.bin（txn id + ops，顺序写）
      2) 应用到内存索引（slot 分配 + mmap 写）
      崩溃恢复：启动时回放 wal.bin（幂等，op 带 txn id）
      回滚：丢弃缓冲（未写任何内容）
```

- 无网络、无重试、无 DLQ、无 circuit breaker：本地失败即为内部错误；
- `VectorSyncCoordinator` 的缓冲结构复用，`commit_transaction` 变为本地同步路径
  （vector_sync.rs:783 现有接口不变，实现替换）。

### 3.7 并发模型

- 每个 collection 一把 `parking_lot::RwLock`（写串行化，读并发）；
- mmap 读路径无锁（slot 位图/压缩期间用读写锁切换）；
- 内置路径为同步实现，`async` 仅在 coordinator 表面保留（qdrant 路径兼容），
  无需 `async_trait` 与 `Arc<dyn VectorEngine>`。

## 4. 中间层简化

### 4.1 删除/降级的组件（内置路径）

| 组件 | 处置 |
|------|------|
| `VectorManager`（vector-client） | 内置路径不再使用；qdrant 路径保留 |
| `VectorEngine` trait + `DisabledEngine` + `create_engine` 分支 | 内置路径删除；qdrant 路径保留 |
| `VectorClientConfig.engine` 枚举 | 内置路径配置移入 graphdb-config 直接持有；vector-client 配置仅供 qdrant |
| 同步机制（outbox/DLQ/circuit breaker/retry/sqlite_outbox） | 内置路径不接线；机制本身保留（可能服务其他 delivery target） |
| `async_trait` / `Arc<dyn VectorEngine>` | 内置路径为具体类型直接调用 |
| `vector_error.rs` 的 `From<VectorClientError>` 映射 | 内置路径改为 vector-search 错误类型映射 |

### 4.2 目标链路

```
查询 → vector_operator → VectorSyncCoordinator → vector-search（本地同步）
     └─ qdrant 路径（feature 门控）→ vector-client → Qdrant
```

### 4.3 类型与错误迁移

- `SearchQuery`/`VectorFilter`/`VectorPoint`/`CollectionConfig`/`DistanceMetric`/
  `SearchResult`/`PointId`/`Payload` 迁入 vector-search；
- vector-client `pub use vector_search::...` 转发，`graphdb-query` 的
  vector_planner、graphdb-api 的 vector_api.rs 同步切换引用（改动小）；
- embedding 服务（EmbeddingService）留在 vector-client 或一并迁移，二选一，
  建议留 vector-client（其本质是外部 HTTP 调用，与 qdrant 同属"外部适配"）。

## 5. 配置与 feature

```toml
# Cargo.toml（root）
[features]
default = ["server"]
vector = ["graphdb-api/vector", "graphdb-server/vector", "graphdb-config/vector",
          "graphdb-sync/vector", "graphdb-query/vector"]
vector-qdrant = ["vector", "graphdb-api/qdrant", "graphdb-server/qdrant",
                 "graphdb-config/qdrant", "graphdb-sync/qdrant", "graphdb-query/qdrant"]
```

- `vector`（默认开启）：内置引擎，无重依赖（不拉 tonic/prost/reqwest）；
- `vector-qdrant`：外部 Qdrant 适配（原 `qdrant` feature 语义迁移至此）；
- `config.toml`：`[vector] engine = "local" | "qdrant"`，新增 `[vector.local]
  data_dir`，默认 `<data_dir>/vector`；
- graphdb-sync 的 `qdrant` feature 拆为 `vector`（本地，必选依赖 vector-search）
  + `vector-qdrant`（可选依赖 vector-client）。

## 6. 实施路线

### Phase A：Tier 0 精确扫描 + 存储 + 集成（先行）

- [ ] `crates/vector-search`：meta/vectors/payloads/keys mmap 存储、tombstone
      位图、压缩、WAL 追加与回放；
- [ ] AVX2 距离核（L2/Cosine/Dot），朴素实现对照测试（防止 SIMD 正确性回归）；
- [ ] `VectorFilter` 后过滤 + score_threshold/offset/limit；
- [ ] graphdb-sync 集成：coordinator 本地路径（提交时同步应用 + WAL）；
- [ ] graphdb-config/config.toml/root feature 改造；类型引用切换；
- [ ] 测试：单元 + 集成 + 删除/压缩/崩溃恢复用例；
- [ ] `benches/vector_scan_bench.rs` 基线。

### Phase B：Tier 1 IVFFlat + 压缩调度

- [ ] 采样 k-means 聚类、list 分配、probe 搜索；
- [ ] 漂移监测与重建调度（10% 规则）；
- [ ] 压缩与重建的并发安全（读写锁切换）；
- [ ] bench：Tier 0 vs Tier 1（延迟/recall/构建时间），据此决定默认层级。

### Phase C：qdrant 路径适配 + 收尾

- [ ] vector-client 类型转发与 feature 重构、错误映射；
- [ ] 文档更新：docs/vector/*、archive 标注本方案取代；
- [ ] CI 覆盖内置路径全链路（不再依赖外部 Qdrant）。

### 预留：HNSW Tier 2

- 仅当 Phase B 基准显示 IVFFlat 延迟不达标时追加；实现为 tombstone + 定期
  重建（对齐 Qdrant 的 vacuum 思路），不自研删除期间的图修复。

## 7. 工作量估算

| 阶段 | 内容 | 估时 |
|------|------|------|
| Phase A | 存储 + SIMD 核 + 过滤 + WAL + 集成 + 测试 | 4~5 天 |
| Phase B | IVF + 压缩调度 + bench | 2~3 天 |
| Phase C | qdrant 适配 + feature/文档/CI | 1~2 天 |
| 合计 | | 7~10 天 |

## 8. 风险与限制

| 风险 | 缓解 |
|------|------|
| SIMD 距离核正确性 | 与朴素实现逐点对照测试（bench 内嵌断言） |
| post-filter 在低选择性过滤下性能差 | 文档写明语义差异；Tier 1 预留 per-list 预过滤 |
| 大索引启动加载时间 | mmap 惰性映射 + 分段加载；WAL 回放按 txn 幂等 |
| Tier 1 聚类漂移导致 recall 下降 | 漂移监测 + 重建调度（pgvector 同款教训） |
| 与图事务的原子性边界 | WAL 回放幂等设计；提交时同步应用消除补偿窗口 |

## 9. 明确不做（本期）

- 分布式/分片；
- 量化（int8/binary）——Tier 2 阶段按需评估；
- HNSW 的在线删除图修复——若实现 HNSW 走 tombstone + 定期重建。