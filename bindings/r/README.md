# Linkrs R 绑定

基于基础 R 外部指针加 C 桥接的嵌入式直调绑定。调用者先构建动态库并指定路径，本包只发布纯 R 与 C 桥接代码；启用 C 工具链为前置要求，同一对象不支持并发调用。

## 前置要求

```shell
cargo build --features c_api
```

安装期由 `configure` 解析 `LINKRS_LIB` 显式路径或工作区构建产物，并写入构建变量：

```shell
export LINKRS_LIB=/path/to/liblinkrs.so
R CMD INSTALL bindings/r
```

调用者保持绑定与动态库来自同一检出。

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

文件数据库用 `linkrs_open(path)`，配置文件打开用 `linkrs_config_new()`、`linkrs_config_file(path)`、`linkrs_config_memory()` 配合 `linkrs_open_with_config(config)`。所有关闭函数可重复调用，终结器提供兜底。

## 范围

首版覆盖数据库、会话、事务、结果集、空间、标签、边类型、查询执行、参数化执行、查询校验与解释、会话统计。查询参数仅支持空、逻辑、整数、双精度、单字符串；双精度即使为整数仍按浮点传递；计数以双精度返回。函数注册、扩展装载、各类钩子回调、繁忙处理器不在首版范围内。

错误映射与其它绑定一致：高频主码发出具名条件（`linkrs_busy`、`linkrs_readonly`、`linkrs_not_found`、`linkrs_schema`、`linkrs_constraint`、`linkrs_mismatch`、`linkrs_misuse`），其余降级为 `linkrs_error` 基类条件并保留数字码，另提供 `linkrs_code()`、`linkrs_extended()` 与 `is_not_found()` 系列辅助判断。

## 资源与线程

子对象持有父引用（环境对象的 `parent` 槽），父先关闭则子调用报误用条件。同一句柄对象不支持并发调用，每个任务使用独立会话。

## 测试

```shell
R_LIBS=/path/to/Rlib Rscript bindings/r/tests/smoke.R
```

冒烟测试覆盖建模、写入、查询、事务、保存点、错误映射全链路。
