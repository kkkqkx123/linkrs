# 存储、索引与同步架构剩余任务

> 更新日期：2026-07-16

## 已完成回顾

- **Section 3.x**: Ordered key codec + prefix bounds; MVCC cursor entity_version 集成; entity_ref 存储; cursor ManifestHandle; 多路径交叉验证
- **Section 4.1**: `GenerationState` 状态机 (`Building → CatchingUp → Publishing → Active → Failed/Cancelled`) + `GenerationBuildState` + 强类型 (IndexGeneration, ManifestEpoch, SnapshotTimestamp, CommitLsn) + 持久化存储 + crash recovery 基础 (`resolve_crash_recovery`)
- **Section 4.2**: 从固定 MVCC snapshot 构建新 generation + 增量 catch-up merge + barrier LSN 建立 + fsync + manifest store/rename 原子发布 + `merge_rebuilt_partition()` 替代旧"清空 B-tree 后重建"路径；fault injection points 框架 (`GenerationFaultPoint`)
- **Section 5.1**: `ManifestCatalog`/`ManifestHandle`/`IndexShard`/`IndexManifest` 全部定义 + `register_native_index()` 创建单 shard manifest + cursor 侧 `ManifestHandle::manifest().scan_ranges()` 筛选相交 shard + `take_reclaimable_files()` 通过 Arc strong_count fence + 组合 checkpoint manifest 已纳入 `publish_checkpoint_manifest()`
- **Section 5.2**: `split_native_index()` 分裂 manifest 元数据 + 验证连续性
- **Section 6.1**: 旧 JSON outbox 已删除，全部使用 postcard 序列化；DML 写操作（vertex/edge upsert/delete）走 WAL intent → SQLite materialize → claim → target 闭环
- **Section 6.2**: `index_frontier`/`commit_targets`/`target_state` 两级 frontier 表 + `skip_event_degraded()` 持久化 degraded range；`FulltextReceiver` 通过 tantivy `commit_with_payload` 持久化 mutation 和 receipts；`VectorReceiver::check_late_arrival()` 单调 LSN 拒绝；circuit breaker 和 batch processor 超时机制；dead-letter 表 + `requeue_dead_letter()`
- **Section 6.3**: `CheckpointManifest`/`CheckpointManifestManager` 定义 + `publish_checkpoint_manifest()` 原子引用三个组件 + WAL truncate 使用 `latest_safe_lsn()` + `load_latest()` 跳过 checksum 损坏的 manifest
- **Section 7.3**: postcard index key 已改用 `OrderedCodec`；字符串 predicate/partition 已清理；旧 rebuild 实现已删除；`stale_skipped()` 仅作诊断计数器
- **指标基础**: `OutboxStats`/`CircuitBreakerStats`/`DeadLetterQueueStats`/`ManifestCatalogStats` 定义

## 剩余任务（按优先级排列）

### P0 — 架构核心缺口

#### 0.1 DDL 接入 outbox 闭环
DDL (create/drop index) 直接操作 metadata + index data manager，不产生 WAL OutboxIntent，不进入 SQLite outbox 表。需要：
- [x] DDL 操作写入 WAL (CreateTagIndex/DropIndex → schema_writer → append_schema_redo)
- [x] WAL recovery (replay_create_tag_index/replay_drop_tag_index 实现)
- [x] OutboxPayload::CreateIndex/DropIndex 消费端清理（delivery no-op，不再 warning）
- [x] DDL 事件生产端接入（`SyncWrapper` 在 create/drop tag/edge index 成功后、提交前 stage outbox intent；带 operation context 的 schema redo 同样进入 `staged_wal`，并在 `commit_staged_writes` 中与 intent 作为同一 WAL transaction batch 追加）

#### 0.2 Writer 按 manifest shard 路由
原 `VertexIndexManager`/`EdgeIndexManager` 的单一全局 BTreeMap 已不再是运行时权威数据：
- [x] vertex 写入按 active manifest 的 `route_key()` 定位 `ShardRuntime`
- [x] edge 写入按 active manifest 的 `route_key()` 定位 `ShardRuntime`
- [x] 每个 index 的 generation 拥有独立 shard maps、版本计数器和 checkpoint 文件；flush/load 均以 active shard 为单位

#### 0.3 Split 实际数据重组
`split_native_index()` 仅分裂 manifest 元数据，不实际拆分数据。多个 shard 的 manifest 查询结果不正确（cursor 扫描全 BTreeMap，不受 shard range 限制）：
- [x] split 时按 boundary 物理拆分当前 index 的 BTreeMap 数据到独立 shard 文件（按 index 类型、当前 shard 范围和 entity reverse record 过滤）
- [ ] 复用 rebuild 的 snapshot/catch-up/barrier/publish fence 协议

