# P1 [query] SHOW CREATE 输出与文档承诺不符

## 现象

文档 §15 承诺 `SHOW CREATE SPACE/TAG/EDGE/INDEX` 返回**单列 `create_statement`**，内含属性定义、
`NOT NULL`、`DEFAULT`、`COMMENT`、TTL 配置，并给出 Person 标签的完整输出样例。

实际四个目标中只有一个生成 DDL 文本：

| 目标 | 位置 | 实际形态 |
|---|---|---|
| TAG | `executor/streaming/operators/ddl_operator/schema_executor/tag.rs:200-228` | **列名叫 `create_tag`（不是 `create_statement`）**；内容为 `CREATE TAG <name> (<prop> <DataType:?>, ...)`，无 `IF NOT EXISTS`、无 `NOT NULL`、无 `DEFAULT`、无 `COMMENT`、无 TTL |
| EDGE | `.../schema_executor/edge.rs:124-179` | `ShowCreate` 与 `Desc` 共用一个 match arm，返回 `Field/Type/Nullable/Default/Comment` 五列（外加两行合成的 endpoint 行） |
| SPACE | `.../schema_executor/space.rs:73-133` | 与 `Desc` 共用一个 arm，返回 `name/id/vid_type/partition_num/replica_factor/comment/status` 七列 |
| INDEX | `.../schema_executor/index.rs:130-183` | 与 `DescTagIndex` 共用一个 arm，返回 `name/index_type/fields/status/unique` 五列 |

即：除 TAG 外，`SHOW CREATE x` 与 `DESC x` 的输出**完全无法区分**，四个目标的列结构互不相同，
与文档的"统一 create_statement 列"不符。

`tests/` 下没有任何 SHOW CREATE 输出断言（grep 仅命中 CLI 帮助文本 `crates/linkrs-cli/src/command/meta_commands.rs:327`）。

## 影响

1. 文档 §15 的「返回结果」样例（含 `NOT NULL`、`DEFAULT 'unknown'`、`COMMENT '主键ID'`、`ttl_duration`、`ttl_col`）**任何一条都产不出**。
2. CLAUDE/工具链若依赖 `SHOW CREATE` 做 schema 迁移（文档说"便于查看对象定义或迁移数据"），拿到的列形态不可预测。
3. TAG 路径虽然生成了 DDL，但用 `{:?}` 打印 `DataType`（会输出 `String`、`BigInt` 这类 Rust 变体名而非 `STRING`/`INT64`），
   且属性可空性、默认值、注释全部丢失，输出的 SQL **无法回灌执行**。

## 根因

`ShowCreate` 在 planner 侧被降级为对应的 `Desc` plan node（`planning/statements/ddl/maintain_planner/show.rs:72-107` 之后各 target
分别落到 `TagManageNode::ShowCreate` / `EdgeManageNode::ShowCreate` / `SpaceManageNode::ShowCreate` / `IndexManageNode::ShowCreate`），
executor 只实现了 TAG 一个真正的 DDL 渲染，其余三个直接复用 Desc 渲染以省事。

## 建议

1. 统一输出契约：四个目标都返回单列 `create_statement`（列名与文档一致）。
2. TAG 渲染补全：`IF NOT EXISTS`、逐属性的类型 / `NOT NULL` / `DEFAULT <value>` / `COMMENT '<text>'`、`ttl_duration` / `ttl_col`（若 TTL 保留）。
3. EDGE 渲染新增真正的 DDL 生成，含 `FROM <src> TO <dst>` 端点约束；SPACE 渲染含 `vid_type` 与 `comment`；INDEX 渲染含 `ON <target>(<props>)`。
4. 数据类型用 `DataType` 的 `Display`（`STRING`/`INT64` 这类 SQL 拼写）而非 `{:?}`；默认值需要 `Value` 的 SQL 字面量形式。
5. 补执行层测试：对四个目标分别断言输出含 `NOT NULL` / `DEFAULT` / `COMMENT` 等子串，而不是只测不报错。

## 严重级

**P1（契约违背 + 迁移场景不可用）**

## 代码位置

- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/tag.rs:200-228`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/edge.rs:124-179`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/space.rs:73-133`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/index.rs:130-183`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/show.rs:72-107`
