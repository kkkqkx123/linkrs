# P1 [query] CREATE SPACE 的参数在 planner/executor 静默丢失

## 现象

文档 §4 定义 `CREATE [SPACE|GRAPH] [IF NOT EXISTS] name [(vid_type=..., comment='...')] [WITH key=value ...]`，
并声明「可添加注释（`comment`，括号内或 `WITH comment='...'` 均可）」。

parser 侧两个入口都实现了：括号内 `comment=`（`create_parser.rs:330-365`）、尾随 `WITH key=value`
（`create_parser.rs:712-731` `parse_space_with_params`）。但从 planner 起被丢弃：

`crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:70-76`
```rust
CreateTarget::Space { name, vid_type, .. } => {          // `..` 丢弃 comment
    let space_info = SpaceManageInfo::new(name.clone()).with_vid_type(vid_type.clone());
```

`SpaceManageInfo` 只有 `space_name` + `vid_type`（`plan/core/nodes/management/space_nodes.rs:206-210`），
`SpaceManageCommand::Create { space_name, vid_type }`（`executor/streaming/plan/arena_builder/specs/ddl.rs:17-20`）也没有 comment 字段，
executor（`schema_executor/space.rs:43-52`）只设置 `with_vid_type`，从不 `with_comment`。

同时 `DESC SPACE` / `SHOW SPACES` 却输出 `comment` 列（`schema_executor/space.rs:103-108`）——该列恒为空，
看起来像"没设注释"而非"注释没被存"。

## 附带问题一：未知 WITH 参数被静默吞掉

`parse_space_with_params`（`create_parser.rs:712-731`）对非 `comment` 键**解析后直接丢弃且不报错**。
文档说"当前单节点版本不支持 `partition_num`、`replica_factor` 参数"，但用户写
`CREATE SPACE s WITH partition_num=10` 得到的不是报错而是**看似成功的空操作**。这违反了误用防护原则。

## 附带问题二：`vid_type=FIXEDSTRING(N)` 被静默降级为 STRING

- `parse_vid_type_value`（`helpers.rs:246-250`）对裸标识符走 `s.to_uppercase()` 直通，返回 `"FIXEDSTRING32"`（**无括号**）。
- `parse_vid_type_str`（`schema_executor/common.rs:35-59`）要求前缀 `FIXED_STRING(` / `FIXEDSTRING(`，
  `"FIXEDSTRING32"` 落进兜底分支变成 `DataType::String`。
- 任意拼错的类型名（如 `vid_type=FOO`）同样静默变成 `DataType::String`。

文档 §4 明确写「支持 `FIXEDSTRING(N)` 等」。现有测试 `stmt_parser/tests.rs:253-259`
（`CREATE SPACE test_space(vid_type=FIXEDSTRING32)`）只验证"能解析"，不验证解析出的类型，故未暴露降级。

## 附带问题三：两处默认值不一致

parser 层默认 `"INT64"`（`create_parser.rs:120,330`），而 `SpaceManageInfo::new()` 的字段默认值是
`"FIXED_STRING(32)"`（`space_nodes.rs:212-224`）。当前 `plan_create` 总是覆盖 parser 值所以实际生效的是 INT64，
但 `SpaceManageInfo::new()` 的默认值是错的，任何新调用点都会踩坑。

## 影响

1. 文档承诺的 space 注释能力实际不可用（DDL 路径）。
2. typo 与不支持的参数都被静默接受，用户无法从报错获知配置未生效——比报错更危险。
3. `FIXEDSTRING(N)` 静默降级为无界 STRING，会在存储层表现为 VID 空间无限扩大，而不是预期的定长。

## 根因

1. planner 用 `..` 解构丢字段；下游数据结构（`SpaceManageInfo` / `SpaceManageCommand::Create`）设计时就只带 vid_type。
2. `parse_vid_type_value`（parser，产生.str）与 `parse_vid_type_str`（executor，消费 str→DataType）两套归一化规则不一致，
   且 executor 的兜底是 `DataType::String` 而非报错。
3. `parse_space_with_params` 把"认识 comment"实现成"其他一律忽略"。

## 建议

1. `SpaceManageInfo` / `SpaceManageCommand::Create` 增加 `comment: Option<String>`，全链路贯通到 `SpaceInfo::with_comment`。
2. `parse_space_with_params` 遇到未知键**显式报错**（列出已支持的键），而不是忽略。
3. 统一 vid_type 归一化：parser 只负责生成规范字符串（`FIXED_STRING(n)` 形式），executor 的 `parse_vid_type_str`
   去掉 `DataType::String` 兜底，改为对无法识别的输入报错；同时修掉 `FIXEDSTRING32` 无括号直通的路径。
4. 把 `SpaceManageInfo::new()` 的默认 vid_type 改为 `INT64`，与 parser 默认值一致（或直接要求调用方显式传入）。
5. 补测试：断言 `comment='x'` 后 `DESC SPACE` 的 comment 列确实是 `x`；断言 `vid_type=FIXEDSTRING32` 得到
   `FixedString(32)` 而非 `String`；断言 `WITH partition_num=10` 报错。

## 严重级

**P1（契约违背 + 静默错误配置）**

## 代码位置

- `crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:321-365,712-731`
- `crates/linkrs-query/src/parser/parsing/ddl_parser/helpers.rs:192-257`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:70-76`
- `crates/linkrs-query/src/planning/plan/core/nodes/management/space_nodes.rs:206-224`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/common.rs:35-59`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/schema_executor/space.rs:43-52,103-108`
- `crates/linkrs-query/src/parser/parsing/stmt_parser/tests.rs:253-259`
