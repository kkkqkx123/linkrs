# 实现差异改进：会话变量表达式变体 + 函数纯度标记

> 状态：**已实施（2026-08-14）**。本文针对两篇已实施方案的**实现差异**
> 提出改进：
>
> - `docs/plan/query_session_variables.md`：实现走了 §2.4 权衡表的备选路径
>   「复用 `Expression::Parameter`」，存在命名空间冲突等四个实质问题，
>   需迁移到推荐的 `Expression::SessionVariable` 变体；
> - `docs/plan/query_expression_optimization.md`：常量折叠的纯度门用
>   硬编码黑名单 `IMPURE_FUNCTIONS`（默认放行），是不安全方向，需迁移到
>   函数注册表纯度标记（默认保守）。
>
> 批量求值的架构差异（`chunk/typed.rs` + `eval.rs` + `policy.rs` 整合式
> 实现 vs 文档设想的独立 `eval_batch.rs`）经核查**无需改进**；表达式编译
> （§2.3）文档自标「长期，仅设计」，维持不动。

## 0. 结论摘要

| 项 | 内容 | 决策 |
|----|------|------|
| A1 | 会话变量复用 `Expression::Parameter` 的命名空间冲突、入口行为不一致、词法旁路、不可见性 | **改为 `Expression::SessionVariable` 变体**（方案 A，即原文档推荐路径） |
| A2 | 查询参数语法归属（`$name` 已被会话变量占用的未决问题） | 推荐 `$name` 归会话变量、查询参数迁至 `@name`；备选：`$name` 归参数、会话变量用 `@name` |
| B | 常量折叠黑名单默认放行（不安全方向） | **函数注册表加纯度标记**（默认 false 保守），删除 `IMPURE_FUNCTIONS`（方案 B） |

**实施记录（2026-08-14）**：方案 A（A2 选项 1）与方案 B 均已落地，详见
第 6 节。

## 1. 差异核查结论（代码事实，均经确认）

### 1.1 会话变量 Parameter 复用的四个问题

1. **命名空间冲突/静默覆盖**：`$name` 同时是查询参数语法（`expr_parser.rs:652-662`
   `TokenKind::Dollar` + identifier → `Expression::Parameter(name)`）与会话变量
   注入通道。但 server 两个主执行路径
   （`graph_service.rs:503` `execute_stream`、`graph_service.rs:611`
   `execute_query_with_permission`）都把 `QueryRequest.parameters` **整体覆盖**
   为 `filter_session_parameters(session.variables_snapshot(), stmt)` 的结果——
   同一语句中客户端显式传入的查询参数不经任何合并直接丢失。
2. **多入口行为不一致**：embedded API（`api/embedded/session.rs:253`、
   `api/embedded/transaction.rs:237`、`api/core/query_api.rs:531`）各自构造
   parameters、完全不经过会话变量注入 → 同一语句 `RETURN $x` 在 server 端
   解析为会话变量、在 embedded 端解析为调用方参数，语义随入口漂移。
3. **词法旁路脆弱**：`statement_parameter_names`（`graph_service.rs:1276`）在
   GraphService 层**重新 lex 一遍**语句提取 `$name` 引用，与查询真实解析路径
   （`expr_parser.rs`）是两套解析，未来语法演进必然失配；其存在理由注释写明
   （`graph_service.rs:604-605`）「plan validator rejects unknown
   parameters」——参数通道带计划验证耦合，只能注入被引用名字。这是把参数通道
   硬当会话变量通道用的直接症状。
4. **不可见性**：EXPLAIN（`physical_plan_explain.rs`）、错误信息、类型推断
   均无法区分 `$x` 是查询参数还是会话变量；原文档步骤 7（EXPLAIN 展示）
   无法落地。

### 1.2 常量折叠纯度门的黑名单方向

