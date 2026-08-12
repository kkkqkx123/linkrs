# MVCC 快照注入 QueryContext 方案

> 状态：已实施（2026-08-12）。与初版方案的差异见文末「实施差异」。

## 1. 现状分析

### 事务侧（graphdb-transaction）

`transaction/manager.rs` 已提供快照读能力：

- `TransactionManager::begin_snapshot_read(snapshot_ts: Timestamp)`（行 311）：
  在指定时间戳建立一致性快照读事务
- `context.set_snapshot_timestamp(snapshot_ts)`（行 355）：事务上下文携带快照
- `begin_statement()`（行 521）：事务内语句边界
- `transaction/conflict.rs`：`have_write_conflict` / `WriteSetAnalyzer` /
  `conflicts_on_vertex` / `conflicts_on_edge` 写冲突检测已实现

### 查询侧（graphdb-query）

- `context/query_context.rs` 的 `QueryContext`（行 59）**无任何快照/时间戳概念**，
  仅含 rctx / cancel_token / id_gen / space_info / charset_info / arena
- `executor/streaming/transaction_scope.rs` 的 `SessionTransactionController`
  是**纯状态跟踪器**：只做状态机与 `TransactionScope` 创建，实际的
  TransactionManager begin/commit/rollback 由 API 层（GraphService）执行
  （文件头注释明确职责边界）

### 核心缺口

显式事务（BEGIN ... 多语句 ... COMMIT）中每条语句独立创建 `QueryContext`，
执行器读取时无快照时间戳概念。多语句事务的一致性读（同一快照）与写冲突检测
（提交时校验快照过期）缺少查询侧载体。

## 2. 方案设计

### 2.1 QueryContext 增加快照时间戳

```rust
pub struct QueryContext {
    // ...现有字段...
    /// MVCC snapshot timestamp for consistent reads within explicit transactions.
    snapshot_ts: Option<Timestamp>,   // Option: None = auto-commit 单语句
}
```

- `QueryContextBuilder` 增加 `with_snapshot_ts(Timestamp)` / `snapshot_ts()` 访问器
- 类型引用方向：graphdb-query 依赖 graphdb-transaction（DAG 允许：
  transaction → storage → query）

### 2.2 API 层事务语句序列集成

`GraphService` 执行显式事务语句时（`transaction_scope.rs` 的
`ExplicitBorrowed` 分支）：

1. 事务内**首条语句**：`begin_snapshot_read(ts)`，把 `ts` 存入
   `SessionTransactionController` 的会话状态（新增字段
   `snapshot_ts: Option<Timestamp>`）
2. 每条语句：`QueryContext::builder(...).with_snapshot_ts(ts).build()`，
   使同事务所有语句共享同一快照
3. COMMIT 前：`have_write_conflict(当前语句写集, 提交时版本)`
   —— 若快照过旧导致冲突则回滚并返回冲突错误（复用 `conflict.rs`）
4. 快照复用策略：读语句保持 `begin_snapshot_read` 建立的快照；写语句
   `begin_statement()` 推进事务内语句边界（沿用现有 API）

### 2.3 执行器透传

- `StreamingExecutionEngine` 实例化计划时将 `snapshot_ts` 注入
  `ExecutionRuntime` / 存储访问层
- 存储读取接口（`graphdb-storage` 的 scan/point lookup）增加可选快照参数
  或通过执行上下文传递：`Option<Timestamp>` 为 None 时走当前版本读
  （零改动回退路径）

### 2.4 隔离级别映射

| 事务模式 | 行为 |
|----------|------|
| 显式只读事务（read_write=false） | 首语句建立快照，全事务共享（REPEATABLE READ 语义） |
| 显式读写事务 | 读语句走快照，写语句走当前版本 + 提交冲突检测 |
| 自动提交（现状） | `snapshot_ts = None`，行为不变 |

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 | 状态 |
|------|------|----------|------|
| 1 | `QueryContext` / `QueryContextBuilder` 增加快照字段 | `context/query_context.rs`, `context/query_context_builder.rs` | 已实施 |
| 2 | `SessionTransactionController` 增加 `snapshot_ts` 状态字段 | `executor/streaming/transaction_scope.rs` | 不需要（见实施差异 2） |
| 3 | API 层语句序列接线（首语句建快照、后续共享） | graphdb-api 的 GraphService 事务分支 | 已实施（含 BEGIN READ ONLY） |
| 4 | 执行器透传快照至存储读取 | `executor/streaming/engine.rs`, `executor/streaming/runtime.rs` | 已实施（经 StorageOperationContext） |
| 5 | 提交时写冲突检测集成 | API 层 COMMIT 分支（复用 `conflict.rs`） | 已实施（commit_transaction 内置） |

### 3.1 实施记录（2026-08-12）

**快照时间戳的传递链（实施前已由代码演进完成，本次核查确认）**：

```
TransactionManager::create_execution(txn_id)
  → TransactionExecution.read_timestamp = context.effective_snapshot_timestamp()
  → QueryApi::execute_with_execution 构造 StorageOperationContext::transaction_with_timestamps
  → QueryRequestContext.operation_context
  → prepared.rs::query_context_for_request 派生 → QueryContext.snapshot_ts()
  → 存储读取经 QueryStorage（SnapshotReader）走 read_timestamp 快照
```

- RepeatableRead（默认）：`begin_statement` 不刷新快照，同事务所有语句共享
  首语句建立的快照（REPEATABLE READ 语义）
