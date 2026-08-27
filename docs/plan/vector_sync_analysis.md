# 向量存储与图存储同步机制分析

> 状态：分析文档（2026-08-27）  
> 前置文档：
> - `docs/sync/architecture.md`（历史同步架构，含已过时的 2PC 描述）
> - `docs/sync/operations.md`（运维与恢复边界）
> - `docs/vector/vector-engine-design.md`（双引擎统一抽象与分数契约）
> - `docs/plan/pgvector_implementation_details.md`（本地引擎 PG 向量对照）
> - 代码基准：`crates/graphdb-sync/src/sync/manager.rs:1`、`vector_sync.rs:1`、`backend.rs:1`、`sqlite_outbox.rs:1`、`receiver.rs:1`、`crates/graphdb-storage/src/storage/engine/sync_wrapper.rs:1`、`crates/graphdb-server/src/startup.rs:86`

---

## 1. 背景与问题定义

图数据（顶点/边/索引 DDL）与向量数据（`Value::Vector(Vec<f32>)` 承载的逻辑向量）需保持同步。图数据是事务性、强一致的（`WAL + SQLite outbox + TransactionManager`）；向量数据由两类后端承载：

- **Local**：`vector-search::LocalVectorEngine`（`crates/vector-search/src/engine.rs:79`）——同进程、mmap + WAL + 后台维护线程
- **Qdrant**：`vector-client::VectorManager`（`crates/vector-client/src/manager.rs`）——远端服务，`gRPC/HTTP` 双传输

同步需回答：以什么为真源？以何种一致性对外？崩溃如何恢复？不同后端是否应走不同路径？

---

## 2. 同步机制总览

### 2.1 架构分层

```
┌──────────────────────────────────────────────────────────────┐
│ Storage Client (GraphStorage)                                │
│  - MVCC + WAL + staged_wal (DashMap<TransactionId,Vec<WalEntry>>) │
│    crates/graphdb-storage/src/storage/engine/graph_storage/context.rs:197 │
└────────────────────────┬─────────────────────────────────────┘
                         │ decorates
┌────────────────────────▼─────────────────────────────────────┐
│ SyncWrapper<S: StorageClient>                                │
│  crates/graphdb-storage/src/storage/engine/sync_wrapper.rs:25 │
│  - StorageClient 装饰器，实现 TransactionCommitSink           │
│  - 拦截写操作 → SyncManager::stage_intent → transaction WAL  │
│  - 拦截 commit/abort → 两阶段 fence                         │
└────────────────────────┬─────────────────────────────────────┘
                         │
┌────────────────────────▼─────────────────────────────────────┐
│ SyncManager                                                  │
│  crates/graphdb-sync/src/sync/manager.rs:52                  │
│  - pending_intents: DashMap<TransactionId,Vec<OutboxIntent>> │
│  - sqlite_outbox: Option<Arc<SqliteOutbox>>                   │
│  - vector_coordinator / vector_receiver / outbox_consumer    │
│  - 负责 staging / materialize / retry_outbox_sync(5s poll)  │
└──────────┬──────────────────────┬────────────────────────────┘
           │                      │
     ┌─────▼──────┐         ┌─────▼──────────────────┐
     │ WAL        │         │ SQLite Outbox           │
     │ WAL file   │         │ sqlite_outbox.rs:207    │
     │ OutboxIntent│◄────────│ events/commit_targets/  │
     │ WalOpType=21│  replay │ projection_state/       │
     └─────┬──────┘         │ target_state/           │
           │                │ generation_state/       │
           │ materialize    │ idempotency/dead_letters│
           └───────────────►│  claim/ack/retry/ddlq  │
                            └──────────┬──────────────┘
                                       │ claim_next → apply
                            ┌──────────▼──────────────┐
                            │ VectorSyncCoordinator   │
                            │ sync/vector_sync.rs:206 │
                            │ - logical_indexes       │
                            │ - backend: VectorBackend│
                            │ - group_id 隔离         │
                            └──────────┬──────────────┘
                                       │
                            ┌──────────▼──────────────┐
                            │ VectorBackend (enum)    │
                            │ sync/backend.rs:30      │
                            │ Local(Arc<LocalEngine>) │
                            │ Qdrant(Arc<VectorMgr>)  │
                            └──────────┬──────────────┘
                                       │
                            ┌──────────▼──────────────┐
                            │ VectorReceiver          │
                            │ sync/receiver.rs:204    │
                            │ - applied_lsn + receipts│
                            │ - vector_receiver_state.bin│
                            └─────────────────────────┘
```

