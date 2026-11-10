# C API 类型与句柄

源码：`crates/linkrs-api/src/embedded/c_api/types.rs`、
`c_api/value.rs`，值布局定义在 `crates/linkrs-core/src/types/c_api.rs`，
经 cbindgen 展开进 `include/linkrs.h`。

## 不透明句柄

| 类型 | 创建 | 释放 | 说明 |
|---|---|---|---|
| `linkrs_t` | `linkrs_open/open_memory/open_with_config` | `linkrs_close` | 库；关闭前先关全部会话 |
| `linkrs_session_t` | `linkrs_session_create` | `linkrs_session_close` | 会话；预编译语句先于所属会话释放 |
| `linkrs_result_t` | 查询/事务/预编译执行产生 | `linkrs_result_free` | 结果集，整体物化 |
| `linkrs_txn_t` | `linkrs_txn_begin*` | `linkrs_txn_commit/rollback` 后 `linkrs_txn_free` | 事务 |
| `linkrs_stmt_t` | `linkrs_prepare` | `linkrs_stmt_free` | 预编译语句，不持有会话 |
| `linkrs_batch_t` | `linkrs_batch_inserter_create` | `linkrs_batch_free` | 批量写入器，借用会话 |
| `linkrs_config_t` | `linkrs_config_new/file/memory` | `linkrs_config_free` | 配置构造器，`open_with_config` 后仍可复用 |
| `linkrs_busy_handler_t` | `linkrs_busy_handler_create` | `linkrs_busy_handler_free` | 调用方自驱动的重试记账 |
| `linkrs_context_t` | 回调借用 | 不释放 | 仅在 UDF 回调内经 `linkrs_context_*` 访问 |

空结构体以 `_dummy` 成员占位，保证 C11/MSVC 合法。

## `linkrs_value_t`

```c
typedef struct linkrs_value_t {
  enum linkrs_value_type_t type_;   /* 类型标签 */
  union linkrs_value_data_t data;   /* 负载 */
} linkrs_value_t;
```

类型标签：`LINKRS_NULL/BOOL/INT/FLOAT/STRING/LIST/MAP/VERTEX/EDGE/PATH/BLOB`。
字符串为借用视图（`data/len`，非 null 结尾），二进制为借用视图，
`LIST/MAP/VERTEX/EDGE/PATH` 以不透明指针穿越，
调用方保证入参在调用期间有效。

## 错误码枚举

- `linkrs_error_code_t`：`LINKRS_OK = 0` 成功；
  常见 `BUSY/LOCKED/READONLY/NOTFOUND/SCHEMA/CONSTRAINT/MISMATCH/MISUSE/RANGE/NOT_IMPLEMENTED` 等，
  数值即各入口 `int` 返回值的契约。
- `linkrs_extended_error_code_t`：`1000` 段语法、`1100` 段求值、
  `1200` 段约束、`1300` 段事务、`1400` 段图语义、`1500` 段内部，
  经 `linkrs_extended_errcode` 读取。

## 钩子与回调类型

`linkrs_scalar_function_callback`、`linkrs_aggregate_step_callback`、
`linkrs_aggregate_final_callback`、`linkrs_function_destroy_callback`（UDF），
`linkrs_trace_callback`（SQL 跟踪）、`linkrs_commit_hook_callback`、
`linkrs_rollback_hook_callback`、`linkrs_update_hook_callback`（会话钩子）。

## JSON 值形态

行与结果集 JSON 导出、批量参数集合、向量/全文结果中的单元格值，
保持引擎 `Value` 的标签序列化形态（如 `{"String": ...}`），
C API 与各语言绑定均不做静默类型擦除。
