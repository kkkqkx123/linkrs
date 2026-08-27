# 向量与图存储同步改进方案

> 状态：方案文档（2026-08-27）  
> 前置文档：
> - `docs/plan/vector_sync_analysis.md`（本改进的全部事实基线）
> - `docs/sync/operations.md`（恢复与 degraded 语义）
> - `docs/vector/vector-engine-design.md`（分数与过滤翻译契约）
> - `docs/sync/architecture.md`（待以本文修订）

---

## 1. 目标与非目标

### 1.1 目标

- 保持“图 `WAL` 单真源 + `SQLite outbox` 持久队列 + `异步投递`”主架构不变（已验证可靠）。
- 补齐一致性暴露与降级语义，使调用方可显式选择 `eventual` vs `read-your-writes`。
- 收敛 `VectorApi` 直写旁路与顶点属性 `Outbox` 的语义分叉。
- 按后端差异化投递与容错，同时保持主干统一以降低测试矩阵。
- 引入背压与并发投递，治理 `Disabled 静默分歧`/`ordering_key 同质化`/`幂等集无界` 等债务。

### 1.2 非目标

- 不引入图-向量跨存储 `2PC/XA`（单节点无收益，显著增可用性风险）。
- 不将 `Qdrant` 与 `Local` 拆为两套独立同步链路（维护成本与语义分叉）。
- 不改变 `VectorBackend` 的 `enum` 零成本分发形态（保持 `backend.rs:30` 设计）。

### 1.3 设计原则

- **单真源不弱化**：即使 Local 同进程，也不绕过 `WAL+Outbox` 形成双写分支。
- **内核统一，策略分支**：`stage/materialize/claim/ack/frontier` 统一；`batch/retry/degraded/disabled` 按后端策略化。
- **可观测优先于智能**：先让 `frontier_lag/dead_letter/disabled_skips/degraded_ranges` 可监控与可干预，再做自愈自动化。
- **失败分类再重试**：`Retryable / NonRetryable / Auth` 显式分类，避免 `DimensionMismatch` 无限重试。
- **兼容与可回滚**：所有持久格式变更带 `OUTBOX_SCHEMA_VERSION` 迁移与 `WAL wire_version` 兼容。

---

## 2. 后端差异化结论

见分析文档 §9。结论重申：

- **不需要**为 `Qdrant`/`Local` 建立两套 `WAL` 与 `SQLite` 链路。
- **需要**在以下策略点按后端分支：
  1. 批大小与并发度（`Local` 小批单并发；`Qdrant` 大批多并发 + `gRPC stream`）
  2. 重试分类与退避（`BackendErrorKind`）
  3. `Disabled` 降级（`Qdrant disabled` 不应 `ack` 丢弃）
  4. 一致性旋钮的 `timeout` 与阈值
  5. 物理回收（`Local delete_collection` vs `Qdrant delete_by_filter(group_id)`）
  6. 派生索引维护（`Local drift/rebuild`，`Qdrant` 服务端自治）

落地形态：`VectorBackend` 保持 `enum`，投递策略抽象为 `BackendDeliveryPolicy { batch_size, max_concurrency, retry_classifier, disabled_behavior }`，由 `startup::attach_vector_coordinator` 按 `VectorConfig.engine` 注入，不在 `manager.rs` 散落 `if is_local`。

---

## 3. 总体方案

```
Graph Write → SyncWrapper::stage_intent(按 payload 类型过滤 target)
           → WAL (graph redo + OutboxIntent)
           → SyncWrapper::finalize_commit → SqliteOutbox::materialize
           → Outbox投递器(统一 claim/ack) → 后端策略分支 → VectorSyncCoordinator → VectorBackend
                                         ├─ Local: 小批 + 同步 WAL 组提交对齐 + 本地 WAL 幂等
                                         └─ Qdrant: 大批 + 并发 + 分类重试 + gRPC stream
           → VectorReceiver 幂等文件 + SQLite frontiers 推进
           → 可选：SEARCH 时 wait_for_minimum_lsn(session) 读己之写
```

---

## 4. 分阶段实施

### Phase 0 — 止血与可观测（1~2 周，不改语义）

