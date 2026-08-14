# docs/plan 目录说明：进度总览与剩余任务

> 本目录存放各查询引擎功能的设计/方案文档。本文档是进度索引：
> 说明各方案的实施状态、剩余任务与验证基线，并链接到详细方案文档。
> 更新日期：2026-08-14。

## 1. 文档索引

| 文档 | 主题 | 状态 |
|------|------|------|
| [query_expression_optimization.md](query_expression_optimization.md) | 表达式求值优化（常量折叠 + 批量求值 + 表达式编译） | 常量折叠、批量/列式求值已完成；表达式编译为设计阶段 |
| [query_session_variables.md](query_session_variables.md) | 会话级用户变量（$var） | 已完成（走 Parameter 复用路径，改进方案见下） |
| [query_followup_improvements.md](query_followup_improvements.md) | 实现差异改进（SessionVariable 表达式变体 + 函数纯度标记） | **已完成（2026-08-14）** |
| [executor_operator_context_refactor.md](executor_operator_context_refactor.md) | 执行器算子上下文重构（拆分 OperatorBase + left/right 角色显式化） | **待实施** |

> 原 `query_verification_backlog.md` 已删除，其遗留验证项与低优先小项并入
> 本文档第 4 节。原 `query_exists_in_subquery*.md` 系列方案文档（P0-P3
> 实现详案）已完成使命删除，实施记录并入下文第 2 节。

## 2. 已完成主线：EXISTS / IN / NOT IN 子查询

P0-P3 四阶段全部完成（P0 双侧键重构、P1 合取位置转换、P2 相关子查询
逐行重执行、P3 表达式级兜底）。

### 2.1 已完成

- **P0 — PatternApplyNode 双侧键重构**：节点字段
  `key_cols`/`left_input_var`/`right_input_var` → `hash_keys`/`probe_keys`
  （与 `SemiJoinNode` 同构）；`LogicalPatternApplyNode` 二进制化；
  unnest 的 `build_semi_join_from_pattern_apply` 变为直通，删除 645 行
  `replace_all_variables`；`ApplySpec::PatternApply { hash_keys, probe_keys,
  anti }` 运行时双侧求值。
- **P1 — binder 放行 + WHERE 合取位置转换**：
  - binder：`bind_subquery_body`（child scope = `BinderScope::with_parent`）
    绑定子查询 MATCH/WHERE/RETURN，外层变量经父 scope 解析为相关变量；
    `BoundExpression::Exists/In` 声明即消费。
  - 解析器：`parse_pattern_string` 模式串规范化（裸模式加括号，可再解析）；
    `peek_token` 缓冲前瞻修复（顺带修复 JSON `->'key'` 后置分支与
    `IN { }` 检测）；DML 边语句 `edge_syntax_mode` 歧义守卫；
    **修复 `NOT IN`（`TokenKind::NotIn`）被静默丢弃的缺陷**。
  - exists 规划器（`exists_planner.rs`）：AND 合取位置提取
    `Exists`/`Not(Exists)`/`In`/`Not(In)` → `ExistsSpec`；键提取
    （等值条件一侧仅引用单个子查询变量 → probe，另一侧不引用 → hash；
    IN 合成 `return_expr = left_expr` 等值）；非等值相关条件报精确错误；
    递归子查询规划（嵌套 EXISTS 自然支持）；物理 + 逻辑镜像
    PatternApply 构建。
  - `WhereClausePlanner` 集成：无 EXISTS 走原 Filter 路径（行为不变）；
    有 EXISTS 走 PatternApply 链，残差条件叠 Filter。
  - EXPLAIN 标注：`describe_visitor` 与 arena
    `physical_plan_explain.rs` 双路径均展示 `hash_keys`/`probe_keys`/`anti`。
  - e2e：`tests/e2e/subquery.rs` 9 例（相关/非相关 EXISTS、NOT EXISTS、
    相关 NOT EXISTS、IN、NOT IN、路径子查询、残差条件、EXPLAIN 算子断言）。
- **P2 — 相关子查询逐行重执行（CorrelatedApply）**：非等值/多变量相关
  （如 `p.age > t.age`）规划为 `CorrelatedApplyNode` + `Argument` 源，
  右子树按行重建执行器执行（`tests/e2e/subquery.rs` 相关用例覆盖）。
