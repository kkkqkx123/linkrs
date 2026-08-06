# 问题：内存序使用不一致（SeqCst 滥用 / Relaxed 脆弱契约 / 无效同步对）

- 状态：新建（已验证，待修复）
- 类型：代码质量缺陷（原子内存序语义）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 K
- 关联：`docs/issue/defect-B-frontier-stall-dead-code.md`（mvcc.rs 同文件）、`docs/issue/defect-J-segment-allocator-dead-counter.md`（record_allocation 同文件）

## 问题描述

并发原语的内存序使用不统一：热路径过度使用 `SeqCst`，`Relaxed` 存在脆弱契约，存在无效的 `Acquire`/`Relaxed` 同步对。

## 根因分析（已代码级确认）

1. **`SeqCst` 滥用**：`crates/graphdb-transaction/src/transaction/mvcc.rs` 共 **30 处** `Ordering::SeqCst`（`write_ts` CAS 循环、`read_ts` load/store、`read_pending`/`write_pending` 计数器等，见 `mvcc.rs:156,204-211,220,238-277,316-413`）。这些全在事务开启/提交最热路径上，绝大多数只需 `Acquire`/`Release`/`Relaxed`。

2. **`Relaxed` 脆弱契约**（`sharded.rs:180-190`）：`record_allocation` 是 `load → max → store` 三步非原子序列，目前安全**仅因为**调用点仍持有 `shard.table.lock()`。字段声明为 `AtomicU32` 暗示可无锁访问——任何未来在锁外调用的改动都会引入**静默的 ID 分配竞态（重复 internal_id）**。既然已被锁保护就不该用 Atomic；既然用了 Atomic 就该用 `fetch_max`。

3. **无效的同步对**（`buffer_pool.rs`）：`usage.load(Ordering::Acquire)`（`:171`）/ `capacity.load(Ordering::Acquire)`（`:85`）配 `fetch_add/fetch_sub(Ordering::Relaxed)`（`:135,144-145,223`）/ `store(Ordering::Relaxed)` 不构成同步关系，`Acquire` 在此毫无作用。

## 影响

- 热路径 `SeqCst` 带来不必要的内存屏障开销（x86 上 store 屏障、ARM 上更强排序）；
- `Relaxed` 契约靠"调用点恰好持锁"维持，脆弱且不可迁移；
- 无效同步对暗示更强的内存序保证，误导维护者。

## 修复方向

1. **`mvcc.rs` 降级**：写写关系用 `Release`/`Acquire`，计数器用 `Relaxed`（如 `fetch_add`/`fetch_sub` 计数器），仅真正需要全局序的点保留 `SeqCst`；
2. **`record_allocation`**：要么改回普通 `u32` 字段（锁内修改），要么用 `fetch_max` 保证锁外安全；
3. **`buffer_pool.rs`**：`usage`/`capacity` 统一为 `Relaxed`（配合原子计数语义）或统一为 `Acquire`/`Release` 成对；消除无效 `Acquire`。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P2-K。

## 验收

- `mvcc.rs` `SeqCst` 数量显著下降（保留点均有注释说明为何需要全局序）；
- 全量 `cargo test --test '*'` + clippy 全绿；
- 写事务基准无回归。
