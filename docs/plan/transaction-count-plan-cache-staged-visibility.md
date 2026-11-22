# 事务内元数据计数与计划缓存可见性修复方案

> 基线：`68891eea update sync and storage backend 1`。
> 触发：`tests/integration_embedded_api.rs` 的 `test_session_text_transaction_commands` 在 `ROLLBACK TO sp1` 后断言 `count(p) == 1` 失败，实得 `0`。

## 0. 目标与约束

本方案修复一个长期被基线编译错误掩盖的缺陷。结论先行：**savepoint 回滚本身是正确的，bug 在查询层**——一个 `count(p)` 计划在无事务上下文编译后被缓存，又在写事务内被复用，复用的元数据计数计划看不到 staged 行。

约束：开发阶段不做向后兼容；代码、注释、日志英文，文档中文；代码注释不得引用计划编号；子 crate 严格 DAG。

## 1. 问题清单（file:line 基于基线）

### 1.1 现象与表象

`test_session_text_transaction_commands`（`tests/integration_embedded_api.rs:201-266`）：

```
BEGIN → INSERT Alice → SAVEPOINT sp1 → INSERT Bob → ROLLBACK TO sp1
→ MATCH (p:person) RETURN count(p) AS c   // 期望 1，实得 0
```

第一次断言（`:231`，全量 ROLLBACK 后 count=0）通过，第二次（`:255`，ROLLBACK TO 后）失败。这个差异是关键线索：同一条 count 语句，在无事务时正确、在事务内错误。

### 1.2 savepoint 回滚链（已排除）

逐层核实回滚实现都是正确的，不是 bug 来源：

- SQL 解析：`parser/ast/stmt/transaction.rs:18-40` 的 `RollbackTransactionStmt.savepoint_name: Option<String>` 区分全量回滚与 `ROLLBACK TO`；`parser/parsing/transaction_parser.rs:88-105` 仅在见到 `To` token 时填充。
- Session 路由：`embedded/session/transactions.rs:177-199`（ROLLBACK TO）、`:218-238`（SAVEPOINT）。
- 核心回滚：`linkrs-transaction/src/context/savepoint.rs:168-278` 按 `undo_log_index` / staged mark / `journal_len` 精确回退，并 `truncate` journal；`linkrs-storage/src/engine/graph_storage/context/txn_staging.rs:540-575` 的 `rollback_to`。
- 实测（插桩后恢复）：savepoint mark 正确，`rewind` 后 staging buffer 仍含 Alice，`inserts=[((1, Text("Alice")), 0)]`。**数据没丢**。

### 1.3 真正根因：元数据计数计划跨事务复用

- 纯 `count(p)` 走元数据计数优化：`arena_builder/specs/graph.rs:142-166` 的 `metadata_count_below` 把聚合降级为 `MetadataCount`。
- 该优化有 staged-write 门控：编译期检查 `reader.operation_context()`，`!read_only && !(auto_commit && auto_commit_group_start.is_none())` 时拒绝降级（走正常扫描，能看到 staged）。**门控本身正确**。
- 但门控结果**没有进入计划缓存 key**。`cache/plan_cache/key.rs:106-113` 的 `PlanCacheContext` 只有 space/schema/index/param/optimizer 维度。于是：
  1. 第一次 count（`:227`，无事务）→ 编译出 MetadataCount 计划 → 缓存；
  2. 第二次 count（`:252`，事务内）→ 命中同一缓存 → 复用 MetadataCount 计划。
- `executor/streaming/operators/source_operator.rs:773-788` 的 `next_metadata_count` 直接调 `count_visible_vertices`（`linkrs-storage/src/engine/graph_storage/reader/vertex.rs:202`），只读已提交表，看不到 staged 行 → 返回 0。
- 注释声称"All checks re-verify at execution through the same snapshot"（`graph.rs:140`），但 staged-write 门控只在编译期做了一次，执行期无复核。

对照实验：`tests/e2e/session_transaction.rs:115-158` 用 `RETURN p.name` 而非 `count(p)`，不走该优化，因此通过——进一步确认问题在计数路径而非 savepoint。

## 2. 设计

### 2.1 原则：让门控决策参与缓存身份

编译期门控依赖 `operation_context`（read_only / auto_commit / auto_commit_group_start）。当这个决策不进缓存 key 时，缓存就会跨"安全/不安全"上下文复用计划。修复的正确抽象层次是**把该决策作为缓存身份的一个维度**，而不是在执行期叠加补救——执行期已无原扫描计划可回退，只能报错，语义不对。

### 2.2 单一判定函数

新增 `staged_writes_visible`（`context/query_request_context.rs:20`，经 `context.rs:39` 导出）：

```rust
pub(crate) fn staged_writes_visible(op_ctx: Option<&StorageOperationContext>) -> bool {
    op_ctx.is_some_and(|context| {
        !context.read_only && !(context.auto_commit && context.auto_commit_group_start.is_none())
    })
}
```

它精确复现原门控条件。编译期门控（`graph.rs:159`）改为调用它，缓存 key 构造也调用它——两处共用一份判定，不会漂移。

### 2.3 缓存 key 增加 `in_write_context` 维度

