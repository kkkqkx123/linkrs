# GraphDB 事务管理

## 概述

GraphDB 提供完整的事务管理功能，确保数据的一致性和可靠性。事务管理模块支持 ACID 特性，提供保存点、两阶段提交等高级功能。

---

## 1. 事务基础

### 1.1 事务生命周期

GraphDB 事务遵循标准的事务生命周期：

```
开始事务 (Active) → 执行操作 → 提交/中止 (Committed/Aborted)
```

### 1.2 事务状态

| 状态 | 说明 |
|------|------|
| Active | 活跃状态，可执行读写操作 |
| Prepared | 已准备（2PC阶段1完成） |
| Committing | 提交中 |
| Committed | 已提交 |
| Aborting | 中止中 |
| Aborted | 已中止 |

---

## 2. BEGIN TRANSACTION - 开始事务

### 功能
显式开始一个新事务。

### 语法结构
```cypher
BEGIN TRANSACTION [READ ONLY]
[WITH TIMEOUT <duration>]
[WITH DURABILITY {NONE | IMMEDIATE}]
[WITH TWO_PHASE_COMMIT]
```

### 参数说明
- `READ ONLY`: 指定为只读事务
- `TIMEOUT`: 事务超时时间（默认30秒）
- `DURABILITY`: 持久性级别
  - `NONE`: 不保证立即持久化（高性能）
  - `IMMEDIATE`: 立即持久化（默认）
- `TWO_PHASE_COMMIT`: 启用两阶段提交

### 示例
```cypher
-- 基础事务
BEGIN TRANSACTION

-- 只读事务
BEGIN TRANSACTION READ ONLY

-- 带超时的事务
BEGIN TRANSACTION WITH TIMEOUT 60s

-- 高性能写事务
BEGIN TRANSACTION WITH DURABILITY NONE

-- 安全写事务（两阶段提交）
BEGIN TRANSACTION WITH DURABILITY IMMEDIATE WITH TWO_PHASE_COMMIT
```

---

## 3. COMMIT - 提交事务

### 功能
提交当前事务，将所有更改持久化到数据库。

### 语法结构
```cypher
COMMIT [TRANSACTION]
```

### 关键特性
- 原子性提交：所有更改要么全部成功，要么全部失败
- 自动释放事务资源
- 支持持久性级别配置
- 支持两阶段提交协议

### 示例
```cypher
BEGIN TRANSACTION
-- 执行数据操作
INSERT VERTEX Person(name) VALUES "p1":("Alice")
INSERT VERTEX Person(name) VALUES "p2":("Bob")
-- 提交事务
COMMIT
```

---

## 4. ROLLBACK - 回滚事务

### 功能
中止当前事务，撤销所有未提交的更改。

### 语法结构
```cypher
ROLLBACK [TRANSACTION]
```

### 关键特性
- 原子性回滚：撤销事务中的所有更改
- 自动释放事务资源
- 支持回滚到保存点

### 示例
```cypher
BEGIN TRANSACTION
-- 执行数据操作
INSERT VERTEX Person(name) VALUES "p1":("Alice")
-- 发生错误，回滚事务
ROLLBACK
```

---

## 5. SAVEPOINT - 保存点管理

### 5.1 创建保存点

#### 功能
在事务内部创建保存点，用于实现部分回滚。

#### 语法结构
```cypher
SAVEPOINT <savepoint_name>
```

#### 示例
```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
INSERT VERTEX Person(name) VALUES "p1":("Alice")
SAVEPOINT sp2
INSERT VERTEX Person(name) VALUES "p2":("Bob")
```

### 5.2 回滚到保存点

#### 功能
回滚到指定的保存点，撤销该保存点之后的所有操作。

#### 语法结构
```cypher
ROLLBACK TO SAVEPOINT <savepoint_name>
```

#### 关键特性
- 支持嵌套保存点
- 回滚后保存点之后的保存点失效
- 事务保持活跃状态

#### 示例
```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
INSERT VERTEX Person(name) VALUES "p1":("Alice")
SAVEPOINT sp2
INSERT VERTEX Person(name) VALUES "p2":("Bob")
-- 回滚到 sp1，撤销 p2 的插入
ROLLBACK TO SAVEPOINT sp1
-- 此时只有 p1 存在
COMMIT
```

### 5.3 释放保存点

#### 功能
释放指定的保存点，释放后不能再回滚到该保存点。

