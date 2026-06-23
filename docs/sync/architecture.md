# 同步系统架构文档

## 概述

本文档描述 GraphDB 的同步系统架构，该系统负责在图数据存储操作时自动同步全文索引和向量索引。

## 架构设计原则

1. **单一职责**：每层只负责自己的职责
2. **事务原子性**：索引同步必须与存储操作保持原子性
3. **清晰的事务边界**：区分事务操作和非事务操作
4. **最小化复杂度**：避免冗余设计

## 架构层次

```
┌─────────────────────────────────────────────────────────────┐
│                    Storage Layer                             │
│  (SyncStorage<S>)                                            │
│  - 包装 StorageClient                                         │
│  - 在存储操作时同步调用 SyncManager                           │
│  - 事务操作：调用 on_vertex_change_with_txn (缓冲)            │
│  - 非事务：调用 on_vertex_insert (立即执行)                   │
└─────────────────────────────────────────────────────────────┘
                              ↓ 调用
┌─────────────────────────────────────────────────────────────┐
│                    SyncManager Layer                         │
│  (SyncManager)                                               │
│  - 统一同步接口                                              │
│  - 事务模式：buffer_operation → TransactionBatchBuffer       │
│  - 非事务：直接 on_change → Processor                        │
└─────────────────────────────────────────────────────────────┘
                              ↓ 协调
┌─────────────────────────────────────────────────────────────┐
│                  SyncCoordinator Layer                       │
│  (SyncCoordinator + VectorSyncCoordinator)                   │
│  - transaction_buffers: DashMap<txn_id, TransactionBatchBuffer>
│  - 按 space/tag/field 组织处理器                             │
│  - 管理事务缓冲 vs 立即执行                                   │
└─────────────────────────────────────────────────────────────┘
                              ↓ 执行
┌─────────────────────────────────────────────────────────────┐
│                  Index Engine Layer                          │
│  (FulltextIndexManager + VectorManager)                      │
│  - 实际索引存储                                              │
│  - 索引查询接口                                              │
└─────────────────────────────────────────────────────────────┘
```

## 核心组件

### 1. SyncStorage

**位置**：`src/storage/event_storage.rs`

**职责**：
- 包装 `StorageClient`，在存储操作时自动同步到索引
- 区分事务操作和非事务操作
- 事务操作：调用 `on_vertex_change_with_txn` 缓冲操作
- 非事务操作：调用 `on_vertex_insert` 立即执行

**关键方法**：
```rust
fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<Value, StorageError> {
    let result = self.inner.insert_vertex(space, vertex.clone())?;
    
    if self.enabled {
        if let Some(ref sync_manager) = self.sync_manager {
            let space_id = self.inner.get_space_id(space)?;
            let txn_id = 0; // 非事务操作
            
            if let Some(first_tag) = vertex.tags.first() {
                sync_manager.on_vertex_insert(txn_id, space_id, &vertex)?;
            }
        }
    }
    
    Ok(result)
}
```

### 2. SyncManager

**位置**：`src/sync/manager.rs`

**职责**：
- 统一同步接口
- 管理 `SyncCoordinator` 和 `VectorSyncCoordinator`
- 提供事务缓冲和立即执行两种模式

**关键 API**：

#### 事务模式
```rust
// 缓冲顶点插入
pub fn on_vertex_insert(
    &self,
    txn_id: TransactionId,
    space_id: u64,
    vertex: &Vertex,
) -> Result<(), SyncError>

// 缓冲顶点变更
pub fn on_vertex_change_with_txn(
    &self,
    txn_id: TransactionId,
    space_id: u64,
    tag_name: &str,
    vertex_id: &Value,
    properties: &[(String, Value)],
    change_type: ChangeType,
) -> Result<(), SyncError>

// 准备事务（2PC Phase 1）
pub async fn prepare_transaction(
    &self,
    txn_id: TransactionId,
) -> Result<(), SyncError>

// 提交事务（2PC Phase 3）
pub async fn commit_transaction(
    &self,
    txn_id: TransactionId,
) -> Result<(), SyncError>
```

#### 非事务模式
```rust
// 立即执行同步
pub async fn on_vertex_change(
    &self,
    space_id: u64,
    tag_name: &str,
    vertex_id: &Value,
    properties: &[(String, Value)],
    change_type: ChangeType,
) -> Result<(), SyncError>
```