| # | 改造 | 涉及文件 | 验收 |
|---|------|----------|------|
| 0.1 | **修复 `ordering_key` 同质化**：`payload_to_intent` 改为 `"{target}:{space}:{tag}:{field}:{entity_ref}"` 或显式 `entity_ordering_key`，使同实体串行栅栏生效；补 `NOT EXISTS earlier` 语义注释 | `sync/manager.rs:1415` `sqlite_outbox.rs:828` | 同顶点并发更新按 `commit_lsn` 串行，无乱序覆盖；`sync/tests/vector_outbox_delivery.rs` 新增并发同实体回归 |
| 0.2 | **按内容过滤投递目标**：`stage_intent` 前按 `OutboxPayload` 内容与 `index_exists` 预过滤 `target`，避免 `vector+fulltext` 各克隆一份的写放大 | `sync/manager.rs:139` | 仅含向量属性的顶点不再产生 `fulltext` 事件，`WAL/SQLite` 写入量下降；`EXPLAIN` 无跨目标冗余 |
| 0.3 | **可观测补齐**：`OutboxDiagnostics` 增加 `disabled_skips` 透出；`StatsManager::record_outbox_state` 增加 `per_target lag` 直方图；`VectorSyncCoordinator::disabled_skip_count` 接入 `metrics` | `vector_sync.rs:258` `manager.rs:443` `metrics` | `Grafana` 可告警 `disabled_skips>0` 或 `frontier_lag>threshold` |
| 0.4 | **文档修订**：以本文替换 `docs/sync/architecture.md:242` 的 `2PC` 描述，标注 `Outbox` 单 fence 语义 | `docs/sync/architecture.md` | 架构图与时序与代码一致 |

### Phase 1 — 一致性与语义收敛（2~4 周）

| # | 改造 | 涉及文件 | 验收 |
|---|------|----------|------|
| 1.1 | **一致性旋钮**：`SearchOptions`/`QueryRequest` 增加 `consistency: Eventual | ReadYourWrites{ timeout_ms }`；`SEARCH` 执行器在 `ReadYourWrites` 时调 `SqliteOutbox::wait_for_minimum_lsn(target,index_id,generation, commit_lsn, timeout)` 阻塞至 `frontier >= session_commit_lsn`，遇 `degraded` 报错而非脏读 | `sync/vector_sync.rs:95` `sync/sqlite_outbox.rs:1150` `query/executor/...` `vector_api.rs:285` | 会话内 `INSERT` 后 `SEARCH` 在 `eventual` 下可能滞后，在 `ryw 2s` 内可见；`tests/vector_query_e2e.rs` 新增 `ryw` 用例 |
| 1.2 | **直写旁路收敛**：`VectorApi::insert_vector/batch` 新增 `mode: Direct | Transactional{txn_id,space,tag,field}`；`Direct` 保留现语义并文档化“非事务”；`Transactional` 走 `SyncManager::stage_intent` 入 `Outbox`，接受 `session` 一致性 | `api/core/vector_api.rs:220` `sync/manager.rs:872` | 事务内 `insert_vector` 可回滚；`Direct` 与 `Outbox` 路径有明确语义分界 |
| 1.3 | **`Disabled` 降级修正**：`VectorBackend::Qdrant disabled` 分支改为 `retry` 保留事件（`retry_count` 不计入 `dead_letter` 阈值或独立 `disabled_retries`），不 `ack`；`Local` 永不 `disabled` 保持 `Err`；新增运维 `requeue_disabled` 接口 | `sync/vector_sync.rs:468` `sync/manager.rs:398` `server/graph_service.rs` | `disabled` 期间事件在重开后可自愈；`retry_outbox_sync` 不丢弃 `degraded_skips` |
| 1.4 | **幂等集治理**：`VectorReceiver` 从全量 `HashSet` 改为 `LSN 水位 + LRU 窗口`（`applied_lsn` 前 `K` 个 `idempotency_key` 保留，或按 `checkpoint` 截断）；`events` `applied/skipped` 按保留策略异步清理（`retention_lsn`） | `sync/receiver.rs:198,311` `sync/sqlite_outbox.rs` | 重启后对 `applied_lsn` 后的重复仍幂等，早于水位的重复依赖 `SQLite` 去重；内存与 `vector_receiver_state.bin` 体积收敛 |
| 1.5 | **ID 规范化**：`point_id` 从 `"{vid}_{tag}_{field}"` 改为 `"{vid}#{tag}#{field}"` 并对 `vid` 做 `base64/url` 或 `hash` 转义，跨后端稳定 | `sync/manager.rs:744` | 含 `_` 的 `vid` 无歧义；`Local` 与 `Qdrant` 点 `ID` 互通 |

### Phase 2 — 性能、背压与容错（3~6 周）

