# C API 库生命周期

源码：`crates/linkrs-api/src/embedded/c_api/database.rs`。

## 打开与关闭

| 函数 | 说明 |
|---|---|
| `linkrs_open(path, db)` | 打开文件库 |
| `linkrs_open_memory(db)` | 打开内存库，`GraphDatabase::open_in_memory` 薄封装，关闭后数据消失 |
| `linkrs_open_with_config(config, db)` | 以配置句柄打开；配置句柄调用后仍有效，可复用，需另行 `linkrs_config_free` |
| `linkrs_close(db)` | 关闭；全部关联会话须先关闭 |
| `linkrs_libversion()` | 库版本静态字符串，无需释放 |
| `linkrs_free_string(str)` | 释放一切库分配的堆字符串 |
| `linkrs_free(ptr)` | 释放库分配的通用内存 |

`db` 均为输出参数（`linkrs_t **`），调用方负责 `linkrs_close`。

## 概览与导出

| 函数 | 说明 |
|---|---|
| `linkrs_database_overview(db)` | 堆上 JSON：空间数、活跃事务指标、共享查询计数器、内存库标记；`linkrs_free_string` 释放 |
| `linkrs_export_to_file(db, query, path, format, out_rows)` | 执行查询并落盘；`format` 0=CSV、1=JSONL，越界 `LINKRS_MISUSE`；`out_rows` 为写入行数（不含 CSV 头） |
| `linkrs_export_result_to_file(session, result, path, format, out_rows)` | 导出已执行的结果句柄，复用一次执行同时做读取与归档；句柄导出后仍可用 |

## 事务运维

| 函数 | 说明 |
|---|---|
| `linkrs_kill_transaction(db, txn_id)` | 按 id 终止另一事务 |
| `linkrs_transaction_count(db)` | 当前活跃事务数 |