- `PlanCacheContext`（`key.rs:122`）与 `PlanCachePutContext`（`key.rs:81`）各加 `in_write_context: bool`。
- `from_query_with_context` 与带 partition 的哈希计算（`key.rs:231,287`）纳入该字段。
- `PlanCachePutContext::from_identity`（`key.rs:103`）把 context 的该字段透传，保证 put 与 get 派生同一 key。
- `plan_cache_context()`（`pipeline.rs:751`）增加参数；三个调用点从 `query_context.request_context().operation_context` 推导：
  - `compiler.rs:215`（compile-or-get-cached）
  - `prepared.rs:261`（DML memo probe）
  - `prepared.rs:293`（plain cache probe）
- `dml_memo_key_for`（`pipeline.rs:778`）相应加参数。

`PlanCachePutContext` 已有的 `is_transaction` 字段语义是"语句是否为事务控制语句（BEGIN/COMMIT）"，与"当前是否在写上下文"不同，不能复用，故新增独立字段。

### 2.4 为什么这样修复是正确的

- 无事务 count → `in_write_context=false`，缓存 MetadataCount 计划；
- 事务内 count → `in_write_context=true`，key 不同 → 缓存未命中 → 重新编译 → `metadata_count_below` 门控（经共享函数）看到写上下文 → 拒绝 MetadataCount → 生成扫描计划 → 合并 staged 行 → count=1。
- autocommit group 模式（`auto_commit && group_start.is_some()`）同样返回 true，也得到隔离——该模式共享 staging，元数据计数同样会漏数据。
- 单语句 autocommit 写（非事务）返回 false，与纯读共享 key 是安全的：不同语句文本本就不同 key，且单语句执行后的写已提交。

## 3. 代码变更清单

| 文件 | 改动 |
| --- | --- |
| `context/query_request_context.rs:20` | 新增 `staged_writes_visible` |
| `context.rs:39` | re-export |
| `executor/streaming/plan/arena_builder/specs/graph.rs:159` | 门控改用共享函数 |
| `cache/plan_cache/key.rs:81,103,122,231,287` | 两个 context 加字段 + 哈希 + 透传 |
| `cache/plan_cache.rs:268,435` | put 路径用 PutContext 的字段构造 key |
| `pipeline.rs:751,778` | `plan_cache_context` / `dml_memo_key_for` 加参数 |
| `pipeline/compiler.rs:215`、`pipeline/prepared.rs:261,293` | 从 operation_context 推导并传入 |
| `cache/plan_cache/tests.rs:23,35,259` | 补字段；覆盖维度测试加 `in_write_context` |
| `tests/integration_embedded_api.rs:282` | 新增回归测试 |

## 4. 验收标准（grep 可核对）

1. `grep -rn "staged_writes_visible" crates/linkrs-query/src/` → 定义在 context，门控 + 三个调用点共 5 处引用。
2. `grep -rn "in_write_context" crates/linkrs-query/src/cache/plan_cache/key.rs` → 哈希计算 2 处、字段 3 处。
3. `test_session_text_transaction_commands` 通过。
4. `test_transaction_count_sees_staged_rows_after_plain_count_is_cached` 通过；禁用 key 哈希后该测试须失败（已实测）。

## 5. 验证

| 目标 | 结果 |
| --- | --- |
| `cargo check --workspace --features linkrs/fulltext,linkrs/vector,linkrs/vector-qdrant` | 通过 |
| `linkrs-query --lib` | 2332 passed |
| `linkrs-query --test ddl` | 214 passed |
| `linkrs-query --test search` | 4 passed |
| `integration_embedded_api`（embedded,vector） | 74 passed（含原失败用例与新增回归测试） |

回归有效性验证：临时移除 `key.rs` 两处 `in_write_context.hash()` 后，新测试精确失败（`left: BigInt(0), right: BigInt(1)`），恢复后通过。

## 6. 迁移影响

无持久化格式变化。计划缓存是进程内结构，哈希算法变化在重启后自然生效，无迁移成本。

## 7. 预存在缺陷修复

本缺陷原本被 `tests/integration_embedded_api.rs` 的编译错误（`ExecutionMetadata` 缺 `compile_time_us` / `execute_time_us`）掩盖；上一轮补齐编译字段后暴露，本轮定位并修复。

## 8. 未纳入本次

- 执行期复核 staged-write 门控：需在编译期保留原扫描计划作为 fallback，成本高；缓存 key 隔离已覆盖唯一复用入口。
- 其他元数据优化（如 `count_visible_edges`）与本方案同源，已由同一门控和同一 key 维度覆盖，无需单独处理。

## 9. 验证结论与失败归因

savepoint 测试由失败转通过。所有验证项通过。config 的 `test_doc_prod_example_config_loads_and_validates` 是另一独立预存在缺陷（见 `index-management-space-and-assembly-unification.md` 第 9 节），亦已修复：测试配置的 embedding 段改为按 feature 条件拼入（`lib.rs:1561-1571`）。

## 10. 待拍板开放点

1. 是否为 `staged_writes_visible` 增加使用真实 `StorageOperationContext` 的单元测试：当前它由端到端测试（`test_transaction_count_sees_staged_rows_after_plain_count_is_cached`）和缓存维度测试间接覆盖，构造 `Timestamp` 的成本与收益待评估。