`FoldConstantsRule`（`optimizer/heuristic/constant_folding.rs:31-41`）用
硬编码 `IMPURE_FUNCTIONS: &[&str]` 黑名单 + **默认放行**（不在名单即视为纯）。
这是不安全方向：未来在注册表新增非纯函数（如 `uuid()`、`random_between`）时
若忘记同步黑名单，会被悄悄折叠成错误结果。原文档 §2.1 明确要求注册表
`pure: bool` **默认 false 保守跳过**。

函数注册表结构：`FunctionRegistry`（`executor/expression/functions/registry.rs:20-24`）
持有 `HashMap<String, BuiltinFunction>`；`BuiltinFunction` 枚举
（`executor/expression/functions.rs:140`）按类别（Math/DateTime/Utility 等）
分派，各子枚举有宏生成的 `name()`/`execute()` 方法。纯度标记以方法形式加在
`BuiltinFunction` 上最自然（与枚举分派同构，无需改注册表数据结构）。

## 2. 方案 A：`Expression::SessionVariable` 表达式变体

### 2.1 语法归属决策（关键前置，先拍板）

`$name` 目前 = 查询参数（客户端经 `QueryRequest.parameters` 注入）。
引入会话变量后同一语法不可二义：

| 选项 | 会话变量 | 查询参数 | 影响面 |
|------|----------|----------|--------|
| **选项 1（推荐）** | `$name`（原文档 §2.1 设计，nGQL 惯例，`LET $x`/`YIELD $x` 集成测试已锁定） | 迁至 `@name`（lexer 新 token + 解析分支 → `Expression::Parameter`） | 需同步 HTTP handler（`http/handlers/query_types.rs`）与 embedded API 的参数入口；`template_extractor.rs` 的 `$N` 占位符为 planner 内部生成（不经过解析器），不受影响 |
| 选项 2 | 迁至 `@name` | 保持 `$name` | 改动面小（不动既有参数链路），但与原文档语法不一致，且 `LET $x = 1` / `RETURN $x` 已写进集成测试与文档 |

选项 1 符合原文档意图与现有 `LET` 语法事实，当前无向后兼容包袱，推荐。

### 2.2 core 变更（graphdb-core）

| 文件 | 改动 |
|------|------|
| `types/expr/def.rs` | `Expression::SessionVariable(String)` 新变体 |
| `types/expr/visitor.rs` / `construction.rs` / `memory_estimation.rs` | 新分支（编译器驱动列出全部 match 站点，逐一补） |
| `types/expr/analysis_utils.rs` | `is_evaluable` 对 `SessionVariable` 返回 false（运行时上下文依赖，天然不可折叠） |
| `types/expr/` 其余序列化/展示 | 表达式字符串化输出 `$name`（与 `expr_parser` 可回读） |

### 2.3 解析器（graphdb-query）

- `expr_parser.rs:652-662` `TokenKind::Dollar` 分支改为产
  `Expression::SessionVariable(name)`（保留 `.prop` 属性访问）；
  lexer 无需新 token（`$^`/`$$`/`$-` 已分别产 `DstRef`/`SrcRef`/`InputRef`，
  区分逻辑不变）。
- 若采用选项 1：查询参数 `@name` 在 lexer 新增 token（如 `Tk::At`）+
  `expr_parser` 新分支产 `Expression::Parameter`；核对
  `stmt_parser.rs:95`（`TokenKind::Dollar => parse_assignment_statement`）等
  Dollar 消费点。

### 2.4 求值器与上下文

- `ExpressionEvaluator::evaluate_recursive`（`expression_evaluator.rs`）新增
  `SessionVariable` 分支 → `ctx.get_session_variable(name)`；
- `ExpressionContext` trait（`evaluator/traits.rs`）新增
  `get_session_variable` 默认实现（**报错**：未定义会话变量是查询错误而非
  NULL，与现状 `filter_session_parameters` 默认 NULL 的行为收紧对齐）；
  行上下文（`BorrowedRowContext`/`DefaultExpressionContext`）覆盖返回实际值；
- 流式执行上下文携带：`EvalEnv`（`streaming/subquery.rs:43-49`，已有
  `params`/`subquery_executor` 字段）新增
  `session_variables: Option<Arc<HashMap<String, Value>>>`，Filter/Project/
  Assign 算子构建 env 时注入；每语句快照一次（沿用
  `session.variables_snapshot()`）。
