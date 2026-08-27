# 同步系统架构文档

> 修订状态：2026-08-27 对齐 `docs/plan/vector_sync_analysis.md` 与 `docs/plan/vector_sync_improvement_plan.md` 的实现；旧版 2PC 描述已作废，详见 §5

## 概述

GraphDB 的索引同步采用 **图 WAL 单真源 + SQLite Outbox 持久队列 + 异步投递** 的 Transactional Outbox 模式。全图数据（顶点/边/索引 DDL）与向量/全文索引最终一致，无跨存储 2PC/XA。

- 图提交经单一 WAL fence 决定可见性（MVCC `commit_lsn` 立即可读）。
- 索引变更以 `OutboxIntent` 随图 redo 原子落盘（`WAL_SYNC_WIRE_VERSION=1`，`WalOpType=21`），再经 SQLite 投递到 `LocalVectorEngine` 或 `Qdrant` 与 `Tantivy`。
- 真源不弱化：即使 `LocalVectorEngine` 同进程，也不绕过 `WAL+Outbox` 形成双写分支。

## 架构设计原则

1. **单真源**：`Graph WAL` 唯一决定提交；`SQLite outbox` 与向量存储均为 WAL 的派生，可重放重建。
2. **最终一致，可观测**：`frontier_lag / degraded / dead_letter / disabled_skips` 可度量，向量滞后可经 `wait_for_minimum_lsn` 显式等待。
3. **内核统一，策略分支**：`stage → WAL → materialize → claim/ack/frontier` 主链路对 `Local/Qdrant` 统一；`batch / retry / disabled / 一致性超时 / 物理回收` 按后端策略化。
4. **失败分类再重试**：`Retryable / NonRetryable / Auth / Disabled` 显式分类，避免 `DimensionMismatch` 无限重试，`Disabled` 保留不进 `dead_letter`。
5. **最小化复杂度**：子 crate 维持严格 DAG `metrics → core → config → search → sync → transaction → storage → query → api → server`；`VectorBackend` 零成本 `enum` 分发，无 `dyn`。

## 架构分层

```
┌──────────────────────────────────────────────────────────────┐
│ Storage Client (GraphStorage)                                │
│  - MVCC + WAL + staged_wal (DashMap<TransactionId,Vec<WalEntry>>) │
└────────────────────────┬─────────────────────────────────────┘
                         │ decorates
┌────────────────────────▼─────────────────────────────────────┐
│ SyncWrapper<S: StorageClient>                                │
│  - StorageClient 装饰器，实现 TransactionCommitSink           │
│  - 拦截写操作 → SyncManager::stage_intent → transaction WAL  │
│  - 拦截 commit/abort → 两阶段 fence (WAL持久 + Outbox物化)   │
└────────────────────────┬─────────────────────────────────────┘
                         │
┌────────────────────────▼─────────────────────────────────────┐
│ SyncManager                                                  │
│  - pending_intents: DashMap<TransactionId,Vec<OutboxIntent>> │
│  - sqlite_outbox: Option<Arc<SqliteOutbox>>                   │
│  - vector_coordinator / vector_receiver / outbox_consumer    │
│  - 负责 staging / materialize / retry_outbox_sync(5s poll)  │
└──────────┬──────────────────────┬────────────────────────────┘
           │                      │
     ┌─────▼──────┐         ┌─────▼──────────────────┐
     │ WAL        │         │ SQLite Outbox           │
     │ WAL file   │         │ events/commit_targets/  │
     │ OutboxIntent│◄────────│ projection_state/       │
     │ WalOpType=21│  replay │ target_state/           │
     └─────┬──────┘         │ generation_state/       │
           │                │ index_frontier/         │
           │ materialize    │ idempotency/dead_letters│
           └───────────────►│  claim/ack/retry/ddlq  │
                            └──────────┬──────────────┘
                                       │ claim_next → apply
                            ┌──────────▼──────────────┐
                            │ VectorSyncCoordinator   │
                            │ - logical_indexes       │
                            │ - backend: VectorBackend│
                            │ - group_id 隔离         │
                            └──────────┬──────────────┘
                                       │
                            ┌──────────▼──────────────┐
                            │ VectorBackend (enum)    │
                            │ Local(Arc<LocalEngine>) │
                            │ Qdrant(Arc<VectorMgr>)  │
                            └──────────┬──────────────┘
                                       │
                            ┌──────────▼──────────────┐
                            │ VectorReceiver          │
                            │ - applied_lsn + receipts│
                            │ - vector_receiver_state.bin│
                            └─────────────────────────┘
```

