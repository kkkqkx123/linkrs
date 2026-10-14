# P0 [query] Cypher DML 触发 Schema 自动创建整节失效

## 现象

`docs/release/03_ddl_schema_definition.md` §3「Schema 自动创建（Cypher DML 触发）」描述了完整能力：
`CREATE (n:Label {name: 'Alice', age: 30})` 在 Label 不存在时自动推断类型并建 Schema，并给出类型推断表与三个示例。

实际执行直接报错：

`crates/linkrs-query/src/binder/bind/bind_ddl.rs:12-25`
```rust
CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. } => {
    // Cypher-style data creation is parsed but has no executable
    // write path (no id generation, no schema inference): fail
    // here instead of planning a statement that cannot run.
    Err(... "CREATE data patterns are not supported yet; use INSERT VERTEX / INSERT EDGE")
}
```

planner 侧同样无路径：`planning/statements/ddl/maintain_planner/maintenance.rs:104-106` 对 `Node|Edge|Path` 返回 `Ok(None)`，
由 `maintain_planner.rs:93-95` 升级为 `UnsupportedOperation("Create target is not supported by MaintainPlanner")`。

## 连带失效的文档内容

| 文档内容 | 状态 |
|---|---|
| §3 数据类型推断表（`'Alice'→STRING`、`30→INT64`、`datetime()→DATETIME`） | DML 路径无任何类型推断实现 |
| §3 「自动创建标签 / 自动创建边类型 / 自动创建多标签」三个示例 | 全部无法执行 |
| §3 注意事项 2「如需约束，请使用 ALTER TAG/EDGE 修改」 | 插入本身走不通，ALTER 无从谈起 |
| §3 「与显式 DDL 的对比」表 | 左列整体不存在 |

## 库内已有的相反证据

`crates/linkrs-query/tests/integration_cypher_create.rs:241-264`（`assert_create_data_rejected`）**固化**了「拒绝 CREATE 数据模式」这一行为。
同文件 291-339 行的 "schema inference" 测试只断言语句可解析，不断言类型被推断——即团队已知此能力缺失并已用测试锁定现状。

唯一的类型推断实现在 session API 的 `CREATE TAG ... AS (query)` 路径上
（`crates/linkrs-api/src/embedded/session.rs:1899-1952`），且与文档推断表不一致：整数映射到 `DataType::Int`（32 位）而非文档表格的 `INT64`，
全 NULL 列被武断定为 `STRING`。

## 影响

1. 文档对外承诺的核心易用性特性完全不存在，新用户按文档操作第一阶段即失败。
2. 项目实际写入入口只有 `INSERT VERTEX` / `INSERT EDGE`，但文档 §2 DML 未说明这一点。
3. "schema inference" 测试名与断言内容不符，容易让后来者误以为能力存在。

## 根因

不是代码 bug，而是**能力缺口 + 文档超前**。DDL（`CREATE TAG/EDGE`）与 DML（`CREATE (n:Label {...})`）的实现投入不对称：
前者有完整的 parser/planner/spec/executor/storage 五层，后者只有 parser 一层。

## 建议

二选一，不允许维持中间态：

- **方案一（补齐）**：把 §3 变成真实能力。需要实现顶点 ID 生成、按值推断属性类型、复用 `TagManageCommand::Create` 路径建 Schema。
  工作量大，且与 `INSERT VERTEX` 职责重叠。
- **方案二（移除，推荐）**：从文档 §3 整节删除，`docs/release/02_dml_data_manipulation.md` 中显式声明「节点/边写入仅支持 `INSERT VERTEX`/`INSERT EDGE`」。
  同时把 `integration_cypher_create.rs` 中名为 "schema inference" 的测试改名为 `parse_only` 语义的名字，并把 `assert_create_data_rejected`
  的错误信息补上文档指引。

## 严重级

**P0（正确性 / 契约违背）** — 文档整节描述不存在的能力，且现有测试反向锁定了缺失状态。

## 代码位置

- `crates/linkrs-query/src/binder/bind/bind_ddl.rs:12-25`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:104-106`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner.rs:85-95`
- `crates/linkrs-query/tests/integration_cypher_create.rs:241-339`
- `crates/linkrs-api/src/embedded/session.rs:1899-1952`
