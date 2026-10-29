# 事务 / 同步 / 死信整改方案

> 配套分析见 `txn-sync-dlq-analysis.md`。本文档是唯一允许出现任务编号的地方，
> 代码注释中不得引用这些编号。

## 任务 T1：删除内存版死信（已确认，优先执行）

- 删除 `crates/linkrs-sync/src/dead_letter_queue.rs` 及其在 `lib.rs` 的
  `mod / pub use`。
- `FulltextSyncCoordinator`：删除 `dead_letter_queue` 字段、`dead_letter_queue()`
  访问器、`recover_dead_letter / retry_dead_letter /
  recover_operations_for_indexes` 方法；批失败路径改为只记日志 +
  按 `failure_policy` 标记 `Inconsistent`，不再 `dlq.add()`。
- `SyncManager`：删除 `dead_letter_queue` 字段、`with_dead_letter_queue`、
  `get_dead_letter_entries / get_unrecovered_entries /
  get_old_dead_letter_entries / remove_dead_letter_entry` 访问器。
- `SyncManagerBuilder`：删除 `with_dead_letter_queue`。
- `SyncApi::get_dlq_size / get_unrecovered_dlq_size` 改为返回
  `outbox_stats().dead_lettered` 的兼容垫片（保持 HTTP / gRPC / 前端字段不断裂）。
- `http/handlers/sync.rs`、`grpc/sync.rs` 的 `dlq_size` 字段改从
  `outbox.dead_lettered` 取值，并在注释说明其为持久化死信计数的别名。
- 更新 `tests/sync/{types,recovery_e2e,dlq_recovery,fault_tolerance}.rs`：
  删除内存 DLQ 用例，保留/改写为持久化 `dead_letters / requeue` 用例。
- 验收：`rg DeadLetterQueue --glob '*.rs' crates/ tests/` 无命中
  （`DeadLetterRow` 持久化类型除外）；`cargo test -p linkrs-sync` 通过。

## 任务 T2：统一 SyncStatus 口径（随 T1 落地）

- `SyncStatusResponse { dlq_size / unrecovered_dlq_size }` 明确为
  `outbox_dead_lettered` 的别名；OpenAPI 描述注明“已废弃的内存计数，
 恒等于持久化死信数”。
- 前端 `mock/handlers/monitoring.ts` 保持字段名不变。
- 验收：停用同步时两处计数均为 0；有死信时三者一致。

## 任务 T3：澄清按事务重试语义

- 现状：`POST /v1/transactions/{id}/outbox/retry` 校验 id 存在后做全局重放。
- 整改（不改变行为契约）：OpenAPI 描述 + 响应体注明 `scope: global`，
  说明“按 id 校验、按全局重放”；前端确认文案同步。
- 远期（不在本次）：`SqliteOutbox` 按 `transaction_id` 过滤重试。
- 验收：接口文档与响应自解释，不再误导为只影响单事务。

## 任务 T4：统一事务观测鉴权

- `GET /v1/transactions`、`GET /v1/transactions/metrics` 补
  `require_admin_session`（与同步系一致）；`kill` 保持 owner 校验并要求
  已登录会话；CLI 透传 `x-transaction-owner`（`--owner` 参数或沿用登录用户）。
- 验收：未登录访问观测接口被拒绝；CLI 可指定 owner。

## 任务 T5：CLI 参数健壮性

- `\sync` 支持 `key=value` 与 `--flag value` 两种写法，保留位置参数兼容；
  非法 target / 数字解析错误信息保持不变。
- 验收：新旧写法等价，`--help` 输出示例。

## 任务 T6：Retention 默认值配置化

- 将 `retention_run` 的 `grace_lsn_distance / max_age_ms` 默认值收敛到
  `SyncConfig`（或 outbox 配置），handler 只做透传。
- 验收：改配置即生效，无需改代码。

## 任务 T7：前端死信运维页面

- 在 `sync/+page`（或 monitoring）补死信列表 / 重入队 / 降级区间 /
  retention 运行面板，复用现有 `syncService`。
- `DeadLetterRow / DegradedRangeRow` 从 `unknown` 收紧为可选类型字段。
- 验收：页面可完成“查死信 → 重入队 → 查降级 → 清区间 → 跑留存”闭环。

## 执行顺序

T1 → T2（同一批代码）→ T3 → T4 → T6 → T5 → T7。