依赖 DAG：`metrics → core → config → search → sync → transaction → storage → query → api → server`（`Cargo.toml:9`）。`graphdb-sync` 拥有全部同步状态；`graphdb-storage` 绝不直连 Qdrant。

### 2.2 真源与一致性模型

- **真源单一**：`Graph WAL` 为唯一真源；`OutboxIntent` 作为 `WAL` 的一种逻辑日志随图 `redo` 原子落盘（`WAL_SYNC_WIRE_VERSION=1`，`core/wal/sync.rs:56`）。
- **最终一致**：图提交即对 MVCC 读可见；向量通过 `SQLite` 异步投递，`frontier_lag` 可观测（`sqlite_outbox.rs:339 SyncDiagnostics`）。
- **无分布式事务**：无 `2PC/XA`，无同步双写；图 `WAL` 单 fence 决定提交，向量为 followers。文档 `docs/sync/architecture.md:242` 描述的 `2PC` 已过时，实际为 `Transactional Outbox`。

---

## 3. 写链路分解

### 3.1 顶点写入（事务内）

以 `sync_insert_vertex` 为例（`sync_wrapper/write_vertex.rs:43`）：

```rust
// 1. 解析 space_id，获取当前 txn_id
let txn_id = self.get_current_txn_id()
    .ok_or("Synchronized writes require an operation transaction context")?;
// 2. 暂存意图
sync_manager.on_vertex_change_with_txn(txn_id, space_id, tag, vid, props, ChangeType::Insert)
```

`on_vertex_change_with_txn`（`manager.rs:872`）→ `stage_intent(txn_id, OutboxPayload::Vertex{...})`（`:139`）：

```rust
fn stage_intent(&self, txn_id, payload) -> Result<()> {
    if !delivery_target_names().is_empty() && sqlite_outbox.is_none() {
        return Err("Synchronized writes require a configured durable outbox");
    }
    let mut intents = pending_intents.entry(txn_id).or_default();
    for target in delivery_target_names() { // ["fulltext","vector"] 按 feature 动态
        intents.push(payload_to_intent(txn_id, seq, target, &payload)?);
    }
}
```

关键：每个 `payload` 为每个 `target` 克隆一份 `OutboxIntent`，`sequence` 为 `u32` 单调递增（`:147`）。

`payload_to_intent`（`manager.rs:1325`）：

- `space_id/index_name/entity_ref/operation` 由 `payload` 派生
- `id = "{txn_id}:{target}:{seq+1}"` → `IdempotencyKey`
- `ordering_key = "{target}:default:{id}"`（注意：每事件唯一，后文分析）
- `target = TargetId("vector"|"fulltext")`，`index_id = stable_hash("{target}:{space}:{index}")`，`generation=1`

### 3.2 提交两阶段

**阶段 1 — WAL 持久**（`sync_wrapper.rs:132 commit_transaction_fact`）：

```rust
let intents = manager.pending_transaction_intents(txn_id)?; // 内存镜像
let commit_lsn = inner.commit_staged_writes_with_durability(txn_id, &intents, durability)?;
// 失败则 abort_staged_writes + rollback_transaction_sync 保证 auto-commit 可重试 (:164)
```

`GraphStorage::commit_staged_writes_with_durability` 追加 `WAL { redo..., OutboxIntent... }` 并 `fsync`（`graph_storage.rs:1717`）。

**阶段 2 — Outbox 物化**（`sync_wrapper.rs:178 finalize_commit_fact`）：

```rust
manager.materialize_committed_transaction(txn_id, commit_lsn, &intents)?;
manager.rollback_transaction_sync(txn_id)?; // 清内存
manager.clear_transaction_intents(txn_id);
manager.retry_outbox_sync()?; // 机会性投递，不等 5s 周期
```

`materialize_commit`（`manager.rs:1028`）→ `SqliteOutbox::materialize_commit`（`sqlite_outbox.rs:668`）事务性：

- 插入 `generation_state('active')`（首个突变自动建活跃代）
- `INSERT OR IGNORE INTO target_state/commit_targets/events/idempotency`
- `UPDATE projection_state.materialized_lsn = MAX(...)`

`auto-commit` 单语句（`sync_wrapper.rs:223 commit_auto_transaction`）：镜像上述两阶段，若遗漏 `finalize_commit_fact` 会使 `checkpoint safe_lsn` 钉死在 0 致 `WAL` 永不截断（`:236` 注释）。