| # | 改造 | 涉及文件 | 验收 |
|---|------|----------|------|
| 2.1 | **背压**：`SyncManager::stage_intent` 引入 `max_pending_intents_per_txn / max_outbox_pending` 限流，`pending` 超阈值时 `Err(OutboxBackpressure)` 使写事务感知退避；`GraphStorage` 暴露 `outbox_pending` 指标驱动客户端限速 | `sync/manager.rs:139` `storage/.../context.rs` | 压测下 `outbox_pending` 有界，`p99` 投递延迟可控，不再 `DashMap` 无界增长 |
| 2.2 | **并发投递**：每 `target` 从单线程轮询改为 `concurrent_claims=1(Local)/4(Qdrant)` + 每 `collection` 并发 `upsert_batch`；`claim_next` 保持 `BEGIN IMMEDIATE + lease_epoch`  fencing，无跨 `consumer` 重叠 | `sync/manager.rs:368,836` | `target=vector` 吞吐 `>5x`，`ORDER BY commit_lsn` 全局有序仍保证，压测无重复 `ack` |
| 2.3 | **错误分类重试**：引入 `VectorErrorKind::{Retryable, NonRetryable, Auth}`；`retry_outbox_sync` 仅对 `Retryable` 指数退避，`NonRetryable`（`DimensionMismatch/InvalidConfig`）直入 `dead_letter`，`Auth` 暂停投递并告警 | `sync/vector_error.rs:212` `sync/manager.rs:398` | `DimensionMismatch` 不再 16 次重试后才 `dead_letter`；`Auth` 失败不空转 |
| 2.4 | **后端策略化**：`BackendDeliveryPolicy` 由 `startup` 按 `VectorConfig` 注入 `SyncManager`，含 `batch_size / lease_ms / max_retries / policy_name`；`supports_payload_index/streaming` 保持 `true` 但 `group_id` 投递路径分支清晰 | `server/startup.rs:390` `sync/backend.rs` `sync/manager.rs:68` | `Local` 与 `Qdrant` 行为可独立调优，测试用 `from_config` 覆盖 |
| 2.5 | **点投递与 `Local WAL` 组提交对齐**：`Local` 批量 `apply_txn` 复用 `collection` `fsync` 组提交，减少图 `WAL fsync` 与向量 `WAL fsync` 的双重抖动 | `vector-search/src/engine.rs:560` `storage/.../context.rs` | `INSERT` 批量 `p95` 下降可测，`WAL` `fdatasync` 次数下降 |

### Phase 3 — 治理与长期演进（6 周后）

| # | 改造 | 涉及文件 | 验收 |
|---|------|----------|------|
| 3.1 | **向量 MVCC 显式文档与可选增强**：默认保持向量无 `MVCC`（避免图-向量跨存储快照复杂度），在 `docs/vector/README.md` 明确隔离级别差异；如需可选 `SSI` 读集校验，再将 `VectorFilter` 读集纳入 `TransactionManager::certify` 可选开关 | `docs/vector/*` `transaction/manager.rs:724` | 行为文档化，有开关与测试隔离 |
| 3.2 | **空间级集合的替代选型**：评估 `space_{id}` → `{space}_{tag}_{field}` 细粒度集合的资源与配置隔离收益；若落地则提供在线迁移（`group_id` 重分布 + `index_generation` `Publishing` 切换） | `sync/vector_sync.rs:156` | 迁移不停服，同 `space` 异构 `dimension` 成为可能 |
| 3.3 | **运维自愈**：`SqliteOutbox::requeue_dead_letter` + `skip_event_degraded` 接入 `HTTP /sync/outbox/*` 与 `CLI`，支持按 `target/index_id` 批量 `requeue`；`degraded_ranges` 可视化 | `sync/sqlite_outbox.rs:1000,1192` `server/http_server.rs` | 故障后人工一键重放 |
| 3.4 | **保留与截断**：`projection_state.retention_lsn` + `events` 归档/截断，与 `checkpoints/manifests` 保留策略联动 | `sync/sqlite_outbox.rs:300` `storage/.../persistence.rs` | 磁盘可控，`WAL` 截断水位不再被无限 `applied` 事件拖慢 |

---

## 5. Qdrant / Local 差异化落地清单

