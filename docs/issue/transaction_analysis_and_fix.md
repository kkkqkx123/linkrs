# Transaction 模块分析与修复记录

## 结论

`graphdb-transaction` 当前已经成为运行时事务管理的核心入口，但内部仍然存在若干未闭环的问题：

1. 事务超时和清理路径没有正确释放 MVCC timestamp。
2. `TransactionContext` 的状态迁移不是原子操作，存在并发竞态。
3. commit / abort 的失败路径和状态机语义不一致，原先的“可重试中间态”并没有真正实现。
4. savepoint 回滚只记录了 operation log 边界，未精确记录 undo log 边界，容易出现回滚范围过大。
5. 若干配置项与独立事务类型已存在，但尚未真正形成完整执行闭环。

## 已修复内容

1. 为 `TransactionCleaner` 注入共享 `VersionManager`，让 expired transaction cleanup 可以显式释放 timestamp。
2. 将 `TransactionContext::transition_to` 改为基于 CAS 的原子状态迁移。
3. 为 savepoint 增加 `undo_log_index`，rollback-to-savepoint 仅回滚 savepoint 之后新增的 undo 记录。
4. 收敛 commit / abort 失败语义：sync 失败后不再依赖“中间态重试”，而是终止事务并释放资源。
5. 增加回归测试，覆盖 cleanup 后 pending count 回收和 savepoint undo 边界。

## 仍需关注

1. `auto_cleanup`、`write_lock_timeout`、`two_phase_commit` 目前仍是部分功能，尚未形成完整执行链。
2. 独立的 `ReadTransaction` / `InsertTransaction` / `UpdateTransaction` 仍然与 `TransactionManager` 的主链路并存，需要后续统一设计。
3. `commit_transaction_with_undo` 目前仍是薄封装，后续可按真实语义决定是否保留。

