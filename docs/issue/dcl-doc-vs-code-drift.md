# P2 [dcl] 文档与实现的零散漂移（改密通道、密码策略示例）

> 单条不足以独立立项的小偏差汇总。每条都给出代码位置与纠偏方向。

## 1. 「管理员可修改其他用户密码」与代码不符（文档 §4）

文档声称管理员可修改其他用户密码。代码
`crates/linkrs-server/src/graph_service/guard.rs:161-168` 仅允许「本人或 GOD」，
ADMIN（非 GOD）被拒并提示 "can only change own password"。只有 HTTP 管理通道的
`admin_reset_password`（`crates/linkrs-server/src/graph_service/session.rs:370-394`，
经 `require_admin`）才允许 Admin 重置他人密码。文档描述的是 DCL 语句能力，与该实现不符。

**纠偏**：二选一并写进文档——DCL 维持「本人或 GOD」（Admin 走 HTTP 管理通道），
或把 DCL 放宽到 Admin。考虑最小权限原则，推荐前者并注明通道分流。

## 2. 默认密码强度策略会拒绝文档中的全部示例密码（文档 §1/§2/§4）

默认策略为 min_length=8 + 大小写 + 数字
（`crates/linkrs-config/src/security.rs:127-139`），校验见 `:156-179`，
在查询通道由 `enforce_new_password_policy`
（`crates/linkrs-server/src/graph_service/query.rs:164-192`，入口 `guard.rs:71`）强制。
文档示例密码 `secure_password`、`password123`、`old_pass`、`new_secure_password`
全部缺少大写字母，按默认配置均被拒。另外该强制点只覆盖 server 查询通道，
嵌入式 / API 路径不经过 `guard.rs`，策略范围未文档化。

**纠偏**：文档示例改用符合默认策略的密码；文档写明默认策略取值与强制范围
（查询通道强制，其他通道不强制）。

## 3. `REVOKE ... FROM` 的 FROM 是必需关键字但文档示例无异常提示（文档 §6）

`REVOKE` 强制要求 `FROM`（`user_parser.rs:302-306`），与 GRANT 的 `TO` 对称，
行为本身正确；仅提示读者不要与 Neo4j 的 `REVOKE ... FROM` 变体混淆，
无需改动。

## 严重级

**P2（文档误导：示例不可复制、通道能力描述不符）**

## 代码位置

`crates/linkrs-server/src/graph_service/guard.rs:161-168`、
`crates/linkrs-config/src/security.rs:127-179`、
`crates/linkrs-server/src/graph_service/query.rs:164-192`
