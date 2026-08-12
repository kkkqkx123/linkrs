# 会话级用户变量（$var）跨语句支持方案

> 状态：待实施。对应分析文档 4.3-9「多语句事务支持」的剩余部分——
> 事务控制语句（SAVEPOINT 等）已核查落地，本方案补齐查询间变量传递。

## 1. 现状分析

### 已核查落地（无需重复投入）

分析文档 4.3-9 建议的三项中，两项已随代码演进完成：

- **SAVEPOINT / 部分回滚**：`GraphService::execute` 已按语句前缀分发
  （`graph_service.rs:415-427`），并有完整实现：
  - `handle_savepoint`（行 1112）：`SAVEPOINT name` → `TransactionManager::create_savepoint`
  - `handle_release_savepoint`（行 1148）：`RELEASE SAVEPOINT name`
  - `handle_rollback_transaction`（行 1042）：`ROLLBACK TO name` 分支 →
    `find_savepoint_by_name` + `rollback_to_savepoint`
  - 底层能力完备：`transaction/context.rs` 的 `create_savepoint`（行 874）、
    `rollback_to_savepoint`（行 922，含 operation log 回滚 / undo 回放 /
    write_set/read_set/redo 恢复）、`release_savepoint`（行 913）
  - 嵌入式 API 亦有 `Transaction::create_savepoint` / `rollback_to_savepoint`
    （`api/embedded/transaction.rs:331,351`）
- **多语句状态机**：`SessionTransactionController`（`transaction_scope.rs:223`）
  已是完整状态机（None/Active/Committing/Committed/RollingBack/RolledBack/
  RollbackOnly），`TxnOperator`（`operators/txn_operator.rs`）提供计划级
  BEGIN/COMMIT/ROLLBACK 算子

### 核心缺口：查询间变量传递

- **无会话级变量存储**：`ClientSession`（`api/server/client/client_session.rs`）
  仅有 space/role/query/transaction/statistics 上下文，无变量存储
- **lexer 的 `$` 仅支持 nGQL 图引用**：`lexer.rs:772-787` 只产出
  `DstRef($$)` / `SrcRef($^)` / `InputRef($-)` / `Dollar($)`，无
  `$var` 用户变量语法
- **现有参数机制是单查询级**：`Expression::Parameter(name)` →
  `ParameterSlot`（`executor/streaming/parameters.rs`），参数帧在计划构建时
  编译、执行时注入，不跨语句存活
- **事务一致性缺口**：即使引入变量，事务内赋值在 ROLLBACK 后仍残留，
  与事务语义冲突（分析文档要求「查询间变量传递的事务一致性保证」）

## 2. 方案设计

### 2.1 语法与 AST

nGQL 风格会话变量，与现有 `$` 图引用语法区分（`$^`/`$$`/`$-` 保留）：

```
LET $name = expr;            -- 赋值语句
YIELD $name;                 -- 引用（表达式内可用 $name）
```

- **lexer**：`$` 后跟标识符（非 `^`/`$`/`-`）时产出新 token
  `Tk::SessionVar`，`value` 为 `name`（`lexing/lexer.rs`）
- **AST**：新增 `Stmt::AssignVariable { name, expression }`
  （`parser/ast/stmt/`），引用侧在表达式解析中把 `$name` 构造为
  `Expression::Parameter("$" + name)` 之外的**新变体**
  `Expression::SessionVariable(name)`（`graphdb-core/types/expr/def.rs`，
  或复用 `Parameter` + 命名前缀约定，见 2.4 权衡）
- **解析**：`stmt_parser.rs` 新增 `parse_let` 分支；`Expression::Variable`
  已处理 `$` 前缀图引用的解析处新增 SessionVar 分支

### 2.2 会话存储与注入

- `ClientSession` 新增 `session_variables: Arc<RwLock<HashMap<String, Value>>>`
  （`client_session.rs`），提供 `set_variable` / `get_variable` /
  `snapshot_variables() -> HashMap<String, Value>`（执行语句前的快照拷贝）
- API 层执行语句时（`GraphService::execute_query_with_permission` 路径）：
  将 `snapshot_variables()` 注入查询的 `QueryRequestContext` /
  `ParameterFrame`，使会话变量成为只读输入（同语句内先赋值后引用在
  流式执行中不成立——语句内赋值须走管道变量 `pipe_variable_resolver.rs`，
  本次不做）