- `chunk/eval.rs` 列式快路径对 `SessionVariable` 返回 None 回退 per-row
  （与 `Exists`/`In` 同模式，`collect_variables` 不收集之）。

### 2.5 API 层（graphdb-api）

- **删除旁路**：`filter_session_parameters`（`graph_service.rs:1303`）与
  `statement_parameter_names`（`graph_service.rs:1276`）整体删除；
- `execute_stream`/`execute_query_with_permission` 不再覆盖
  `QueryRequest.parameters`；会话变量快照改为经新通道
  （`QueryRequest` 增加 `session_variables` 字段，或直接进 `EvalEnv`）注入，
  与查询参数完全解耦；
- 事务语义（`ClientSession` 的 `VariableOp` overlay / COMMIT / ROLLBACK /
  SAVEPOINT，`client_session.rs:24-33,252-366`）**原样保留**——快照时机不变，
  仅改注入通道；
- 若选项 1：HTTP handler 与 embedded 的参数入口同步迁移到 `@name`。

### 2.6 EXPLAIN（原文档步骤 7）

- `describe_visitor.rs` / `physical_plan_explain.rs`：表达式字符串化天然带
  `$name` 前缀（依赖 §2.2 字符串化输出），无需额外字段。

## 3. 方案 B：函数纯度标记（常量折叠）

### 3.1 `BuiltinFunction` 纯度方法

- `functions.rs:140` `BuiltinFunction` 增加 `pub fn is_pure(&self) -> bool`，
  默认 `true`；非纯函数（`Math::Rand`/`Rand32`/`Rand64`、
  `DateTime::Now`/`Timestamp`/`CurrentDate`/`CurrentTimestamp`、
  `Utility::GenRandomUuid`、`Sleep`）覆盖返回 `false`；
- 注册表（`registry.rs`）`register_builtin` 无需改动（纯度随函数枚举携带）。

### 3.2 `FoldConstantsRule` 改造

- `constant_folding.rs:31-41` 删除 `IMPURE_FUNCTIONS` 黑名单；
- `is_pure`（:54-94）中 `Expression::Function` 分支改为查注册表：
  函数名 → `FunctionRegistry::get_builtin` → `is_pure()`；未注册函数
  **保守跳过**（不折叠）。`is_evaluable` 门保持不变；
- 递归结构（List/Map/Case/Cast/Subscript/Range/Path 子树）原样保留。

## 4. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| 1 | 语法归属决策（§2.1 选项 1 或 2） | 决策记录 |
| 2 | core `Expression::SessionVariable` 变体 + 全部 match 站点 | `cargo test -p graphdb-core` 全量 + `cargo check -p graphdb-query`（编译器列出遗漏站点） |
| 3 | 解析器：`$name` → SessionVariable（+ 选项 1 时 `@name` → Parameter） | lexer/解析单测：`$name` vs `$^`/`$$`/`$-` vs `@name` |
| 4 | 求值器 + `ExpressionContext::get_session_variable` + `EvalEnv` 注入 | 单测：已定义/未定义变量行为；列式快路径回退 |
| 5 | API 层：删 `filter_session_parameters`/`statement_parameter_names`，会话变量快照改新通道 | `tests/integration_session_variables.rs` 5 例回归 + 新增「查询参数与会话变量同名不冲突」用例 |
| 6 | EXPLAIN 展示 `$name` | EXPLAIN 单测断言 `$name` 出现在表达式串 |
| 7 | 方案 B：`BuiltinFunction::is_pure` + `FoldConstantsRule` 查注册表 | 单测：`1+2` 折叠、`rand()` 不折叠、新增非纯函数默认不折叠；`tests/dql/constant_folding.rs` 回归 |
| 8 | 全量回归 | `cargo test -p graphdb-query --lib`、`cargo test --test integration_session_variables`、`cargo test --test integration_e2e subquery`、clippy 全 features |

