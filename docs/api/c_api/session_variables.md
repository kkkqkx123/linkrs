# C API 会话变量

源码：`crates/linkrs-api/src/embedded/c_api/session_variables.rs`。
会话级 `$name` 变量：赋值、查询、删除、全量快照。
值以 `linkrs_value_t` 封送，与 `linkrs_execute_params` 的参数转换规则一致；
变量名不带 `$` 前缀。

| 函数 | 说明 |
|---|---|
| `linkrs_session_set_variable(session, name, value)` | 赋值；文本开启的事务内事务感知，回滚恢复旧值 |
| `linkrs_session_get_variable(session, name, out_value)` | 查询 |
| `linkrs_session_remove_variable(session, name)` | 删除；同样事务感知，回滚撤销删除 |
| `linkrs_session_variables_snapshot(session)` | 全量快照堆 JSON |

`linkrs_execute_with_variables` / `linkrs_txn_execute_with_variables`
把参数与变量快照联合传入一次执行，见[查询](query.md)与[事务](transaction.md)。
