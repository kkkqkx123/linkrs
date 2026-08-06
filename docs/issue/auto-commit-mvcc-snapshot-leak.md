# 问题：auto-commit DML 路径 MVCC 快照泄漏导致写入 O(n²)（数据装载卡死）

- 状态：已修复（2026-08-05）
- 类型：性能缺陷（写入路径 / MVCC 快照生命周期）
- 触发：`cargo test --test integration_e2e data_driven::test_optimizer_aggregate`（及同文件的 `test_optimizer_vertex_count`）
- 关联：`tests/e2e/data_driven.rs`、`crates/graphdb-query/src/query/pipeline/{prepared,execution}.rs`、`crates/graphdb-storage/src/storage/vertex/vertex_table/core.rs`

## 问题描述

`test_optimizer_aggregate` 运行即卡死（worker 线程 99% CPU 空转，数分钟不结束）。两个 optimizer 测试共用 `optimizer_data.gql`（10000 顶点 + 10000 边，共 20003 条语句），均卡死 → 卡点不在聚合查询，而在**数据装载（逐条 INSERT）阶段**。

规模对照：`ecommerce_data.gql`（约 8553 条 INSERT）单测耗时 29.9s+，验证写入路径存在随语句数二次放大的退化。

## 根因分析

### 1. 快照泄漏：auto-commit 操作存储从不 finalize

`QueryApi::execute`（query_api.rs:287）→ `execute_with_operation_context_and_storage(query, ctx, None, None)`，未传入 operation_storage。

`prepare_request`（prepared.rs:194）检测到 `needs_write && operation_storage.is_none() && auto_commit` 时**自动绑定** auto-commit 上下文：
- `bind_auto_commit_storage()` → `with_auto_commit_context()`（accessors.rs:88-106）在每个顶点表注册一个 MVCC 快照（`register_snapshot` → `active_snapshots` 计数 +1），并在所有边分区表注册。

执行完成后**无人调用 finalize**：
- pipeline 侧：`execute_query_with_request_scope` → `execute_prepared`（prepared.rs:254）从不调用 `finalize_operation_storage`；只有 `execute_query_with_space`（execution.rs:35-44）正确配对，而测试不经过它。
- API 侧：`execute_with_operation_context_and_storage` 的 finalize 分支（query_api.rs:370-376）仅当调用方传入 storage（`operation_finalizer`）时生效，本路径为 None，形同虚设。

结果：每条 DML 语句在 `active_snapshots` 留下 1 个条目，**只增不减**。插桩实测：`register_snapshot` 调用 3500 次后 `active_snapshots.len()` 已达 146 且单调增长（约每 8 条语句 1 个新时间戳）。

### 2. 放大机制：`min_active_snapshot_ts` 每次全量扫描

`register_snapshot`（vertex_table/core.rs:723-727）每次注册都重算 `min_active_snapshot_ts`：
```rust
self.mvcc.min_active_snapshot_ts = self.mvcc.active_snapshots.keys().min()...;
```
泄漏使该 map 线性增长 → 每次注册 O(泄漏数)，总复杂度 O(N²)。perf 采样确认：该 `min()`（`HashMap<Timestamp,usize>` 上约 52% CPU）是绝对热点。

### 3. 次要热点（非主因）

每次写入还经 `check_write_admission` → `resource_snapshot` → `memory_usage_bytes` 全量遍历所有索引 generations×shards（gdb 曾采样到此），插桩实测单次 <5ms，非主导。

### 泄漏的衍生影响

- 边表/顶点表 tombstone GC 阈值（`min_active_snapshot_ts`）被陈旧的泄漏条目锁死，MVCC 垃圾回收失效，数据量越积越大。
- 写时间戳租约（`write_timestamp_lease`）同样永不 commit/release。

## 修复方案

### A. 修复泄漏（本次实现）

1. `PreparedRequest`（prepared.rs）新增 `owns_operation_storage: bool`：
   - `prepare_request` 自动绑定时置 `true`，调用方传入 storage 时置 `false`。
2. 新增 `PreparedRequest::finalize_owned_operation(committed)`：仅当 `owns_operation_storage` 时对 `operation_storage` 调 `finalize_operation(committed)`（注销 MVCC 快照 + 提交/释放写时间戳）。
3. 执行完成点统一收尾：
   - `execute_prepared` / `execute_prepared_materialized`：成功 `finalize_owned_operation(true)`，失败 `finalize_owned_operation(false)`（内部实现拆为 `*_inner` + 包装）。
   - `execute_prepared_streaming`：对自动绑定的 storage，通过 `set_transaction_finalizer_with_result` 挂到流上——流耗尽时 commit(true)，错误/提前 drop 时 abort(false)；编译失败则立即 finalize(false)。
   - `execute_query_with_space`（execution.rs）：删除手工 finalize，改由 `execute_prepared_materialized` 统一处理（避免双重 finalize）。
   - `execute_query_with_profile`（execution.rs）：其绕过 `execute_prepared` 自行编译执行，在所有错误返回点补 `finalize_owned_operation(false)`，成功路径补 `finalize_owned_operation(true)`。
   - 删除不再被引用的 `finalize_operation_storage` 辅助函数。
4. 兼容性：调用方显式传入 operation_storage 的路径（embedded session、`execute_with_operation_storage`、streaming `operation_owned`）保持由调用方 finalize，pipeline 因 `owns=false` 不重复处理。

### B. 后续加固（暂不实现，可选）

- `min_active_snapshot_ts` 改为增量维护（仅当删除当前最小值时重算），或改用有序结构，消除该类 O(n) 扫描。
- `finalize_operation` 提供幂等保护（防未来路径再次双 finalize）。

## 验收

- `cargo test --test integration_e2e data_driven::test_optimizer_aggregate` 通过；
- 同文件其余测试通过，`test_ecommerce_vertex_counts` 由 ~30s 降至秒级；
- `active_snapshots` 不再随语句数增长（回归插桩/单元断言）。
