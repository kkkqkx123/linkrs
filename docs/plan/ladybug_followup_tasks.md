# Ladybug 后续任务清单

> 编写日期：2026-09-11。覆盖 `ladybug_gap_analysis.md` 中仍未完成的工作、
> 阶段四有意留下的限制，以及本轮测试中发现并已修复的问题备查。
> 术语沿用缺口分析文档（SPACE 即 GRAPH 命名空间别名，Tag 即节点表，Edge 即关系表）。

## 已完成基线（本轮结束状态）

- `cargo test -p graphdb-query --lib`：2042 通过，0 失败
- `cargo test -p graphdb-storage --lib`：844 通过，0 失败
- `cargo test -p graphdb-core --lib`：446 通过，0 失败
- `cargo test -p graphdb-query --test ddl --test dml --test dql`：全过
  （3 个历史 `DESCRIBE EDGE` 行数断言已同步端点约束行，见 T4.4）
- `cargo test -p graphdb --features embedded --test integration_embedded_api`：
  76 通过，0 失败（含文本事务死锁修复，见 T4.5）
- `cargo check --workspace`：通过
- `cargo clippy --workspace --lib`：0 警告（仅第三方 `proc-macro-error2`
  的 future-incompat 提示）

## T1 递归复合端到端执行（缺口 4.2 收尾）

- 现状：递归复合解析完整；绑定器与规划器对递归复合（含附着在边模式上的形式）
  一律返回明确错误，不再静默降级为普通遍历；端到端绑定-过滤-投影执行尚未实现。
- 验收：常用递归子集可执行并返回正确投影；超集保持明确报错。
- 建议切入点：
  - `crates/graphdb-query/src/binder/bind/bind_match.rs`（递归分支放行与绑定）
  - 遍历规划器与 `graph_operator` / `traversal runtime` 执行路径
  - 单测进 `stmt_parser/tests.rs`，端到端进 `tests/integration_embedded_api.rs`
    （沿用本轮测试归位约定，不新建阶段性集成文件）
- 细化设计（2026-09-11）：分两阶段，不写新遍历引擎，复用现有算子。
  - 语义拆解：`-[e* (v,r | WHERE … | {…},{…})]->` 含五个正交维度——
    步进局部变量 `v/r`（仅 WHERE/投影内可见，不泄漏外层）、
    per-step 过滤（遍历中剪枝）、per-step 投影（收集为 list）、
    外层 `e` 与内层 `r` 遮蔽规则、与 `range`/`PathSemantic` 的组合。
  - T1a（有用子集，先做）：filter-only + 投影后置收集。
    Binder 新增 `BoundRecursiveComprehension`，为 `v/r` 建子 scope
    （复用 `bind_subquery_body` 的 `BinderScope::with_parent` 模式）；
    Planner 将 `Recursive(rc)` 改写为 `AllPaths/BFS` 展开 + 串联 `Filter(step_filter)`，
    投影用现有 `Project` 算子在遍历后计算并 `collect_list`
    （正确性一致、剪枝性能次优，文档注明）。
  - T1b（完整语义，按需）：改写为 Recursive-CTE Fixpoint
    （anchor 为 1 跳展开，step 为 `steps JOIN expand + WHERE`），
    复用 `RecursiveCteNode + Fixpoint operator + CteScan` 实现 per-step 剪枝；
    联动放行 `template_extractor.rs`、`from_pattern.rs`、`merge.rs` 三处占位分支。
  - 不采用：给 `RecursiveFragmentSpec` 加闭包谓词（违反最小化 `dyn` 规范，
    且遍历热路径引入求值器借用穿透，改动面最大）。

## T2 附着源跨源查询改写（缺口 2.7 收尾）

- 现状：`ATTACH / DETACH DATABASE` 目录级挂载与 `SHOW ATTACHED DATABASES`
  已实现；挂载表为进程级全局注册（`attached.rs` OnceLock），跨源查询计划改写尚未实现。
  限定名（`mydb.Person`）目前在解析层以 confusing 的 `Expected RParen, found Dot` 失败。
