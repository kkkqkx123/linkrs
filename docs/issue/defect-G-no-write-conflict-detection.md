# 问题：存储层无写写冲突检测，自动提交路径 Last-Writer-Wins 静默丢失更新

- 状态：新建（已验证，待修复）
- 类型：正确性缺陷（并发写语义）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 G
- 关联：`docs/issue/defect-F-no-version-chain.md`（属性覆盖来源）、`docs/plan/storage-concurrency-correctness-rework-design.md` P0-G

## 问题描述

`crates/graphdb-storage` 的写路径直接 `arc.write()` 后修改，**无任何写写冲突检测**。并发更新同一顶点/边是 Last-Writer-Wins，静默丢失更新。

## 根因分析（已代码级确认）

- 冲突检测能力**只存在于 `graphdb-transaction` crate**：`grep "have_write_conflict\|WriteSetAnalyzer" crates/graphdb-storage/src` = **0 命中**，`graphdb-transaction/src` = 19 命中；
- storage 写路径直接修改：
  - `engine/transaction/ops.rs:116-154` `add_edge` → `arc.write()` → `insert_edge`；
  - `engine/transaction/ops.rs:246-271` `update_vertex_property_by_vid` → `arc.write()` → `set_property`；
- auto-commit 路径（`context/accessors.rs:81` `auto_commit: true`）绕过 transaction manager 直接走 storage API（`mutation_recorder: None`），**无事务状态可做冲突判定**。

### 现有机制边界

storage 层有 undo log 机制（`ops.rs` 返回 `UndoLogResult`、`writer.rs` 记录 `MutationResult`/`UndoLogEntry`），但那是**回滚记录**，不是**冲突检测**——两个并发写者都成功提交时，后者覆盖前者，无任何失败信号。

## 影响

- 并发写同一实体静默丢更新，应用无感知；
- 与缺陷 F 叠加：无版本链 + 无冲突检测 = 并发写在正确性上完全没有保障。

## 修复方向

- **方案 1（推荐）**：storage 层写路径接入 `WriteSetAnalyzer`（transaction crate 已有实现），写入时记录 entity key 到写集，提交时校验冲突并返回 `write-conflict` 错误；
- **方案 2（务实）**：明确将自动提交路径语义降级为 ReadCommitted / 串行写（如引入全局或 per-entity 写锁），并在文档声明"自动提交不保证丢失更新检测"；
- 同步：修复缺陷 F 的 undo 覆盖范围（auto-commit 也记录 before-image），使冲突检测失败后可正确回滚。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P0-G。

## 验收

- 并发写同一顶点：一个提交返回 write-conflict（或按文档语义明确串行化），不静默覆盖；
- 冲突事务回滚后数据为第一个成功写者的值；
- 全量 `cargo test --test '*'` + clippy 全绿。