## 核心组件

### 1. SyncWrapper

**位置**：`crates/graphdb-storage/src/storage/engine/sync_wrapper.rs:25`

- 包装 `StorageClient`，在存储操作时自动同步到索引。
- 写链路：`sync_insert_vertex` → `SyncManager::on_vertex_change_with_txn` → `stage_intent(txn_id, OutboxPayload::Vertex{...})`。`payload_to_intent` 为每个需要的 `target` 生成 `OutboxIntent`（`index_id = stable_hash("{target}:{space}:{index}")`，`ordering_key = "{target}:{space}:{index}:{entity}"` 按实体序列化，见 §8.2 R5 修复）。
- 索引 DDL：`create_tag_index / drop_tag_index` → `stage_index_create/drop` 同样进 `OutboxPayload::CreateIndex/DropIndex`，随事务原子落盘；`space/tag/edge type` 的 `create/drop` 不进 outbox。
- 提交两阶段：① `commit_staged_writes_with_durability` 追加 `WAL { redo..., OutboxIntent... }` 并 `fsync`；② `finalize_commit` 调 `materialize_committed_transaction` 进 SQLite 并 `retry_outbox_sync` 机会性投递。`auto-commit` 同步 `commit+finalize`，避免 `pending_intents` 泄漏与 `checkpoint safe_lsn` 钉死。
- 回滚/Savepoint：`abort_transaction_fact` 丢弃 `staged_wal + pending_intents`；`rollback_to_sequence_sync` 按 `intent_sequence` 截断。

### 2. SyncManager

**位置**：`crates/graphdb-sync/src/sync/manager.rs:52`

- `pending_intents: DashMap<TransactionId, Vec<OutboxIntent>>` 内存暂存；`sqlite_outbox / vector_coordinator / vector_receiver / outbox_consumer{batch=128,lease=30s,max_retries=16}`。
- `stage_intent` 按负载内容与 `index_exists` 预过滤 `target`，避免 `vector+fulltext` 各克隆一份的写放大（`ChangeType::Delete` 空属性时扇出全量逻辑索引判断）。
- `materialize_committed_transaction` → `SqliteOutbox::materialize_commit` 事务性插入 `generation_state/commit_targets/events/idempotency` 并推进 `materialized_lsn`。
- `retry_outbox_sync` 每目标独立 `batch_size` 预算，`claim_next` → `apply_index_mutation` → `acknowledge / retry(with backoff 100ms*2^n capped 300s) / dead_letter / retry(Disabled固定5s不计dead_letter阈值)`。成功后更新 `StatsManager::record_outbox_state + record_target_frontier_lag + record_vector_disabled_skips`。
- `claim_next` `BEGIN IMMEDIATE` 串行化 + `lease_epoch` fencing，`ordering_key` 同实体栅栏（每事件唯一旧逻辑已修复），`generation_state='active'` 围栏。
- 后台常驻：`start()` 每 `5s` 轮询，`stop()` 终止；`configure_outbox` 时校验活库、必要时从 `latest_manifest_outbox_snapshot` 或目录全扫描恢复，并打开 `VectorReceiver`。
- 一致性：`wait_for_minimum_lsn(target,index_id,generation, commit_lsn, timeout)` 阻塞至 `frontier >= session_commit_lsn`，遇 `degraded` 抛错。

### 3. SqliteOutbox

**位置**：`crates/graphdb-sync/src/sync/sqlite_outbox.rs:23`