### 3. SyncCoordinator

**位置**：`src/sync/coordinator/coordinator.rs`

**职责**：
- 管理全文索引同步
- 按 space/tag/field 组织索引处理器
- 提供事务缓冲机制

**核心数据结构**：
```rust
pub struct SyncCoordinator {
    fulltext_manager: Arc<FulltextIndexManager>,
    vector_manager: Option<Arc<VectorManager>>,
    fulltext_processors: DashMap<(u64, String, String), Arc<FulltextProcessor>>,
    vector_processors: DashMap<(u64, String, String), Arc<VectorProcessor>>,
    transaction_buffers: DashMap<TransactionId, Arc<TransactionBatchBuffer>>,
    config: BatchConfig,
    // ...
}
```

**关键方法**：
```rust
// 立即执行索引变更
pub async fn on_change(&self, ctx: ChangeContext) -> Result<(), SyncCoordinatorError>

// 缓冲事务操作
pub fn buffer_operation(
    &self,
    txn_id: TransactionId,
    ctx: ChangeContext,
) -> Result<(), SyncCoordinatorError>

// 2PC 准备阶段
pub async fn prepare_transaction(
    &self,
    txn_id: TransactionId,
) -> Result<(), SyncCoordinatorError>

// 2PC 提交阶段
pub async fn commit_transaction(
    &self,
    txn_id: TransactionId,
) -> Result<(), SyncCoordinatorError>
```

### 4. TransactionBatchBuffer

**位置**：`src/sync/batch/processor.rs`

**职责**：
- 缓冲事务内的索引操作
- 按事务 ID 组织缓冲操作
- 支持 Prepare/Commit/Rollback 语义

**数据结构**：
```rust
pub struct TransactionBatchBuffer {
    pending: DashMap<TransactionId, DashMap<IndexKey, TransactionBufferEntry>>,
}

pub struct TransactionBufferEntry {
    pub operations: Vec<IndexOperation>,
}
```

### 5. VectorSyncCoordinator

**位置**：`src/sync/vector_sync.rs`

**职责**：
- 管理向量索引同步
- 提供事务缓冲支持

**关键方法**：
```rust
// 缓冲向量变更
pub fn buffer_vector_change(
    &self,
    txn_id: TransactionId,
    ctx: VectorChangeContext,
) -> Result<(), VectorCoordinatorError>

// 立即执行向量变更
pub async fn on_vector_change(
    &self,
    ctx: VectorChangeContext,
) -> Result<(), VectorCoordinatorError>

// 提交事务
pub async fn commit_transaction(
    &self,
    txn_id: TransactionId,
) -> Result<(), VectorCoordinatorError>
```

## 事务流程

### 2PC（两阶段提交）流程

```
┌─────────────┐
│ Active      │
└──────┬──────┘
       │ begin_transaction
       ↓
┌─────────────┐
│ Committing  │◄───┐
└──────┬──────┘    │
       │           │
       ├───────────┘
       │ prepare_transaction (Phase 1)
       │ - 验证事务缓冲
       ↓
┌─────────────┐
│ Committing  │
└──────┬──────┘
       │
       │ commit storage (Phase 2)
       │ - redb::WriteTransaction::commit()
       ↓
┌─────────────┐
│ Committed   │
└──────┬──────┘
       │
       │ commit_transaction (Phase 3)
       │ - 应用所有缓冲的索引操作
       ↓
┌─────────────┐
│ Completed   │
└─────────────┘
```

### 事务内索引同步流程

```rust
// 1. 开启事务
let txn_id = manager.begin_transaction(options)?;

// 2. 执行存储操作（自动缓冲索引变更）
storage.insert_vertex(space, vertex)?;
// ↓ SyncStorage 调用
// ↓ SyncManager::on_vertex_change_with_txn
// ↓ SyncCoordinator::buffer_operation
// ↓ TransactionBatchBuffer::prepare

// 3. 提交事务
manager.commit_transaction(txn_id)?;
// ↓ Phase 1: prepare_transaction
// ↓ Phase 2: storage commit
// ↓ Phase 3: commit_transaction
//   - SyncCoordinator::commit_transaction
//   - VectorSyncCoordinator::commit_transaction
```

