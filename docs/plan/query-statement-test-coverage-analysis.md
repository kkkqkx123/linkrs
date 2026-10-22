# Query 语句语义测试覆盖分析与补充方案

本文对 `crates/linkrs-query` 的语句（Statement）测试覆盖做一次系统盘点，回答两个问题：

1. 现有测试是否验证了**各语句的语义正确性与完整性**；
2. 为达到该目标，**还需要补充哪些测试**。

分析对象是查询语言本身的可观测语义——解析结果、绑定/计划形态、执行结果的数据内容与状态副作用——而不是"能不能跑通"。

处置方案见 `docs/plan/query-deadend-statement-implementation.md`。

---

## 1. 分析范围

### 1.1 纳入范围

- `crates/linkrs-query/src/parser/ast/stmt.rs` 定义的 `Stmt` 枚举全部 **84 个变体**。
- 每个变体在三个层面的覆盖情况：
  - **解析层**：parser 能否把 SQL 文本变成对应的 AST 变体。
  - **计划层**：binder / planner 能否为该变体产出可执行计划。
  - **执行层**：语句能否真正跑到 executor 并产出结果/副作用。
- 执行层之上再区分**语义断言强度**：
  - **强**：断言了返回行的具体内容、列名、属性值、计划形状，或状态副作用（数据被写入/回滚）。
  - **中**：断言了行数、集合基数、结果条目数。
  - **弱**：只断言"不报错"或"成功"。
  - **仅拒绝**：语句只被用来验证"被明确拒绝"，没有任何成功路径。
  - **无**：没有任何测试触及。

### 1.2 不在范围内

- 存储层、事务层、同步层的语义（另有专门测试套件）。
- HTTP / gRPC / C API 的传输层行为。
- 性能与基准。

---

## 2. 分析方法

| 步骤 | 手段 |
|---|---|
| 语句清单 | 直接读取 `Stmt` 枚举与 `Stmt::kind()` / `StmtCategory` 映射 |
| 路由链 | 追踪 `Binder::bind_stmt`（`src/binder/bind.rs`）与 `PlannerEnum::from_bound_statement`（`src/planning/planner.rs`）的分支，确认每类语句落到哪个 planner；再确认 `src/pipeline/compiler.rs` 中"无 planner"时的错误路径 |
| 测试扫描 | 全仓 `rg` 检索 `crates/linkrs-query/tests/`、`crates/linkrs-query/src/**` 的 `#[cfg(test)]`、`tests/`（根 crate）、`crates/linkrs-api/src`、`crates/linkrs-server/src` |
| 断言强度 | 统计每个测试文件中"值断言"（`assert_result_contains` / `assert_vertex_props` / `assert_edge_props` / `assert_result_columns` / `assert_plan_contains` / `assert_vertex_count` 等）与 `#[test]` 的比例，并抽样人工核对 |
| **可达性实测** | 编写一次性探针测试，对每条存疑语句跑真实 pipeline（`TestScenario`），记录实际返回的解析/计划/执行错误 |

最后一步是本次分析的关键：静态读代码只能推断"能否计划"，实测才能区分**真正的解析失败**、**内部错误泄漏**和**有意的显式拒绝**。

实测结论（语句 → 实际结果）：

| 语句 | 实测结果 |
|---|---|
| `SEARCH INDEX t MATCH 'x'` | `[planning] No suitable planner found: No planner for bound statement: SEARCH` |
| `LOOKUP ON t INDEX i WHERE 'x'` | `No planner for bound statement: LOOKUP FULLTEXT` |
| `MATCH 'x' WHERE FULLTEXT_MATCH(f,'x')` | `No planner for bound statement: MATCH FULLTEXT` |
| `DROP FULLTEXT INDEX i` | `No planner for bound statement: DROP FULLTEXT INDEX` |
| `ALTER FULLTEXT INDEX i REBUILD` | `No planner for bound statement: ALTER FULLTEXT INDEX` |
| `SHOW FULLTEXT INDEX` | `No planner for bound statement: SHOW FULLTEXT INDEX` |
| `DESCRIBE FULLTEXT INDEX i` | `No planner for bound statement: DESCRIBE FULLTEXT INDEX` |
| `LOAD FROM 'f.csv'` | `No planner for bound statement: LOAD FROM` |
| `CALL foo()` | `No planner for bound statement: CALL` |
| `LOAD / INSTALL / UNINSTALL / UPDATE EXTENSION` | `No planner for bound statement: <动作> EXTENSION` |
| `MIGRATE PLAN / EXECUTE / ROLLBACK` | `No planner for bound statement: MIGRATE <子句>` |
| `CLEAR SPACE s` | `[parse] Unexpected token: Clear`（lexer 有 `CLEAR` token，parser 无规则） |
| `QUERY MATCH (n:t) RETURN n` | `[parse] Unexpected token: Query` |
| `SET p.age = 1` | `[planning] standalone SET cannot resolve a tag qualifier; use UPDATE ... ON <tag> SET instead` |
| `REMOVE p.age` | `[planning] REMOVE is not wired to storage yet` |
| `EXPORT DATABASE 'x'` / `IMPORT DATABASE 'x'` | 执行成功 |
| `CHECKPOINT` | 到达执行阶段（本次因只读事务作用域被拒） |
| `SHOW VECTOR INDEX` / `DESCRIBE VECTOR INDEX` | 到达执行阶段，报"需要 vector 特性" |
| `SHOW HOSTS` / `SHOW PARTS` | `[parse] Unknown SHOW target`（语法不支持） |
| `PURGE EXPIRED IN s` | 到达执行阶段（本次因空间不存在被拒） |

