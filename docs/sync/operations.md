# 存储与同步运维手册

本文说明单节点部署中 WAL、SQLite outbox、checkpoint manifest 与 native index generation 的恢复和诊断流程。

## 恢复边界

已发布的 `checkpoint_<seq>` 只有在对应的组合 manifest 已原子发布后才可恢复。组合 manifest 同时引用存储 checkpoint、outbox snapshot 和 native-index manifest；WAL 只能截断到该 manifest 的 `safe_lsn`。

启动时会执行以下处理：

- 删除 `checkpoint_*.tmp` 临时目录；
- 删除序号高于最新有效组合 manifest 的 checkpoint 目录；
- 从最新 checksum 有效的组合 manifest 恢复；若最新 manifest 损坏，自动尝试更早的有效 manifest；
- SQLite outbox 文件缺失或无法打开时，从 `outbox_snapshots/` 中最新 checksum 有效的 snapshot 恢复，再从其 `materialized_lsn` 之后回放已提交 WAL intent。

因此不得手动删除 `outbox_snapshots/`、已发布 checkpoint 或 manifest 中引用的 native-index 文件。需要释放空间时，应由 checkpoint 保留策略统一回收。

## checksum 损坏处理

1. 停止进程并保留现场副本，尤其是 `checkpoints/manifests/`、`outbox/`、`outbox_snapshots/` 和 WAL 目录。
2. 重启服务。manifest loader 会跳过 checksum 或引用文件校验失败的 manifest，选择上一份有效 manifest。
3. 若 SQLite 文件损坏，保留原文件后删除或隔离它；启动恢复会使用最近有效 outbox snapshot。没有可用 snapshot 时，不应继续写入，应从备份恢复并保留 WAL 供人工处理。
4. 恢复完成后创建新的 checkpoint，确认新的组合 manifest 已发布，再按正常策略回收旧文件。

## dead-letter、requeue 与 degraded consistency

投递失败超过重试上限时，事件进入 SQLite `dead_letters`，原事件状态为 `dead_letter`。先定位目标端失败原因并修复，再调用 `SqliteOutbox::requeue_dead_letter(event_id)` 将事件恢复为 `pending`。不要直接修改 `events`、`commit_targets` 或 frontier 表。

若必须放弃某个事件，应通过 `skip_event_degraded()` 标记。该操作会写入 `degraded_ranges` 并推进 frontier，但会使受影响 target/index generation 的一致性永久降级。调用 `wait_for_minimum_lsn()` 遇到覆盖所请求 LSN 的 degraded range 会返回错误，而不是把“已跳过”误报为已一致。

## rebuild、split 与旧 generation 回收

rebuild 和 split 依次经历 `Building`、`CatchingUp`、`Publishing`、`Active` 状态。`Publishing` 保存 barrier LSN；只有新 generation 完整追平该 LSN、generation 文件已 fsync 且 manifest 已发布后，writer 才会切换到新 generation。

诊断时读取 `SyncManager::sync_diagnostics()` 或 `SqliteOutbox::diagnostics()`：

- `materialized_lsn` 是 WAL intent 已投影到 SQLite 的边界；
- target 项提供 applied frontier、lag、pending/retrying/leased/dead-letter 数、最老待投递事件年龄和 degraded 标记；
- index 项提供 generation lifecycle 状态、barrier LSN、index frontier、lag 与 degraded 标记。

`ManifestCatalogStats` 还提供 active epoch/generation、active readers、retired generations、发布和回收计数。只要长读仍持有旧 manifest handle，旧 generation 文件都不得删除；当 active readers 归零并被 catalog 标记为可回收后，才可由 checkpoint 回收流程删除。

## 演练建议

- 定期演练：删除 SQLite outbox，保留最近 snapshot 和后续 WAL，验证 pending event 能恢复。
- 定期演练：损坏最新 manifest 或 outbox snapshot，验证自动回退到前一有效版本。
- 在压测环境中对 checkpoint 的 redo 前、intent 中间、commit 中间、fsync 后和 visibility publish 前故障点逐一注入故障，确认未发布的 checkpoint 不会被恢复。
- 在有并发写和长读的环境验证 rebuild/split 后，查询结果、frontier 与旧 generation 文件回收时机均符合预期。