### 3.3 投递循环

`retry_outbox_sync`（`manager.rs:368`）每目标独立 `batch_size=128` 预算：

```
for target in outbox.delivery_targets() { // DISTINCT status IN ('pending','retry','leased')
  while processed < batch_size {
    claim_next(target, consumer_id, now, 30s) -> Option<ClaimedEvent>
    match apply_index_mutation(event.mutation, commit_lsn) {
      Ok => acknowledge(event) // UPDATE events='applied' + commit_targets.applied_count + advance_frontier
      Err(e) => if retry_count+1 >= max_retries(16) { dead_letter(event) } else { retry(event, now+backoff) }
    }
  }
}
```

- `backoff = 100ms * 2^retry_count capped 300s`（`:429`）
- `acknowledge` 同时推进 `target_state.applied_lsn` 与 `index_frontier`（`sqlite_outbox.rs:887`）
- 成功后记录 `StatsManager::record_outbox_state`（`:443 frontier_lag/degraded/pending`）

`claim_next`（`sqlite_outbox.rs:797`）`BEGIN IMMEDIATE` 串行化：

```sql
SELECT ... FROM events e
 WHERE e.target=? AND e.status IN ('pending','retry','leased')
   AND e.next_attempt_at_ms<=? AND (lease_owner IS NULL OR lease_until_ms<=?)
   AND EXISTS (SELECT 1 FROM generation_state g WHERE g.target=e.target AND g.index_id=e.index_id AND g.generation=e.generation AND g.state='active')
   AND NOT EXISTS (SELECT 1 FROM events earlier WHERE earlier.target=e.target AND earlier.ordering_key=e.ordering_key AND (earlier.commit_lsn<e.commit_lsn OR (commit_lsn=e.commit_lsn AND intent_sequence<e.intent_sequence)) AND earlier.status NOT IN ('applied','skipped'))
 ORDER BY e.commit_lsn, e.intent_sequence LIMIT 1
-- 然后 UPDATE events SET status='leased', lease_epoch=lease_epoch+1 WHERE id=? AND lease_epoch=?  fencing
```

后台常驻：`SyncManager::start()`（`manager.rs:836`）`tokio::spawn` 每 `5s` 调用 `retry_outbox_sync`，`stop()` 终止。

### 3.4 向量投递分支

`apply_index_mutation`（`manager.rs:472`）`postcard` 解码 `OutboxPayload`，按 `mutation.target=="vector"` 分发至 `apply_vector_mutation`（`:688`）：

```rust
let late = receiver.check_late_arrival(commit_lsn, idempotency_key).await;
if !late.accepted { if duplicate { return Ok(()) } else { return Err(reason) } }

match payload {
  Vertex{space_id, tag, vertex_id, properties, change_type} => {
    for (field, value) in properties {
      if !coordinator.index_exists(space_id, tag, field) { continue; }
      let (vec, ty) = match (value.as_vector(), change_type) {
        (Some(v), Insert|Update) => (v.to_vec(), Insert),
        _ => (Vec::new(), Delete),
      };
      contexts.push(VectorChangeContext{ location: (space_id,tag,field), change_type: ty, data: {id:"{vid}_{tag}_{field}", vector:vec, payload: HashMap<properties>} })
    }
    if Delete { // 补齐未在 properties 中的字段，保证全 tag 的向量都清
      for meta in coordinator.list_indexes() if meta.space_id==space_id && meta.tag==tag && !staged_fields.contains(field) { contexts.push(Delete for meta) }
    }
  }
  CreateIndex{space_id, schema, fields} => for (field, value_type) if value_type.as_vector().len() => coordinator.create_vector_index(space_id, schema, field, size, Cosine).await?
  DropIndex{...} => coordinator.drop_vector_index(...).await?
  EdgeInsert/Delete => {} // 边不触发向量（向量索引按顶点维护）
}
if !contexts.is_empty() { coordinator.on_vector_change_batch(contexts).await?; }
receiver.record_application(commit_lsn, idempotency_key).await // 先落向量，后持久化 receipt，失败可重试
```

`VectorSyncCoordinator::on_vector_change_batch`（`vector_sync.rs:464`）：

