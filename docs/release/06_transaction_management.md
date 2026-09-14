# GraphDB 事务管理

## 概述

GraphDB 提供事务管理功能，确保数据的一致性和可靠性。当前单节点版本支持显式事务的开始、提交、回滚以及保存点部分回滚。

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
| Committed | 已提交 |
| Aborted | 已中止 |

---

## 2. BEGIN TRANSACTION - 开始事务

### 功能
显式开始一个新事务。

### 语法结构
```cypher
BEGIN [TRANSACTION] [READ ONLY | READ WRITE]
```

### 参数说明
- `TRANSACTION` 关键字可省略（`BEGIN` 与 `BEGIN TRANSACTION` 等价）
- `READ ONLY`: 指定为只读事务
- `READ WRITE`: 指定为读写事务（默认）
- 不指定读写模式时默认为读写事务

> **注意：** 当前版本不支持 `WITH TIMEOUT`、`WITH DURABILITY`、`WITH TWO_PHASE_COMMIT` 等选项；超时时间等行为由服务端配置控制（见第 7 节）。

### 示例
```cypher
-- 基础事务
BEGIN TRANSACTION

-- 省略 TRANSACTION 关键字
BEGIN

-- 只读事务
BEGIN TRANSACTION READ ONLY
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
中止当前事务，撤销所有未提交的更改；或回滚到指定保存点。

### 语法结构
```cypher
ROLLBACK [TRANSACTION] [TO <savepoint_name>]
```

### 关键特性
- 完整回滚：撤销事务中的所有更改，释放事务资源
- 部分回滚：`ROLLBACK TO <savepoint_name>` 回滚到指定保存点，事务保持活跃
- 回滚到保存点后，该保存点之后的保存点失效

> **注意：** 语法为 `ROLLBACK TO <name>`，不需要 `SAVEPOINT` 关键字。

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
ROLLBACK [TRANSACTION] TO <savepoint_name>
```

#### 关键特性
- 支持嵌套保存点
- 回滚后保存点之后的保存点失效
- 事务保持活跃状态，可继续操作并最终提交

#### 示例
```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
INSERT VERTEX Person(name) VALUES "p1":("Alice")
SAVEPOINT sp2
INSERT VERTEX Person(name) VALUES "p2":("Bob")
-- 回滚到 sp1，撤销 p2 的插入
ROLLBACK TO sp1
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

## 6. 事务隔离级别

GraphDB 存储引擎采用单写者多读者模型：

- **读操作**：支持并发读取，不阻塞其他读操作
- **写操作**：同一时间只允许一个写事务
- **读写冲突**：写事务阻塞其他写事务，但不阻塞读事务

---

## 7. 事务配置

### 7.1 配置参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| default_timeout | 30秒 | 默认事务超时时间 |
| max_concurrent_transactions | 1000 | 最大并发事务数 |
| auto_commit | false | 每条成功语句是否自动提交 |

### 7.2 配置示例

```toml
[transaction]
default_timeout = 30
max_concurrent_transactions = 1000
auto_commit = false
```

> **注意：** 当前版本没有 `enable_2pc`、`auto_cleanup`、`cleanup_interval` 等配置项。

---

## 8. 最佳实践

### 8.1 事务使用建议

1. **保持事务简短**：长时间运行的事务会占用资源，增加冲突概率
2. **及时提交或回滚**：避免事务长时间处于活跃状态
3. **使用保存点**：对于复杂操作，使用保存点实现部分回滚
4. **注意并发限制**：并发事务数受 `max_concurrent_transactions` 约束

### 8.2 错误处理

```cypher
BEGIN TRANSACTION
SAVEPOINT sp1
-- 执行操作
-- 如果发生错误
ROLLBACK TO sp1
-- 或者完全回滚
ROLLBACK
```

### 8.3 性能优化

1. **批量操作**：将多个操作放在一个事务中，减少事务开销
2. **使用只读事务**：对于查询操作，使用 `READ ONLY` 选项

---

## 9. 错误代码

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
