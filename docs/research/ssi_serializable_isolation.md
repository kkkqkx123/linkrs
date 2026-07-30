# SSI (Serializable Snapshot Isolation) 重构方案

## 问题根因

当前 `check_write_set_conflict` 中 Serializable 隔离级别的冲突检测分三层：

| 层级 | 位置 | 复杂度 | 触发条件 |
|------|------|--------|----------|
| 活跃事务冲突 | lines 664-685 | O(A) | 所有写事务 |
| 已提交写集实体冲突 | lines 760-800 | O(R) | Serializable 读集非空 |
| 已提交写集全扫描 | lines 811-834 | **O(N × V × R)** | `is_full_scan=true` |

第三层是瓶颈。`is_full_scan` 在 `read_set.size() > 10_000` 时为 true，触发 O(N) 扫描所有已提交写集。`read_ranges` 字段从未被填充（死代码路径）。

## SSI 算法原理

基于 Cahill et al. (2008) 的 SSI 算法：

1. **rw-dependency**：T1 读了资源 R，T2 写了资源 R → T1 →rw T2
2. **危险结构检测**：T1 →rw T2 且 T2 →rw T1 → 存在环 → 中止较晚提交的事务
3. **核心优势**：无需扫描已提交写集，所有冲突检测 O(1) 每资源

## 修改范围

### 1. `types.rs` — 新增 ResourceId 和 SsiState

```rust
/// 统一资源标识符，用于 SSI rw-dependency 追踪
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceId {
    Vertex(VertexId),
    Edge(EdgeKey),
    Schema(String),
    Index(String),
}

/// 每事务 SSI 状态
pub struct SsiState {
    /// 本事务读过的资源（用于检测写→读冲突）
    read_resources: HashSet<ResourceId>,
    /// 本事务写过的资源（用于检测读→写冲突）
    write_resources: HashSet<ResourceId>,
}
```

### 2. `context.rs` — TransactionContext 新增 SSI 状态

```rust
// 新增字段
ssi_state: RwLock<SsiState>,
```

新增方法：
- `record_ssi_read(resource: ResourceId)` — 读操作时调用
- `record_ssi_write(resource: ResourceId)` — 写操作时调用
- `get_ssi_read_resources() -> HashSet<ResourceId>`
- `get_ssi_write_resources() -> HashSet<ResourceId>`

### 3. `manager.rs` — TransactionManager 新增 SSI 追踪器

```rust
/// SSI rw-dependency 追踪器
struct SsiTracker {
    /// 每资源的活跃读锁：resource → Vec<(txn_id, start_ts)>
    read_locks: RwLock<HashMap<ResourceId, Vec<(TransactionId, Timestamp)>>>,
}
```

新增方法：
- `ssi_register_read(txn_id, resource)` — 事务读资源时注册
- `ssi_unregister_reads(txn_id)` — 事务提交/中止时清除
- `ssi_check_write_conflict(txn_id, write_resources) -> Result<(), TransactionError>` — 提交时检测

### 4. `manager.rs` — 修改 check_write_set_conflict

**删除** lines 811-834 的 O(N) 扫描。

**替换为** SSI 危险结构检测：

```rust
if serializable {
    // 已有的 O(1) 空间索引检查（保留）
    // ...

    // 新增：SSI 危险结构检测
    // 对写集中的每个资源，检查是否有其他事务已读过（rw-dependency）
    for resource in txn_write_set.ssi_resources() {
        self.ssi_check_write_conflict(txn_id, &resource, ctx.start_timestamp)?;
    }
}
```

### 5. `manager.rs` — 修改 commit_transaction

在 final-review 阶段（line 1123-1210）：

```rust
// 提交时：
// 1. 注册写锁（写过的资源 → 其他事务的读锁会被检查）
// 2. 清除本事务的读锁
// 3. 检测危险结构
```

### 6. `manager.rs` — 修改 begin_insert_transaction

事务开始时初始化 SSI 状态。

### 7. `manager.rs` — 修改 abort_transaction

中止时清除 SSI 追踪。

### 8. `manager.rs` — 修改 prune_committed_write_sets

新增 SSI read_locks 的清理（与已提交写集同步清理）。

## 修改文件清单

| 文件 | 修改内容 |
|------|----------|
| `types.rs` | 新增 `ResourceId`、`SsiState`，为 `WriteSet` 添加 `ssi_resources()` 方法 |
| `context.rs` | 新增 `ssi_state` 字段和读写方法 |
| `manager.rs` | 新增 `SsiTracker`，修改冲突检测、提交、中止、清理流程 |
| `conflict.rs` | 新增 SSI 危险结构检测工具函数 |
| `manager_test.rs` | 新增 SSI 冲突检测单元测试 |

## 保持兼容性

- Non-Serializable 隔离级别（ReadCommitted、RepeatableRead）不受影响
- SingleWriter 模式不受影响
- 已有的 4 个空间索引保留（用于 Write-Write 冲突检测）
- `committed_write_sets` 向量保留（用于 final-review 防竞态）
- `read_ranges` 字段保留但标记为 deprecated

## 测试策略

1. SSI 基本冲突检测：两个事务读写同一资源 → 一个中止
2. SSI 无冲突：两个事务读写不同资源 → 都提交
3. SSI 长事务读写：长时间运行的读事务 + 写事务 → 正确检测
4. SSI 全扫描消除：验证 O(N) 扫描不再触发
5. 混合隔离级别：Serializable + ReadCommitted 混合使用
6. 已有测试不退化