- `pool(8)` + `WAL+FULL`；表：`events/commit_targets/projection_state/target_state/generation_state/index_frontier/dead_letters/degraded_ranges`。
- `claim_next`：`target,status,next_attempt_at_ms,lease` + `generation_state='active'` + `NOT EXISTS ordering_key` 围栏，`ORDER BY commit_lsn,intent_sequence`。
- `acknowledge` 推进 `target_state.applied_lsn` 与 `index_frontier`；`skip_event_degraded` 标记 `degraded` 并写 `degraded_ranges`。
- `stats / diagnostics / wait_for_minimum_lsn / has_degraded_range / requeue_dead_letter / prune_applied_events(retention_lsn)` 等运维原语；`create_snapshot(VACUUM INTO)+verify` 供 checkpoint 组合发布。

### 4. VectorSyncCoordinator / VectorBackend / VectorReceiver

**位置**：`vector_sync.rs:206 / backend.rs:30 / receiver.rs:204`

- `VectorBackend` `enum { Local(Arc<LocalVectorEngine>), Qdrant(Arc<VectorManager>) }` 零成本分发；`is_disabled()` 仅 Qdrant 可能 `engine.name()=="disabled"`。
- `VectorSyncCoordinator::on_vector_change_batch` 按 `collection_name="space_{space_id}"` 聚合 `upsert/delete`，注入 `group_id="{tag}_{field}"` payload 隔离；`Disabled` 时 Qdrant 分支返回 `EngineDisabled` 使 outbox 保留重试（`disabled_skips` 计数），`Local` 永不 disabled 直接抛错。
- 逻辑索引 `logical_indexes: DashMap<VectorIndexLocation,IndexMetadata>` 追踪，`index_exists / list_indexes / set_index_name` 支持 `CREATE VECTOR INDEX` 名称解析与代价估算。
- `VectorReceiver` 文件闸 `applied_lsn + receipts(HashMap<idempotency_key,commit_lsn>)`，`LSN水位+LRU窗口（8192条，retention 100k LSN）` 有界，超限按水位与最老 LSN 淘汰，早于水位的重复由 SQLite `idempotency` 去重；`record_application` 先落向量后持久 receipt（`write(tmp)+fsync+rename+dirfsync`）。
- `point_id` 规范化：`"{vid}#{tag}#{field}"`，`vid` 以 `%23/%25` 转义 `#/%`，跨后端稳定（旧 `_` 格式的数据需重建）。

### 5. VectorApi（传输无关）

**位置**：`crates/graphdb-api/src/api/core/vector_api.rs:53`

- 直写旁路收敛：`insert_vector/batch` 新增 `VectorWriteMode::Direct | Transactional{txn_id,space,tag,field}`。`Direct` 保留直调 `backend.upsert`（非事务），`Transactional` 走 `SyncManager::stage_intent` 入 outbox，可回滚并享 `RYW` 一致性。
- `search_with_options` 透传 `SearchConsistency::Eventual | ReadYourWrites{timeout_ms}` 与 `minimum_lsn`，`RYW` 时调 `outbox.wait_for_minimum_lsn`，`degraded` 抛错，超时返回 `Timeout`。

## 提交 / 回滚 / 恢复生命周期

### 显式事务 `BEGIN; INSERT; COMMIT;`

`GraphService::execute` → `TransactionManager::commit_transaction`：① `check_write_set_conflict + transition_to(Committing)` ② 重试 `commit_sink.commit_transaction_with_descriptor`（`backoff 100ms*2^n capped 10s`）③ `certifier.publish()` SSI 终审 ④ `finalize_commit`（失败记 `CommitDurableButUnfinalized`）⑤ `SyncWrapper::finalize_commit` 物化并机会投递；读 MVCC 立即可见，向量滞后由 `diagnostics.frontier_lag` 度量。

### Auto-commit 单语句

`StorageOperationContext { auto_commit, transaction_id }`（`1<<62` 起）`commit_auto_transaction` 同步 `commit+finalize`。