---

## 3. 测试体系现状

### 3.1 规模

| 位置 | `#[test]` 数量 | 说明 |
|---|---|---|
| `crates/linkrs-query/tests/` | 1552 | 集成测试（真实存储 + 完整 pipeline） |
| `crates/linkrs-query/src/**` 的 `#[cfg(test)]` | 2271 | 单元测试（339 个测试模块） |
| 根 crate `tests/` | 647 | API / 端到端 / 事务 / 同步 |
| 其中 `tests/e2e/` | 113 | 真正的端到端套件 |

规模本身不是问题。**问题是覆盖的分布**：大量测试集中在"语句能跑通"，而非"语句的结果对不对"。

### 3.2 约定

- 集成测试统一用 `TestScenario`（`crates/linkrs-query/tests/common/test_scenario.rs`）作为 harness，流式断言 API（`assert_success` / `assert_result_count` / `assert_vertex_props` / `assert_plan_contains` 等）。
- `crates/linkrs-query/tests/*.rs` 中的 `dql.rs` / `ddl.rs` / `dml.rs` / `dcl.rs` / `optimizer.rs` / `pipeline.rs` 是纯 `#[path]` 聚合器，子目录文件用 `use super::common;` 引入共享 helper。
- 既有设计文档：`docs/tests/integration_test_design.md`（AAA 模式、断言表）、`docs/tests/integration_test_references.md`。

### 3.3 现有"覆盖缺口"设施

- `crates/linkrs-query/tests/integration_coverage_gaps.rs`：14 条手工枚举的测试，**不是**语句级枚举器，也**不会**在运行时报告缺口。
- `crates/linkrs-query/tests/corpus_parity.rs`：约 110 条 SQL 的语料，但断言的是 **tree-sitter 与手写 parser 的语法树一致性**（`has_error == false`、token span 合法）。**它是纯语法测试，不能算作语义覆盖**。该文件中出现的 `CLEAR SPACE` / `SHOW FULLTEXT INDEX` / `DESCRIBE VECTOR INDEX` / `LOAD FROM` / `CALL` / `EXPORT DATABASE` 等语句，正是"看起来有覆盖、实则没有"的典型。
- `crates/linkrs-query/tests/dcl/role.rs::test_dcl_parser_kind_coverage`：全仓**唯一**的语句 kind 枚举测试，但只覆盖 9 个 DCL 变体。
- `crates/linkrs-query/tests/common/query_helpers.rs` 与 `data_fixtures.rs`：**零引用**的死代码。
- `crates/linkrs-query/tests/join_partition_probe.rs`：**零断言**的诊断探针。

---

## 4. 逐语句语义覆盖矩阵

图例：**解析** ✓=有 parser 测试；**计划** ✓=能产出计划；**断言** 强/中/弱/仅拒/无；**死端**=能解析但无 planner。

### 4.1 查询/管道类（Query）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `QUERY` | ✗ | ✗ | 无 | **不可达 AST 变体**：`StmtFactory::query`（`src/parser/ast/utils.rs`）从未被调用；无 parser 生产者，无 binder/planner 分支 |
| `PIPE` | ✓ | ✓ | 中 | `tests/integration_management_extended.rs`（`GO ... \| YIELD \| WHERE \| RETURN`）只断言行数 |
| `SET OPERATION` | ✓ | ✓ | 强 | `tests/dql/set_operations.rs`：UNION/UNION ALL/INTERSECT/MINUS 的结果基数 |
| `ASSIGNMENT` | ✓ | ✓ | 弱 | `tests/integration_management_extended.rs` 只断言成功，未验证 `$result` 后续可读 |

### 4.2 数据查询类（DQL）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `MATCH` | ✓ | ✓ | 强 | `tests/dql/match_query.rs`、OPTIONAL MATCH、变长模式、ORDER BY / LIMIT |
| `GO` | ✓ | ✓ | 中 | `tests/dql/go.rs`：方向、多步，只断言行数 |
| `FETCH` | ✓ | ✓ | 强 | `tests/dql/fetch.rs`：断言返回属性值 |
| `GROUP BY` | ✓ | ✓ | 强 | `tests/dql/aggregation.rs`：ROLLUP / CUBE / GROUPING SETS + 聚合值 |
| `LOOKUP` | ✓ | ✓ | 强 | `tests/dql/lookup.rs`：索引路径 + 回退路径 + EXPLAIN 计划 |
| `SUBGRAPH` | ✓ | ✓ | **弱** | `tests/dql/subgraph.rs` 的 5 个执行用例**全部只有 `assert_success()`**，不校验返回的顶点/边集合 |
| `FIND PATH` | ✓ | ✓ | 中 | `tests/dql/find_path.rs`：最短路径、环/游走策略，只断言行数 |
| `UNWIND` | ✓ | ✓ | 强 | `tests/dql/subquery.rs` |
| `RETURN` | ✓ | ✓ | 强 | `tests/integration_management_extended.rs`、`tests/dql/aggregation.rs` |
| `WITH` | ✓ | ✓ | 强 | `tests/dql/subquery.rs` |
| `YIELD` | ✓ | ✓ | 强 | `tests/dql/execution_contracts.rs` |
| `FILTER`（管道 `\| WHERE`） | ✓ | ✓ | 中 | 仅 `tests/integration_management_extended.rs` 一处 |
| `COLLECT`（管道 `\| COLLECT`） | ✓ | ✓ | **弱** | 全仓仅一处用例（`tests/integration_management_extended.rs`），只断言成功，**聚合出的列表内容从未被检查** |
| `LET`（`AssignVariable`） | ✓ | ✓ | 强 | 根 crate `tests/integration_session_variables.rs`：变量值 + 在 INSERT 中解析 |
| `LOAD FROM` | ✓ | **死端** | 无 | 仅有 parser 测试；**planner 无分支**，但 executor 已有完整实现（CSV / GLOB / 表函数 / 查询源） |
| `CALL` | ✓ | **死端** | 无 | 仅有 parser 测试；`InQueryCallNode` 存在但**从无 planner 构造** |