- 求值：`ExpressionEvaluator::evaluate_recursive` 的
  `Expression::SessionVariable` 分支从上下文读（复用 `ExpressionContext`
  trait 新增 `get_session_variable` 默认实现，与 `evaluate_label` 同模式）

### 2.3 事务一致性（关键设计）

会话变量与显式事务绑定，采用**快照-恢复**模型：

- **赋值时机**：`LET` 语句在事务外=立即生效；事务内=仅更新
  `SessionTransactionController` 持有的**事务变量覆盖层**
  （新增字段 `variable_overlay: Vec<(String, Option<Value>)>`，记录
  set/delete 操作序列）
- **读取时机**：事务内语句读 `覆盖层 → 会话层` 合并视图；事务外读会话层
- **COMMIT**：覆盖层合并进会话层并清空（或直接废弃覆盖层）
- **ROLLBACK / ROLLBACK TO SAVEPOINT**：丢弃覆盖层中
  savepoint 之后（或全部）的变量操作，恢复先前值
- **隔离**：变量是会话私有状态，不进入事务 undo log / 写集
  （存储层不可见），仅保证「回滚后变量回到事务开始前」的语义

### 2.4 实现路径权衡

| 方案 | 优点 | 缺点 |
|------|------|------|
| 复用 `Expression::Parameter`（前缀 `$` 命名） | 零 core 变更，参数帧注入链路现成 | 参数槽按计划编译，需在每次执行前重编译；语义混淆 |
| 新增 `Expression::SessionVariable`（推荐） | 语义清晰，evaluator/EXPLAIN 可区分 | core 表达式枚举 + 各 visitor 需新增分支（已有 `Property` 级先例） |

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | lexer 识别 `$name` → `Tk::SessionVar` | `parser/lexing/lexer.rs`, `parser/core/token.rs` |
| 2 | 表达式解析：`$name` → `Expression::SessionVariable`；`LET` 语句 AST | `parser/ast/stmt/`, `parser/parsing/stmt_parser.rs` |
| 3 | core 表达式新增变体 + visitor/构造器/内存估计支持 | `graphdb-core/types/expr/`（`def.rs`, `visitor.rs`, `construction.rs`, `memory_estimation.rs`） |
| 4 | `ExpressionContext` 新增 `get_session_variable`（默认报错，行上下文覆盖） | `executor/expression/evaluator/traits.rs`, `evaluation_context/*.rs` |
| 5 | `ClientSession` 变量存储 + 快照注入 `QueryRequestContext`/参数帧 | `api/server/client/client_session.rs`, `api/core/query_api.rs` |
| 6 | `LET` 语句计划：单行求值算子写回会话；变量覆盖层 + COMMIT/ROLLBACK 合并 | `executor/streaming/operators/`, `streaming/transaction_scope.rs`, `api/server/graph_service.rs`（事务分支） |
| 7 | EXPLAIN 展示会话变量引用（`$name` 列名） | `executor/explain/` |

## 4. 验证方法

- 单元测试：lexer `$name` 与 `$^`/`$$`/`$-` 区分；`LET` 解析；事务内
  变量覆盖/回滚恢复（含 `ROLLBACK TO SAVEPOINT`）
- 端到端（tests/）：`LET $x = 1` → `YIELD $x`；事务内 `LET` 后 ROLLBACK，
  `YIELD $x` 回到旧值；会话间变量隔离
- 回归：`cargo test -p graphdb-query`、`cargo test -p graphdb-api`、
  `cargo test -p graphdb-core` 全量

## 5. 预期收益

- 补齐分析文档 4.3-9 最后一项：查询间变量传递 + 事务一致性
- 为嵌入式 API / C API 暴露 `session_set_variable` 等稳定接口铺路

## 6. 风险与回退

- **风险**：core 表达式枚举新增变体波及所有 match 站点。缓解：先
  编译器驱动列出站点，逐一补分支；或退回 2.4 参数复用方案（步骤 2-4 取消）
- **风险**：变量覆盖层与快照事务的交互（同事务内读到变量 vs 数据
  版本不一致）。缓解：变量读取始终走「覆盖层→会话层」合并视图，
  与 MVCC 快照正交
- **回退**：仅保留步骤 1-5（语法+存储+注入，无事务覆盖层），
  变量语义退化为「即时生效、无回滚保证」，不影响既有功能