- 验收：跨附着源的 MATCH 查询可用；`DETACH` 后相关查询返回明确错误。
- 建议切入点：`attached.rs` 目录、`session.rs` 会话拦截、绑定器表解析。
- 备注：若将来需要会话级隔离，需将会话拦截从全局目录迁移为会话作用域。
- 决策（2026-09-11）：三种语义中选 **C（目录记录 + 明确错误）**，暂缓 A（真联邦执行）。
  理由：单节点本地库的数据迁移已有 `EXPORT/IMPORT DATABASE` + `LOAD FROM File/Glob/TableFunc`
  覆盖；真联邦需要 Kuzu/parquet 文件格式驱动 + 跨库事务 + 优化器成本模型，
  投入产出比极低。B（同引擎目录挂载）可作为后续可选。
  - 本轮已落地 C 的上限：`attached::is_attached` + `qualified_reference_message`，
    节点标签与边类型解析遇到 `alias.table` 时返回可操作错误
    （已附着：指引 `IMPORT DATABASE` / `LOAD FROM` 物化；
    未附着：提示限定名不支持），绑定器 `resolve_tags` / `resolve_edge_types`
    另有同名兜底守卫。
  - 全局注册迁移为会话作用域属破坏性改动（执行器在 `graphdb-query` 内，
    会话在 `graphdb-api` 内，需将会话级注册表穿透 QueryApi → planner → executor），
    留待真需要多租户隔离时再做；现状已在模块文档中诚实注明。

## T3 物化创建限制放宽（可选）

- 现状：`CREATE TAG / EDGE AS (query)` 经会话层物化实现；
  显式事务内与嵌套 AS 当前返回明确错误（有意限制）。
- 验收（按需）：事务内 AS 要么支持原子物化，要么保持报错但文档化。
- 建议切入点：`crates/graphdb-api/src/embedded/session.rs` 物化执行函数。
- 决策（2026-09-11）：**保持报错 + 文档化，不做事务原子化**。
  理由：当前实现是“先 `execute(inner)` 再 DDL + bulk load”，非原子；
  放开需存储层 savepoint/回滚 + DDL 参与两阶段提交（触及 `UndoTarget` +
  `TransactionManager`），为一个 DDL 便利语法不值。嵌套 AS 无实际用例，
  递归物化语义模糊（内层表名冲突、列推导顺序），禁止是对的。
  替代做法（写入用户文档）：显式事务外执行，或先 `MATCH … RETURN` 再落表。
  遗留小修（可选）：`is_create_as_query` 的 `contains(" AS ")` 前缀嗅探
  会误判字符串字面量含 `AS (` 的语句，改为直接试解析（失败则走正常 pipeline）。

## T4 测试健康备查（本轮已修复）

1. `collect_returns_estimates_for_every_node` 曾失败：
   直接构造的计划节点 id 全为占位 `-1`，按 id 聚合的估计表自然坍缩为 1 条。
   生产计划由 planner 赋唯一 id，因此修复测试使其显式编号
   （`clone_with_new_id`），而非改动聚合逻辑。
   位置：`crates/graphdb-query/src/optimizer/cost_based/row_estimates.rs`。
2. 一批单测引用已不存在的 `deps` 字段导致测试无法编译：
   已按访问器（`dependencies()` / `dependencies_ref()` / `dependencies_mut()`）改写。
3. 集成测试引用已更名的 `BatchConfig::auto_commit`：
   已同步为 `auto_flush` / `with_auto_flush`。
4. 本轮（2026-09-11）回归确认并已修复的预存失败：
   - `ddl::edge_alter::test_alter_edge_execution_add_multiple`、
     `ddl::edge_alter::test_alter_edge_execution_drop_multiple`、
     `ddl::edge_basic::test_desc_execution_edge`——`DESCRIBE EDGE` 行数断言过期。
     根因：5.3 落地后 `DESC EDGE` 固定输出 2 行端点约束（`src_tag`/`dst_tag`）+
     属性行，断言仍按纯属性行计数。修复：测试期望同步为“属性行 + 2”，
     `test_desc_execution_edge` 另断言端点行存在。
5. 本轮（2026-09-11）修复的嵌入式文本事务挂起（`integration_embedded_api`）：
   - 现象：`test_session_text_transaction_commands` 在首个 `ROLLBACK` 处永久挂起，
     导致整个测试二进制无法完成；stash 基线复现，属预存问题。
   - 根因：自死锁。`Session::execute_transaction_command` 的 `ROLLBACK` /
     `ROLLBACK TO` 分支持有 `db.storage.write()` 守卫跨越
     `execute_command_plan`，而查询执行内部重入 `storage.read()`；
     parking_lot RwLock 同线程重入即死锁。
     位置：`crates/graphdb-api/src/embedded/session.rs`。
   - 修复：将写守卫作用域收窄到 undo 应用语句块，守卫释放后再跑命令计划；
     同文件其余三处 `storage_mut()` 均为终端存储操作、无重入，未动。
