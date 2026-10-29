# 事务 / 同步 / 死信状态与操作分析

> 日期：2026-10-29；范围：`linkrs-transaction`、`linkrs-sync`、`linkrs-api`、`linkrs-server`、`linkrs-cli`、`frontend`。
> 代码注释中不得出现计划编号；本文档是唯一允许出现任务编号的地方。

## 1. 事务

### 1.1 状态模型（`linkrs-transaction::types`）

- `TransactionState`：`Active → Committing / Aborting → Committed / Aborted`，共 5 态。
  `can_commit() = Active`；`can_abort() = Active / Committing / Aborting`；
  终态仅 `Committed / Aborted`。`Committing` 失败不清零，保留可恢复语义。
- `TransactionType`：`ReadOnly / Write / Checkpoint / Recovery / Dummy`。
  `is_user_transaction()` 仅前两者；`is_system()` 为后三者；
  `requires_wal() = Write / Checkpoint`；`uses_mvcc() = ReadOnly / Write`。
  系统事务不得暴露给用户 API。
- 选项：`TransactionOptions { read_only, timeout, durability, isolation_level }`，
  `IsolationLevel` 为事务语义（与存储拓扑的 `SpaceIsolationLevel` 不同）。
- 子状态：`Savepoint { create / rollback_to / release / list }`，
  需配合 `StagedWriteMark + UndoTarget` 使用。
- 可观测快照：`TransactionInfo { id / state / owner / read_ts / write_ts / elapsed /
  last_activity / rollback_only / staged_bytes / undo_bytes / blocking_reason }`；
  `TransactionStats { begun / active / committed / aborted / timeout / conflict /
  disconnect / recovery_abort / cleanup_failure }` +
  `TransactionResourceMetrics { active_snapshots / pending_writes / frontier_lag /
  staged_wal / undo_bytes / checkpoint_drain }`。
- 写冲突走 OCC（`WriteSet / SSI / ConflictReport`），不是独立状态，
  只体现在 `metrics.conflict` 计数上。

### 1.2 操作

- `TransactionApi { begin / commit / rollback / get_status / is_active /
  active_count / savepoint* }`；`TransactionManager { list_transactions /
  kill_transaction / check_transaction_owner / retry_outbox_projection /
  recover_stale_rebuilds }`。
- HTTP：`POST /v1/transactions`（begin）、事务提交/回滚（会话级）、
  `GET /v1/transactions`（列表）、`GET /v1/transactions/metrics`、
  `POST /v1/transactions/{id}/kill`（需 `x-transaction-owner`）、
  `POST /v1/transactions/{id}/outbox/retry`。
- gRPC：`list / kill / get_metrics / retry_outbox` 对等实现。
- CLI 会话级：`\begin / commit / rollback [to] / savepoint / release /
  autocommit / isolation`；管理级：`\transactions list | metrics |
  kill <id> | retry-outbox <id>`。
- 前端：`operations.ts { transactions / killTransaction /
  retryTransactionOutbox }` + `statistics.ts { transactionMetrics }`，
  `transactions/+page.svelte` 为全功能页（含 kill / retry 确认框）。

## 2. 同步

同步只做协调（引擎在 `linkrs-fulltext / simvec / vector-client`），
持久化真相在 SQLite `events` 表。

- `events.status`：`pending / retry / leased / applied / skipped / dead_letter`，
  附 `retry_count / lease_owner / lease_epoch / next_attempt_at_ms / commit_lsn`。
- `SyncDiagnostics { materialized_lsn / targets[] / indexes[] /
  vector_disabled_skips }`、`TargetSyncDiagnostics`、`IndexSyncDiagnostics
  { target / index_id / generation / state / barrier_lsn / applied_lsn / lag /
  degraded }`：前沿水位 + 降级视图。
- `OutboxStats { pending / retries / oldest_age / dead_lettered / leased /
  write_amplification_bytes / lock_wait / persist_ops }` 由 SQL 聚合得到；
  其中 `write_amplification_bytes` 实际是 DB 文件大小，命名误导。