### 4.3 数据操作类（DML）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `INSERT`（顶点） | ✓ | ✓ | 强 | `tests/dml/insert_vertex.rs`：类型、日期、数值、地理、多标签、arity 错误 |
| `INSERT`（边） | ✓ | ✓ | **弱** | `tests/dml/insert_edge.rs` 23 个测试仅 2 处值断言；rank、时间属性、自环、重复边**只断言存在，不校验属性值与方向语义** |
| `MERGE` | ✓ | ✓ | 强 | `tests/dml/upsert.rs`：ON MATCH / ON CREATE 分支语义 |
| `UPDATE` / `UPSERT` | ✓ | ✓ | 强 | `tests/dml/update.rs`、`tests/dml/upsert.rs` |
| `DELETE` | ✓ | ✓ | 强 | `tests/dml/delete.rs`：顶点/边删除、rank、级联、悬挂边 |
| `SET`（独立语句） | ✓ | ✓ | **仅拒** | `tests/dml/standalone_guards.rs` 7 个用例全部断言"被拒绝"；**没有任何成功路径** |
| `REMOVE`（独立语句） | ✓ | ✓ | **仅拒** | 同上；planner 报"REMOVE is not wired to storage yet" |
| `COPY` | ✓ | ✓ | 强 | `tests/copy_e2e.rs`：CSV→COPY→CSV 往返、顶点/边计数与属性 |

### 4.4 模式定义类（DDL）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `CREATE`（TAG/EDGE/SPACE/Cypher 节点） | ✓ | ✓ | 强 | `tests/ddl/tag_basic.rs`、`edge_basic.rs`、`constraints.rs`、`type_system.rs` |
| `DROP` | ✓ | ✓ | 强 | `tests/ddl/tag_basic.rs` 等 |
| `ALTER` | ✓ | ✓ | 强 | `tests/ddl/tag_alter.rs`、`edge_alter.rs`、`schema_evolution.rs` |
| `DESC` | ✓ | ✓ | 中 | 只断言行数，不校验列名与元数据内容 |
| `SHOW CREATE` | ✓ | ✓ | 强 | `tests/ddl/show_create.rs`：校验 DDL 文本内容 + **回灌重解析** |
| `CLEAR SPACE` | **✗** | ✓ | 无 | **不可达 AST 变体**：lexer 有 `CLEAR` token，parser 无规则；binder/planner/executor 链路齐全却无人能触达 |
| `CREATE FULLTEXT INDEX` | ✓ | ✓ | **弱** | `tests/e2e/extended_types.rs` 只断言成功，且被 `fulltext` 特性门控（默认构建不运行） |
| `DROP FULLTEXT INDEX` | ✓ | **死端** | 无 | 无 planner 分支；`DropFulltextIndexNode` **从无任何 planner 构造** |
| `ALTER FULLTEXT INDEX` | ✓ | **死端** | 无 | 同上 |
| `SHOW FULLTEXT INDEX` | ✓ | **死端** | 无 | 同上 |
| `DESCRIBE FULLTEXT INDEX` | ✓ | **死端** | 无 | 同上 |
| `CREATE VECTOR INDEX` | ✓ | ✓ | 强 | `tests/vector_query_e2e.rs`：索引生效（检索命中正确 id） |
| `DROP VECTOR INDEX` | ✓ | ✓ | 强 | `tests/vector_query_e2e.rs`：状态行 + 删除后检索报 "Index not found" |
| `SHOW VECTOR INDEX` | ✓ | ✓ | **无** | planner 存在（AST 回退路径），但**零执行测试**；仅出现在 `corpus_parity.rs` |
| `DESCRIBE VECTOR INDEX` | ✓ | ✓ | **无** | 同上 |
| `CREATE MACRO` / `DROP MACRO` | ✓ | ✓ | 强 | `tests/ddl/macro.rs`：默认值、重复、参数错误、递归拒绝、嵌套展开 |
| `CREATE TYPE` / `DROP TYPE` | ✓ | ✓ | 强 | `tests/ddl/type_alias.rs`：别名链、环检测、cast、INSERT…SELECT |
| `ATTACH DATABASE` / `DETACH DATABASE` | ✓ | ✓ | 强 | `tests/ddl/attach_detach.rs`：目录行内容 |
| `PURGE EXPIRED` | ✓ | ✓ | 强 | `tests/ddl/tag_basic.rs`：断言 `removed_rows` 的具体数值 |

### 4.5 权限控制类（DCL）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `CREATE USER` / `ALTER USER` / `DROP USER` / `CHANGE PASSWORD` | ✓ | ✓ | 中 | `tests/dcl/user_management.rs`：只断言成功/失败，不校验目录状态 |
| `GRANT` / `REVOKE` | ✓ | ✓ | **弱** | `tests/dcl/permission.rs`：`test_grant_revoke_execution` 授完权后**不验证权限是否真的生效**（授权后被授权身份能否执行受限语句） |
| `DESCRIBE USER` | ✓ | ✓ | 强 | `tests/dcl/role.rs`：断言精确列名 + 单元格值 |
| `SHOW USERS` / `SHOW ROLES` | ✓ | ✓ | 强 | `tests/dcl/role.rs`：断言精确列名 + 值 + 密码不泄漏 |