### 恢复（崩溃 → 重启） `startup.rs:256`

1. `manager.configure_outbox(outbox.sqlite)`：`verify_live_database` 失活则优先恢复 `latest_manifest_outbox_snapshot(work_dir)` 否则目录全扫描；`SqliteConnectOptions{journal=WAL,sync=FULL, max_connections=8}` 打开，`vector_receiver: VectorReceiver::open(work_dir/vector_receiver)` 加载；`vector_coordinator.set_outbox(outbox)` 注入一致性等待句柄。
2. `manager.retry_outbox_sync()` 冷启动立即投递。
3. `inner_storage.recover_outbox_projection(manager)`：读 `materialized_lsn` 为下界，`LocalWalParser.parse_all_entries` + `collect_committed_transactions` 校验 `checksum/sorted`，逐 `commit_lsn > snapshot_lsn` 重 `materialize`。
4. `manager.start()` 启动 `5s` 周期后台。
5. `TransactionManager.startup_recovery` 清收未完成事务。
6. `SyncWrapper::create_checkpoint` 先 `create_checkpoint_outbox_snapshot`（`VACUUM INTO`）再 `inner.create_checkpoint()`，形成组合 `manifest` 原子发布。

## 查询链路与一致性

`VectorApi::search_with_options` 与 `VectorSyncCoordinator::search_with_options` 均注入 `must(match_value(group_id))`，经 `VectorBackend::search` 直查物理集合（`Local` 精确/近似，`Qdrant` 含 filter 翻译与 `1/(1+d)` 归一）。

- 默认 `Eventual`：图提交后立即可见，向量滞后可观测，不阻塞。
- `ReadYourWrites{timeout_ms, minimum_lsn}`：`SEARCH` 执行器阻塞至 `index_frontier >= minimum_lsn`（未指定则取 `materialized_lsn`），遇 `degraded` 报错而非脏读；`Local` 典型 `500ms`，`Qdrant` `2000ms`。
- 向量搜索当前不纳入 `SSI` 读集校验，`REPEATABLE READ` 对向量不生效（显式文档化，见 § 未来改进）。

## 一致性与容错保障

| 维度 | 机制 | 证据 |
|------|------|------|
| 图原子提交 | WAL append+fsync + `commit_write_timestamp`，`staged_wal` 按 `txn_id` 隔离；`undo log` 回滚 | `transaction/manager.rs:655` `graph_storage.rs:1701` |
| 向量最终一致 | staged payload → WAL Intent → SQLite materialize → VectorReceiver 幂等 → Local/Qdrant | `manager.rs:1028` `sqlite_outbox.rs:668` `receiver.rs:204` |
| 恰好一次效果 | `(target,idempotency_key)` 唯一，`leased+epoch` fencing，原子 `record_application` 后才 ack | `sqlite_outbox.rs:232,854` |
| 顺序 | `commit_lsn,intent_sequence` 全局有序 `claim ORDER BY`；同代 `generation_state='active'` 围栏；`ordering_key="{target}:{space}:{index}:{entity}"` 同实体串行栅栏 | `sqlite_outbox.rs:828,803` `manager.rs:1325` |
| 幂等重放 | postcard 负载 + `VectorReceiver` 文件 `HashMap` LRU + SQLite `idempotency` | `receiver.rs:311` `engine.rs:560 apply_txn` |
| 崩溃恢复 | WAL 重放 + SQLite `VACUUM INTO` 快照 + `vector_receiver_state.bin` | `startup.rs:256` `sqlite_outbox.rs:110` `receiver.rs:256` |
| 故障降级 | `Local` 永不 disabled 抛错；`Qdrant disabled` 时保留事件固定 5s 重试，不进 `dead_letter`，`disabled_skip_count` 可观测 | `vector_sync.rs:468` `manager.rs:398` |
| 可观测 | `SyncDiagnostics{materialized_lsn, vector_disabled_skips, targets[], indexes[]}` `OutboxStats` `frontier_lag/degraded/pending + per-target lag + disabled_skips` | `sqlite_outbox.rs:38,1339` `manager.rs:443` |