- `disabled` 引擎：`disabled_skips += len`，`warn!`，`Ok(())`（不阻塞 `frontier`）
- 活跃：按 `collection_name = "space_{space_id}"`（`:170` 空间级粒度）聚合 `upsert_by_collection` / `delete_by_collection`，注入 `group_id = "{tag}_{field}"` payload，单条 `upsert`，多条 `upsert_batch`。

`VectorIndexLocation`（`vector_sync.rs:138`）显式警告：同 `space` 内不同 `(tag,field)` 不可异构 `dimension/metric`，否则 `CollectionConfigConflict`（`vector_sync.rs:374`）。

### 3.5 索引 DDL 链路

`create_tag_index / drop_tag_index`（`sync_wrapper.rs:810`）→ `validate_schema_sync_context()` 要求事务上下文 → `stage_index_create/drop`（`:264`）同样进 `OutboxPayload::CreateIndex/DropIndex`，随事务原子落盘。注意：`space`/`tag`/`edge type` 的 `create/drop` 不进 `outbox`（无向量语义）。

### 3.6 回滚与 Savepoint

- **全回滚**：`TransactionManager.abort_transaction` → `SyncWrapper.abort_transaction_with_descriptor`（`:385`）先 `execute_undo_logs` 再 `abort_transaction_fact` 丢弃 `staged_wal` + `pending_intents`。
- **Savepoint**：`manager.rs:165 rollback_transaction_to_sequence_sync(txn, seq)` `intents.retain(seq <= saved)`，由 `TransactionContext::rollback_to_savepoint` 带 `sync_sequence` 调用。

---

## 4. 核心组件职责与关键数据结构

| 组件 | 位置 | 关键状态 | 职责 |
|------|------|----------|------|
| `SyncWrapper` | `storage/engine/sync_wrapper.rs:25` | `inner:S` `sync_manager:Option<Arc<SyncManager>>` `enabled` | 装饰器，桥接存储与同步；实现 `TransactionCommitSink` |
| `SyncManager` | `sync/manager.rs:52` | `pending_intents` `sqlite_outbox` `vector_coordinator` `vector_receiver` `outbox_consumer{batch=128,lease=30s,max_retries=16}` | 暂存→物化→投递三步；`configure_outbox`、`retry_outbox_sync`、`materialize_committed_transaction` |
| `SqliteOutbox` | `sync/sqlite_outbox.rs:23` | `pool(8)` `path` 表：`events/commit_targets/projection_state/target_state/generation_state/index_frontier/dead_letters/degraded_ranges` | 持久队列；`claim_next` 租约 fencing；`acknowledge/retry/dead_letter/skip_event_degraded`；`diagnostics()` 前沿健康 |
| `OutboxPayload` | `sync/outbox.rs:6` | `Vertex{space_id,tag,vertex_id,properties,change_type}` `EdgeInsert/Delete` `CreateIndex/DropIndex` | `WAL` 与 `SQLite` 之间 `document_or_vector:postcard` 负载 |
| `OutboxIntent/IndexMutation` | `core/wal/sync.rs:56` | `wire_version=1` `transaction_id` `intent_sequence` `mutation{target,index_id,generation,entity_ref,operation,document_or_vector,idempotency_key,ordering_key}` | `WalOpType=21` 物理载体 |
| `VectorSyncCoordinator` | `sync/vector_sync.rs:206` | `backend` `logical_indexes:DashMap<VectorIndexLocation,IndexMetadata>` `disabled_skips:AtomicU64` `runtime:Handle` | 逻辑索引注册；物理集合管理；批处理 `upsert/delete`；`group_id` 隔离；`disabled` 降级 |
| `VectorBackend` | `sync/backend.rs:30` | `Local(Arc<LocalVectorEngine>)` `Qdrant(Arc<VectorManager>)` | 零成本 `enum` 分发，无 `dyn`；统一 `is_local/is_disabled` |
| `VectorReceiver` | `sync/receiver.rs:204` | `state: RwLock<VectorCommitState{applied_lsn, receipts:HashSet<String>}}` `recovery_path/vector_receiver_state.bin` | 幂等与迟到栅栏；`check_late_arrival`/`record_application` 原子 `write(tmp)+fsync+rename+dirfsync` |
| `VectorApi` | `api/core/vector_api.rs:53` | `backend` `coordinator:Option<Arc<VectorSyncCoordinator>>` | 直写 API：`insert_vector/upsert_batch/delete/search` 绕过 `outbox` 直调 `backend` |

`OutboxConsumerConfig`（`manager.rs:68`）`consumer_id = "sync-manager-{uuid}"`；`VectorSyncCoordinator::VectorEngineState::{Disabled,Active}`（`vector_sync.rs:59`）。