### P1 — 核心功能缺口

#### 1.1 新旧 generation 共存窗口
- [x] publish fence 内安装新 generation runtime 并发布 manifest；fence 后 writer 自动路由 active generation
- [x] cursor 在同一 read fence 内取得 ManifestHandle 与 generation snapshot，旧 reader 继续读取旧数据
- [x] `take_reclaimable_manifests()` 在最后一个 handle 释放后同时回收 retired generation runtime 和 checkpoint 文件

#### 1.2 VectorReceiver 无持久化
`VectorReceiver` 的 idempotency_keys 和 applied_lsn 原先为纯内存，重启后丢失：
- [ ] 原子持久化 mutation + idempotency receipt（receiver 状态和 receipt 已原子落盘并在重启时恢复；尚未与实际 vector mutation 形成同一可证明持久协议）

#### 1.3 多 shard cursor 全局有序 merge
- [x] cursor 创建时对固定 generation 的各 shard forward snapshot 做 ordered merge，生成单一 immutable scan snapshot
- [x] offset/limit 在 merge 输出层应用；cursor 持有与 snapshot 相同的 ManifestHandle

#### 1.4 PartitionSelector 未接入查询
query 层尚未把 `PartitionSpec` 的物理分区传递给 storage；当前 predicate 仅尝试派生 KeyRange，不能替代完整 ordered index key 的 partition bridge：
- [ ] `PartitionView` → `PartitionSelector::KeyRange` 转换桥接
- [ ] index scan 只打开相交 shard

#### 1.5 minimum_lsn 等待接口
- [x] 实现一致性等待接口，调用方可以等待 index frontier >= 目标 LSN；degraded skip 返回一致性错误

#### 1.6 SQLite 丢失后自动重建
- [x] 从最近有效 outbox snapshot 恢复 SQLite；outbox 路径统一为 `outbox/outbox.sqlite`，snapshot 统一在工作目录 `outbox_snapshots/`
- [x] 回放剩余 committed WAL intents（以 SQLite 实际 `materialized_lsn` 为下界，避免恢复失败时错误跳过 WAL）
- [ ] 演练：SQLite 丢失 + 早期 WAL 已回收

### P2 — 测试与健壮性

#### 2.1 并发写 + rebuild crash 测试
- [ ] build 期间持续并发写入，验证 start_lsn 后更新不丢不重
- [ ] 覆盖 vertex、edge、included columns 和删除操作
- [ ] Publishing 状态 crash 恢复 manifest 完整性验证

#### 2.2 Checkpoint crash point 测试
- [ ] 每个 checkpoint crash point（redo 前、intent 中间、commit 中间、fsync 后、visibility publish 前）
- [ ] checkpoint checksum 损坏时回退到上一有效 checkpoint

#### 2.3 Transport timeout/乱序/barrier 集成测试
- [ ] timeout 后晚到、乱序 ack、frontier hole、generation barrier、target 隔离
- [ ] split 前后 equality/range/prefix/covering/全扫描一致性
- [ ] 长读持有旧 manifest 时文件不回收

### P3 — 阶段 7 收尾

#### 3.1 指标与诊断
- [ ] target/index frontier lag、backlog、oldest event、retry、dead-letter、degraded skip
- [ ] generation lifecycle、rebuild/split replay lag、publish latency
- [ ] transport latency、materializer lag、snapshot/checkpoint LSN

#### 3.2 运维文档
- [ ] WAL/outbox snapshot 恢复、checksum 损坏回退
- [ ] dead-letter 修复、requeue、skip、degraded consistency
- [ ] rebuild/split 状态诊断、失败恢复、旧 generation 回收

#### 3.3 旧路径清理
- [ ] 迁移 adapter、废弃配置、旧 fixture、过时 benchmark
- [ ] PersistentOutbox 生产依赖、direct sync

## 最终验收

```shell
cargo fmt --check
cargo clippy --all-targets --all-features
cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant
cargo test --lib -- --nocapture
cargo test --test '*' -- --nocapture
```

## 完成定义

1. DDL 和 DML 统一走 WAL → outbox → target 闭环
2. writer 按 manifest shard 路由；多 shard cursor 全局有序 merge
3. rebuild/split 使用同一持久增量追平与 publish fence 协议，新旧 generation 共存窗口正确
4. 所有 secondary index（fulltext、vector、native）持久化且可恢复
5. checkpoint 原子引用所有组件，checksum 损坏可回退，WAL 按 safe LSN 清理
6. 所有禁止路径已删除，无双运行实现
7. 最终验收矩阵全部通过，保存至少一次 SQLite 恢复演练和一次并发 split/recovery 演练
