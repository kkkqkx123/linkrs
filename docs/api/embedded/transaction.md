## Transaction

事务结构体，封装事务的生命周期管理。

### 创建事务

通过 Session 创建：

```rust
let txn = session.begin_transaction()?;
```

### 查询执行

#### execute()
在事务中执行查询。

```rust
pub fn execute(&self, query: &str) -> CoreResult<QueryResult>
```

#### execute_with_params()
在事务中执行参数化查询。

```rust
pub fn execute_with_params(
    &self,
    query: &str,
    params: HashMap<String, Value>
) -> CoreResult<QueryResult>
```

### 事务控制

#### commit()
提交事务。

```rust
pub fn commit(mut self) -> CoreResult<()>
```

```rust
let txn = session.begin_transaction()?;
txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;
txn.commit()?;
```

#### rollback()
回滚事务。

```rust
pub fn rollback(mut self) -> CoreResult<()>
```

### 保存点管理

#### create_savepoint()
创建保存点。

```rust
pub fn create_savepoint(&self, name: Option<String>) -> CoreResult<SavepointId>
```

```rust
let sp = txn.create_savepoint(Some("checkpoint1".to_string()))?;
txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;

// 如果需要，可以回滚到保存点
txn.rollback_to_savepoint(sp)?;
```

#### rollback_to_savepoint()
回滚到保存点。

```rust
pub fn rollback_to_savepoint(&self, savepoint_id: SavepointId) -> CoreResult<()>
```

#### release_savepoint()
释放保存点。

```rust
pub fn release_savepoint(&self, savepoint_id: SavepointId) -> CoreResult<()>
```

#### find_savepoint()
通过名称查找保存点。

```rust
pub fn find_savepoint(&self, name: &str) -> Option<SavepointId>
```

#### list_savepoints()
获取所有活跃保存点。

```rust
pub fn list_savepoints(&self) -> Vec<SavepointInfo>
```

### 状态检查

#### is_active()
检查事务是否处于活动状态。

```rust
pub fn is_active(&self) -> bool
```

#### is_committed()
检查事务是否已提交。

```rust
pub fn is_committed(&self) -> bool
```

#### is_rolled_back()
检查事务是否已回滚。

```rust
pub fn is_rolled_back(&self) -> bool
```

#### info()
获取事务信息。

```rust
pub fn info(&self) -> CoreResult<TransactionInfo>
```

```rust
let info = txn.info()?;
println!("事务ID: {}, 状态: {}, 运行时间: {}ms",
    info.id, info.state, info.elapsed_ms);
```

---