---

## 5. 提交 / 回滚 / 恢复生命周期

### 5.1 显式事务示例 `BEGIN; INSERT; COMMIT;`

`GraphService::execute`（`server/graph_service.rs:417`）→ `parse_command` → `transaction_manager.commit_transaction(txn_id)`（`transaction/manager.rs:608`）：

1. `check_write_set_conflict` + `transition_to(Committing)`
2. 重试 `commit_sink.commit_transaction_with_descriptor`（`manager.rs:875 backoff 100ms*2^n capped 10s, 3次`）
3. `certifier.publish()` SSI 终审
4. `finalize_commit` 同重试；失败 `recovery.record` 并发出 `CommitDurableButUnfinalized`
5. `SyncWrapper::finalize_commit` 物化并机会性投递

读取：MVCC `commit_write_timestamp(commit_lsn)` 立即可见，向量滞后由 `diagnostics.frontier_lag` 度量。

### 5.2 Auto-commit 单语句

`StorageOperationContext { auto_commit, transaction_id }`（`storage/engine/graph_storage/context.rs:1016`）`next_auto_transaction_id = 1<<62` 保证 `SQLite` `i64` 列非负；`SyncWrapper::commit_auto_transaction`（`sync_wrapper.rs:223`）在 `insert` 成功后同步 `commit + finalize`，避免 `pending_intents` 泄漏与 `checkpoint safe_lsn` 钉死。

### 5.3 恢复（崩溃 → 重启）

`startup.rs:256` 序列：

1. `manager.configure_outbox(outbox.sqlite)`：`verify_live_database`（`sync/outbox_recovery.rs:21` `SELECT 1` 探测），失活则优先恢复 `latest_manifest_outbox_snapshot(work_dir)`（组合 checkpoint 引用），否则目录全扫描；`SqliteConnectOptions{journal=WAL,sync=FULL, max_connections=8}` 打开，`vector_receiver: VectorReceiver::open(work_dir/vector_receiver)` 同步加载
2. `manager.retry_outbox_sync()` 冷启动立即投递
3. `inner_storage.recover_outbox_projection(manager)`（`graph_storage.rs:1727`）：读 `materialized_lsn` 为下界，`LocalWalParser.parse_all_entries` + `collect_committed_transactions`（`transaction/wal/commit.rs:49` 校验 `checksum/sorted`），逐 `transaction.commit_lsn > snapshot_lsn` 重 `materialize`
4. `manager.start()` 启动 `5s` 周期后台投递（`manager.rs:836`）
5. `TransactionManager.startup_recovery` 清收未完成事务

`SyncWrapper::create_checkpoint`（`sync_wrapper.rs:965`）先 `create_checkpoint_outbox_snapshot`（`manager.rs:1133` `outbox_snapshot_{lsn}.sqlite` via `VACUUM INTO` + `checksum`），再 `inner.create_checkpoint()`，形成组合 `manifest` 原子发布约束（`sync/operations.md:5`）。

---

## 6. 查询链路与隔离性

`VectorApi::search_with_options`（`api/core/vector_api.rs:285`）与 `VectorSyncCoordinator::search_with_options`（`vector_sync.rs:608`）均注入 `must(match_value(group_id))`，经 `VectorBackend::search` 直查物理集合：

- `Local`：`LocalVectorEngine::search`（`vector-search/src/engine.rs:448`）`Tier0` 精确扫描 / `Tier1` `IVF/HNSW` 近似，`WAL` 派生索引，派生可丢弃
- `Qdrant`：`VectorManager::search` 经 `gRPC/HTTP`，含 `filter` 翻译与 `Euclid` 分数归一化 `1/(1+d)`（`vector/vector-engine-design.md:30`）

**隔离性缺口**：向量搜索不感知 `TransactionManager` 的 `read_timestamp`/`snapshot`，不纳入 `SSI` 读集；同一 `space` 内并发读写可出现图可见而向量不可见（或相反）的读偏斜。详见 §8。

---

## 7. 一致性与容错保障