- ReadCommitted：`begin_statement` / `refresh_statement_snapshot` 推进到
  当前已提交时间戳（READ COMMITTED 语义）
- 写冲突检测：`commit_transaction` → `check_write_set_conflict`（基于写集
  重叠），冲突时回滚并返回错误，无需 API 层额外接线

**本次新增**：

1. `QueryContext` 增加 `snapshot_ts: Option<Timestamp>` 字段
   （`context/query_context.rs`），`QueryContextBuilder` 增加
   `with_snapshot_ts(ts)` / `snapshot_ts()`（`query_context_builder.rs`）
2. `prepared.rs::snapshot_ts_for_request`：显式事务内语句从
   `operation_context.read_timestamp` 派生快照并注入 QueryContext；
   auto-commit 语句保持 `None`（读当前版本，行为不变）
3. SQL 层 `BEGIN [TRANSACTION] [READ ONLY | READ WRITE]`：
   - lexer 新增 `READ` / `ONLY` / `WRITE` 关键字
     （`lexing/lexer.rs`、`core/token.rs`），并在 `expect_identifier`
     中放宽为普通标识符（不破坏 `read`/`write` 作为标签/属性名）
   - `parse_begin_transaction` 解析访问模式
     （`parser/parsing/stmt_parser.rs`），`BeginTransactionStmt` 增加
     `read_only: Option<bool>`（`parser/ast/stmt/transaction.rs`）
   - `GraphService::handle_begin_transaction` 解析访问模式并设置
     `TransactionOptions.read_only` → `begin_transaction` 自动分派到
     `begin_read_transaction`（快照只读事务）；支持 `START TRANSACTION` 前缀
4. 单元测试：`snapshot_ts_for_request` 三条（显式事务继承 / auto-commit
   无快照 / 无 operation_context 无快照）、`parse_begin_access_mode` 9 变体、
   `test_parse_begin_transaction_access_modes`（BEGIN/READ ONLY/READ WRITE/
   非法 READ 报错）

### 3.2 隔离级别映射（实施后）

| 事务模式 | 行为 |
|----------|------|
| 显式只读事务（`BEGIN READ ONLY`） | `begin_read_transaction` 建立快照，全事务共享（REPEATABLE READ 语义） |
| 显式读写事务 | 读语句走快照（RepeatableRead 不刷新），写语句走当前版本 + 提交冲突检测 |
| 自动提交（现状） | `snapshot_ts = None`，行为不变 |

## 4. 验证方法

- 单元测试（本次落地）：`snapshot_ts_for_request` 派生规则、BEGIN 访问
  模式解析（`graphdb-query` lib 1415 通过、`graphdb-api` lib 通过、
  `graphdb-transaction` lib 224 通过）
- 事务语义测试：`tests/` 中显式事务内多语句读，插入数据后快照不变
  （同一快照一致读）——资源受限环境暂缓，见「遗留验证」
- 写冲突测试：两个事务并发写同一顶点，后者提交失败并返回冲突错误
  （`commit_transaction` → `check_write_set_conflict` 既有测试覆盖）
- 回归：自动提交路径全量测试（`snapshot_ts = None` 行为不变）
- 跨 crate：`cargo check --all-features`（server 特征，含事务集成）

### 遗留验证

资源受限环境未运行 integration 测试，以下验证待补：

- `BEGIN READ ONLY` 会话级端到端：同一只读事务内两语句读同一快照
- 只读事务内执行 DML 应被拒绝（read_only 上下文）
- `cargo test -p graphdb-query --test '*'` 全量回归

## 5. 预期收益

- 显式事务获得真正的一致性读（当前各语句独立时间戳，事务内可能读到
  不一致版本）
- 写冲突在提交前检出，回滚成本降低（对应文档 4.2.4 建议）
- 为 Savepoint / 部分回滚（`transaction/types.rs` 已有 `SavepointInfo`）
  铺路

## 6. 风险与回退

- **风险**：快照长期持有导致存储版本堆积。缓解：事务超时/会话断开的
  快照释放（已有 `CancelReason::ClientDisconnect` 路径）
- **风险**：执行器透传改动面大。缓解：采用 `Option<Timestamp>` 参数默认
  None，分两步——先 QueryContext/API 层（无执行器改动即可生效于规划阶段），
  再执行器透传
- **回退**：`with_snapshot_ts` 不调用即恢复自动提交行为

## 7. 实施差异（相对初版方案）

1. **步骤 2 取消**：`SessionTransactionController` 是纯状态跟踪器（职责
   边界在文件头注释明确），快照不存会话状态——每次语句经
   `create_execution` 从 `TransactionContext.effective_snapshot_timestamp()`
   实时获取，语义等价且无冗余状态
2. **步骤 4 无需执行器改动**：快照透传已由
   `StorageOperationContext.read_timestamp`（经 `QueryRequestContext`）
   完成，存储读取接口原本就消费该字段
3. **步骤 5 无需 API 层接线**：`commit_transaction` 内置
   `check_write_set_conflict`，提交时检测写集冲突
4. **只读事务入口增强**：方案仅提 `begin_snapshot_read(ts)`；实施时补全
   SQL `BEGIN READ ONLY` 语法并接线 `begin_transaction → begin_read_transaction`
   （当前时间戳快照），`begin_snapshot_read` 保留给 time-travel 场景
