# P1 [query] DROP 语句的批量、GRAPH 同义与 IF EXISTS 三处语义缺口

## 现象

文档 §12 声称：

- 「TAG/EDGE 支持**批量删除**与 IF EXISTS」
- 「`DROP SPACE` 与 `DROP GRAPH` **等价**，支持 IF EXISTS」
- 「`DROP INDEX` 默认按 TAG 索引处理；`ON <space_name>` 可选」

三个断言在代码中分别对应三类缺口：

### 1. 批量删除：解析成功、计划期拒绝

`crates/linkrs-query/src/parser/parsing/ddl_parser/drop_parser.rs:54-57`、`84-87` 正确解析为
`DropTarget::Tags(Vec<String>)` / `DropTarget::Edges(Vec<String>)`。计划期直接拒绝：

`crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:369-375`
```rust
if tag_names.len() > 1 {
    return Err(PlannerError::UnsupportedOperation(
        "DROP TAG with multiple names is not yet supported; drop one tag per statement"));
}
```
`384-390` 对 EDGE 同样处理。

现有测试只测解析、不测执行：`crates/linkrs-query/tests/ddl/tag_basic.rs:175-204` 的
`test_drop_tag_parser_multiple` / `..._multiple_with_if_exists` 只断言 `stmt.kind() == "DROP"`。

### 2. `DROP GRAPH` 缺失，与 CREATE GRAPH / USE GRAPH 不对称

`drop_parser.rs:23` 只 `match_token(TokenKind::Space)`。而 CREATE 侧对 `GRAPH` 有专门同义处理
（`create_parser.rs:32-41` `consume_space_or_graph`），`USE GRAPH` 同样可用。
`DROP GRAPH basketball` 直接落到 `:165-171` 的 `"Expected SPACE, TAG, EDGE, INDEX, SEQUENCE, MACRO, TYPE, or USER"` 错误。
错误信息里也没提 GRAPH，用户无从推断。

### 3. IF EXISTS 在两类对象上失效

**a) `DROP SPACE IF EXISTS`**：parser 正确解析（`drop_parser.rs:24-28`），但 `maintenance.rs:399-405` 的
`DropSpaceNode::new(...)` 不接受该标志，executor（`schema_executor/space.rs:53-58`）直接调
`StorageSchemaOps::drop_space`。删除不存在的 space 仍报错——`IF EXISTS` 被 silently 吞掉。

**b) `DROP TAG INDEX / EDGE INDEX / INDEX ... IF EXISTS`**：这三个分支根本不解析 `IF EXISTS`
（`drop_parser.rs:36-47`、`66-77`、`94-104`），并且统一在 `:176-180` 构造 `if_exists: false`。
因此 `DROP TAG INDEX IF EXISTS i` 是语法错误而非静默忽略——能力面与其他 DROP 对象（TAG/EDGE/SPACE/USER/SEQUENCE/MACRO/TYPE 都支持）不一致。

## 影响

1. §12 的三条声明全部与实际不符。
2. `DROP TAG a, b, c` 的失败点在**计划期**而非解析期，错误信息与"语法没错"的直觉相悖，排查成本高。
3. `DROP SPACE IF EXISTS` / `DROP TAG INDEX IF EXISTS` 的失败模式不一致（一个静默吞标志、一个语法报错），难以对用户形成一致预期。

## 根因

1. `DropSpaceNode` / `DropTagIndexNode` / `DropEdgeIndexNode` 及对应 spec 命令没有 `if_exists` 字段，planner 无处安放。
2. 批量删除的 executor 侧也未实现循环删除（TAG 删除在 storage 层还会按 tag 逐个校验数据非空）。
3. `CREATE` 与 `DROP` 的关键字同义表没有共享同一处定义，`GRAPH` 只加在了 CREATE/USE 侧。

## 建议

按"最小可交付→完整语义"排序：

1. **统一关键字同义词**：抽出 Space/Graph 判别函数供 CREATE/USE/DROP 共用，`DROP GRAPH` 直接可用，错误信息补上 `GRAPH`。
2. **`DROP SPACE IF EXISTS`**：`DropSpaceNode` / `SpaceManageCommand::Drop` 增加 `if_exists`，executor 在 space 不存在时按标志返回空结果。
3. **`DROP TAG|EDGE INDEX ... IF EXISTS`**：三个分支统一解析 `IF EXISTS` 并贯通到 plan/spec/executor。
4. **批量删除**：executor 对 `Vec<String>` 逐项删除并在结果中逐项汇报；已存在的 `Tags(_)` / `Edges(_)` 空 Vec 兜底分支
   （`maintenance.rs:442-459`）删除，因为 parser 保证非空，该分支只会构造空名节点。

## 严重级

**P1（契约违背 + 错误模式不一致）**

## 代码位置

- `crates/linkrs-query/src/parser/parsing/ddl_parser/drop_parser.rs:23-34,36-47,48-63,66-77,78-93,94-104,165-180`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:369-398,399-405,442-459`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/space.rs:53-58`
- `crates/linkrs-query/tests/ddl/tag_basic.rs:175-204`
- 同义处理正例：`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:32-41`
