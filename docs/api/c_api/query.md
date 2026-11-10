# C API 查询

源码：`crates/linkrs-api/src/embedded/c_api/query.rs`。
结果句柄统一以 `linkrs_result_t **` 输出，调用方 `linkrs_result_free` 释放。

## 执行

| 函数 | 说明 |
|---|---|
| `linkrs_execute(session, query, out_result)` | 直接执行 |
| `linkrs_execute_params(session, query, values, count, out_result)` | 位置绑定，`@param_{i}`，`linkrs_value_t` 数组 |
| `linkrs_execute_named_params(session, query, names, values, count, out_result)` | 命名绑定，`names[i]` 对应 `values[i]` |
| `linkrs_execute_with_variables(session, query, names, values, count, out_result)` | 参数加会话变量联合执行：绑定参数与 `$name` 快照一次传入 |

`values` 借用调用方内存，调用期间须有效；`names` 为 UTF-8 字符串数组。

## 校验与解释

| 函数 | 说明 |
|---|---|
| `linkrs_validate_query(session, query)` | 仅校验，不执行 |
| `linkrs_explain_query(session, query, out_result)` | 执行计划，结果集形式返回 |

失败细节经线程错误槽（`linkrs_errmsg`）与会话扩展码/偏移
（`linkrs_extended_errcode`/`linkrs_error_offset`）读取。