### 非事务操作流程

```rust
// 直接调用（txn_id = 0）
storage.insert_vertex(space, vertex)?;
// ↓ SyncStorage 调用
// ↓ SyncManager::on_vertex_insert
// ↓ SyncCoordinator::on_change (立即执行)
```

## 配置

### IndexBufferConfig

```rust
pub struct IndexBufferConfig {
    pub max_buffer_size: usize,      // 默认：1000
    pub flush_timeout_ms: u64,       // 默认：100
}
```

### BatchConfig

```rust
pub struct BatchConfig {
    pub batch_size: usize,           // 批处理大小
    pub flush_interval_ms: u64,      // 刷新间隔
    pub max_pending_batches: usize,  // 最大待处理批次
}
```

## 错误处理

### SyncError 类型

```rust
pub enum SyncError {
    FulltextError(String),
    VectorError(String),
    BufferError(String),
    CoordinatorError(String),
    Internal(String),
}
```

### 事务回滚

当事务回滚时：
1. `TransactionManager::abort_transaction` 被调用
2. 自动回滚 redb 事务
3. 调用 `SyncManager::rollback_transaction`
4. 清理 `TransactionBatchBuffer`

## 架构演进

### 历史变更

**2024-XX-XX**: 移除 SyncHandle 机制
- 原因：`SyncHandle` 在 2PC 流程中从未被使用，增加复杂度但无实际收益
- 影响：简化了事务上下文，减少了代码量
- 替代方案：使用 `TransactionBatchBuffer` 直接管理事务缓冲

### 设计决策

1. **为什么区分事务/非事务模式？**
   - 事务模式需要保证原子性，必须先缓冲后统一提交
   - 非事务模式可以立即执行，提高性能

2. **为什么使用 DashMap？**
   - 支持高并发访问
   - 无锁设计，适合读多写少场景

3. **为什么 2PC 分为三个阶段？**
   - Phase 1 (Prepare): 验证所有操作可以执行
   - Phase 2 (Commit): 提交存储事务（不可逆）
   - Phase 3 (Confirm): 应用索引变更（失败只记录日志）

## 最佳实践

### 1. 事务使用

```rust
// 推荐：使用事务保证原子性
let txn_id = manager.begin_transaction(options)?;
storage.insert_vertex(space, vertex1)?;
storage.insert_vertex(space, vertex2)?;
manager.commit_transaction(txn_id)?;

// 不推荐：非事务操作（除非确实不需要原子性）
storage.insert_vertex(space, vertex)?;
```

### 2. 错误处理

```rust
match manager.commit_transaction(txn_id) {
    Ok(()) => {
        // 事务成功提交，索引也已同步
    }
    Err(TransactionError::SyncFailed(e)) => {
        // 存储已提交，但索引同步失败
        // 记录日志，考虑补偿机制
        log::error!("Index sync failed: {}", e);
    }
    Err(e) => {
        // 其他错误，事务已回滚
        log::error!("Transaction failed: {}", e);
    }
}
```

### 3. 性能优化

```rust
// 调整批处理配置
let config = BatchConfig {
    batch_size: 1000,        // 增大批次提高吞吐量
    flush_interval_ms: 50,   // 缩短间隔降低延迟
    ..Default::default()
};
```

## 监控指标

### SyncMetrics

- `active_transactions`: 活跃事务数
- `index_operations`: 索引操作总数
- `sync_errors`: 同步错误数
- `batch_size_histogram`: 批次大小分布

### 查看统计

```rust
let stats = sync_manager.stats();
println!("Active transactions: {}", stats.active_transactions);
println!("Total sync operations: {}", stats.total_operations);
```

## 未来改进

1. **异步索引构建**: 支持后台异步构建索引
2. **增量同步**: 优化增量数据同步性能
3. **分布式支持**: 为未来分布式扩展预留接口
4. **索引版本管理**: 支持索引 schema 版本演进

## 相关文档

- [事务管理文档](../transaction/README.md)
- [存储引擎文档](../storage/README.md)
- [全文索引文档](../search/README.md)
- [向量索引文档](../vector/README.md)
