# DDL（`docs/release/03_ddl_schema_definition.md`）文档与实现不一致审计

> 审计对象：`docs/release/03_ddl_schema_definition.md` 对照 `crates/linkrs-query/src`（parser → planner → spec → executor）、
> `crates/linkrs-core`、`crates/linkrs-storage`、`crates/linkrs-server`。审计日期：2026-10-14。
>
> 本文是该次审计的**总清单**，逐条明细见下方各分册。所有结论均给出代码位置，可复核。

## 一句话结论

DDL 的核心病因**不是"没实现"，而是"实现了语法、丢弃了语义"**：parser 产出完整 AST，planer / spec / executor 逐层用 `..` 或窄化的命令枚举把字段抹掉。
用户拿到的反馈是"语法通过、行为静默失效或运行期才报 not supported"，比明确的 feature-missing 更难排查。

## 总览表

| 编号 | 问题 | 位置 | 文档位置 | 级别 |
|---|---|---|---|---|
| A | Cypher DML 触发 Schema 自动创建整节失效 | `binder/bind/bind_ddl.rs:12-25` | §3 | P0 |
| B | TTL 词法器不产 token，全链路死代码 | `ddl_parser/create_parser.rs:632-639` | §1、§2、汇总表 | P0 |
| C | 全文索引 ANALYZER/BOOST/OPTIONS/index_name 全部丢弃 | `spec/fulltext.rs:10-16` | §6 | P0 |
| D | CREATE/DROP/ALTER SEQUENCE 均不可用 | `maintenance.rs:107-110,280-282,460-462` | §8、§12 | P1 |
| E | CREATE TAG/EDGE AS (query) 查询路径拒绝 | `maintenance.rs:111-122` | §1、§2 | P1 |
| F | SHOW CREATE 仅 TAG 生成 DDL 且残缺 | `tag.rs:200-228` 等 | §15 | P1 |
| G | DROP 批量 / DROP GRAPH / DROP SPACE IF EXISTS / DROP INDEX IF EXISTS | `drop_parser.rs`、`maintenance.rs` | §12 | P1 |
| H | Space comment、未知 WITH 参数、vid_type FIXEDSTRING(N) 等 AST 字段被丢弃或降级 | `maintenance.rs:70-76` 等 | §4 | P1 |
| I | 向量索引限定仅顶点，与文档"标签或边类型"冲突 | `vector_planner.rs:86,186` | §7 | P2 |
| J | ALTER FULLTEXT INDEX 五个动作仅 OPTIMIZE 可用 | `fulltext_planner.rs:527-562` | §6 | P2 |
| K | ALTER ... CHANGE 丢失默认值/注释/可空性 | `maintenance.rs:271-275` | §10 | P2 |
| L | 类型表、SHOW HOSTS/PARTS、环检测时机、省略类型等文档漂移 | 多处 | §1、§9、§14 | P2 |
| M | 死代码与不可达兜底分支 | `create_parser.rs:44-191` 等 | 无 | P3 |

## 分册索引

- [P0 ddl-schema-auto-create-unsupported.md](ddl-schema-auto-create-unsupported.md) — A
- [P0 ddl-ttl-dead-parse-path.md](ddl-ttl-dead-parse-path.md) — B
- [P0 ddl-fulltext-index-config-dropped.md](ddl-fulltext-index-config-dropped.md) — C
- [P1 ddl-sequence-unsupported.md](ddl-sequence-unsupported.md) — D
- [P1 ddl-create-tag-as-query-session-only.md](ddl-create-tag-as-query-session-only.md) — E
- [P1 ddl-show-create-output-incomplete.md](ddl-show-create-output-incomplete.md) — F
- [P1 ddl-drop-semantics-gaps.md](ddl-drop-semantics-gaps.md) — G
- [P1 ddl-ast-fields-silently-dropped.md](ddl-ast-fields-silently-dropped.md) — H
- [P2 ddl-vector-index-scope-mismatch.md](ddl-vector-index-scope-mismatch.md) — I
- [P2 ddl-alter-change-loses-constraints.md](ddl-alter-change-loses-constraints.md) — K
- [P2 ddl-doc-vs-code-drift.md](ddl-doc-vs-code-drift.md) — L
- [P3 ddl-dead-code-and-unreachable-branches.md](ddl-dead-code-and-unreachable-branches.md) — M

## 贯穿性问题：为什么这些问题能长期共存

1. **测试只覆盖 parser，不覆盖执行**。`tests/ddl/tag_basic.rs:175-204` 对 `DROP TAG a, b, c` 只断言"能解析"；
   `tests/ddl/constraints.rs` 对 DEFAULT/NOT NULL 同样只测解析。执行层缺口因此没有测试红灯。
2. **没有端到端的"文档示例可执行"回归**。`docs/release/03_*.md` 中的示例语句没有任何机制保证能跑通。
3. **planner 的 `..` 解构是静默的**。`CreateTarget::Tag { name, properties, .. }` 少写两个字段名不报错、不警告。
4. **`Ok(None)` 被当作"暂不支持"的通用逃生门**，调用方统一转成 `UnsupportedOperation`，错误信息与真实原因（AST 字段缺失）脱节。

## 修复原则（建议在动代码前冻结）

- **文档是发布契约**。每条不一致二选一：补齐实现，或从语法与文档中同时移除。不允许保留"可解析但静默失效"的中间态。
- **不允许用兜底掩盖缺口**。空名分支、`unwrap_or_default()` 式的默认落点一律删除，改为显式错误。
- **先修"假能力"再修"弱能力"**。显形化（本审计 P0/P1）优先于补强（P2）。
- 配套的分阶段修改方案见 `docs/plan/ddl_definition_alignment.md`。