| 维度 | 机制 | 证据 |
|------|------|------|
| **图原子提交** | `WAL append + fsync + commit_write_timestamp`，`staged_wal` 按 `txn_id` 隔离；`undo log` 支持回滚 | `transaction/manager.rs:655` `graph_storage.rs:1701` |
| **向量最终一致** | `staged payload → WAL Intent → SQLite materialize → VectorReceiver 幂等 → Local/Qdrant` | `manager.rs:1028` `sqlite_outbox.rs:668` `receiver.rs:204` |
| **恰好一次效果** | `(target,idempotency_key)` 唯一，`leased+epoch`  fencing，原子 `record_application` 后才 `ack` | `sqlite_outbox.rs:232,854` |
| **顺序** | `commit_lsn,intent_sequence` 全局有序 `claim ORDER BY`；同代 `generation_state='active'` 围栏；`ordering_key` 同事务内防并发（实际每事件唯一，见 §8 缺陷） | `sqlite_outbox.rs:828,803` |
| **幂等重放** | `postcard` 负载 + `VectorReceiver` 文件 `HashSet`；`LocalVectorEngine::apply_txn(txn_id)` 按 `txn_id` 幂等 | `receiver.rs:311` `engine.rs:560 apply_txn` |
| **崩溃恢复** | `WAL` 重放 + `SQLite VACUUM INTO` 快照 + `vector_receiver_state.bin` 持久 receipt | `startup.rs:256` `sqlite_outbox.rs:110` `receiver.rs:256` |
| **故障静默降级** | `DisabledEngine`：写 `skip+count`，读 `Err(EngineDisabled)`；`pending`/`retry`/`leased`/`dead_letter`/`degraded` 指标 | `vector_sync.rs:58,468` `sqlite_outbox.rs:1048 stats` |
| **可观测** | `SyncDiagnostics{materialized_lsn, targets[], indexes[]}` `OutboxStats{pending,retries,leased,dead_lettered}` `frontier_lag` | `sqlite_outbox.rs:38,1339` `manager.rs:443` |

---

## 8. 现有设计合理性评价

### 8.1 优点

1. **单真源 Transactional Outbox 成熟**：图 `WAL` 单 fence 决定提交，规避分布式 `2PC` 的可用性与延迟代价；向量可用性不阻塞图写入，符合单节点轻量图库定位。
2. **持久投递可靠**：`SQLite WAL + FULL sync` + `VACUUM INTO` 快照 + `commit_lsn` 水位，使 `向量 = WAL 派生` 的重放语义与图 `recovery` 同构。
3. **租约幂等完善**：`lease_epoch`  fencing 防并发 `claim`，`idempotency_key` 全局唯一，迟到栅栏兼顾 `Local` 文件与 `Fulltext` `Tantivy commit_payload`。
4. **边界闭环**：`savepoint` 截断、`auto-commit finalize` 钉死防护、`checkpoint manifest` 原子发布、`DDL` 原子入队，关键注释均有回归测试（`sync/tests/vector_outbox_delivery.rs:35`）。
5. **后端抽象干净**：`VectorBackend` `enum` 分发无 `dyn` 开销；`group_id` 逻辑隔离在空间级物理集合上兼顾资源效率与检索正确性，度量校验前置（`vector_sync.rs:30 validate_metric`）。
6. **可观测与可运维**：`diagnostics/stats`、`skipped/dead_letter/degraded_ranges`、`wait_for_minimum_lsn` 为上层提供自检与人工干预抓手。

### 8.2 问题与风险

