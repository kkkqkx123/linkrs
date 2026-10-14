# P2 [query] ALTER ... CHANGE 丢失默认值 / 注释 / 可空性，且 EDGE 路径与 TAG 路径不一致

## 现象

文档 §10 列 `ALTER TAG <name> CHANGE (<old_prop> <new_prop>: <prop_type> [, ...])`，
描述为「支持属性重命名 + 类型修改」。

`crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:271-275`
```rust
for change in changes {
    let prop = PropertyDef::new(change.new_name.clone(), change.data_type.clone());
    alter_info.additions.push(prop);
    alter_info.deletions.push(change.old_name.clone());
}
```

`PropertyDef::new` 只带 `name` + `data_type`，因此：

- `nullable` 回到默认 `true`
- `default`、`comment` 丢失
- `serial` 丢失

原来 `age: INT NOT NULL DEFAULT 0 COMMENT '年龄'` 被 CHANGE 成 `new_age: INT` 后变成可空、无默认、无注释。

## TAG 与 EDGE 路径不一致

TAG 走 `TagManageCommand::Alter` 的 `changes` 字段（spec 层有），executor 走
`schema_executor/tag.rs:89-98` 的 `rename_tag_property`（语义正确，只改名不重建）。
EDGE 走上面的 add+drop 组合，且 `EdgeManageCommand::Alter` **根本没有 `changes` 字段**。

即同一句 `ALTER EDGE e CHANGE (a b: STRING)` 与 `ALTER TAG t CHANGE (a b: STRING)` 走的是两条不同实现路径，
一条是"真重命名"，一条是"删旧增新"——后者还会丢失数据。

## 影响

1. CHANGE 是有损操作但文档未提示。
2. TAG / EDGE 行为不一致，同样的语法在不同对象上语义不同。
3. EDGE 上做 CHANGE 等于删列增列，可能触发存储层对已存在 NULL/默认值数据的处理差异。

## 根因

1. `PropertyDef::new` 是精简构造器，planner 用法没打算保留其他字段。
2. `EdgeManageCommand::Alter` 的字段集漏了 `changes`，无法把"重命名"作为一种独立操作传下去。

## 建议

1. CHANGE 的语义先冻结为「仅改名（+可选改类型）」，文档明确：**不保留也不修改默认值/注释/可空性**——
   若需要调整这些，用 `DROP` + `ADD` 显式表达。
2. EDGE 侧补齐 `changes` 字段与 `rename_edge_property` 路径，与 TAG 对齐；`EdgeAlterInfo` 增加 `with_changes`。
3. 若保留"改类型"，需要明确旧值不兼容时的行为（报错还是清空），并在文档写清。

## 严重级

**P2（语义不一致 + 有损操作未声明）**

## 代码位置

- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:246-279`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/tag.rs:83-98`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/edge.rs:83-97`
- `crates/linkrs-query/tests/ddl/tag_alter.rs`、`edge_alter.rs`
