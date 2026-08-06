# 问题：无版本链，属性更新破坏快照隔离（RepeatableRead 承诺不成立）

- 状态：新建（已验证，待修复）
- 类型：正确性缺陷（MVCC 隔离级别兑现）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 F
- 关联：`docs/issue/defect-G-no-write-conflict-detection.md`（写路径配套）、`docs/plan/storage-concurrency-correctness-rework-design.md` P0-F

## 问题描述

默认隔离级别声明为 `IsolationLevel::RepeatableRead`（`crates/graphdb-transaction/src/transaction.rs:109`），但属性更新是**破坏性原地覆盖**，无 MVCC 版本链，旧快照读不到旧值。

## 根因分析（已代码级确认）

- `crates/graphdb-storage/src/storage/vertex/vertex_timestamp.rs:9-12`：`VertexTimestamp` 只有平坦的 `(start_ts, end_ts)`，只能表达"存在/删除"两态，**无属性版本概念**；
- `crates/graphdb-storage/src/storage/vertex/vertex_table/core.rs:361-362` → `column_store.rs:1409-1419` `set_property` → `col.set(row_idx, value)`：**物理覆盖旧值**，不保留旧版本。

**并发正确性风险**：事务 T1 在 ts=100 开启并读取顶点 V 的属性 P；事务 T2 在 ts=105 更新 P；T1 再次读取 P 时看到新值——旧值已被物理覆盖。违反 Snapshot Isolation / Repeatable Read。

### 现有 undo 机制的边界（澄清）

storage 层存在 before-image / undo 机制，但不改变上述结论：

- `writer.rs:117-148` `record_vertex_property_update` 通过 `mutation_recorder` 记录 `old_value`（`UndoLogEntry::UpdateVertexProp`）——**仅用于未提交事务的回滚**，不能服务旧时间戳的并发读者；
- auto-commit 路径 `context/accessors.rs:82` 显式设 `mutation_recorder: None`，**连 undo 都不记录**；
- `storage/mvcc.rs` 的 `TieredTombstoneManager` 只管删除墓碑，对更新无能为力。

## 影响

- 对用户的错误承诺：声明 RepeatableRead 但并发读在事务内能看到别的已提交事务的新值；
- 长事务的"读己之写"与"读他人之写"不可区分，应用层一致性假设被破坏。

## 修复方向

- **P0（最小正确性）**：为属性更新引入 before-image / undo 记录并覆盖 auto-commit 路径，至少保证**回滚**语义；同时将自动提交路径的隔离级别**明确降级为 ReadCommitted** 并在文档声明，避免错误承诺；
- **P1（完整）**：引入列级版本链（按版本区间 [start,end) 存属性快照，读按 ts 取可见版本），对标 ladybug `UpdateInfo`/`VectorUpdateInfo` 版本链；
- 边界：顶点存在性已有 `(start_ts, end_ts)` 支持快照读，缺陷集中在**属性值**与**边属性**的版本化。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P0-F。

## 验收

- RepeatableRead 语义单测：T1 事务内重复读同一属性，T2 并发提交更新，T1 两次读到相同旧值；
- auto-commit 路径的隔离级别文档与实际一致；
- 全量 `cargo test --test '*'` + clippy 全绿。