| # | 风险 | 描述 | 严重度 | 证据 |
|---|------|------|--------|------|
| R1 | **读写时序错觉** | 图提交后立即可见，向量滞后 `frontier_lag`；同一事务/会话内 `INSERT` 后 `SEARCH` 无 `read-your-writes`，易被误判为强一致 | 高 | `manager.rs:368` 5s 周期；查询未调 `wait_for_minimum_lsn` |
| R2 | **直写旁路双模型** | `VectorApi::insert_vector/batch`（`api/core/vector_api.rs:220`）直调 `backend.upsert` 绕过 `WAL/Outbox`，与顶点属性向量走 `Outbox` 事务语义分叉，崩溃/回滚不一致 | 高 | `vector_api.rs:228` vs `manager.rs:688` |
| R3 | **Disabled 静默分歧** | `disabled` 批量直接 `Ok` 并 `ack`（`vector_sync.rs:468`），事件永久丢失，重开后不自愈，`disabled_skip_count` 仅内存可观测 | 高 | `vector_sync.rs:470` |
| R4 | **空间级集合耦合** | `space_{id}` 单物理集合（`vector_sync.rs:170`）使同 `space` 不同 `(tag,field)` 不可异构 `dimension/metric`，错误仅在 `create` 期 `CollectionConfigConflict` 暴露；`delete` 需扇出全量逻辑索引 | 中 | `vector_sync.rs:156` 架构注释 |
| R5 | **`ordering_key` 同质化** | `ordering_key="{target}:default:{txn}:{seq}"` 每事件唯一，使 `claim_next:828 NOT EXISTS ordering_key` 围栏恒真，实际仅靠全局 `commit_lsn` 排序，并发同顶点更新仍 `last-write-wins`，文档误导 | 中 | `manager.rs:1415` |
| R6 | **投递写放大** | `stage_intent` 对 `vector+fulltext` 各克隆一份（`:146`），不管 `payload` 是否含向量/文本内容；大事务 `pending_intents` 无界，缺乏背压 | 中 | `manager.rs:139` |
| R7 | **单消费者瓶颈** | 单 `consumer_id` 单线程 `5s` 轮询 `batch 128`，热点 `target` 可饿死他者（虽有每目标预算，仍单并发），无并发 `claim` 与流控 | 中 | `manager.rs:379` |
| R8 | **向量无 MVCC** | 搜索不携带 `read_timestamp`/`TransactionExecution`，不参与 `SSI` 读集校验，图 `REPEATABLE READ` 对向量不生效 | 中 | `server/graph_service.rs:588` 流式与 `api/core/query_api.rs:161` |
| R9 | **幂等集无界** | `VectorReceiver` `HashSet<String>` 全量常驻并全量 `postcard` 重写（`receiver.rs:311`），`events` `applied/skipped` 永不清理 | 低 | `receiver.rs:198` |
| R10 | **ID 编码碰撞** | `point_id="{vid}_{tag}_{field}"` 未转义，`vid` 含 `_` 时虽不致唯一性破坏（`vid` 为 `Value` 字符串化），但可读性与跨后端迁移需规范 | 低 | `manager.rs:744` |

**总体结论**：当前设计对单节点、异步异构索引的取舍是合理的——以简单、可靠的 `Outbox` 换取可用性与可恢复性。上述风险不推翻架构，但需在一致性暴露、降级语义与背压上补齐。

---

## 9. Qdrant 与 Local 是否需要差异化处理

### 9.1 结论先行

**内核统一，策略分支**。`Graph WAL → Outbox → SQLite frontiers` 的真源与重放语义应对两后端保持一致；但投递策略、容错与运维需后端感知。本地引擎不应为“快”而绕过 `Outbox`，远端也不应为“慢”而降低真源强度。

### 9.2 后端能力对照

| 维度 | Local (`LocalVectorEngine`) | Qdrant (`VectorManager`) | 同步层含义 |
|------|-----------------------------|--------------------------|------------|
| **部署与故障域** | 同进程、`mmap` 目录（`startup.rs:94 LocalVectorEngine::open(vector_data_dir)`） | 异进程/异机，`connection.host/http_port/api_key`（`startup.rs:164`） | Local 崩溃与图同命运；Qdrant 崩溃图仍可提交，需独立重试/降级 |
| **写入延迟** | 同步 `WAL fsync` + 内存应用，`after_mutation` 调度 `compaction/rebuild`（`engine.rs:402`） | 网络 `RTT` + 服务端 `WAL+f sync`，批量 `upsert_batch` 收益更高 | Local 可小 `batch` 低延迟；Qdrant 宜大 `batch` 高吞吐 |
| **持久化与幂等** | 自带 `WalTxn{txn_id, ops}` 幂等重放（`engine.rs:560 apply_txn`），派生索引可丢弃重建 | 服务端 `WAL` + 复制因子，客户端仅 `operation_id` 可观测 | Local 双 `WAL`（图+向量）存在双重 `fsync`，可优化为组提交对齐；Qdrant 幂等依赖 `idempotency_key` 翻译 |
| **配置语义** | `index_type={HNSW/IVF/FLAT}` `hnsw_config full_scan_threshold=10k` `ivf auto_promotion=false` `quantization`（`engine.rs:191`） | `HnswConfig{m=16,ef=100,payload_m=16}` `quantization` `shard_number/replication`（`vector_sync.rs:344`） | 同名 `CollectionConfig` 在两端语义不同；`startup.rs:102` 已做默认分支 |
| **Payload 索引** | 早期内存过滤，`supports_payload_index()=true` 但 `group_id` 仍需显式建 | 服务端倒排真实加速，`create_payload_index(Keyword)` 关键 | `create_vector_index` 中 `group_id` 索引 `best-effort warn`（`:363`）对 Qdrant 更重要 |
| **错误谱** | `IO/InvalidConfig/DimensionMismatch` 本地可判定，瞬时不可重试 | `Timeout/Transport/Auth/RateLimit/NotFound` 可重试/需退避，`Auth` 不可重试 | 重试分类器需后端感知（见改进方案） |
| **Disabled 语义** | 永不 `disabled`（`backend.rs:83 Local=>false`） | `manager.engine().name()=="disabled"` 显式降级 | 统一 `VectorEngineState` 已覆盖，但 `skip` 策略应差异化（Qdrant 倾向重试，Local 倾向失败） |
| **运维** | `vector_metrics::spawn_vector_metrics_sampler`（`startup.rs:157`）拉本地度量 | `spawn_remote_vector_metrics_sampler` 拉 `host:port` 远端度量 | 监控分流已落地；`rebuild/drift` 仅本地有意义 |

