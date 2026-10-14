# P1 [dcl] 自助 CHANGE PASSWORD 断链：会话用户名从不回填

## 现象

文档 §4 的首要示例是不带用户名的自助改密（`CHANGE PASSWORD 'old_pass' TO 'new_pass'`）。
该形态下 `ChangePasswordStmt.username = None`，planner 原样填入 `PasswordInfo`
（`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:125-136`），
`build_user_manage_spec` 不注入任何会话信息
（`crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs:540-547`，
`_exec_ctx` 未使用），而 `ExecutionContext` 不含会话用户字段
（`crates/linkrs-query/src/executor/base/execution_context.rs:40-104`）。
最终 storage 的 `change_password` 对 `username: None` 直接报
"Username cannot be empty"（`crates/linkrs-core/src/types/user_storage.rs:158-163`）。

即自助改密无论旧密码对错都必然失败。测试 `test_change_password_self`
（`crates/linkrs-query/tests/dcl/user_management.rs:355-366`）只断言了错误路径，
未察觉正确旧密码也失败。对照之下，准入与密码策略两处都做了 self 解析
（`guard.rs:162`、`crates/linkrs-server/src/graph_service/query.rs:186` 的
`unwrap_or(username)`），唯独执行链没做。

## 影响

文档主示例不可用；用户只能以 `CHANGE PASSWORD <自己> 'old' TO 'new'` 形式绕开，
自助改密能力实际不存在。

## 根因

查询执行链（planner / arena builder / executor）全程不携带已认证用户身份，
server 侧的身份解析没有通道传递到规划期。

## 建议

沿 `password_history_depth` 已有的 threading 模式
（`linkrs-server` 注入 → `crates/linkrs-api/src/api_core/query_api.rs:171` →
`QueryContext`）增加已认证用户名的传递，planner 在 `username` 为空时回填。
该模式在仓库内有完整先例，不引入新机制。

## 严重级

**P1（文档主示例不可用）**

## 代码位置

`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:125-136`、
`crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs:540-547`、
`crates/linkrs-core/src/types/user_storage.rs:158-163`、
`crates/linkrs-query/src/executor/base/execution_context.rs:40-104`