DCL 目录下的 `tests/dcl/` 有 186 个测试，但**值断言几乎全部集中在 `role.rs` 一个文件**（`permission.rs` 15 个测试 0 值断言，`user_management.rs` 43 个测试 0 值断言，`edge_cases.rs` 32 个测试 0 值断言）。

### 4.6 管理/会话类（Admin）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `USE` | ✓ | ✓ | 强 | `tests/integration_management_extended.rs`：切换空间后 FETCH 命中 |
| `SHOW`（SPACES/TAGS/EDGES/INDEXES/STATS/MACROS/ATTACHED DATABASES/EXTENSIONS） | ✓ | ✓ | 中 | `tests/ddl/show_catalog.rs` 等：多为计数断言 |
| `EXPLAIN` | ✓ | ✓ | 强 | `tests/dql/lookup.rs`、`tests/dql/constant_folding.rs`：断言计划中算子名 |
| `PROFILE` | ✓ | ✓ | 强 | `tests/dql/optimizer.rs` |
| `ANALYZE` | ✓ | ✓ | 强 | `tests/integration_analyze.rs`：统计值、幂等、DDL 失效 |
| `SHOW SESSIONS` / `SHOW QUERIES` | ✓ | 服务端 | 强（服务端）/ 仅拒（内嵌） | 内嵌 pipeline 明确报"属 API 层职责" |
| `KILL QUERY` | ✓ | 仅服务端 | 强（服务端）/ 仅拒（内嵌） | planner 明确拒绝并指向服务端 |
| `SHOW CONFIGS` | ✓ | ✓ | 弱 | `tests/integration_management.rs`：只断言 `ExecutionResult::ShowConfigs` 的意图字段 |
| `UPDATE CONFIGS` | ✓ | ✓ | 中 | `tests/integration_management.rs`：断言 `ConfigUpdate` 结构，非实际生效值 |
| `CHECKPOINT` | ✓ | ✓ | **无** | 能跑到执行阶段，但**零测试**（仅 `corpus_parity.rs` 语法条目） |
| `EXPORT DATABASE` / `IMPORT DATABASE` | ✓ | ✓ | **无** | 实测执行成功，但**零语义断言**（仅 `corpus_parity.rs` 语法条目） |
| `LOAD / INSTALL / UNINSTALL / UPDATE EXTENSION` | ✓ | **死端** | 无 | 无 planner 分支 |

### 4.7 检索类（Search）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `SEARCH`（全文） | ✓ | **死端** | 无 | 实测报 "No planner for bound statement: SEARCH"。但 `tests/e2e/extended_types.rs` 用 `assert_query_ok` 断言它**应当成功**——该测试被 `fulltext` 特性门控，默认构建不运行 |
| `LOOKUP FULLTEXT` | ✓ | **死端** | 无 | 同上 |
| `MATCH FULLTEXT` | ✓ | **死端** | 无 | 同上 |
| `SEARCH VECTOR` | ✓ | ✓ | 强 | `tests/vector_query_e2e.rs`：命中 id、相似度阈值、过滤、offset |
| `LOOKUP VECTOR` | ✓ | ✓ | 强 | `tests/vector_query_e2e.rs` |
| `MATCH VECTOR` | ✓ | ✓ | **无** | 该文件头注释声称覆盖 MATCH VECTOR，但**全文没有一条 `MATCH VECTOR` SQL**——注释与事实不符 |

### 4.8 事务类（Transaction）

| 语句 | 解析 | 计划 | 断言 | 证据 / 问题 |
|---|---|---|---|---|
| `BEGIN TRANSACTION` | ✓ | ✓ | 强（API/服务端）/ 仅拒（内嵌） | 根 crate `tests/integration_session_variables.rs` |
| `COMMIT TRANSACTION` | ✓ | ✓ | 强 | 同上，提交后变量可见 |
| `ROLLBACK TRANSACTION` | ✓ | ✓ | 强 | 同上，回滚后变量值恢复 |
| `SAVEPOINT` | ✓ | ✓ | 强 | 同上 |
| `RELEASE SAVEPOINT` | ✓ | ✓ | **弱** | 只断言成功标志，不断言保存点状态变化 |

### 4.9 汇总

下表把 84 个 `Stmt` 变体逐个归类，合计等于 84，无重复无遗漏。

| 分类 | 变体数 | 变体 |
|---|---|---|
| **强**——断言了值 / 列名 / 属性 / 计划形状 / 状态副作用 | 44 | `Create` `Match` `Delete` `Update` `Fetch` `Use` `Explain` `Profile` `Analyze` `GroupBy` `Lookup` `Insert` `Merge` `Unwind` `Return` `With` `Yield` `Drop` `Alter` `DescribeUser` `ShowUsers` `ShowRoles` `ShowCreate` `ShowSessions` `ShowQueries` `KillQuery` `SetOperation` `CreateVectorIndex` `DropVectorIndex` `SearchVector` `LookupVector` `BeginTransaction` `CommitTransaction` `RollbackTransaction` `Savepoint` `AssignVariable` `Copy` `CreateMacro` `DropMacro` `CreateType` `DropType` `AttachDatabase` `DetachDatabase` `PurgeExpired` |
| **中**——只断言行数 / 集合基数 | 5 | `Go` `FindPath` `Filter` `Pipe` `UpdateConfigs` |
| **弱**——只断言"成功" | 14 | `Show` `Subgraph` `Collect` `Desc` `CreateUser` `AlterUser` `DropUser` `ChangePassword` `Grant` `Revoke` `ShowConfigs` `Assignment` `CreateFulltextIndex` `ReleaseSavepoint` |
| **仅拒绝**——只有被拒绝的断言，无成功路径 | 2 | `Set` `Remove` |
| **可执行但零测试** | 6 | `MatchVector` `ShowVectorIndex` `DescribeVectorIndex` `Checkpoint` `ExportDatabase` `ImportDatabase` |
| **死端**——可解析但无 planner | 11 | `Migrate` `DropFulltextIndex` `AlterFulltextIndex` `ShowFulltextIndex` `DescribeFulltextIndex` `Search` `LookupFulltext` `MatchFulltext` `LoadFrom` `InQueryCall` `Extension` |
| **不可达 AST 变体**——无 parser 生产者 | 2 | `Query` `ClearSpace` |

