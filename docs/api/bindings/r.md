# R 绑定

目录 `bindings/r`（包 `linkrs`，`DESCRIPTION` 版本 1），
基础 R 外部指针加 C 桥接（`src/linkrsr.c`），只发布纯 R 与 C 桥接代码，
需 C 工具链；同一对象不支持并发调用。

## 前置要求

```shell
cargo build --features c_api
export LINKRS_LIB=/path/to/liblinkrs.so   # 或依赖工作区构建产物
R CMD INSTALL bindings/r
```

安装期 `configure` 解析 `LINKRS_LIB` 显式路径或工作区构建产物并写入构建变量；
依赖 `jsonlite` 做信封解析。

## 用法

```r
library(linkrs)
db <- linkrs_open_memory()
on.exit(linkrs_close(db), add = TRUE)
linkrs_create_space(db, "g")
session <- linkrs_session(db)
linkrs_use_space(session, "g")
result <- linkrs_execute(session, "SHOW SPACES")
linkrs_rows(result)
linkrs_result_free(result)
txn <- linkrs_begin(session)
linkrs_txn_execute(txn, "SHOW SPACES")
linkrs_txn_commit(txn)
linkrs_txn_free(txn)
```

文件库 `linkrs_open(path)`，配置打开以 `linkrs_config_new()` /
`linkrs_config_file(path)` / `linkrs_config_memory()` 配合
`linkrs_open_with_config(config)`。所有关闭函数可重复调用，终结器兜底。

## API 一览（`R/linkrs.R`）

- 库：`linkrs_open/linkrs_open_memory/linkrs_open_with_config/linkrs_close/linkrs_closed`、
  `linkrs_errcode/linkrs_last_message`（`linkrs_libversion` 未封装）。
- 配置：`linkrs_config_new/file/memory/free`。
- 会话：`linkrs_session/session_close/session_closed`、`linkrs_use_space/current_space`、
  `linkrs_execute/execute_params`、`linkrs_validate_query/explain_query`、`linkrs_statistics`。
- 结果：`linkrs_result_free`、`linkrs_column_count/row_count/column_names`、
  `linkrs_rows/linkrs_row`。
- 事务：`linkrs_begin/begin_readonly`、`linkrs_txn_execute/execute_params`、
  `linkrs_txn_commit/rollback/free`、
  `linkrs_savepoint/release_savepoint/rollback_to_savepoint`。
- 模式：`linkrs_create_space/drop_space/list_spaces/get_space`、
  `linkrs_create_tag/drop_tag/list_tags/get_tag`、
  `linkrs_create_edge_type/drop_edge_type/list_edge_types/get_edge_type`。
- 错误（`R/errors.R`）：返回码三元组（码/扩展码/文本）转条件，
  七组具名与基类划分与其他四端一致。

## 测试

```shell
Rscript bindings/r/tests/smoke.R   # 烟雾测试（需先 R CMD INSTALL）
```
