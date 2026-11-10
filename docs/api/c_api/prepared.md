# C API 预编译语句

源码：`crates/linkrs-api/src/embedded/c_api/prepared.rs`。
`linkrs_prepare` 创建，`linkrs_stmt_free` 释放。
语句句柄不持有会话：先释放语句，再关闭所属会话。

## 准备与执行

| 函数 | 说明 |
|---|---|
| `linkrs_prepare(session, sql, out_stmt)` | 预编译，本地校验复用 |
| `linkrs_stmt_execute(stmt, out_result)` | 执行并返回结果集 |
| `linkrs_stmt_execute_update(stmt, out_changes)` | 执行并返回影响行数 |
| `linkrs_stmt_query(stmt)` | 回读预编译时的 SQL 原文堆字符串；`linkrs_free_string` 释放 |
| `linkrs_stmt_free(stmt)` | 释放 |

## 绑定

参数名为去 `@` 前缀形态（如 `x` 对应 `@x`），与 `linkrs_execute_params` 的
值转换规则一致；重复绑定同名即替换。

| 函数 | 说明 |
|---|---|
| `linkrs_stmt_bind(stmt, name, value)` | 绑定单个命名参数 |
| `linkrs_stmt_bind_many(stmt, names, values, count)` | 一次绑定多个命名参数；同名后者覆盖前者，遇首个非法条目即停，此前收集的绑定保留 |
| `linkrs_stmt_clear_bindings(stmt)` | 清除全部绑定 |
| `linkrs_stmt_reset(stmt)` | 重置执行状态（保留绑定） |

## 参数元信息与批量执行

| 函数 | 说明 |
|---|---|
| `linkrs_stmt_param_count(stmt, out_count)` | 参数个数 |
| `linkrs_stmt_param_name(stmt, index)` | 按位置（0 起，prepare 文本顺序）取参数名堆字符串（去 `@` 前缀），越界返回 NULL；`linkrs_free_string` 释放 |
| `linkrs_stmt_execute_repeated(stmt, batches_json, out_total_rows)` | 按行参数集合批量执行：`batches_json` 为对象数组，形如 `[{"x": {"BigInt": 1}}]`，值用引擎 `Value` serde 编码；遇错即停，`out_total_rows` 累计已成功遍次返回行数；每遍触发一次 trace/更新钩子，`$name` 取每遍 live 快照 |

`$` 会话变量不经过绑定，仍走执行期快照。
