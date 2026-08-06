# 问题：每个事务对全库每表每分片注册 MVCC 快照（事务开销与 schema 规模成正比）

- 状态：新建（已验证，待修复）
- 类型：正确性 / 可扩展性缺陷（事务开启路径）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 A
- 关联：`docs/issue/defect-B-frontier-stall-dead-code.md`（同享 catalog 写锁）、`docs/issue/defect-C-flush-under-catalog-write-lock.md`（同把 catalog 写锁）、`docs/issue/auto-commit-mvcc-snapshot-leak.md`（快照生命周期同域）

## 问题描述

事务开启时对全库**所有**点标签表、**所有**边分区表逐一注册 MVCC 快照，与事务实际访问的数据量无关。只读一个顶点的事务也要为全库 schema 规模付费。

## 根因分析（已代码级确认）

1. `crates/graphdb-storage/src/storage/engine/graph_storage/context/accessors.rs:88-106`：
   - `with_vertex_tables_mut` 遍历全部点标签逐一 `register_snapshot`，**持有 catalog 写锁**（`data_store.rs:560 → write_vertex_tables → vertex_tables.write()`，`data_store.rs:423`）；
   - `for_all_edge_partitions_mut` 遍历全部边分区 `register_snapshot`。
2. `sharded.rs:367-373`：每个 `register_snapshot` 遍历并锁住全部分片（`shards[0]` + `shards[1..]`）。
3. `vertex_table/core.rs:663-671`：每个分片内部对 `active_snapshots` HashMap 做 O(活跃快照数) 全量 min 扫描。

**总复杂度：每事务 O(点标签数 × 分片数 + 边分区数) 次加锁 + 同量级 O(活跃快照数) 扫描，全程持 catalog 写锁。**

20 点标签 / 50 边分区 / 100 并发的场景下，单事务开启 ≈ `20×8 + 50 = 210` 次互斥加锁 + 210 次 O(100) 扫描。对照 ladybug 的对应操作是原子 load，O(1)。

## 影响

- 事务开启是所有读写的必经路径，全库事务在此串行化；
- 与缺陷 C 叠加：flush 也要同一把 catalog 写锁，事务开启 + 落盘 = 全局停顿；
- schema 规模越大，单事务固定成本越高，与数据规模解耦的扩展性失效。

## 修复方向

- **惰性注册**：首次访问某表时才 register（`accessors.rs` 记录已注册表集合，访问路径补注册），未访问的表不注册；
- **引用计数上提**：把 `active_snapshots` 从每表维护上提为全局 `SnapshotTracker` 单点（每事务仅 1 次 O(log n) 更新），表级 GC 改读全局 min-active-snapshot；
- **锁内只收集 Arc**：scatter-gather，短读锁收集表引用后释放 catalog 锁，注册改为逐表加锁（对标 `data_store.rs:623-641` 已有模式）；
- **min 维护增量**：`min_active_snapshot_ts` 改为增量维护（仅当删除当前最小值时重算）。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P0-A。

## 验收

- 高 schema 规模（20+ 标签）下单事务开启开销与并发事务数解耦（插桩对比注册调用次数）；
- 只读单顶点事务不再持有 catalog 写锁；
- 全量 `cargo test --test '*'` + clippy 全绿。
