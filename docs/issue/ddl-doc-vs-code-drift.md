# P2/P3 [query] 文档与实现的零散漂移

> 单条不足以独立立项的小偏差汇总。每条都给出代码位置与纠偏方向。

## 1. 整数类型名与位宽映射违反直觉（文档 §1 类型表）

`crates/linkrs-core/src/types/parse.rs:77-79`
```rust
"SMALLINT" | "INT2" => Ok(DataType::SmallInt),
"INT" | "INTEGER" | "INT4" | "INT16" | "INT32" => Ok(DataType::Int),
"BIGINT" | "INT8" | "INT64" => Ok(DataType::BigInt),
```

`INT16` → `Int`（32 位）、`INT32` → `Int`（32 位）。按名字读 `INT16` 应该是 16 位
（项目里的 `SmallInt`）。文档 §1 只笼统写「整数类型」，读者无法预期。

**纠偏**：文档的类型表补一列实际 `DataType` 与位宽；或修正映射使 `INT16→SmallInt`、`INT32→Int`。
二者都涉及已建 schema 的兼容，需要先决策。

## 2. `TIMESTAMP` 不是独立类型（文档 §1 类型表）

`parse.rs:86-88`：`"DATETIME" | "TIMESTAMP" => Ok(DataType::DateTime)`，注释说明「TIMESTAMP 归一为 DATETIME」。
文档 §1 把 `TIMESTAMP` 与 `DATETIME` 并列成两行，未说明二者等价。

**纠偏**：文档在类型表标注「TIMESTAMP 为 DATETIME 的别名」。

## 3. `TIME` 类型存在但文档未列

`parse.rs:85` 有 `DataType::Time`，文档 §1 类型表没有 `TIME` 行。

**纠偏**：文档补 `TIME` 行，或确认 Time 类型不上报并在文档说明。

## 4. 「属性名后省略 `:` 与类型」不可用（文档 §1 关键特性）

文档称「支持属性名后省略 `:` 与类型（省略时类型由绑定阶段处理）」。
`crates/linkrs-query/src/parser/parsing/ddl_parser/helpers.rs:151-159`：
```rust
let name = ctx.expect_identifier()?;
ctx.match_token(TokenKind::Colon);              // ':' 确实可选
let mut serial = false;
let dtype = if ctx.match_token(TokenKind::Serial) { ... }
            else { self.parse_data_type(ctx)? };  // 但类型必填
```
`CREATE TAG x(name)` 会以 "Expected data type" 失败。无对应测试。

**纠偏**：从文档删除该条特性（parser 的 `match_token(Colon)` 放宽是半截实现），
或实现为"省略类型时推导为 STRING 并文档明示"。

## 5. `SHOW HOSTS` / `SHOW PARTS` 与 `SHOW SPACES` 无法区分（文档 §14）

`crates/linkrs-query/src/parser/parsing/show_parser.rs:85,93` 两者都映射到 `ShowTarget::Spaces`。
文档 §14 把它们与 `SHOW SPACES` 并列成三种能力。

**纠偏**：文档删除 `SHOW HOSTS` / `SHOW PARTS`，或为它们定义独立 `ShowTarget` 与真实语义。

## 6. 类型别名环检测时机描述错误（文档 §9 注释）

文档注释写「环检测在计划期执行」。实际：
- 创建期检查：`crates/linkrs-core/src/metadata/type_alias_manager.rs:229-255`
- 解析期兜底检查：同文件 `:188-224`
- 计划期只校验空名/空类型：`maintenance.rs:199-204`

**纠偏**：文档改为「环检测在别名创建与解析时执行」。

## 7. SERIAL 的隐性限制未文档化（文档 §1 约束表）

- SERIAL 不能带 DEFAULT：`crates/linkrs-storage/src/engine/graph_storage/schema_writer.rs:31-47`
- SERIAL 不能作为主键：同文件 `:333-342`
- 每个 tag/edge 最多一个 SERIAL 列：同文件

文档的 SERIAL 行只写「自增整数，隐含 NOT NULL」。

**纠偏**：文档 §1 SERIAL 说明补齐三项限制。

## 8. `DEFAULT` 常量函数在**解析期**求值（文档 §1 措辞易误读）

文档写「DEFAULT 值可以是字面量（含负数）或常量函数调用（如 `datetime()`），在创建 Schema 时即求值」——这句本身是对的。
但读者容易理解为"建 Schema 时确定一次"，实际是**解析 SQL 的那一刻**：
`crates/linkrs-query/src/parser/parsing/ddl_parser/helpers.rs:128-145` 的 `parse_and_eval_function_call`
在 parser 内直接 `ExpressionEvaluator::evaluate`。`DEFAULT datetime()` 会在 DDL 被解析时冻结成常量，
之后所有插入行共享同一个时间戳，而不是每行取当前时间。

**纠偏**：文档明确"常量函数在 DDL 语句解析时求值一次并冻结为常量值"。

## 9. `CREATE SPACE` 括号内未知参数与 WITH 未知参数错误模式不一致

括号内无法识别的键会因缺少逗号解析而报错（`create_parser.rs:333-353` 的循环末尾
`expect_token(RParen)`），而 `WITH` 未知键被静默忽略（`create_parser.rs:712-731`）。
同一类拼写错误，两种反馈。

**纠偏**：见 `ddl-ast-fields-silently-dropped.md` 建议 2，统一为显式报错。

## 严重级

**P2（文档误导，多为表述问题）** / 第 4、5 条涉及实现取舍，可升 P1。

## 代码位置

见各条内联引用。
