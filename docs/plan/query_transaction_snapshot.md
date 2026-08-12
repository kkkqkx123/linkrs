# MVCC 快照注入 QueryContext 方案

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

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | `QueryContext` / `QueryContextBuilder` 增加快照字段 | `context/query_context.rs`, `context/query_context_builder.rs` |
| 2 | `SessionTransactionController` 增加 `snapshot_ts` 状态字段 | `executor/streaming/transaction_scope.rs` |
| 3 | API 层语句序列接线（首语句建快照、后续共享） | graphdb-api 的 GraphService 事务分支 |
| 4 | 执行器透传快照至存储读取 | `executor/streaming/engine.rs`, `executor/streaming/runtime.rs` |
| 5 | 提交时写冲突检测集成 | API 层 COMMIT 分支（复用 `conflict.rs`） |

## 4. 验证方法

- 事务语义测试：`tests/` 中显式事务内多语句读，插入数据后快照不变
  （同一快照一致读）
- 写冲突测试：两个事务并发写同一顶点，后者提交失败并返回冲突错误
- 回归：自动提交路径全量测试（`snapshot_ts = None` 行为不变）
- 跨 crate：`cargo check --all-features`（server 特征，含事务集成）

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
