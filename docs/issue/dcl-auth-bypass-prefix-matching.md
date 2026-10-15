# P0 [dcl] 授权分类按字符串前缀匹配，用户管理门禁可被绕过

> 对照文档：`docs/release/04_dcl_user_management.md`。修复方案由 `docs/plan/ddl_leftover_long_term_supplement.md`
> 第 3 章承接并已实施（查询通道权限判定改为按解析后语句类别推导，单判定入口，解析失败显式拒绝；
> 原 `docs/plan/dcl_user_management_fix.md` 阶段零未实施，不再作为依据）。

## 现象

server 对用户管理语句的准入判定完全依赖原始字符串前缀
（`crates/linkrs-server/src/graph_service/guard.rs:106-130`）：

```text
upper.starts_with("CREATE USER") / ("DROP USER") / ("ALTER USER")
/ ("GRANT ") / ("REVOKE ") / ("CHANGE PASSWORD")
/ ("SHOW USERS") / ("SHOW ROLES") / ("DESCRIBE USER")
```

前缀不命中时语句不进入 `enforce_user_management`，而是落入通用粗粒度通道
（`guard.rs:77-89`），按首关键词映射为 Read/Write/Delete/Schema 之一。两类绕过输入：

1. **粘连词形**：词法器把 `CREATEUSER` / `ALTERUSER` / `DROPUSER` / `CHANGEPASSWORD`
   识别为单 token（`crates/linkrs-query/src/parser/lexing/lexer/token.rs:103-106`），
   dispatch 正常路由到 UserParser（`crates/linkrs-query/src/parser/parsing/stmt_parser/dispatch.rs:105-108`）。
   `"CREATEUSER ...".starts_with("CREATE USER")` 为 false，预过滤器漏过，
   随后按 `CREATE` 前缀降级为 `Permission::Write`：
   - `CREATEUSER x PASSWORD 'p'`：任何具备 Write 的角色（含 USER）可创建用户；
   - `DROPUSER root`：任何具备 Delete 的角色可删除任意用户（含 root）；
   - `ALTERUSER USER x PASSWORD 'p'`（粘连形态仍需补一个 `USER` token，见 `user_parser.rs:94`）：
     按 `ALTER` 前缀降级为 Schema，DBA 可修改他人密码；
   - `CHANGEPASSWORD other 'old' TO 'new'`：落入 default Read，任何已认证用户（含 GUEST）
     可对他人发起改密（仍受旧密码校验约束，但「本人或 GOD」规则被绕过）。
2. **空白变体**：前缀要求恰好一个空格。`CREATE  USER`（双空格）、`CREATE\tUSER`、
   `CHANGE  PASSWORD` 均可正常解析（`user_parser.rs:627-643` 的白空格测试即覆盖双空格形态），
   但同样漏过预过滤器，产生与粘连词形相同的降级。

`is_user_management_statement`（`crates/linkrs-server/src/graph_service/query.rs:149-157`）
使用同一套前缀，执行后的状态对齐（会话驱逐、角色同步）对这些变体同样失效。

## 影响

普通 USER 角色可创建/删除任意账户（拒绝服务与账户名抢占），DBA 可越权修改他人密码，
GUEST 可对他人发起改密尝试。可远程触发的权限提升路径。

## 根因

准入分类输入是原始字符串而非解析后的语句。同一仓库里解析能力已具备
（`guard.rs:98-104` 的 `parse_stmt`），但分类逻辑没有使用它。

## 建议

准入与执行后对齐均改为按 `Stmt` 变体判定，两处共用同一个判定函数；不再有任何
基于字符串前缀的分支。通用粗粒度映射中确保 DCL 语句不再落入。

## 严重级

**P0（安全：权限提升）**

## 代码位置

`crates/linkrs-server/src/graph_service/guard.rs:30-49,77-130`、
`crates/linkrs-server/src/graph_service/query.rs:149-157`、
`crates/linkrs-query/src/parser/lexing/lexer/token.rs:103-106`、
`crates/linkrs-query/src/parser/parsing/stmt_parser/dispatch.rs:105-108`
