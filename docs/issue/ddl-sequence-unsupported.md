# P1 [query] CREATE / DROP / ALTER SEQUENCE 均不可用

## 现象

文档 §8 定义 `CREATE SEQUENCE [IF NOT EXISTS] name [START] [INCREMENT] [MINVALUE] [MAXVALUE] [CYCLE|NOCYCLE]`，
§12 定义 `DROP SEQUENCE [IF EXISTS] name`。

parser 完整实现了这些子句（`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:455-508`，
`alter_parser.rs:30-72`，`drop_parser.rs:121-134`），AST 有 `CreateTarget::Sequence { name, start, increment, min_value, max_value, cycle }`。

但三层全不通：

| 语句 | 位置 | 结果 |
|---|---|---|
| CREATE SEQUENCE | `maintenance.rs:107-110` | `plan_create` 返回 `Ok(None)` → `maintain_planner.rs:93-95` 抛 `UnsupportedOperation("Create target is not supported by MaintainPlanner")` |
| ALTER SEQUENCE | `maintenance.rs:280-282` | `UnsupportedOperation("ALTER SEQUENCE planning not yet implemented")` |
| DROP SEQUENCE | `maintenance.rs:460-462` | `UnsupportedOperation("DROP SEQUENCE planning not yet implemented")` |

`PlanNodeEnum` 中没有 `SequenceManageNode`，因此 `DdlSpec::SequenceManage` 永不被构造；
`ddl_operator.rs:580-599` 的 `SequenceManageCommand::Create` 甚至 `let _ = (start, increment, min_value, max_value, cycle);` 丢弃全部参数后返回假 `"ok"`。

## 已有的孤立设施

`crates/linkrs-core/src/metadata/sequence.rs` + `sequence_manager.rs` 完整实现了
`SequenceDef { current_value, increment, min_value, max_value, cycle, start_value }`、带 CAS 重试的 `next_value()`、
`create_sequence()`、`update_sequence()`；`executor/base/execution_context.rs:100,213-218` 已挂 `sequence_manager()` 访问器；
`executor/expression/functions/builtin/sequence.rs:57-61` 已备好调用点（缺 manager 时报 "requires execution context with SequenceManager"）。
与 TTL 同类：设施齐备，唯一缺的是 query 侧 parser→plan→spec→executor 的接线。

## 影响

文档语法可解析但必然抛 not supported；`linkrs-core` 中一整块序列实现是无人调用的死重量。

## 根因

planner 从未为 Sequence 增加 plan node 与 spec builder，`Ok(None)` 被当作"暂缓"的通用逃生门，
而调用方把它转译成与真实原因不符的 "Create target is not supported by MaintainPlanner"。

## 建议

补齐接线（工作量主要在 planner/spec/executor 三层，core 侧无需改动）：

1. 在 `PlanNodeEnum` 增加 `SequenceManage(SequenceManageNode)`，定义 `SequenceManageInfo` 携带五个参数。
2. spec builder 产出 `DdlSpec::SequenceManage { command: SequenceManageCommand::Create{...}/Alter{...}/Drop{...} }`，
   参数全量传递，删除 `ddl_operator.rs:580-599` 里的 `let _ = ...`。
3. executor 调 `rt.sequence_manager()` 的 `create_sequence` / `update_sequence` / drop，`next_value()` 经 `nextval` 类函数暴露。
4. 若短期不接线，则从文档 §8、§12 删除序列语法，并删除 core 中无调用者的序列模块——不允许保留"语法可用、执行必败"。

## 严重级

**P1（契约违背 + 死代码）**

## 代码位置

- `crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:455-508`
- `crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs:30-72`
- `crates/linkrs-query/src/parser/parsing/ddl_parser/drop_parser.rs:121-134`
- `crates/linkrs-query/src/parser/ast/stmt/ddl.rs:56-63`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner/maintenance.rs:107-110,280-282,460-462`
- `crates/linkrs-query/src/planning/statements/ddl/maintain_planner.rs:85-95`
- `crates/linkrs-query/src/executor/streaming/operators/ddl_operator.rs:580-599`
- `crates/linkrs-core/src/metadata/sequence.rs`、`sequence_manager.rs`
- `crates/linkrs-query/src/executor/base/execution_context.rs:100,213-218`
