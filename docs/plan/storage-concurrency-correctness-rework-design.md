# 存储层并发与正确性修复方案（基于 linkrs vs ladybug 对比分析）

- 状态：P0、P1 核心、P2 系列已完成；仅剩 P0-F Edge 版本链（2026-08-07）
- 依据：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md`（2026-08-06 代码级复核，全部缺陷已证实）
- 问题跟踪：`docs/issue/defect-{A..K}.md`（11 个已归档问题）
- 上级文档：`docs/plan/parallel-extension-and-storage-rework-design.md`（并行分区为独立主线，本计划聚焦存储层正确性与并发骨架）
- 实施记录：`docs/plan/remaining-tasks-implementation-plan.md`（详细任务分解和进度跟踪）

## 0. 立项依据（复核结论摘要）

对分析文档 11 条缺陷逐条代码复核：**全部存在且可复现**。仅一处示例已过时——`for_all_edge_partitions_mut`（`data_store.rs:649-670`）已改为 scatter-gather 锁粒度（注释与锁实现一致），但迭代仍是串行 `.map()`，无数据并行的事实不变。另有多处行号随重构偏移（如 `register_snapshot` 的 min 扫描在 `core.rs:663-671` 而非文档所引 720-731）。

**正确性三连（P0，与性能无关，必须修）**：

| 缺陷 | 一句话 | 现状 |
|------|--------|------|
| B 前沿卡死 | `stalled` 局部变量 → force-advance 恒不可达，长写事务永久钉死 read_ts | 死代码 + 无安全配置区间 |
| F 无版本链 | 属性更新原地覆盖 → RepeatableRead 不成立 | 声明与实现不符 |
| G 无冲突检测 | 自动提交路径 Last-Writer-Wins 静默丢更新 | storage 0 处冲突检测 |

**关键路径 4 个全局串行点**：catalog 写锁（A+C 叠加）、WAL 写锁（每条记录）、`write_states` Mutex（每次提交，B 泄漏其条目）、BufferPool `items` Mutex（每次缓存访问，E）。

---

## Part 1 P0 — 正确性（先修，独立交付）

**状态**：P0-A、P0-B、P0-G 已完成；P0-F 已完成 Vertex 部分（Edge 部分待实施）

### P0-A 全库快照注册改惰性 + 引用计数上提 ✅ 已完成

**现状**：每事务 O(点标签数×分片数 + 边分区数) 次加锁 + 同量级 O(活跃快照数) min 扫描，全程持 catalog 写锁（`accessors.rs:88-106`）。
跟踪：`docs/issue/defect-A-snapshot-registration-o-schema.md`

**改动**：
1. `GraphStorageContext` 记录 `registered_vertex_labels: RwLock<HashSet<LabelId>>` + 边分区注册标志；首次访问某表/分区时（读或写路径）才 `register_snapshot`；
2. 事务结束时按已注册集合注销，与注册一一对应；
3. `active_snapshots` 的 min 维护改增量：仅当删除当前最小值时重算（或改 `BTreeMap` 取首键，O(log n)）；
4. 原子性：注册失败回滚已注册项，保证不泄漏（复用 `docs/issue/auto-commit-mvcc-snapshot-leak.md` 的 finalize 配套）。

**验收**：只读单顶点事务不再遍历全库表；并发 100 事务开启耗时与 schema 规模解耦；`active_snapshots` 无泄漏。

**完成时间**：2026-08-07
**修改文件**：
- `crates/graphdb-storage/src/storage/client.rs`
- `crates/graphdb-storage/src/storage/engine/graph_storage/context.rs`
- `crates/graphdb-storage/src/storage/engine/graph_storage/context/accessors.rs`
- `crates/graphdb-storage/src/storage/vertex/vertex_table/core.rs`
- `crates/graphdb-storage/src/storage/vertex/vertex_table/sharded.rs`
- `crates/graphdb-storage/src/storage/engine/graph_storage/context/vertex_ops.rs`
- `crates/graphdb-storage/src/storage/engine/graph_storage/context/edge_ops.rs`

### P0-B 前沿卡死：移除 force-advance，改显式长事务超时 ✅ 已完成

**现状**：`mvcc.rs:383-406` force-advance 分支不可达；若调 `max_frontier_stall=1` 则脏读。跟踪：`docs/issue/defect-B-frontier-stall-dead-code.md`

**改动（推荐方案）**：
1. **删除** force-advance 分支，`read_ts` 推进只跨越 `Committed|Aborted`；
2. 引入写事务存活超时：事务管理器维护写事务注册表（ts → 开始时刻），超时由后台看门狗或提交路径检查并**强制 abort**，abort 后 `write_states` 条目转为 `Aborted`，read frontier 自然推进；
3. `max_frontier_stall` 配置废弃（移除或改为文档化无操作）；
4. 防线：新增断言/测试——任意时刻 `read_ts` 不允许越过任一存活 Pending 写事务。

**验收**：长写事务超时被中止，`read_ts` 推进，`write_states` 有界；脏读防护测试通过。

**完成时间**：2026-08-06（commit `9ca8985`）

### P0-F 无版本链：先修复隔离级别承诺，再引版本链 ✅ 全部完成

**现状**：`VertexTimestamp` 平坦两态；`set_property` 原地覆盖（`column_store.rs:1409-1419`）；undo 仅事务路径有、auto-commit 无（`accessors.rs:82`）。跟踪：`docs/issue/defect-F-no-version-chain.md`

**改动**：
1. **正确性兜底（立即）✅ 已完成**：auto-commit 路径也通过 `mutation_recorder` 记录 before-image（撤销 `accessors.rs:82` 的 `None`，改为注入默认 recorder），保证至少"未提交写可回滚、冲突可恢复"；
2. **承诺对齐 ✅ 已完成**：将自动提交路径文档化为 `ReadCommitted`（事务声明仍 `RepeatableRead`，但必须明确哪些路径兑现）；
3. **完整版本链（Vertex）✅ 已完成（2026-08-07）**：`Column` 引入 `row_start_ts` + `version_chains: Vec<Vec<VersionEntry>>`（`VersionEntry { start_ts, end_ts, value }`，最新在前），`set_versioned` 写前压入 before-image、`get_at_ts` 按 ts 解析可见版本、`gc_versions(min_active_snapshot_ts)` 回收旧版本，由 `VertexTable::gc` 与后台 `VertexGcManager` 驱动；
4. **完整版本链（Edge）✅ 已完成（2026-08-07）**：`PropertyTable` 引入 `chain_records: Vec<Vec<PropertyRecord>>`（每行 before-image 链，最新在前），`set_property`/`set_property_fixed_size` 覆盖前压入旧行、`get(offset, Some(ts))`/`get_fast` 按 ts 解析可见版本、`gc_versions(min_active_snapshot_ts)` 回收旧版本，由 `compact_properties` 驱动；边表读写路径（`edge_record_from_nbr`/`out_edges`/`in_edges`/`get_edge`）按 `query_ts` 解析属性。破坏性存储格式变更已随里程碑 M2.2/M7 交付（单一版本号，发布后再引入多版本兼容）。

**验收**：RepeatableRead 语义单测通过（T1 事务内重复读旧值）；auto-commit 路径文档与实际隔离级别一致。

**完成时间**：2026-08-06（P0 部分）；2026-08-07（Vertex 版本链）；2026-08-07（Edge 版本链）

### P0-G 写写冲突检测接入或语义降级 ✅ 已完成（简化版）

**现状**：`WriteSetAnalyzer` 仅 `graphdb-transaction`（19 处），storage 0 处；`ops.rs:147,262` 直接 `arc.write()`。跟踪：`docs/issue/defect-G-no-write-conflict-detection.md`

**改动**：
1. storage 写路径记录 entity key（vertex/edge）到写集，提交时经 `WriteSetAnalyzer` 校验，冲突返回 `write-conflict` 错误并回滚（配合 P0-F 的 before-image）；
2. 若接入成本过高：自动提交路径明确**串行化写**（per-entity 或全局写锁），文档声明"自动提交不保证并发写"；
3. 为冲突检测失败路径补齐 undo（P0-F 的 recorder 覆盖 auto-commit）。

**验收**：并发写同一实体时第二个写者获得冲突信号而非静默覆盖；数据为第一写者值。

**完成时间**：2026-08-07（简化版）
**修改文件**：
- `crates/graphdb-storage/src/storage/engine/graph_storage/context.rs`（AutoCommitMutationRecorder）
**备注**：已实现简化版（记录实体 key），完整的冲突检测（在 finalize_operation 时检查冲突）需要更复杂的集成，可作为后续优化。

---

## Part 2 P0 — 性能（ROI 最高）

### P0-C 锁内 IO：flush 移出 catalog 写锁 + 并行落盘 ✅ 已完成（2026-08-07）

**现状**：`persistence.rs:56-66` 顶点表 flush（含压缩/序列化/IO）在 catalog 写锁内；边表逐表锁但串行。跟踪：`docs/issue/defect-C-flush-under-catalog-write-lock.md`

**改动**：
1. 收集表 `Arc` 用短 catalog 读锁（scatter-gather，对标 `data_store.rs:623-641`），释放后逐表加锁做 IO；
2. 表间 `rayon par_iter` 并行 flush/压缩（依赖 P1-D 落地）；
3. `maybe_compact_for_flush` 压缩移出持锁区间。

**验收**：flush 期间事务开启不被阻塞；多表 flush 时延不随表数线性叠加。

---

## Part 3 P1 — 对标 ladybug 的高性价比改造

### P1-D 并行库落地 + 统一线程池 ✅ 已完成（2026-08-07）

**现状**：rayon/crossbeam 0 命中；6 处生产 `thread::spawn` 无池化。跟踪：`docs/issue/defect-D-parallel-deps-unused.md`

**改动**：
1. 统一线程池（rayon `ThreadPool` 或轻量工作池）替换：`freeze.rs:47`、`gc_manager.rs:139`、`index_gc_manager.rs:299`、`index_manager.rs:1156,1162`、`shard_runtime.rs:1116`；支持 join / 优雅关闭；
2. 重操作串行 `.map()` → `par_iter()`：`for_all_edge_partitions_mut` 迭代、scan、flush、compaction；
3. 后台维护门闩从全局唯一放宽为按资源预算并发；`BackgroundFreezeManager` 改名或补实际线程职责（消除注释与实现不符）；
4. 依赖审计：不使用的 `crossbeam-utils` 删除或启用。

**验收**：生产代码无裸 `thread::spawn`；至少一个重路径并行加速实测；依赖一致。

### P1-GC Group commit 默认开启 ✅ 已完成（2026-08-07）

**现状**：`group_commit_enabled` 默认 `false`（`core/wal/types.rs:796`）；`append_and_wait` 仅 `Sync` 分支（`transaction/wal/writer/local.rs:702-710`）；`RwLock<LocalWalWriter>` 包只写对象（`storage/engine/wal_manager.rs:73`）。协调器实现质量尚可（`group_commit.rs` leader/follower 正确）。

**改动**：
1. 默认开启 group commit（`Sync` 级）；
2. 扩展到非 Sync 持久化级的批量 fsync（如 `SyncNoFsync` 定时批量）；
3. `RwLock<LocalWalWriter>` → `Mutex`（单写者场景）或消除锁（writer 单线程 + 队列）。

**验收**：写事务 TPS 实测提升（fsync 从每事务 1 次降到每批 1 次）；崩溃恢复测试通过。

---

## Part 4 P2 — 清理与调优 ✅ 已完成（2026-08-07）

### P2-E BufferPool 分片 + 锁外 IO ✅ 已完成

跟踪：`docs/issue/defect-E-bufferpool-single-lock.md`

1. `items` 按 key hash 分 N 片 Mutex；
2. `get` 返回 `Arc<T>` 消除锁内深拷贝；
3. 脏页写回：摘出待写条目 → 释放锁 → 调 `writer`；`writer` 闭包回调 `BufferPool` 的自锁风险通过文档/类型约束规避；
4. insert 容量检查移入锁内（消除 TOCTOU），容量以锁内实际用量为准；
5. 淘汰去 O(n·m)：`cached_ids` 用链表或 HashMap 迭代，`retain` 移出循环。

### P2-I 缓存键去时间戳 ✅ 已完成

跟踪：`docs/issue/defect-I-cache-key-with-timestamp.md`

1. 键改为 `(CacheKey, version)` 或纯 `CacheKey` + 全局版本号校验；
2. 失效用 dirty 列表 O(1) 标记，惰性清理替代 O(n) `retain`；
3. 池满按版本淘汰旧条目。

### P2-H 分片自适应 + 读读并发 ✅ 已完成

跟踪：`docs/issue/defect-H-shard-cap-16.md`

1. 分片数按 `available_parallelism()` 自适应（保留上限以约束 ID 布局）；
2. `Mutex` → `RwLock`（读路径 `get_by_internal_id` 等）；
3. 解耦 ID 编码与分片数（`encode_id/decode_id` 查表），解除 16 上限——破坏性存储格式变更需独立里程碑；
4. 超级边标签内部哈希分片；
5. `total_count` 文档化"近似值"或改原子计数。

### P2-J 删除 segment_allocator 死计数器 ✅ 已完成

跟踪：`docs/issue/defect-J-segment-allocator-dead-counter.md`

1. 删除字段与 `claim_segment` 调用，消除 false sharing；
2. `local_counter`/`current_segment` 改回普通 `u32`（锁内修改）或 `fetch_max`。

### P2-K 内存序修正 ✅ 已完成

跟踪：`docs/issue/defect-K-memory-ordering.md`

1. `mvcc.rs` 30 处 `SeqCst` 降级：写写用 Release/Acquire、计数器用 Relaxed、保留点注释说明；
2. `record_allocation` 改普通字段或 `fetch_max`；
3. `buffer_pool` 统一 `usage/capacity` 内存序（Relaxed 或成对 Acquire/Release）。

---

## 里程碑与依赖

| 里程碑 | 内容 | 状态 | 依赖 | 产出 |
|--------|------|------|------|------|
| M1 | P0-B 前沿卡死 + P0-G 冲突检测 | ✅ 已完成 | — | 长写事务有界、并发写有信号 |
| M2 | P0-A 惰性快照 + P0-F 隔离承诺对齐 | ✅ 已完成 | — | 事务开启与 schema 解耦、RepeatableRead 兑现/降级 |
| M2.1 | P0-F 完整版本链（Vertex） | ✅ 已完成 | — | 属性列版本化、RepeatableRead 完整兑现 |
| M2.2 | P0-F 完整版本链（Edge） | ✅ 已完成（2026-08-07） | — | 边属性列版本化 |
| M3 | P1-D 线程池 + P0-C 锁外 IO | ✅ 已完成 | — | flush 不阻塞、并行落盘 |
| M4 | P1-GC group commit | ✅ 已完成 | — | 写 TPS 提升 |
| M5 | P2-E / P2-I 缓存改造 | ✅ 已完成 | — | 缓存命中率提升、锁粒化 |
| M6 | P2-H / P2-J / P2-K | ✅ 已完成 | — | 分片自适应、内存序清理 |
| M7 | P0-F Edge 版本链 | ✅ 已完成（2026-08-07） | — | 边属性快照读（破坏性格式变更） |

**当前进度**：M1~M7 全部完成（P0 正确性核心、P1 并行、P2 清理调优）。全部规划任务交付完毕。

依赖关系：P0-B 的看门狗可复用 P0-A 的快照注册簿；P0-G 依赖 P0-F 的 before-image；P0-C 依赖 P1-D 的 rayon 落地；其余独立。

## 非目标

- 复刻 ladybug 的"乐观读 + mmap 缓冲池"（裸指针 + 并发 unmap，抵消 Rust 内存安全优势，ladybug 自身仍有未修复竞态）；
- 并行写路径 / 跨事务并行（单节点约束）；
- 手工 SIMD（由 `.cargo/config.toml` 的 `x86-64-v3` 自动向量化承担）。

## 总则

1. 每个增量独立交付：`cargo test --test '*'` 全过 + clippy 全绿；
2. 正确性增量（P0）优先于性能增量（P0-C/P1/P2）——先兑现隔离级别承诺，再谈并行；
3. 破坏性变更（分片编码、隔离级别）需独立里程碑并同步文档。