- `ConsistencyState { Consistent / Inconsistent / Rebuilding }`
  （`linkrs-core::types::index`）：引擎级健康，`Inconsistent` 拒绝写入，
  需在线重建或删建。
- `GenerationState { Building → CatchingUp → Publishing → Active }`
  （`linkrs-storage::index::manifest`）：原生索引重建机，崩溃可恢复。
- `VectorEngineState { Disabled / Active }`、`CircuitState { Closed / Open /
  HalfOpen }`：向量降级计数 + 远端熔断。
- `SyncApi`：`is_running / outbox_stats / retry_outbox_projection /
  sync_diagnostics / inconsistent_*_indexes / recover_stale_rebuilds /
  list_dead_letters / requeue_* / list_degraded_ranges / clear_degraded_range /
  retention_lsn / prune / run_retention_once`。
- HTTP（`handlers/sync.rs`，8 组）：`GET /v1/sync/status |
  /outbox/diagnostics | /outbox/dead_letters | /outbox/degraded_ranges |
  /outbox/retention/status`，`POST /outbox/retry | /outbox/requeue |
  /outbox/degraded/clear | /outbox/retention/run`。
- CLI `\sync status | diagnostics | dead-letters | requeue | retry |
  degraded | clear | retention-status | retention-run`，破坏性操作二次确认。
- 前端 `services/sync.ts` 全量覆盖，但 `sync/+page` 只展示 status，
  死信/降级/留存只有 service 没有专属页面。

## 3. 死信（双轨并存，待整改）

| 实现 | 形态 | 操作 |
|---|---|---|
| 内存 `DeadLetterQueue { DeadLetterEntry { operation / error / retry_attempts / first_failure / last_failure / recovered } }` | `add / get_all / get_unrecovered / remove / mark_recovered / cleanup / clear / get_stats` | `FulltextSyncCoordinator` 本地批失败路径 + `SyncManager` 可选字段 |
| 持久 SQLite `dead_letters` + `DeadLetterRow` + `DegradedRangeRow` | `list_dead_letters / requeue_dead_letter / requeue_batch / list_degraded / clear_degraded / retention` | HTTP / CLI / 前端实际走这条路 |

`SyncStatusResponse { dlq_size / unrecovered_dlq_size }` 读内存，
`outbox_dead_lettered` 读 SQLite，两处计数可能打架。

## 4. 前端 / CLI 覆盖矩阵

| 能力 | CLI | 前端 | 后端 |
|---|---|---|---|
| 事务列表 / 指标 / kill | ✅ | ✅ | ✅ |
| 事务 savepoint | ✅（会话） | ❌ | ✅（API） |
| sync status / diagnostics | ✅ | ✅（status）/ 半（diag 无页） | ✅ |
| 死信查询 / 重入队 / 全局重试 | ✅ | ✅（service，无页） | ✅ |
| degraded 查询 / 清除 | ✅ | ✅（service，无页） | ✅ |
| retention 运行 / 水位 | ✅ | ✅（service，无页） | ✅ |

## 5. 设计评价

合理处：用户/系统事务显式隔离；`Committing` 可 abort 不可 commit 的恢复语义；
SQLite 为真相的投递机；重建机崩溃语义明确；破坏性操作二次确认 + 同步系 admin 鉴权。

问题：

1. 双死信并存，内存与 SQLite 计数口径不一致（已决定删除内存版）。
2. `POST /v1/transactions/{id}/outbox/retry` 名为按事务重试，实为全局重放。
3. 鉴权不一致：同步全系要求 admin，而 `list_transactions / metrics` 无鉴权，
   `kill` 只验 owner 头；CLI `kill` 不暴露 owner 参数。
4. 终因（超时/冲突/kill）只在 metrics 计数，不在状态机；列表只返活跃事务。
5. `write_amplification_bytes` 名不副实；`requeue / retry / retention` 对运维不直观；
   前端死信/降级/留存无页面。
6. CLI `\sync` 全位置参数，易错；retention 默认值硬编码在 handler。