| 策略项 | Local | Qdrant | 落地位置 |
|--------|-------|--------|----------|
| `batch_size / lease_ms` | `128 / 30s` 保持 | `512 / 60s`，`max_retries=32` | `OutboxConsumerConfig` 按 `VectorConfig.engine` 分支 |
| 并发 | 单并发 `claim` + 单集合串行 `apply` | `claim 4 并发` + `collection 并发` | `SyncManager::retry_outbox_sync` 策略分支 |
| 重试分类 | `DiskFull/DimensionMismatch` 直 `dead_letter` | `Timeout/Unavailable` 重试，`Auth/InvalidArgument` 直 `dead_letter` | `VectorErrorKind` |
| `Disabled` | 永不 `disabled`，失败抛错 | `disabled` 时 `retry` 挂起，不 `ack` | `VectorSyncCoordinator::on_vector_change_batch:468` |
| 物理回收 | `delete_collection` 删目录（`is_local && siblings==0`） | `delete_by_filter(group_id)` 逻辑删 | `vector_sync.rs:416` 已分支，保持 |
| 一致性超时 | `ryw timeout 500ms` 典型 | `ryw timeout 2000ms` 典型 | `SearchOptions.consistency` |
| 派生索引 | `maintenance_loop` `drift/rebuild/compaction` | 无，由 Qdrant 服务端自治 | `vector-search/src/engine.rs:676` |
| 监控 | `spawn_vector_metrics_sampler` | `spawn_remote_vector_metrics_sampler` | `startup.rs:157,167` |

---

## 6. 兼容性与迁移

- **`OUTBOX_SCHEMA_VERSION`** 递增时：`SqliteOutbox::migrate` 幂等新增列/索引；旧 `events.ordering_key` 语义修正需一次性 `UPDATE` 回填（`target:space:tag:field:entity`），离线脚本执行。
- **`WAL wire_version`** 保持 `1`，新增 `payload` 类型经 `postcard` 向前兼容。
- **`vector_receiver_state.bin`** 格式变更时：`postcard` 解码失败回退空集重建（`receiver.rs:238` 已有 `ok().and_then` 容错），结合 `materialized_lsn` 重放保证不丢。

---

## 7. 测试与验证

| 测试 | 覆盖 | 位置 |
|------|------|------|
| 回归：`staged_vector_intent_is_delivered_to_local_backend` | `Outbox`→`Local` 端到端 | `sync/tests/vector_outbox_delivery.rs:34` |
| 新增：同实体并发序列 | `ordering_key` 栅栏正确性 | 新增 `sync/tests/vector_ordering.rs` |
| 新增：`read-your-writes` | 会话 `commit_lsn` ≤ `frontier` 前 `SEARCH` 阻塞/超时语义 | `tests/vector_query_e2e.rs` 扩充 |
| 新增：`disabled` 重开自愈 | `Qdrant disabled` 期间事件重放 | `sync/tests/vector_disabled_recovery.rs` |
| 新增：`Direct vs Transactional` `VectorApi` | 旁路语义分界与回滚 | `api/tests/vector_api_sync.rs` |
| 压测：`batch insert 10k vertices` | `outbox_pending` 有界、并发投递吞吐 | `benches/vector_sync_bench` |

---

## 8. 运维与发布

- **灰度**：`Phase 0` 可直接全量；`Phase 1` 一致性旋钮默认 `eventual` 保持兼容，`ryw` 显式开启。
- **回滚**：任一 `Phase` 回滚仅影响投递策略，不丢 `WAL` 真源；`SqliteOutbox` 为派生，重建成本为重放 `WAL`。
- **监控新增**：`sync_outbox_pending`、`sync_outbox_frontier_lag{target,index}`、`sync_vector_disabled_skips`、`sync_dead_letter_count`、`vector_local_pending_len` 告警阈值。

---

## 9. 替代方案讨论

| 方案 | 结论 |
|------|------|
| 跨存储 `2PC`（图+向量同原子提交） | 否：单节点引入协调者与 `prepare` 阻塞，得不偿失；`Outbox` 已满足 `durable + retry` |
| `CDC` 文件尾随（`tail WAL` 异步线程直推向量） | 功能等价于当前 `Outbox`，但 `SQLite frontiers` 提供的 `claim/lease/幂等/degraded` 成熟度更高，保留 `SQLite` |
| `Local` 绕过 `Outbox` 同步写 | 否：破坏单真源，引入回滚不一致与双 `WAL` 顺序不确定性；仅在 `P2.5` 中做 `fsync` 组提交对齐而非旁路 |

---

## 10. 风险与对策

- **`ordering_key` 迁移**：回填期间 `claim_next` 可能短暂无 `NOT EXISTS` 命中，属预期；灰度窗口控制写流量。
- **`ryw` 阻塞放大延迟**：默认不开启，调用方按会话 `SLA` 显式选择 `timeout`，超时回退 `eventual` 并打点。
- **并发投递乱序**：全局 `commit_lsn` 排序 + 同实体 `ordering_key` 栅栏保证串行；多 `consumer` 时 `lease_epoch`  fencing 防止重叠。

