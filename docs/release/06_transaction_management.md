# Linkrs 事务管理

## 概述

Linkrs 提供事务管理功能，确保数据的一致性和可靠性。当前单节点版本支持显式事务的开始、提交、回滚以及保存点部分回滚。

查询执行路径不独立管理事务状态：文本事务命令须经会话层先执行事务管理器的
状态变更，再进入查询计划执行；绕过会话预调用的直调路径会得到明确的状态错误。

---

## 1. 事务基础

### 1.1 事务生命周期

Linkrs 事务遵循标准的事务生命周期：

```
开始事务 (Active) → 执行操作 → 提交/中止 (Committed/Aborted)
```

### 1.2 事务状态

| 状态 | 说明 |
|------|------|
| Active | 活跃状态，可执行读写操作 |
| Committing | 提交处理中（客户端一般不可见的中间态） |
| Aborting | 中止处理中（客户端一般不可见的中间态） |
| Committed | 已提交 |
| Aborted | 已中止 |

> **注意：** 仅 `BEGIN` 用于开始事务，不支持 `START TRANSACTION` 写法。
> 会话层另有命令控制器状态（空闲/提交中/回滚中等），用于约束 `BEGIN`、`COMMIT`、
> `ROLLBACK` 的合法调用顺序；非法顺序调用返回明确的状态错误而非事务错误码。

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
- `READ ONLY`: 指定为只读事务；只读事务内的写操作在计划、日志与提交认证三层均被拒绝
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

> **注意：** 语法为 `ROLLBACK TO <name>`，`SAVEPOINT` 关键字可选
> （`ROLLBACK TO SAVEPOINT <name>` 同样接受）。

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
在事务内部创建保存点，用于实现部分回滚。同一事务内的保存点名称必须唯一，
重复创建同名保存点会明确报错（不存在遮蔽语义）。

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
`RELEASE` 仅删除指定的单个保存点，不级联删除其后创建的保存点；
`ROLLBACK TO` 则会使目标之后创建的保存点全部失效。两者语义不同，不可互换。

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

Linkrs 存储引擎默认采用乐观并发控制（MVCC 快照 + 提交期写集冲突认证）：

- **读操作**：基于快照并发读取，不阻塞写操作，也不被写操作阻塞
- **写操作**：并发写事务被允许并行执行，冲突在提交时裁决；
  写写冲突的提交失败错误为写集冲突类错误（`serialization_failed`），
  客户端应按冲突重试而非按阻塞等待处理
- **单写者模式**：可选的严格串行写模式，仅在该模式下抢占失败才报告写事务冲突
  （`write_transaction_conflict`，发生于事务开始时而非提交时）

隔离级别：默认可重复读；单语句内读到已提交的刷新按读已提交处理；
可选的可串行化级别附带 SSI 与幻读防护（全扫阈值可配）。

---

## 7. 事务配置

### 7.1 配置参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| default_timeout | 30秒 | 默认事务超时时间；未指定超时的事务继承该值，提交期发现过期将自动中止 |
| max_concurrent_transactions | 1000 | 最大并发事务数（读、写合并计数，无排队，超限直接报错） |
| auto_commit | false | 文件配置默认：显式事务控制。服务端会话在内存中默认自动提交，`BEGIN` 后切换为手动 |

事务管理器内部另有过期事务自动清理（默认开启，不在文件配置中暴露）。
语句级与查询级超时仅在语句结束时检查；`TransactionExpired` 与 `TransactionTimeout`
视为同一类超时条件。

### 7.2 配置示例

```toml
[transaction]
default_timeout = 30
max_concurrent_transactions = 1000
auto_commit = false
```

> **注意：** 文件配置中没有 `enable_2pc`、`cleanup_interval` 等配置项（未知字段会被拒绝）。

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

事务错误经 API 边界以字符串形式返回，客户端按错误文本中的错误码名匹配。
常见错误码及重试建议：

| 错误代码 | 说明 | 客户端建议 |
|----------|------|-----------|
| BeginFailed | 事务开始失败 | 检查服务端状态后重试 |
| CommitFailed | 事务提交失败 | 按失败原因处理，不盲目重试写 |
| AbortFailed | 事务中止失败 | 记录并告警 |
| TransactionNotFound | 事务未找到 | 重新开始事务 |
| TransactionTimeout / TransactionExpired | 事务超时 | 缩短事务或调大超时后重试 |
| TooManyTransactions | 并发事务数过多 | 退避后重试 |
| WriteTransactionConflict | 写事务冲突（单写者模式抢占失败） | 退避后重试 |
| SerializationFailed | 提交期写写冲突（乐观并发默认模式） | 退避后重试整个事务 |
| ReadOnlyTransaction | 只读事务内出现写操作 | 改走读写事务 |
| SavepointNotFound | 保存点未找到 | 检查保存点名称 |
| InvalidStateTransition | 无效的状态转换 | 检查调用顺序，不重试 |

其他内部错误码（如恢复、持久化、同步、预算、快照类）同样以字符串形式返回，
含义见错误文本。