6. 死锁修复后暴露的两个被掩盖问题（同轮修复）：
   - 空输入全局聚合 0 行：`MATCH (p:person) RETURN count(p)` 在空表上返回 0 行，
     无事务也可复现，属引擎语义缺口（Cypher 要求恰好一行）。
     修复：`next_aggregate` 内存输出分支在无分组键且结果为空时，
     用新鲜累加器 `finalize()` 补一行默认值（`count = 0` 等）；
     有分组键的 `GROUP BY` 路径不受影响（空输入零分组即零行，语义正确）。
     位置：`blocking/aggregate_operator.rs`。
   - 过期 `LET` 断言：测试仍断言 `LET $x = 1` 报“not supported in embedded”，
     但 `LET` 早已实现（`execute_variable_assignment` + 会话变量存储）。
     修复：测试改为断言赋值成功且 `x = 1`。
7. 本轮同步修复的嵌入式套件其余预存失败（stash 基线复现，均与 ladybug 无关）：
   - `test_batch_error_create`：`BatchError::new(.., "测试错误")` 后断言
     `error == "test error"`——构造器原文存储，不存在翻译；按英文规范修正测试输入。
   - `test_transaction_commit` / `test_transaction_rollback` /
     `test_session_with_transaction` /
     `test_session_with_transaction_rollback_on_error`：函数体全同步却套用
     `#[tokio::test]`，结束时在 runtime worker 线程内 drop 数据库持有的
     tokio runtime 而 panic。改为普通 `#[test]`。全文件仅此 4 处用 tokio 宏。

## T5 存量 Clippy 警告清理（可选）

- 本轮确认无新增警告；剩余多为存量：
  - 重命名执行臂的 `needless_borrow`
  - `notify_dml` 调用点的 `unnecessary_cast`
  - 其余散布警告可按 `cargo clippy` 输出逐项清理。
- 细化清单（2026-09-11，`cargo clippy -p graphdb-query --lib` 实测）：
  以下各项本轮已全部修复；收官时 `cargo clippy --workspace --lib` 0 警告
  （仅剩第三方 `proc-macro-error2` 的 future-incompat 提示）：
  - `executor/expression/functions/udf/loader.rs:133` `needless_question_mark`
    （`Ok(path.canonicalize()…?)` 去掉外层 `Ok` + `?`）。
  - `ddl_operator/schema_executor.rs:438,657` `needless_borrow`
    （`rename_tag/rename_edge_type` 的 `&old_name, &new_name` 改为按值）。
  - `recursive_fragment_operator.rs:776` `extend` 改 `append`。
  - `streaming/runtime.rs:1113` `Some(ref qm) + as_ref()` 形成 `&&`。
  - `optimizer/cost/node_estimators/control_flow.rs:100` `useless conversion u64`。
  - `optimizer/engine.rs:507` `needless_borrow`、`engine.rs:1223`
    `expect after is_some` 改 `if-let`。
  - `traversal_parser.rs` RC 解析区 `collapsible if`×2。
  - `control_flow_node.rs:1116` `replace_box`（`Box::new` 改复用分配的 `*anchor =`）。
  - 跨 crate 存量：`graphdb-core` `type_complexity`（抽 `EventCallbacks<E>` 别名）、
    `clone_on_copy`（`*func`）、`graphdb-storage` `manual_map`×3（改 `.map` 链）、
    `graphdb-transaction` 文档缩进。

## 已知行为偏差（有意为之，改动前请复核）

- 附着目录为进程全局，非会话隔离。
- 多名 DROP 返回明确的“不支持”错误，而非逐个执行。
- 物化创建仅走会话 API，规划器内直接遇到 AS 返回明确错误。
- 标识符规范化会将 `user` 首字母大写（如需小写表名应避免该命名或调整规范化规则）。
- 处置（2026-09-11）：
  - 进程全局：见 T2 决策，迁移为会话作用域需穿透执行器，暂缓，模块文档已注明。
  - 多名 DROP：保持报错（原子性语义不清），错误信息应注明与 Kuzu 逐个语义的差异。
  - 物化创建：见 T3 决策，保持。
  - `user`/`order` 大写：根因为 `ParseContext::expect_identifier`
    对 `TokenKind::User`/`TokenKind::Order` 返回硬编码 `"User"`/`"Order"`、
    而非 `lexeme` 原文——本轮已修复为保留原文（与 `Data`/`Transaction`/
    `Read` 等分支一致），`CREATE USER user …` 等大小写敏感场景回归正常；
    其余关键字分支（`status`→小写、`CONTAINS`→大写等）暂保持不动，改动前复核单测。