## 5. 风险与回退

| 风险 | 缓解 |
|------|------|
| core 枚举新增变体波及 match 站点多 | 编译器驱动逐一补分支（先 `cargo check` 列出全部站点）；本工程 `Property` 级先例已证明可行 |
| 选项 1 迁移查询参数语法波及 HTTP/embedded 入口 | 无向后兼容包袱，直接迁移；`template_extractor` 占位符为内部生成不受影响 |
| 未定义会话变量从 NULL 收紧为报错改变现状行为 | 行为收紧是明确改进；文档与测试同步更新 |
| 纯度标记漏标导致语义变化 | 默认 false 保守（与黑名单默认放行相反），漏标只损失折叠收益、不产生错误结果 |
| 回退 | 方案 A 回退 = 恢复参数复用（现网代码可回滚）；方案 B 回退 = 恢复黑名单 |

## 6. 实施记录（2026-08-14）

### 方案 A：`Expression::SessionVariable` + `@name` 查询参数

- core：`Expression::SessionVariable(String)` 新变体，全量 match 站点
  （visitor / construction / memory_estimation / display / inspection /
  traverse / type_deduce / visitor_checkers / analysis_utils）补齐；
  字符串化输出 `$name`（`display.rs`），`Parameter` 输出 `@name`。
- 解析器：`expr_parser.rs` `$name` → `SessionVariable`（保留 `.prop` 属性
  访问），`@name` → `Parameter`（lexer 复用既有 `At` token）。
- 求值链路：`ExpressionContext::get_session_variable`（默认报错——未定义
  会话变量是查询错误而非 NULL）；`EvalEnv.session_variables` 每语句快照
  注入（`subquery.rs`）；行上下文 / `ExecutionContext` /
  `QueryBindings` / DML `StandaloneValues` 全链路携带。
- API 层：删除 `filter_session_parameters` / `statement_parameter_names`
  词法旁路；`QueryRequest` 新增 `session_variables` 字段，`parameters`
  只承载 `@name` 绑定；HTTP handler（`query_types.rs`）参数值升级为
  JSON 并新增 `session_variables` 字段，经 `graph_service.execute_with_params`
  注入；embedded API 新增 `execute_with_params_and_variables`。
- 计划 `template_extractor`：参数占位符改 `@__dml_N`，`@name` 也参与
  参数化，`$name` 不参与；plan_cache 正则同步。
- 验证：`integration_session_variables.rs` 6 例（含新增「同名共存」
  `@x`/`$x` 互不冲突与 EXPLAIN 展示 `$x`）；`integration_embedded_api.rs`
  参数用例迁移 `$name`→`@name` + 新增同名共存用例；`e2e` 99 例全过。

### 方案 B：函数纯度标记

- `BuiltinFunction::is_pure()`（`functions.rs`）：默认 `true`，非纯变体
  显式列出——`Math::Rand/Rand32/Rand64`、
  `DateTime::Now/TimeStamp/CurrentDate/CurrentTimestamp`、
  `Utility::GenRandomUuid`（代码库中不存在 `sleep` 函数，旧黑名单条目
  直接移除）。
- `FoldConstantsRule::is_pure`：删除 `IMPURE_FUNCTIONS` 黑名单，`Function`
  分支改查 `global_registry_ref().get_builtin(name)` → `is_pure()`；未注册
  函数保守不折叠。单测 +2（未注册函数不折叠、纯函数 `abs(-5)` 折叠）。

### 附带修复

- `parse_fetch_statement`：`FETCH PROP ON <edge> src -> dst` 中的 `->` 此前
  被表达式解析器贪心消费为 `JsonGet`，导致 FETCH EDGE 被误解析为
  FETCH VERTICES；目标解析改在 `with_edge_syntax_mode` 内进行。
- embedded 特性门控：`database.rs` 在未启用 `qdrant` 时补齐 `(None, None)`
  分支与 `FulltextIndexManager` 条件导入，`--features embedded` 可独立编译。
