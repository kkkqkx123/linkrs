# P1 [dcl] CREATE USER 重名静默成功，IF NOT EXISTS 语义失效

## 现象

`UserStorage::create_user` 对已存在用户返回 `Ok(true)`
（`crates/linkrs-core/src/types/user_storage.rs:183-190`）：

```rust
if users.contains_key(&info.username) {
    return Ok(true);
}
```

不带 `IF NOT EXISTS` 的 `CREATE USER` 重名也静默成功，且不修改既有账户。
`if_not_exists` 旗标由 parser 解析（`user_parser.rs:45-50`）并传入 plan node，
但 storage 层不区分，「抑制重名报错」这一关键字的存在意义被完全消解，
两种写法行为一致。

## 影响

拼写已有用户名时无任何反馈，运维上容易误判账户已按预期创建/更新；
依赖重名报错做校验的脚本失效。

## 根因

storage 的创建语义是「幂等插入」，没有把旗标传导到决策点；
旗标在 planner 到 executor command 的传递中存在
（`crates/linkrs-query/src/executor/streaming/operators/spec/ddl.rs` 的
`UserManageCommand::Create`），只是末端未使用。

## 建议

按标准语义实现：重名且带 `IF NOT EXISTS` 时静默成功，否则返回「用户已存在」错误。

## 严重级

**P1（静默失败，旗标失效）**

## 代码位置

`crates/linkrs-core/src/types/user_storage.rs:183-190`、
`crates/linkrs-query/src/parser/parsing/user_parser.rs:40-76`、
`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs:25-51`