## 配置与差异化策略

`VectorConfig.engine = Local | Qdrant`，`startup::attach_vector_coordinator` 注入 `BackendDeliveryPolicy { batch_size, lease_ms, max_retries, disabled_behavior }`。

| 策略项 | Local | Qdrant |
|--------|-------|--------|
| `batch_size / lease_ms` | `128 / 30s` 保持 | `512 / 60s`，`max_retries=32`（待 Phase2 策略化） |
| 并发 | 单并发 claim + 单集合串行 apply | 规划 `claim 4 并发 + collection 并发`（Phase2） |
| 重试分类 | `DiskFull/DimensionMismatch` 直 dead_letter | `Timeout/Unavailable` 重试，`Auth/InvalidArgument` 直 dead_letter（Phase2 `VectorErrorKind`） |
| `Disabled` | 永不 disabled，失败抛错 | disabled 时 retry 挂起，不 ack |
| 物理回收 | `delete_collection` 删目录（`is_local && siblings==0`） | `delete_by_filter(group_id)` 逻辑删 |
| 一致性超时 | `ryw timeout 500ms` 典型 | `ryw timeout 2000ms` 典型 |
| 派生索引 | `maintenance_loop` `drift/rebuild/compaction` | 无，由服务端自治 |
| 监控 | `spawn_vector_metrics_sampler` | `spawn_remote_vector_metrics_sampler` |

## 运维与发布

- **灰度**：Phase0 可全量；Phase1 一致性旋钮默认 `eventual` 保持兼容，`ryw` 显式开启；`VectorWriteMode::Direct` 保留。
- **回滚**：任一 Phase 回滚仅影响投递策略，不丢 WAL 真源；`SqliteOutbox` 为派生，重放 WAL 可重建。
- **迁移**：`OUTBOX_SCHEMA_VERSION` 递增时 `SqliteOutbox::migrate` 幂等新增列/索引；`ordering_key` 语义修正对旧 `pending` 行可离线 `UPDATE` 回填；`vector_receiver_state.bin` 旧 `HashSet` 格式自动迁为 `HashMap`；`point_id` 旧 `_` 格式需重建或双删兼容。
- **保留与截断**：`projection_state.retention_lsn + events` 归档/截断（`prune_applied_events`），与 `checkpoints/manifests` 保留联动；`VectorReceiver` `applied_lsn` 前 `K` 个 `idempotency_key` 保留。
- **监控新增**：`sync_outbox_pending`、`sync_outbox_frontier_lag{target,index}`、`sync_vector_disabled_skips`、`sync_dead_letter_count`、`vector_local_pending_len` 告警阈值。

## 未来改进

1. **向量 MVCC 显式文档与可选增强**：默认保持向量无 MVCC，`docs/vector/README.md` 明确隔离差异；可选将 `VectorFilter` 读集纳入 `TransactionManager::certify`。
2. **空间级集合的替代选型**：评估 `space_{id}` → `{space}_{tag}_{field}` 细粒度集合的资源与配置隔离收益；在线迁移 `group_id` 重分布 + `index_generation` `Publishing` 切换。
3. **并发投递与错误分类**：`VectorErrorKind::{Retryable,NonRetryable,Auth}` 与 `BackendDeliveryPolicy` 由 `startup` 按 `VectorConfig` 注入（Phase2）。
4. **保留与截断联动**：与 `checkpoints/manifests` 保留策略联动，`WAL` 截断水位不再被无限 `applied` 事件拖慢。

## 相关文档

- `docs/plan/vector_sync_analysis.md`（事实基线）
- `docs/plan/vector_sync_improvement_plan.md`（分阶段方案）
- `docs/sync/operations.md`（恢复与 degraded 语义）
- `docs/vector/vector-engine-design.md`（分数与过滤翻译契约）
- `crates/vector-search/src/engine.rs:79,191,360,560`（本地引擎 WAL 与幂等）

