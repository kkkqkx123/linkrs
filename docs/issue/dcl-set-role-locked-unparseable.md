# P1 [dcl] ALTER USER 的 SET ROLE / SET LOCKED 子句不可解析

## 现象

文档 §2 给出的两种 SET 形态（`ALTER USER x SET ROLE = DBA`、
`ALTER USER x SET LOCKED = true`）在词法层即不成立：

1. **`=` 词元不匹配**：词法器对单个 `=` 产出 `Tk::Assign`，只有 `==` 才产出 `Tk::Eq`
   （`crates/linkrs-query/src/parser/lexing/lexer/token.rs:261-268`）。两处 ALTER USER
   实现均 `expect_token(TokenKind::Eq)`
   （`crates/linkrs-query/src/parser/parsing/user_parser.rs:114,117`、
   `crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs:233,236`），
   而 `expect_token` 是严格相等匹配
   （`crates/linkrs-query/src/parser/parsing/parse_context.rs:381-383`），必然报错。
2. **布尔值不是标识符**：`true/false` 被映射为 `BooleanLiteral`
   （`lexer/token.rs:238-239`），代码却用 `expect_identifier()` 读取
   （`user_parser.rs:118`、`alter_parser.rs:237`），该函数不接受布尔词元
   （`parse_context.rs:399-542` 无对应分支）。

全仓库对 SET 形态零测试覆盖（`SET ROLE` / `SET LOCKED` 仅出现在文档与
`user_parser.rs:111` 的注释中）。由于 server 预过滤器按 `ALTER USER` 前缀命中后会先
解析语句，用户实际看到的不是语法错误，而是误导性的
"Permission check failed: unable to classify user management statement"
（`crates/linkrs-server/src/graph_service/guard.rs:124-128`）。

即使解析修好，`is_locked = Some(value.to_lowercase() == "true")` 也会把
`SET LOCKED = no`、`SET LOCKED = yes` 等任意标识符静默当成 `false`（解锁）。

## 影响

文档示例全部不可用；账户锁定/解锁在查询通道没有可用入口（仅 HTTP 管理通道可启用/禁用）。

## 根因

语法作者按「`=` 即 Eq、布尔即标识符」的直觉书写，未核对词法器的实际产出；
且缺少针对 SET 子句的解析测试，缺陷从写入起未被发现。

## 建议

等号改按 `Tk::Assign` 接受；布尔值按 `BooleanLiteral` 读取，其余词元显式报
「期望布尔值」，取消 `to_lowercase() == "true"` 的模糊匹配。两处 ALTER USER 实现
同步修改（或按 `dcl-duplicate-parser-paths.md` 先收敛为一份再修）。

## 严重级

**P1（文档示例即坏，无测试覆盖）**

## 代码位置

`crates/linkrs-query/src/parser/parsing/user_parser.rs:112-121`、
`crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs:231-240`、
`crates/linkrs-query/src/parser/lexing/lexer/token.rs:238-268`
