# 问题：读前沿防卡死逻辑是死代码（stalled 局部变量恒不触发 force-advance）

- 状态：新建（已验证，待修复）
- 类型：正确性缺陷（MVCC read frontier 推进 / GC 卡死）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 B
- 关联：`docs/issue/defect-A-snapshot-registration-o-schema.md`（快照生命周期同域）、`docs/issue/defect-G-no-write-conflict-detection.md`（长写事务来源）

## 问题描述

`crates/graphdb-transaction/src/transaction/mvcc.rs:383-406` 的 read frontier 推进逻辑声称"Prevents a single long-lived write transaction from blocking version GC indefinitely"（注释，`mvcc.rs:63-66`），但 force-advance 分支是**不可达死代码**：`stalled` 是函数局部变量，单次调用内命中 `Pending` 后 `stalled = 1`，`1 >= max_stalled(10000)` 恒为假，永远走 `break`。

```rust
let max_stalled: u64 = self.config.max_frontier_stall;   // 默认 10_000 (mvcc.rs:76)
let mut frontier = self.read_ts.load(Ordering::SeqCst);
let mut stalled: u64 = 0;                                 // 函数局部变量，每次调用从 0 开始
loop {
    let next = frontier.saturating_add(1);
    match states.get(&next).copied() {
        Some(Committed | Aborted) => { frontier = next; states.remove(&next); stalled = 0; }
        Some(Pending) => {
            stalled += 1;                                 // 变成 1
            if stalled >= max_stalled {                   // 1 >= 10000 恒为 false
                frontier = next; states.remove(&next); stalled = 0;
            } else {
                break;                                    // ← 永远走这里
            }
        }
        _ => break,
    }
}
```

## 根因分析

- `stalled` 未持久化为跨调用的水位，其语义本应是"连续多少次推进受阻"，但被实现成"本次调用内遇到第几个 Pending"，最多只能到 1；
- `max_frontier_stall` 没有正确的配置区间：若运维将其调成 1 来"修复卡顿"，force-advance 就会把 `read_ts` 推过一个**仍处于 Pending 的写事务**，新读事务看到该事务的部分写入 → **脏读**。

**要么无效，要么不安全。**

## 影响

- 一个长写事务把 `read_ts` 永久钉死在其 ts−1，所有新读事务拿到陈旧快照；
- `write_states: Mutex<BTreeMap<Timestamp, WriteTimestampState>>`（`mvcc.rs:118`）中卡点之后的条目永不清理，内存无界增长；
- `get_safe_gc_timestamp`（`mvcc.rs:416-424`）停滞 → 墓碑与旧版本永不回收。

## 修复方向

- **方案 1（推荐）**：彻底移除 force-advance，改为显式长事务超时中止——写事务持有超过阈值时由事务管理器强制 abort，保证 Pending 不会无限停留；
- **方案 2**：把 `stalled` 改为跨调用持久化的字段（基于时间或累计次数），且推进时校验目标 ts 对应的写事务是否已被事务管理器判死，避免推过存活 Pending；
- 无论哪种方案，都需保证 `read_ts` 永不超过任一存活 Pending 写事务的 ts，否则即脏读。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P0-B。

## 验收

- 长写事务超过阈值后被强制中止，`read_ts` 正常推进，`write_states` 不再无界增长；
- 任意配置下不存在"新读事务看到 Pending 写事务部分写入"的路径（脏读防护单测）；
- 全量 `cargo test --test '*'` + clippy 全绿。