补充说明：
- 死端的 11 个变体展开为 **16 种语句写法**（`Extension` 有 4 个动作、`Migrate` 有 3 个子句形态）。
- "强"这一档里，`Insert` 的顶点路径是强的、边路径偏弱；`KillQuery` / `ShowSessions` / `ShowQueries` 的强断言在**服务端**测试中，嵌入式 pipeline 里是"仅拒绝"。
- 全文检索相关的一切（`CreateFulltextIndex` 及其后 6 个变体）在默认构建下全部落入"弱"或"死端"，因为 `fulltext` 不在默认特性里。

---

## 5. 关键发现

### 发现一：11 个语句变体（16 种写法）"能解析但跑不起来"，且错误信息泄漏内部实现

这类语句的共同形态是：parser 接受 → binder 落入兜底分支 → planner 无对应分支 → `src/pipeline/compiler.rs` 抛出 `No planner for bound statement: <KIND>`。

这是**最差的一种状态**：
- 用户看到的是内部路由失败，而不是"该语句尚未支持"或"该语句应在服务端执行"。
- 语料测试（`corpus_parity.rs`）把它们列为"已覆盖"，掩盖了缺口。
- `FulltextSearchPlanner` 中，`SEARCH` / `LOOKUP FULLTEXT` 的实现（`transform_lookup_fulltext_with_metadata` 等）全部带 `#[cfg(test)]` 标记，即**只在单元测试里存在，产物里是死代码**；`DROP` / `ALTER` / `SHOW` / `DESCRIBE FULLTEXT INDEX` 则**连实现都没有**。
- `DropFulltextIndexNode` / `AlterFulltextIndexNode` / `ShowFulltextIndexNode` / `DescribeFulltextIndexNode` / `LoadFromNode` / `InQueryCallNode` 六个 plan node **从无任何 planner 构造**（已逐一检索确认）。

对比之下，`KILL QUERY` 是**有意的显式拒绝**——planner 明确写着"在网络服务中针对活跃查询跟踪执行，此处不可计划"，把用户引向正确的端点。`EXTENSION` 与 `MIGRATE` **不属于这一类**：
- `EXTENSION`：API 层只把它分类为 `SessionCommand::Extension`（`crates/linkrs-api/src/api_core/session_command.rs`），**没有任何消费方**，最终仍然落到通用的"无 planner"错误。
- `MIGRATE`：真正的能力在 gRPC / HTTP 接口上（`crates/linkrs-server/src/grpc/migration.rs`），SQL 写法从未被接上，但也没有任何提示把用户引向 gRPC。

因此这一组里只有 `KILL QUERY` 的错误信息是可接受的，其余都应按 T2 处理。

### 发现二：两个 AST 变体根本不可达

`QUERY` 与 `CLEAR SPACE` 在 `Stmt` 枚举中存在、有 `kind()`、有 `StmtCategory` 映射、有（`CLEAR SPACE`）完整的 binder/planner 链路，但**没有任何 parser 规则能产出它们**。这类变体是"代码存在 ⇒ 看起来受支持"的假信号。

### 发现三：断言强度普遍停留在"跑通了"层面

按"值断言 / 测试数"统计的抽样：

| 文件 | 测试数 | 值断言数 | 评价 |
|---|---|---|---|
| `tests/dql/subgraph.rs` | 15 | 0 | 10 个 parser 用例 + **5 个执行用例全部只有 `assert_success()`** |
| `tests/ddl/index_basic.rs` | 4 | 0 | 索引 DDL 无内容校验 |
| `tests/dml/memo_skip_bind.rs` | 4 | 0 | — |
| `tests/dml/standalone_guards.rs` | 7 | 0 | 仅拒绝语义 |
| `tests/dcl/permission.rs` | 15 | 0 | 授权不验证生效 |
| `tests/dcl/user_management.rs` | 43 | 0 | 只断言成功/失败 |
| `tests/dcl/edge_cases.rs` | 32 | 0 | 同上 |
| `tests/dql/find_path.rs` | 38 | 9 | 只断言路径条数，不校验路径内容 |
| `tests/dml/insert_edge.rs` | 23 | 2 | 边属性不校验 |

典型后果：`GET SUBGRAPH FROM 1` 返回 0 个顶点、5 个顶点、或者返回别人的顶点，测试全绿——因为它只断言"没报错"。

### 发现四：特性门控让默认构建的覆盖面比看上去小

- 根 crate 默认特性含 `vector`，但**不含** `fulltext`。
- `tests/e2e/extended_types.rs` 中 4 个全文检索用例被 `#[cfg_attr(not(feature = "fulltext"), ignore)]` 跳过；3 个向量用例被 `vector-qdrant` 跳过。
- 也就是说：`SEARCH`（全文）语句的**唯一**"应当成功"的断言，从未在默认 CI 构建中运行过。这直接掩盖了发现一的死端问题——该用例一旦在开启 `fulltext` 的构建中运行，就会失败。