- **P3 — 表达式级兜底**：OR 之下 / RETURN 等非合取位置的 EXISTS/IN 由
  `plan_expression_subqueries` 编译为独立子计划，挂在
  Filter/Project/Assign 的算子 spec 上（`subquery_runners`），物化阶段
  实例化 per-operator `SubqueryExecutor`（`subquery.rs`）：重置协议复用
  执行器、非相关结果缓存、相关帧注入、NULL 永不匹配；EXPLAIN 展示
  `subquery: N` 计数。

### 2.2 剩余任务

| # | 任务 | 说明 | 优先级 |
|---|------|------|--------|
| 1 | **全量回归** | `cargo test --test '*'`（integration_e2e 全量未跑完）；`cargo clippy -p graphdb-query --all-targets` 全 features | 高（上线前） |

## 3. 其他方案状态

### 3.1 表达式求值优化（query_expression_optimization.md）

- **已完成**：常量折叠 `FoldConstantsRule`（启发式优化器挂载，10 个单测 +
  `tests/dql/constant_folding.rs` 集成测试）；批量/列式求值
  （`chunk/typed.rs` SIMD 友好 TypedBatch 批量运算 + `chunk/eval.rs`
  列引用零拷贝/常量广播快路径 + `chunk/policy.rs` 跨查询自适应策略 +
  逐行回退）；**常量折叠纯度门**（黑名单 → 注册表
  `BuiltinFunction::is_pure` 纯度标记，未注册函数保守不折叠，见
  [query_followup_improvements.md](query_followup_improvements.md) 方案 B）。
- **待实施**：
  - §2.3 表达式编译（闭包链 / 字节码，文档自标长期仅设计）。

### 3.2 会话级用户变量（query_session_variables.md）

- **已完成**：
  - `ClientSession` 变量存储：base 存储 + 事务 overlay（`VariableOp` 序列），
    `set_variable`/`variable_value`/`variables_snapshot`；
  - 事务语义：`commit_variables` / `rollback_variables` /
    `rollback_variables_to`（ROLLBACK TO SAVEPOINT）/ `push_variable_savepoint`
    / `release_variable_savepoint`；
  - `graph_service` 语句分发处理 `LET $name = expr`（求值后写回会话变量）；
  - **`Expression::SessionVariable` 表达式变体**：`$name` 解析为会话变量、
    `@name` 为查询参数（两通道独立、同名共存）；词法旁路
    （`filter_session_parameters`/`statement_parameter_names`）已删除；
    每语句快照经 `QueryRequest.session_variables` 注入；HTTP handler 与
    embedded API 同步接入；EXPLAIN 展示 `$x`；集成测试 6 例通过，详见
    [query_followup_improvements.md](query_followup_improvements.md) 方案 A。

## 4. 遗留验证项与低优先小项（原 backlog 并入）

| # | 项 | 说明 | 优先级 |
|---|----|------|--------|
| 1 | `BEGIN READ ONLY` 会话级端到端 | 同一只读事务内两语句读同一快照（插入数据后快照不变） | 中 |
| 2 | 只读事务内执行 DML 被拒绝 | 断言返回错误（`ensure_can_write` 路径） | 中 |
| 3 | SAVEPOINT 真实服务器端到端 | `SAVEPOINT sp` → 写 → `ROLLBACK TO sp` → 数据恢复；`RELEASE` 后回滚报错（现有用例为 mock） | 中 |
| 4 | 属性裁剪基准收益度量 | `benches/` 对比裁剪前后 GetVertices/GetEdges/GetNeighbors 吞吐与内存 | 低 |
| 5 | EXPLAIN 中间算子列级 projected 展示 | 目前仅 Source 层展示 `projected`；Unary（AppendVertices/Project）与 Join 未展示 | 低 |
| 6 | 分区策略扩展与工作窃取 | 仅支持 Range 均分；Hash/RoundRobin 分区 + `MorselWorkerPool` 工作窃取（设计先行，基准验证负载倾斜后再实施） | 低 |

## 5. 验证基线（2026-08-14）

- `cargo test -p graphdb-query --lib`：**1491 passed**（基线 1430 → +61）。
- `cargo clippy -p graphdb-query --all-targets`：新增警告为零（仅存既有
  测试代码警告）。
- `cargo test --test integration_e2e subquery`：**28 passed**（含
  表达式级子查询全量回归）；`cargo test --test e2e`：**99 passed**。
- `cargo test --test integration_session_variables`：**6 passed**；
  `cargo test --test integration_embedded_api --features embedded,fulltext-search`：
  **61 passed**（2 个既有 batch 断言失败与本次改动无关）。
- `cargo test --test '*'` 全量 integration 回归：**尚未执行完毕**（见 2.2 #1）。
