# P3 [dcl] 死词元、错误信息错粘贴与凭据文件明文存储

> 单条不足以独立立项的代码卫生问题汇总。

## 1. 死词元 `TokenKind::AdminRole`

`crates/linkrs-query/src/parser/core/token.rs:147` 定义的 `AdminRole` 永不被词法器产出
（`ADMIN` 只映射到 `Tk::Admin`，`crates/linkrs-query/src/parser/lexing/lexer/token.rs:65`），
但 `user_parser.rs:225` 与 `parse_context.rs:472-481` 仍在匹配它。

**纠偏**：删除该词元定义及两处匹配分支。

## 2. `RoleType::FromStr` 错误信息错粘贴

parser 本地 RoleType 的 FromStr 报 "Unknown character type"
（`crates/linkrs-query/src/parser/ast/stmt/dcl.rs:57-70`，注释亦写「character type」），
系从别处复制未改。用户输入非法角色时看到误导信息。

**纠偏**：错误信息改为 "Unknown role type"。

## 3. `users.json` 明文持久化 bcrypt 哈希

`crates/linkrs-core/src/types/user_storage.rs:87-113` 以明文 JSON 写盘
（原子写入为 tmp+rename，本身没问题），哈希无加密、无文件权限约束说明。
`UserStorageSnapshot` 仍接受 version 1 装载（`:136`），与项目「不做历史数据兼容」的
约束相悖。

**纠偏**：补充文件权限与安全注释；version 1 兼容装载评估后删除；
是否加密作为独立议题记录，不在 DCL 修复方案内实现。

## 严重级

**P3（代码卫生与排查体验）**

## 代码位置

`crates/linkrs-query/src/parser/core/token.rs:147`、
`crates/linkrs-query/src/parser/ast/stmt/dcl.rs:57-70`、
`crates/linkrs-core/src/types/user_storage.rs:87-136`