### 发现五：语料测试是纯语法测试，却承担了"覆盖清单"的隐式职责

`corpus_parity.rs` 约 110 条语句中，有近 20 条对应的是"死端"或"不可达"语句。它断言的是 tree-sitter 语法树无错误，与语义无关。若把它当作覆盖清单，会系统性高估覆盖率。

### 发现六：差分/等价性测试存在已知盲区

- `tests/optimizer/result_equivalence.rs` 明确**排除 ORDER BY 查询**，注释指出存在"optimizer Sort 节点破坏"的已知缺陷。一个已知会破坏结果顺序的缺陷被测试绕开，等于没有回归防护。
- `tests/join_partition_probe.rs` **零断言**，只打印计划。
- `tests/dql/optimizer.rs` 的等价性用例只比较 `count()`。

### 发现七：没有"语句可达性"的契约测试

全仓唯一的语句 kind 枚举测试 `test_dcl_parser_kind_coverage` 只覆盖 9 个 DCL 变体。这意味着：
- 新增一个 `Stmt` 变体并接上 parser 后，**不会有任何测试失败**，即使它没有 planner、没接 executor。
- 现状下 11 个死端变体正是这样静默存在的。

---

## 6. 需要补充的测试

按优先级分三批。每条给出**落点文件**与**断言要点**（不含代码，描述语义要求）。

### 6.1 第一批：把"假覆盖"变成"显式契约"（最高优先级）

**T1. 语句可达性契约测试**

- 新增 `crates/linkrs-query/tests/statement_reachability.rs`。
- 建立一张**语句清单常量**：每个 `Stmt` 变体对应一条代表性 SQL、一个期望状态。
- 期望状态分三类，逐一断言：
  - `Executable`：语句必须成功，且结果需通过至少一条内容级断言（不能只断言成功）。
  - `Rejected`：语句必须失败，且错误信息必须落在**用户可理解**的类别内——明确的"不支持"、明确的"应在服务端执行"、明确的参数错误。**`No planner for bound statement` 属于内部错误泄漏，不算合格**。
  - `Unreachable`：语法层必须直接拒绝（parse error），不得进入 binder。
- 这张清单是本次分析产出的核心资产：它把 84 个变体的当前状态固化下来，任何新增语句漏接 planner / executor 都会立刻红灯。

**T2. 死端语句的处置与锁定**

对发现一列出的 11 个语句变体（16 种写法），逐个二选一：
- **接入**：补 planner 分支 + executor 语义，并纳入 T1 清单的 `Executable` 类。
- **显式拒绝**：在 parser 层（或 binder 层）给出明确的支持性错误，移除 `#[cfg(test)]` 专属的不可达实现与从未被构造的 plan node，并在 T1 清单中标为 `Rejected`。
- 无论如何，这 16 条都必须在 T1 清单中出现，且必须断言到**具体错误文案**，不能只断言"失败了"。

**T3. 不可达 AST 变体的处置**

- `QUERY`：确认设计意图。若不保留，从 `Stmt` 枚举、`kind()`、`StmtCategory` 映射中移除；若保留，补 parser 生产者与测试。
- `CLEAR SPACE`：同上。移除时需一并清理 `bind_clear_space`、`plan_clear_space`、`ClearSpaceNode` 与 executor 分支。
- 在 T1 清单中不出现这两个变体（它们要么有 SQL 入口，要么不存在）。

**T4. 语义断言补强——已有覆盖但太弱的部分**

| 目标 | 落点 | 断言要点 |
|---|---|---|
| `GET SUBGRAPH` | `tests/dql/subgraph.rs` | 返回的顶点 id 集合、边集合、步数上限截断行为、`YIELD vertices, edges` 的列名与内容 |
| 管道 `COLLECT` | `tests/integration_management_extended.rs` | 收集列表的**元素内容与顺序**，空输入时是空列表而非空结果集 |
| `FIND PATH` | `tests/dql/find_path.rs` | 路径的节点序列、边类型、权重、`YIELD path` 的结构 |
| `INSERT EDGE` | `tests/dml/insert_edge.rs` | rank、时间属性的具体值、方向语义、自环、重复边的属性覆盖行为 |
| `GO` | `tests/dql/go.rs` | 目标 id 集合（而非仅行数）、`REVERSELY` / `BIDIRECT` 的差异 |
| `DESC` / `SHOW` | `tests/ddl/tag_basic.rs`、`show_catalog.rs` | 精确列名 + 关键元数据值（标签属性列表、索引定义） |
| `UPDATE CONFIGS` | `tests/integration_management.rs` | 配置项**实际生效值**，而非仅 `ConfigUpdate` 结构 |
| `RELEASE SAVEPOINT` | 根 crate `tests/integration_session_variables.rs` | 释放后再回滚到该保存点必须失败（保存点已消失） |

**T5. DCL 授权生效的语义断言**

- 落点：`crates/linkrs-query/tests/dcl/permission.rs`（复用 `TestScenario::exec_dcl_as`）。
- 断言链路：`CREATE USER` → `GRANT` → **以该身份执行受限语句应成功** → `REVOKE` → **同一语句应失败**。当前测试只到"GRANT 不报错"就结束，完全没有验证权限的实际效果。
- `tests/dcl/cascade_operations.rs` / `transaction_consistency.rs` / `concurrent_operations.rs` / `cross_operation_consistency.rs` 同样只有成功/失败断言，应补目录状态与生效结果的校验。

**T6. `SET` / `REMOVE` 的语义决策**

