# 事件-钩子系统内置实现分析

## 1. 现有机制

### 1.1 事务生命周期回调

**位置**: `crates/graphdb-transaction/src/manager.rs`

```rust
pub type CommitCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
pub type RollbackCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
```

- `register_commit_callback()` / `register_rollback_callback()`：运行时注册
- `emit_commit_event()` / `emit_rollback_event()`：分发时 `catch_unwind` 隔离 panic
- 内建回调：统计 `txn_commit` / `txn_rollback` 计数

**事件类型**:
- `Committed` — 事务提交完成
- `Aborted` — 事务回滚
- `CommitDurableButUnfinalized` — WAL 持久化但未最终可见
- `BudgetWarning` — 资源配额警告

### 1.2 三阶段提交 Sink

**位置**: `crates/graphdb-transaction/src/participant.rs`

`TransactionCommitSink` trait 定义了 commit → finalize → recover 的三阶段协议。`SyncWrapper` 将存储引擎桥接到事务管理器。

### 1.3 迁移事件监听

**位置**: `crates/graphdb-migration/src/event.rs`

`MigrationEventListener` trait + `MigrationEvent` enum（Started, StepStarted, StepCompleted, Completed, Failed, RolledBack）。通过 `BroadcastEventListener` 桥接到 SSE 流。

### 1.4 同步 Outbox 模式

**位置**: `crates/graphdb-sync/src/manager.rs`

`SyncManager` 通过 SQLite outbox 表管理 fulltext/vector 索引更新事件。这是管道模式（staging → claiming → applying → acknowledging），而非传统事件系统。

## 2. 缺失的内置实现

以下事件在系统中**应该存在但尚未实现**：

### 2.1 Schema 变更事件

**需求**: 当 space/tag/edge type/index 被创建、修改、删除时，通知相关子系统。

**影响范围**:
- fulltext 索引需要在 schema 变更后重建
- vector 索引需要感知新标签/属性
- 缓存层需要失效
- 外部监控系统需要感知 DDL 操作

**建议实现**:
```
SchemaChangeEvent {
    SpaceCreated { space_id, space_name },
    SpaceDropped { space_id },
    TagCreated { space_id, tag_id, tag_name },
    TagAltered { space_id, tag_id, changes },
    TagDropped { space_id, tag_id },
    EdgeTypeCreated { space_id, edge_type_id, type_name },
    EdgeTypeDropped { space_id, edge_type_id },
    IndexCreated { index_name, target, properties },
    IndexDropped { index_name },
}
```

**注册点**: `SchemaManager` 的 DDL 操作方法（`create_space`, `drop_space`, `create_tag` 等）

### 2.2 存储层事件

**需求**: 存储引擎内部的关键生命周期事件。

**影响范围**:
- 监控系统需要感知 compaction/checkpoint 进度
- 冷热分层需要触发数据迁移
- WAL 管理需要感知 truncation 事件

**建议实现**:
```
StorageEvent {
    CheckpointStarted { sequence },
    CheckpointCompleted { sequence, duration_ms },
    CompactionStarted { space_id },
    CompactionCompleted { space_id, reclaimed_bytes },
    WalTruncated { up_to_lsn },
    SnapshotCreated { sequence },
    GcRun { reclaimed_entries },
}
```

**注册点**: `PersistenceCoordinator`, `WalManager`, `SnapshotManager`

### 2.3 索引生命周期事件

**需求**: fulltext/vector 索引构建、合并、刷新的事件。

**影响范围**:
- 查询优化器需要知道索引就绪状态
- 监控系统需要索引构建耗时
- 运维工具需要索引健康状态

**建议实现**:
```
IndexEvent {
    FulltextBuildStarted { index_name },
    FulltextBuildCompleted { index_name, docs_count },
    FulltextRefresh { index_name },
    VectorBuildStarted { index_name },
    VectorBuildCompleted { index_name, vectors_count },
    IndexMergeStarted { index_name, segments },
    IndexMergeCompleted { index_name },
}
```

**注册点**: `FulltextIndexManager`, `VectorSyncCoordinator`

### 2.4 会话/连接事件

**需求**: 客户端连接、断开、查询开始/结束的事件。

**影响范围**:
- 审计日志
- 连接池管理
- 慢查询检测

**建议实现**:
```
SessionEvent {
    SessionCreated { session_id },
    SessionDestroyed { session_id },
    QueryStarted { session_id, query_id, sql },
    QueryCompleted { session_id, query_id, duration_ms, rows },
    SlowQueryDetected { session_id, query_id, duration_ms, threshold },
}
```

**注册点**: `SessionManager`, 查询执行器入口

## 3. 优先级排序

| 优先级 | 事件类别 | 理由 |
|--------|---------|------|
| **P0** | Schema 变更事件 | DDL 操作是基础功能，子系统需要同步 |
| **P1** | 会话/连接事件 | 审计和慢查询检测是生产必需 |
| **P2** | 存储层事件 | 监控和运维需要，但可通过日志补充 |
| **P3** | 索引生命周期事件 | 重要但非阻塞，可通过状态查询替代 |

## 4. 实现建议

### 4.1 统一事件总线（可选）

当前事务回调是独立的闭包列表。如果需要跨子系统的事件分发，可考虑：

```rust
pub struct EventBus {
    transaction_callbacks: RwLock<Vec<CommitCallback>>,
    schema_callbacks: RwLock<Vec<Arc<dyn Fn(&SchemaChangeEvent) + Send + Sync>>>,
    storage_callbacks: RwLock<Vec<Arc<dyn Fn(&StorageEvent) + Send + Sync>>>,
    session_callbacks: RwLock<Vec<Arc<dyn Fn(&SessionEvent) + Send + Sync>>>,
}
```

但这会引入额外复杂度。**建议先在各子系统内部独立实现回调**，后续如需统一分发再抽取 EventBus。

### 4.2 与现有架构的集成

- SchemaManager 的 DDL 方法添加 `emit_schema_change()` 调用
- 存储引擎的 checkpoint/compaction 路径添加事件发射
- 查询执行器的入口/出口添加 session 事件
- 所有回调使用 `catch_unwind` 隔离（复用事务回调的模式）

### 4.3 不变式

- 事件回调不得持有锁跨事件（避免死锁）
- 事件回调应快速返回（重操作异步化）
- 事件回调 panic 不得影响主流程
- 事件发射是尽力而为（best-effort），不保证所有观察者都收到
