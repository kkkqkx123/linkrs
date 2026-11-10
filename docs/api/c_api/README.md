# Linkrs C API

C API 是嵌入式 Rust API 的 FFI 投影，对 `crates/linkrs-api/src/embedded/c_api`
下各模块的一对一导出。C 头文件为 `include/linkrs.h`，Rust 实现按源文件分模块，
两侧一致性由 `tests/c_api_header_consistency.rs` 保证。

## 启用与生成

```shell
cargo build --features c_api          # 产生 target/{debug,release}/liblinkrs
cargo test --test c_api_header_consistency
```

- `c_api` 特性隐含 `embedded`。
- `include/linkrs.h` 由 cbindgen 从源码生成（见 `cbindgen.toml` 与 `build.rs`），
  提交库内的头文件副本；无 cbindgen 的离线构建保留已有头文件，
  漂移由头文件一致性测试拦截。
- 符号总数当前为 172 个导出函数，另有错误码枚举、不透明句柄、
  值类型与回调类型定义。

## 通用约定

- **返回码**：所有返回 `int` 的入口以 `linkrs_error_code_t` 主码报告结果，
  `LINKRS_OK = 0` 为成功。详见[错误处理](error.md)。
- **不透明句柄**：`linkrs_t`（库）、`linkrs_session_t`（会话）、
  `linkrs_result_t`（结果集）、`linkrs_txn_t`（事务）、`linkrs_stmt_t`（预编译语句）、
  `linkrs_batch_t`（批量写入器）、`linkrs_config_t`（配置构造器）、
  `linkrs_busy_handler_t`（繁忙处理器）、`linkrs_context_t`（函数执行上下文）。
  全部由库分配、调用方以配套 `free/close` 释放，释放后不得再用。
- **字符串入参**：均为 UTF-8 null 结尾字符串，调用方保留所有权（借用）。
- **字符串出参**：凡文档注明“堆上字符串”的出参（`char **`、`char *` 返回值），
  均由库分配，调用方必须以 `linkrs_free_string` 释放。
- **JSON 穿越**：复杂值（参数、行、统计、配置批量更新、向量/全文请求与结果、
  迁移计划）一律走 JSON 文本穿越 FFI 边界，值编码复用引擎 `Value` 的 serde 形态。
- **空指针**：空句柄/空出参指针一律返回 `LINKRS_MISUSE`，不崩溃；
  值为 NULL 的 `free/close` 为空操作（批量缓冲查询返回 -1 告警除外）。
- **线程错误槽**：除主码外，详细信息写入线程本地错误槽，
  经 `linkrs_errmsg` 取回，会话级扩展信息见 `linkrs_extended_errcode` 与
  `linkrs_error_offset`。

## 模块索引

| 文档 | Rust 源 | 内容 |
|---|---|---|
| [库生命周期](database.md) | `c_api/database.rs` | `open/open_memory/open_with_config/close`、版本、概览、导出、事务运维 |
| [配置](config.md) | `c_api/config.rs` | 构造期 setter 与运行期 `get/update/reset/bulk` |
| [会话](session.md) | `c_api/session.rs` | 会话、autocommit、`busy_timeout`、trace/hooks、中断 |
| [查询](query.md) | `c_api/query.rs` | `execute` 系列位置/命名带参与变量联合执行、`validate`/`explain` |
| [结果集](result.md) | `c_api/result.rs` | 行列元信息、按名/按下标取值器、JSON 导出、计时 |
| [事务](transaction.md) | `c_api/transaction.rs` | 开启（含配置化）、执行、保存点、信息枚举、`kill/list/metrics` |
| [预编译语句](prepared.md) | `c_api/prepared.rs` | `prepare/bind/execute`、参数元信息、批量执行 |
| [模式](schema.md) | `c_api/schema.rs` | 空间/标签/边类型结构化 CRUD、版本历史读 |
| [会话变量](session_variables.md) | `c_api/session_variables.rs` | `$name` 赋值/查询/删除/快照 |
| [批量写入](batch.md) | `c_api/batch.rs` | `BatchInserter` 单条/多条添加、计数、结果汇总 |
| [函数与扩展](function.md) | `c_api/function.rs` | 标量/聚合 UDF、`context_*`、动态扩展装载管理 |
| [向量与全文](vector_search.md) | `c_api/vector.rs`、`c_api/fulltext.rs` | 向量索引/搜索、全文/混合搜索 |
| [迁移](migration.md) | `c_api/migration.rs` | `plan/execute/rollback` 在线编排 |
| [统计](statistics.md) | `c_api/statistics.rs` | 会话 `changes/last-id/statistics`、繁忙处理器 |
| [错误处理](error.md) | `c_api/error.rs` | 主码/扩展码、`errmsg/errstr/offset` |
| [类型与句柄](types.md) | `c_api/types.rs`、`c_api/value.rs`、`linkrs-core/src/types/c_api.rs` | `linkrs_value_t`、句柄一览、JSON 值形态 |

## 有意不覆盖

用户管理、同步运维、流式/游标、Web 管理不在 C API 范围内，
与嵌入式 Rust API 的职责划分一致（见功能矩阵）。
向量/全文符号始终声明，但无对应编译特性时运行时返回
`LINKRS_NOT_IMPLEMENTED`。