- 当前：永久拒绝，且错误文案是面向开发者的（"not wired to storage yet"）。
- 需要先决策：实现，还是明确不支持。
- 若明确不支持：错误文案应面向用户（说明不支持的原因与替代写法），并在 parser 层拒绝，避免占用 planner/executor 代码路径。
- 若实现：需要完整语义测试（属性存在/不存在、类型转换、批量语义、与 `UPDATE ... ON ... SET` 的等价性）。
- 无论哪种决策，都要在 T1 清单中锁定。

### 6.2 第二批：补齐"可执行但零测试"与被门控的覆盖

**T7. `CHECKPOINT` / `EXPORT DATABASE` / `IMPORT DATABASE`**

- 落点：新建 `crates/linkrs-query/tests/admin/maintenance.rs`，并新建 `crates/linkrs-query/tests/admin.rs` 聚合器（与 `dql.rs` / `ddl.rs` / `dml.rs` / `dcl.rs` 同样的 `#[path]` 聚合模式）。这三个语句在 `StmtCategory` 里属于 Admin，放进 `ddl/` 会与分类不符。
- 断言要点：
  - `CHECKPOINT`：执行成功；执行后数据仍可读；重复执行幂等。
  - `EXPORT DATABASE`：产出的文件/目录确实存在且可被 `IMPORT DATABASE` 回灌；回灌后顶点/边计数与属性与导出一致（**往返等价**）。
  - `IMPORT DATABASE`：导入到已存在数据的空间时的行为（合并 / 拒绝）需明确定义并断言；导入失败时不得留下半成品状态。

**T8. 向量索引目录语句**

- 落点：`tests/vector_query_e2e.rs`（已有 `vector` 特性门控）。
- 断言要点：`SHOW VECTOR INDEX` 返回的列名与索引条目（名称、所属 schema、字段、维度）；`DESCRIBE VECTOR INDEX` 返回的字段与距离度量；不存在的索引名必须报"索引不存在"而非空结果。
- 顺带修正 `tests/vector_query_e2e.rs` 头部注释——它声称覆盖 MATCH VECTOR，实际没有。

**T9. `MATCH VECTOR` 语义**

- 落点：同上。
- 断言要点：命中 id 与相似度阈值、过滤条件、`THRESHOLD` 剪枝行为、与 `SEARCH VECTOR` 的一致性。

**T10. 全文检索语句的特征无关测试**

- 当前全文检索的 `SEARCH` / `LOOKUP FULLTEXT` 实现带 `#[cfg(test)]` 标记、`DROP` / `ALTER` / `SHOW` / `DESCRIBE FULLTEXT INDEX` 连实现都没有，导致这 7 个变体在默认构建下一律是死端，且唯一的"应当成功"断言被特性门控跳过。
- 落点：新建 `crates/linkrs-query/tests/search/fulltext_contract.rs`。
- 内容分两层：
  - **特性无关层**：不依赖 `fulltext` 特性即可断言——语句被明确拒绝时，错误文案必须是"该特性未启用 / 该语句尚未支持"这类用户可理解的信息，绝不能是 `No planner for bound statement`。
  - **特性开启层**：`#[cfg(feature = "fulltext")]` 下断言索引创建 → 写入 → 检索命中 → 索引管理语句（SHOW/DESCRIBE/DROP/ALTER）的目录内容与错误路径。
- 同步处理 `tests/e2e/extended_types.rs` 中被门控的 4 个用例：至少保留一条**特性无关**的契约断言，让默认构建也能守住"错误信息不泄漏内部实现"。

**T11. 语句 kind 枚举测试扩展到全类别**

- 落点：新建 `crates/linkrs-query/tests/statement_inventory.rs`（与 T1 共用清单常量）。
- 断言：清单里每条 SQL 解析出的 `stmt.kind()` 与预期字符串一致，且 `StmtCategory` 映射符合预期。
- 这解决"新增变体忘记加测试"的问题：清单是显式的，新变体不加入清单就会让清单与枚举的数量一致性断言失败。

### 6.3 第三批：完善语义深度与差分测试

**T12. 边界与空集语义**

- 覆盖：空输入的聚合（`COUNT` 返回 0 而非 1）、NULL 的三值逻辑传播、类型提升与溢出、除零、空字符串与空列表、深度嵌套上限、超大 `UNWIND` 列表的内存边界。
- 落点：新建 `crates/linkrs-query/tests/dql/edge_semantics.rs`。

**T13. 补齐 ORDER BY 的优化器等价性**

- `tests/optimizer/result_equivalence.rs` 当前显式排除 ORDER BY（已知 Sort 节点破坏结果顺序的缺陷）。
- 处置：先作为**已知缺陷的显式回归标记**（断言当前行为或断言缺陷存在），使缺陷可见；随后修复缺陷，把 ORDER BY 纳入常规等价性比较。
- 同时把 `tests/dql/optimizer.rs` 的等价性用例从只比 `count()` 升级为比全量行。

**T14. `join_partition_probe.rs` 补断言**

- 当前零断言。至少断言串行与分区执行在相同数据下产出一致的结果集，使它从打印探针升级为回归测试。

**T15. 语义语料从语料测试中分离**

- 把 `corpus_parity.rs` 的语句清单拆成两份：
  - **语法语料**（继续做 tree-sitter 一致性）：保持现状，但明确其职责边界。
  - **语义语料**（新增）：每条语句带一组"输入数据 + 期望结果"，由 T1/T4 的用例驱动。
- 落地形式：新建 `crates/linkrs-query/tests/corpus/semantic/` 目录，每条语句一个 `.gql` 片段 + 期望断言，由统一 runner 驱动。这样语料既能覆盖新语句，又不至于把语法覆盖误读成语义覆盖。

