# P3 [dcl] 同一语法多份重复实现且行为漂移

## 现象

- **ALTER USER 两份近乎相同的实现**：`crates/linkrs-query/src/parser/parsing/user_parser.rs:89-133`
  （用户名走 `expect_dcl_name`，支持连字符与引号名）与
  `crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs:210-252`
  （用户名走 `expect_identifier`，不支持）。`ALTER USER user-name-1 ...` 在粘连入口与
  通用入口下一条通一条不通。
- **CREATE USER 三个入口**：粘连 `CreateUser` token
  （`crates/linkrs-query/src/parser/parsing/stmt_parser/dispatch.rs:105`）、
  `Create→User`（`crates/linkrs-query/src/parser/parsing/stmt_parser/misc.rs:85-87`）、
  DDL 重复实现（`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:156-183`）。
- **不可达死方法**：`UserParser::parse_describe_user_statement` /
  `parse_show_users_statement` / `parse_show_roles_statement`
  （`user_parser.rs:321-374`）无任何调用方；实际路径是
  `ddl_parser/describe_parser.rs:55-68`、`show_parser.rs:120-126`、
  `util_stmt_parser.rs:71-89`，且两处 space 名解析方式不同
  （`expect_dcl_name` vs `expect_identifier`）。

## 影响

同一语句的语法支持面取决于入口；修缺陷时容易只改一处（如
`dcl-set-role-locked-unparseable.md` 的 SET 子句就有两份拷贝），回归面翻倍。

## 根因

DCL 拆入 parser 时先落了一份 UserParser，通用 DDL 入口又各自实现了副本，
后续没有收敛；dispatch 同时保留粘连 token 与通用 token两条路由。

## 建议

每个 DCL 语句保留唯一解析实现与唯一入口，其余删除；用户名/space 名统一走
`expect_dcl_name`。`create_parser.rs` 的 CREATE USER 副本确认不可达后删除。

## 严重级

**P3（代码卫生，放大回归面）**

## 代码位置

`crates/linkrs-query/src/parser/parsing/user_parser.rs:89-133,321-374`、
`crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs:210-252`、
`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:156-183`、
`crates/linkrs-query/src/parser/parsing/stmt_parser/misc.rs:85-87`
