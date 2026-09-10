# Event Hook 整改方案

现状分析见上轮结论：无统一 `EventBus`，6 套独立回调 + migration trait + C-API veto 钩子。
共性是同步 inline + `catch_unwind` + best-effort。本方案只做收敛和补洞，不引入统一总线和异步分发。

## 目标

1. 统一分发底座，消灭重复实现。
2. 订阅可注销 + 可过滤，解决泄漏和全量轰炸。
3. migration 监听 panic 隔离。
4. 补事件缺口，语义保真。
5. C-API update_hook 保真改进。

## 任务清单

### R1 统一分发底座

- `graphdb-transaction/src/manager.rs:142 emit_commit_event / 158 emit_rollback_event` 改调
  `graphdb_core::event_dispatch::dispatch_event_callbacks`，保留 `increment_cleanup_failure` 计数语义。
- 不改触发顺序（终端态后发射）和 CoW 快照语义。

验收：`cargo test -p graphdb-transaction lifecycle_callbacks` 通过。

### R2 可注销订阅 + 过滤

- 在 `graphdb-core/src/event_dispatch.rs` 新增通用注册表：

```rust
pub type SubscriptionId = u64;
pub struct EventSubscriptions<E> { /* next_id + RwLock<Vec<Entry<E>>> */ }
// Entry { id, callback: Arc<dyn Fn(&E)+Send+Sync>, filter: Option<Arc<dyn Fn(&E)->bool+Send+Sync>> }
```

`add / add_filtered / remove / len / is_empty / dispatch(owner, event)`。
`dispatch` 按 filter 筛选后复用 `dispatch_event_callbacks` 的 panic 隔离。
- 迁移以下存储保持方法名兼容（`register_*` 改为返回 `SubscriptionId`，调用方可忽略）：
  `TransactionManager(commit/rollback)`、`SchemaManager`、`IndexManager`、
  `PersistenceCoordinator`、`FulltextIndexManager`、`VectorIndexManager`、
  `QueryManager`、`GraphSessionManager`。
  每个新增 `unregister_* (id) -> bool` 和 `register_*_filtered`。
- `schema_callback_count / storage_callback_count` 等保留，改调 `len()`。

验收：每个管理器单测覆盖注册—触发—注销后不再触发—filter 生效—panic 隔离。

### R3 migration 监听 panic 隔离

- `graphdb-migration/src/event.rs` 新增 `notify_migration_listener(listener, event)`，
  内部 `catch_unwind(AssertUnwindSafe)` + `log::error`，返回 `bool` 表示是否存活。
- `executor.rs` 所有 `listener.on_event(...)` 改调该 helper（约 8 处：Started/dry_run Completed/Failed、
  checksum Failed、空/纯 schema Completed、StepStarted/Failed/StepCompleted/Completed）。
- 保持 `MigrationEventListener` trait 签名不变（按值 `Clone` 语义不变）。

验收：panic 的 listener 不中断迁移，后续步骤事件仍可达；现有 migration 测试通过。

### R4 事件缺口补齐

- R4a 批量删索引补事件：`index_manager.rs:251 drop_tag_indexes_by_tag` 和
  `:314 drop_edge_indexes_by_type` 先收集被删索引名（写锁内），`drop` 锁后逐个发射
  `TagIndexDropped / EdgeIndexDropped`。空集不发射。
- R4b compaction 语义正名：`persistence.rs:500/549` 的 `space_id=0` 是有意的全局哨兵
  （compaction 跨 space）。引入 `storage_events::GLOBAL_COMPACTION_SPACE_ID: u64 = 0`
  具名常量 + 文档，两处调用点改用常量。不伪造分 space 事件。
- R4c `BudgetWarning` 可观测：`TransactionEvent::BudgetWarning` 今天只定义无发射，
  而 `context.rs:853/877` 只打 `log::warn`。在 `TransactionContext` 新增
  pending 队列 `drain_budget_warnings()`（exactly-once），warn 触发点同时入队；
  `TransactionManager` 在提交路径顺手 drain 并经 `emit_commit_event` 扇出；
  对外暴露 `drain_context_budget_warnings(&ctx)` helper。解决“定义了收不到”。
- R4d 重放标记：`TransactionEvent::Committed` 新增 `replayed: bool` 字段
  （正常提交 `false`，`commit.rs:441 recover_pending_finalization` 重放 `true`），
  消费者可去重。字段新增允许（项目无 backward-compat 约束）。

验收：批量删索引测试收到 N 个 Dropped；重放提交 `replayed=true`；budget warning drain 测试。

### R5 C-API update_hook 保真

文件 `graphdb-api/src/embedded/c_api/query.rs:205 detect_data_modification`：

- 容忍前导空白/行注释（`--`、`//`、`#`、`/*`）后再判关键字。
- 关键字覆盖 Cypher/类 SQL：`INSERT/UPDATE/DELETE/REMOVE`（保留映射 1/2/3/2）+
  `CREATE/MERGE/SET/DETACH/MATCH...DELETE/SET/DROP/CREATE INDEX` 等价映射到 INSERT/UPDATE/DELETE。
  明确注释这是启发式，精确变更仍以 `QueryResult.metadata.rows_returned` 为准。
- `rowid` 不再恒 0：取 `result.metadata().rows_returned as i64`（无 metadata 时回退 0）。
- `graphdb_execute_params` 补 `handle.trace(query_str)`（今天只有 `graphdb_execute` 有）。
- `session.rs:73 invoke_update_hook` 注释明确 `database=space_name, table=""` 的图语义。

验收：C-API 相关单测通过；新增 `detect_data_modification` 单元测试（注释前缀、Cypher 关键字、rows 回填）。

## 非目标

- 不做统一 `EventBus`，不做异步 channel 分发，不做 BEFORE 否决触发器。
  内部钩子保持“事后通知、不可否决”；唯一否决语义仍只属于 C-API `commit_hook`。
- 不改 `SyncManager` outbox 管道和 tantivy `WatchCallback`（独立体系）。

## 风险

- `register_*` 返回值新增：调用方忽略返回值即可，兼容。
- `Committed` 新增字段：构造处（`commit.rs` 2 处 + 测试）需同步更新，编译器会指引。
- `EventSubscriptions` 引入 `parking_lot` 或 `std` 锁选型以各 crate 现有依赖为准，
  core 内用 `std::sync::RwLock` 避免新增依赖（写少读多，快照在 dispatch 内完成）。
