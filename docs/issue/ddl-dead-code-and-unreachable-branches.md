# P3 [query] DDL 相关死代码与不可达兜底分支

> 这些不直接产生错误行为，但会误导读者、掩盖真实缺口，并增加后续改动的推理成本。

## 1. 两份 CREATE 解析器并存，活的那份是重复实现

`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs`：

- `:44-191` `DdlParser::parse_create_statement` —— **从未被调用**（dispatch 走 `misc.rs:99-109` → `:193-620`）。
- `:193-620` `parse_create_after_token` —— 活路径。

活路径内部还重复实现了两轮 SPACE / TAG INDEX / EDGE INDEX：`:199-229`（TAG INDEX 快路径）与 `:395-424`（同名分支，
因前一条已 `return` 而不可达）；EDGE INDEX 同理（`:260-290` 与 `:425-454`）；SPACE 在 `:321-365` 与 `:156-183` 的
CREATE USER 路径相邻但只有一份。

死函数体量约 150 行，其中包含 parser 默认值、TTL 解析等逻辑的**第二份拷贝**，改一处漏一处的风险明确存在。

## 2. `parse_show_create_statement` 死函数

`crates/linkrs-query/src/parser/parsing/ddl_parser/describe_parser.rs:70-98` 已死，
活路径是 `parser/parsing/util_stmt_parser.rs:41-68`（由 `show_parser.rs:127-128` 调用）。

## 3. `plan_drop` 的空名兜底分支

`crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:442-459`：

```rust
DropTarget::Tags(_) => { ... DropTagNode::new(..., String::new()).with_if_exists(if_exists) ... }
DropTarget::Edges(_) => { ... DropEdgeNode::new(..., String::new()) ... }
```

这两条在 `:369-383` / `:384-398` 的 guarded arms（`if !tag_names.is_empty()`）之后。
parser 保证 `vec![ctx.expect_identifier()?]` 至少一个元素（`drop_parser.rs:54,84`），空 Vec 不可达。
若将来可达，它会去删除一个**空名字**的 tag。

这是典型的"掩护式兜底"：它不产生正确行为，只是让 match 看起来 exhaustive。

## 4. `AlterTarget::Space` 无 parser 产出

`crates/linkrs-query/src/parser/ast/stmt/ddl.rs` 定义了 `AlterTarget::Space { space_name, comment }`，
`maintenance.rs:229-244` 也为它做了 planning 并产出 `AlterSpaceNode`，但 parser 不生成该变体
（`alter_parser.rs:79-83` 只接受 TAG/EDGE/USER）。

更糟的是 executor 对它的实现是错的：`schema_executor/space.rs:59-66` 把 `space_name` 当注释内容用、
`space_id` 硬编码 `0`。即这条路径两端都不可用。

## 5. `SequenceManageCommand` 的假实现

`crates/linkrs-query/src/executor/streaming/operators/ddl_operator.rs:580-599` 的
`SequenceManageCommand::Create` 中 `let _ = (start, increment, min_value, max_value, cycle);`
丢弃全部参数后返回 `"ok"` / `"if_not_exists"` 状态行。由于该 spec 永不被构造，这段代码既不可达也不正确。

## 6. 词的 `..` 解构静默丢字段（系统性）

以下位置用 `..` 忽略了 AST 字段，均无编译告警：

| 位置 | 被忽略的字段 |
|---|---|
| `maintenance.rs:77-86` | `CreateTarget::Tag` / `EdgeType` 的 `ttl_duration`、`ttl_col` |
| `maintenance.rs:70-76` | `CreateTarget::Space` 的 `comment` |

建议为 planer 引入"AST 字段必须显式消费"的检查（clippy 自定义 lint 或 code review 清单），
否则后续每加一个 DDL 字段都会重复这个坑。

## 建议

按依赖顺序清理：

1. 删除 `create_parser.rs:44-191` 与不可达的重复 SPACE/TAG INDEX/EDGE INDEX 分支，只留 `parse_create_after_token`。
2. 删除 `describe_parser.rs:70-98`。
3. 删除 `maintenance.rs:442-459` 的空名兜底分支，改为 `unreachable!()` 或直接由 guarded arms 穷尽。
4. `AlterTarget::Space`：补齐 parser（`ALTER SPACE <n> [WITH] COMMENT = '...'`）并修 executor 的字段读取，
   或整体删除该变体与 `AlterSpaceNode`。
5. `SequenceManageCommand` 随 `ddl-sequence-unsupported.md` 的处置一并决定。

## 严重级

**P3（可维护性）** —— 但第 3、4 条与 P1 问题耦合，建议在同一轮改动中处理。

## 代码位置

- `crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:44-191,395-424,425-454`
- `crates/linkrs-query/src/parser/parsing/ddl_parser/describe_parser.rs:70-98`
- `crates/linkrs-query/src/parser/parsing/util_stmt_parser.rs:41-68`
- `crates/linkrs-query/src/parser/parsing/stmt_parser/misc.rs:99-109`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:442-459,229-244`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/space.rs:59-66`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator.rs:580-599`