**T16. 跨语句组合语义**

- 单一语句的语义基本覆盖到位后，缺口在组合：
  - 同一 session 内 `USE` → DDL → DML → DQL 的状态传递。
  - 事务内 DDL + DML 的原子性（回滚后 DDL 是否也回滚）。
  - 管道 / 子查询 / 集合运算的嵌套组合。
  - `COPY` 与 `INSERT` 在同一事务中的可见性。
- 落点：新建 `crates/linkrs-query/tests/session_semantics.rs`。

**T17. 错误信息契约测试**

- 为每个语句类别锁定一条代表性错误，断言错误文案的关键片段稳定。
- 目的：既防止错误信息回退成内部实现细节（发现一），也防止文案漂移导致的测试脆弱。
- 落点：新建 `crates/linkrs-query/tests/error_contract.rs`。

### 6.4 批次与依赖

| 批次 | 前置 | 说明 |
|---|---|---|
| 第一批 T1–T6 | 无 | 先建立清单与契约，把当前真实状态固化；T2/T3 涉及生产代码改动 |
| 第二批 T7–T11 | 第一批 T1 | 复用同一份清单常量 |
| 第三批 T12–T17 | 第一、二批 | 依赖稳定的错误文案与已确定的语句支持状态 |

T2 与 T3 涉及删除生产代码中的死实现（无 planner 分支的语句、不可达的 plan node、不可达的 AST 变体），需与"实现"路线二选一后一并推进——**不要为了保留代码而保留不可达分支**。

---

## 7. 测试基础设施改进建议

| 项 | 现状 | 建议 |
|---|---|---|
| 语句清单 | 分散在各测试文件里 | 抽成单一常量表（SQL 文本 + kind + category + 期望状态），供 T1/T11/语料 runner 共用 |
| 断言粒度 | 以 `assert_result_count` 为主 | 增加"整行集合比较"与"列名 + 单元格值"的批量断言，减少只能写计数断言的处境 |
| 死代码 helper | `tests/common/query_helpers.rs`、`data_fixtures.rs` 零引用 | 删除，或让新用例改用它们（避免两套 harness） |
| 探针式测试 | `join_partition_probe.rs` 零断言 | 补断言后并入常规回归；确属诊断用途的应加 `#[ignore]` 并说明用途 |
| 特性门控 | 7 个用例被 `cfg_attr(ignore)` 静默跳过 | 每个门控用例都必须有一条**特性无关**的伴生断言（至少覆盖错误契约） |
| 语料文件位置 | 内联在 `corpus_parity.rs` 的 Rust 源里 | 抽到 `tests/corpus/` 目录，便于新增与分派语法/语义两种 runner |
| 注释与事实一致性 | `tests/vector_query_e2e.rs` 声称覆盖 MATCH VECTOR 但实际没有 | 随 T9 一并修正；新增测试时要求注释描述实际断言内容 |

---

## 8. 验收标准

1. **清单完整**：`Stmt` 枚举的每个变体在 T1 清单中都有条目（不可达变体按 T3 的决策结果从枚举移除）。清单条目数与变体数的一致性由自动断言守护。
2. **无内部错误泄漏**：全仓测试中不再出现 `No planner for bound statement` 作为面向用户的错误结果。
3. **无静默死端**：不存在"有 parser、有 AST、有 plan node，但无 planner 构造点"的语句。
4. **断言强度**：所有列入 `Executable` 的语句都至少有一条内容级断言（返回行的值 / 列名 / 状态副作用），不允许只断言成功。
5. **零测试清零**：`CHECKPOINT`、`EXPORT DATABASE`、`IMPORT DATABASE`、`SHOW VECTOR INDEX`、`DESCRIBE VECTOR INDEX`、`MATCH VECTOR` 六个语句的语义测试补齐。
6. **默认构建即有效**：不开启 `fulltext` / `vector-qdrant` 特性时，默认构建中运行的测试集足以守住全部语句的错误契约。
7. **差分测试无已知盲区**：ORDER BY 纳入优化器等价性比较（缺陷修复或显式标记）。

---

## 附录 A：关键证据索引

| 主题 | 位置 |
|---|---|
| 语句枚举与分类 | `crates/linkrs-query/src/parser/ast/stmt.rs` |
| 绑定路由（兜底分支） | `crates/linkrs-query/src/binder/bind.rs` |
| 计划路由（`None` 分支） | `crates/linkrs-query/src/planning/planner.rs` |
| 无 planner 的错误出口 | `crates/linkrs-query/src/pipeline/compiler.rs` |
| 全文检索 planner（实现仅 `#[cfg(test)]`） | `crates/linkrs-query/src/planning/fulltext_planner.rs` |
| 向量 DDL planner（AST 回退路径） | `crates/linkrs-query/src/planning/vector_ddl_planner.rs` |
| 集成测试 harness | `crates/linkrs-query/tests/common/test_scenario.rs` |
| 语料测试（纯语法） | `crates/linkrs-query/tests/corpus_parity.rs` |
| 手工覆盖缺口清单 | `crates/linkrs-query/tests/integration_coverage_gaps.rs` |
| 唯一的语句 kind 枚举测试 | `crates/linkrs-query/tests/dcl/role.rs` |
| 优化器等价性（排除 ORDER BY） | `crates/linkrs-query/tests/optimizer/result_equivalence.rs` |
| 零断言探针 | `crates/linkrs-query/tests/join_partition_probe.rs` |
| 特性门控用例 | `tests/e2e/extended_types.rs` |
| 特性无关的测试设计约定 | `docs/tests/integration_test_design.md`、`docs/tests/integration_test_references.md` |