#### 语法结构
```cypher
RELEASE SAVEPOINT <savepoint_name>
```

#### 示例
```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
INSERT VERTEX Person(name) VALUES "p1":("Alice")
-- 确认操作无误，释放保存点
RELEASE SAVEPOINT sp1
COMMIT
```

---

## 6. 两阶段提交（2PC）

### 6.1 概述

两阶段提交（Two-Phase Commit）是一种分布式事务协议，确保跨多个资源的事务一致性。

### 6.2 2PC 事务状态

| 状态 | 说明 |
|------|------|
| Preparing | 准备阶段1：正在收集参与者投票 |
| AllPrepared | 所有参与者已投票准备 |
| VoteAbort | 至少一个参与者投票中止 |
| Committing | 提交阶段2：正在提交 |
| Committed | 已提交 |
| Aborting | 正在中止 |
| Aborted | 已中止 |
| Timeout | 超时 |

### 6.3 启用两阶段提交

#### 语法结构
```cypher
BEGIN TRANSACTION WITH TWO_PHASE_COMMIT
```

#### 示例
```cypher
BEGIN TRANSACTION WITH TWO_PHASE_COMMIT
-- 执行跨资源操作
INSERT VERTEX Person(name) VALUES "p1":("Alice")
-- 2PC协调器自动管理提交过程
COMMIT
```

---

## 7. 事务监控

### 7.1 SHOW TRANSACTIONS - 显示事务列表

#### 功能
显示当前所有活跃事务的信息。

#### 语法结构
```cypher
SHOW TRANSACTIONS
```

#### 返回信息
- 事务ID
- 事务状态
- 开始时间
- 运行时长
- 是否只读
- 修改的表
- 保存点数量

#### 示例
```cypher
SHOW TRANSACTIONS
```

### 7.2 事务统计信息

GraphDB 自动收集以下事务统计信息：

| 统计项 | 说明 |
|--------|------|
| total_transactions | 总事务数 |
| active_transactions | 活跃事务数 |
| committed_transactions | 已提交事务数 |
| aborted_transactions | 已中止事务数 |
| timeout_transactions | 超时事务数 |

---

## 8. 事务配置

### 8.1 配置参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| default_timeout | 30秒 | 默认事务超时时间 |
| max_concurrent_transactions | 1000 | 最大并发事务数 |
| enable_2pc | false | 是否启用2PC |
| auto_cleanup | true | 是否自动清理过期事务 |
| cleanup_interval | 10秒 | 清理任务执行间隔 |

### 8.2 配置示例

```toml
[transaction]
default_timeout = 30
max_concurrent_transactions = 1000
enable_2pc = true
auto_cleanup = true
cleanup_interval = 10
```

---

## 9. 事务隔离级别

GraphDB 基于 redb 的存储引擎，采用单写者多读者模型：

- **读操作**：支持并发读取，不阻塞其他读操作
- **写操作**：同一时间只允许一个写事务
- **读写冲突**：写事务阻塞其他写事务，但不阻塞读事务

---

## 10. 最佳实践

### 10.1 事务使用建议

1. **保持事务简短**：长时间运行的事务会占用资源，增加冲突概率
2. **及时提交或回滚**：避免事务长时间处于活跃状态
3. **使用保存点**：对于复杂操作，使用保存点实现部分回滚
4. **设置合理的超时**：根据操作复杂度设置超时时间

### 10.2 错误处理

```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
-- 执行操作
-- 如果发生错误
ROLLBACK TO SAVEPOINT sp1
-- 或者完全回滚
ROLLBACK
```

### 10.3 性能优化

1. **批量操作**：将多个操作放在一个事务中，减少事务开销
2. **选择合适的持久性级别**：对于非关键数据，可使用 `DURABILITY NONE` 提高性能
3. **使用只读事务**：对于查询操作，使用 `READ ONLY` 选项

---

## 11. 错误代码

| 错误代码 | 说明 |
|----------|------|
| BeginFailed | 事务开始失败 |
| CommitFailed | 事务提交失败 |
| AbortFailed | 事务中止失败 |
| TransactionNotFound | 事务未找到 |
| TransactionTimeout | 事务超时 |
| TooManyTransactions | 并发事务数过多 |
| WriteTransactionConflict | 写事务冲突 |
| ReadOnlyTransaction | 只读事务 |
| SavepointNotFound | 保存点未找到 |
| InvalidStateTransition | 无效的状态转换 |
