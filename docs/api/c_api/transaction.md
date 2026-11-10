# C API 事务

源码：`crates/linkrs-api/src/embedded/c_api/transaction.rs`。

## 开启与结束

| 函数 | 说明 |
|---|---|
| `linkrs_txn_begin(session, txn)` | 开启读写事务 |
| `linkrs_txn_begin_readonly(session, txn)` | 开启只读事务 |
| `linkrs_txn_begin_with_config(session, config_json, txn)` | 配置化开启；`config_json` 可为 NULL（默认配置）。键全可选：`read_only`（bool）、`timeout_ms`/`query_timeout_ms`/`statement_timeout_ms`/`idle_timeout_ms`（非负整数，0/缺省为未设置）、`durability`（`none/async/sync`，默认 sync）、`isolation`（`repeatable_read/read_committed/serializable`，默认 repeatable read），大小写不敏感 |
| `linkrs_txn_commit(txn)` | 提交 |
| `linkrs_txn_rollback(txn)` | 回滚 |
| `linkrs_txn_free(txn)` | 释放事务句柄；提交/回滚后仍需调用 |

## 事务内执行

| 函数 | 说明 |
|---|---|
| `linkrs_txn_execute(txn, query, out_result)` | 直接执行 |
| `linkrs_txn_execute_params(txn, query, values, count, out_result)` | 位置绑定（与会话侧同为 `@param_{i}`） |
| `linkrs_txn_execute_named_params(txn, query, names, values, count, out_result)` | 命名绑定 |
| `linkrs_txn_execute_with_variables(txn, query, names, values, count, out_result)` | 参数加会话变量联合执行 |

## 保存点

| 函数 | 说明 |
|---|---|
| `linkrs_txn_savepoint(txn, name)` | 创建，返回 savepoint id（`int64`，出错返回 -1） |
| `linkrs_txn_release_savepoint(txn, savepoint_id)` | 释放 |
| `linkrs_txn_rollback_to_savepoint(txn, savepoint_id)` | 回滚到保存点 |
| `linkrs_txn_list_savepoints(txn, out_json)` | 枚举保存点堆 JSON |
| `linkrs_txn_find_savepoint(txn, name, out_id)` | 按名查找 id |

## 信息与运维

| 函数 | 说明 |
|---|---|
| `linkrs_txn_info(txn, out_json)` | 单事务 JSON（字段同 `list` 条目） |
| `linkrs_transaction_list(db)` | 全库事务列表堆 JSON |
| `linkrs_transaction_metrics(db)` | 事务指标堆 JSON |
| `linkrs_kill_transaction(db, txn_id)` / `linkrs_transaction_count(db)` | 见[库生命周期](database.md) |

堆 JSON 出参统一以 `linkrs_free_string` 释放。
