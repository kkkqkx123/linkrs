# 问题：rollback_bench 在 ~524 次迭代后 `commit_ordered` 报 InvalidTimestamp

- 状态：新建（待定位）
- 类型：MVCC 契约冲突（bench 与库两侧均可能）
- 复现：`cargo bench -p linkrs-storage --bench rollback_bench`（2026-10-09，~3 分钟后 panic）

## 问题描述

panic 位于 rollback_bench.rs:124：

```text
ordered commit: InvalidTimestamp(524)
```

调用链（crates/linkrs-transaction/src/mvcc/write.rs）：

- `commit_ordered(start)`（write.rs:64-68）= `reserve_commit_timestamp(start)` + publish；
- `reserve_commit_timestamp`（write.rs:122-133）要求 `write_states[start_ts] == Pending`，
  否则返回 `InvalidTimestamp(start_ts)`（write.rs:128）。

bench 每次迭代的顺序（rollback_bench.rs:78-124）：

1. `acquire_insert_timestamp()` 取 ts（应为 Pending 槽位）；
2. `bind_operation_context(transaction_with_timestamps(txid, ts, Some(ts), ...))` 后逐条写边；
3. **`drop(txn)`**（commit 尚未显式调用）；
4. `commit_staged_writes(txid, &[])`；
5. `commit_ordered(ts)` ← 第 524 个 ts 上失败。

前 ~523 次迭代成功，说明并非每次都失败；第 524 次时 524 的槽位已不再是 Pending——
疑似 `drop(txn)` 的隐式 abort 路径、`commit_staged_writes` 或快照回收（watermark GC）中
某一方提前 settle/移除了该槽位，且该行为依赖某种累积状态（迭代次数 / 内存水位）。

## 影响

- 显式“staged 写入 + 手工 commit_ordered”提交路径在长序列事务下不可靠；
  rollback/abort 基线（EDGE_COUNTS 三档）无法产出。

## 修复方向

- 开发板上用 `RUST_BACKTRACE=1` 复跑，定位 524 槽位被谁 settle
  （在 `reserve_commit_timestamp` 失败分支加调试日志，打印 states 中 524 的最终状态）。
- 审查 `Drop for operation context / txn` 的隐式 abort 是否会 `abort_write_timestamp`；
  若是，bench 应在 `drop(txn)` 前显式 commit 或改用不触发 Drop-abort 的句柄——
  同时在文档中明确该顺序契约。
- 检查 `write_states` 的 GC/水位清理是否会回收 Pending 槽位。