### 9.3 是否需要不同处理

**需要，但限于策略层，不改真源**：

1. **投递批大小与并发**：Local `batch=64~128 + 单并发` 可满足；Qdrant 宜 `batch=256~1024 + 每 collection 并发 2~4 + gRPC stream`。
2. **重试与退避**：引入 `BackendErrorKind::{Retryable, NonRetryable, Degraded}`；Qdrant `Unavailable/Timeout` 指数退避，`Auth/InvalidArgument` 直接 `dead_letter`；Local `DiskFull/DimensionMismatch` 直接 `dead_letter`。
3. **Disabled 降级**：Local 不应出现 `disabled`；Qdrant `disabled` 当前 `skip+ack` 永久丢数据，应改为 `retry` 保留事件（或 `degraded` 悬挂），由运维 `requeue` 自愈。
4. **一致性旋钮**：对外暴露 `consistency: eventual | read-your-writes(session)`；后者对两后端均走 `wait_for_minimum_lsn`，Qdrant 需更长 `timeout` 与 `frontier` 校准。
5. **DDL 语义**：`create_vector_index` 两端冲突检测一致，但 `drop` 时 `Local` 需物理 `delete_collection` 回收目录（`vector_sync.rs:416`），`Qdrant` 侧为 `delete_by_filter(group_id)` 逻辑隔离，行为已分支。
6. **派生索引维护**：`drift/rebuild/compaction` 仅 Local 后台线程有意义（`engine.rs:676 maintenance_loop`），Qdrant 侧由服务端自治，同步层不再调度。

**不需要不同的**：`staging → WAL → SQLite → claim/ack` 主链路、`idempotency/ordering` 键设计、`generation_state` 围栏、`checkpoint manifest` 原子发布约束，应保持统一以降低心智与测试矩阵复杂度。

---

## 10. 关键代码索引

- `crates/graphdb-sync/src/sync/manager.rs:52,139,368,688,836,1028,1325` — 暂存/投递/物化/向量分支/后台/键合成
- `crates/graphdb-sync/src/sync/sqlite_outbox.rs:110,207,668,797,887,1048,1150,1192` — 快照/表结构/物化/claim/ack/stats/wait/skip
- `crates/graphdb-sync/src/sync/vector_sync.rs:30,138,170,315,403,464,553,608` — 度量校验/物理集合/逻辑索引/DDL/批投递/搜索
- `crates/graphdb-sync/src/sync/backend.rs:30,82,184` — `VectorBackend` 零成本分发
- `crates/graphdb-sync/src/sync/receiver.rs:204,276,311` — 幂等文件闸
- `crates/graphdb-storage/src/storage/engine/sync_wrapper.rs:25,132,178,223,810` — 装饰器与两阶段
- `crates/graphdb-storage/src/storage/engine/graph_storage.rs:1701,1727` — 提交与回放
- `crates/graphdb-server/src/startup.rs:86,256,390` — 引擎装配与恢复序列
- `crates/graphdb-api/src/api/core/vector_api.rs:220,285` — 直写旁路与搜索契约
- `crates/vector-search/src/engine.rs:79,191,360,560` — 本地引擎 `WAL` 与幂等

---

## 11. 风险清单与下一步

上表 `R1~R10` 中 `R1/R2/R3` 建议进入 `P0/P1` 改进；`R5/R6/R7` 为性能与语义债务；`R8/R9/R10` 为长期治理。详见配套文档 `docs/plan/vector_sync_improvement_plan.md`。

