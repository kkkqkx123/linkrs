# 事务回调快照与 panic 隔离优化

## 背景

参考 `docs/analysis/transaction-callback-dynamic-dispatch-analysis.md` 第 7 节，分析指出动态分发
（`Arc<dyn Fn>`）本身在开放注册契约下为必要设计，但存在两个值得优化的附带问题。

## 优化内容

### 1. 快照分配成本转移（7.1 节）

**原实现：**

```rust
commit_callbacks: RwLock<Vec<CommitCallback>>,
rollback_callbacks: RwLock<Vec<RollbackCallback>>,

fn emit_commit_event(&self, event: TransactionEvent) {
    let callbacks = self.commit_callbacks.read().clone(); // 分配新 Vec + N 次 Arc::clone
    for callback in callbacks {
        callback(&event);
    }
}
```

**优化后：**

```rust
commit_callbacks: RwLock<Arc<[CommitCallback]>>,
rollback_callbacks: RwLock<Arc<[RollbackCallback]>>,

fn emit_commit_event(&self, event: TransactionEvent) {
    let callbacks = Arc::clone(&self.commit_callbacks.read()); // 仅克隆外层 Arc
    for callback in callbacks.iter() {
        // ...
    }
}
```

**思路：**

- 派发从「分配 Vec + N 次 Arc 增量」变为「单次 Arc 增量」，将分配成本从高频派发
  路径移到低频注册路径。
- 事务终结远多于回调注册（注册通常只在启动阶段），这是正确的成本转移方向。

**注册侧代价：**

```rust
pub fn register_commit_callback(&self, callback: CommitCallback) {
    let mut guard = self.commit_callbacks.write();
    let mut buf = guard.to_vec();       // 将现有切片拷贝为 Vec
    buf.push(callback);                 // 追加新回调
    *guard = Arc::from(buf);            // 转为新 Arc<[T]>
}
```

### 2. Panic 隔离（7.3 节）

**原实现：** 回调 panic 会栈展开穿透事务终结 API，调用方可能误判提交结果。

**优化后：** 每个回调包裹在 `std::panic::catch_unwind(AssertUnwindSafe(|| ...))` 中，
panic 仅记录 `log::warn!`，继续派发剩余回调。

```rust
fn emit_commit_event(&self, event: TransactionEvent) {
    let callbacks = Arc::clone(&self.commit_callbacks.read());
    for callback in callbacks.iter() {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            callback(&event);
        }))
        .is_err()
        {
            log::warn!("commit callback panicked; continuing dispatch");
        }
    }
}
```

## 未改动部分

- `Arc<dyn Fn(&TransactionEvent) + Send + Sync>` 类型别名保留。
- 注册接口 `register_commit_callback` / `register_rollback_callback` 签名不变。
- 派发时序（持有快照、释放锁后调用）语义不变。

## 影响范围

- 仅修改 `crates/graphdb-transaction/src/transaction/manager.rs` 中存储类型、构造、
  注册、派发四个位置。
- 所有现有测试（含 `lifecycle_callbacks_observe_terminal_events`、
  `checkpoint_transaction_is_monitored_and_emits_commit`）通过。