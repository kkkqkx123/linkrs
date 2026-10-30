## Session

会话结构体，作为查询执行的上下文。

### 创建会话

```rust
let mut session = db.session()?;
```

### 图空间管理

#### use_space()
切换图空间。

```rust
pub fn use_space(&mut self, space_name: &str) -> CoreResult<()>
```

```rust
session.use_space("test_space")?;
```

#### current_space()
获取当前图空间名称。

```rust
pub fn current_space(&self) -> Option<&str>
```

#### current_space_id()
获取当前图空间 ID。

```rust
pub fn current_space_id(&self) -> Option<u64>
```

### 查询执行

#### execute()
执行查询语句。

```rust
pub fn execute(&self, query: &str) -> CoreResult<QueryResult>
```

```rust
let result = session.execute("MATCH (n:User) RETURN n LIMIT 10")?;
for row in result.iter() {
    if let Some(vertex) = row.get_vertex("n") {
        println!("{:?}", vertex);
    }
}
```

#### execute_with_params()
执行参数化查询。

```rust
pub fn execute_with_params(
    &self,
    query: &str,
    params: HashMap<String, Value>
) -> CoreResult<QueryResult>
```

```rust
use std::collections::HashMap;
use linkrs::core::Value;

let mut params = HashMap::new();
params.insert("id".to_string(), Value::Int(1));
params.insert("name".to_string(), Value::String("Alice".to_string()));

let result = session.execute_with_params(
    "MATCH (n:User {id: $id, name: $name}) RETURN n",
    params
)?;
```

### 事务管理

#### begin_transaction()
开始事务。

```rust
pub fn begin_transaction(&self) -> CoreResult<Transaction<S>>
```

```rust
let txn = session.begin_transaction()?;
txn.execute("CREATE TAG user(name string)")?;
txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;
txn.commit()?;
```

#### begin_transaction_with_config()
使用配置开始事务。

```rust
pub fn begin_transaction_with_config(
    &self,
    config: TransactionConfig
) -> CoreResult<Transaction<S>>
```

```rust
use std::time::Duration;

let config = TransactionConfig::new()
    .read_only()
    .with_timeout(Duration::from_secs(60));

let txn = session.begin_transaction_with_config(config)?;
```

#### with_transaction()
在事务中执行操作（自动提交/回滚）。

```rust
pub fn with_transaction<F, T>(&self, f: F) -> CoreResult<T>
where
    F: FnOnce(&Transaction<S>) -> CoreResult<T>
```

```rust
let result = session.with_transaction(|txn| {
    txn.execute("CREATE TAG user(name string)")?;
    txn.execute("INSERT VERTEX user(name) VALUES \"1\":(\"Alice\")")?;
    Ok(42)
})?;
```

### 自动提交模式

#### set_auto_commit()
设置自动提交模式。

```rust
pub fn set_auto_commit(&mut self, auto_commit: bool)
```

```rust
session.set_auto_commit(false);  // 关闭自动提交
```

#### auto_commit()
获取自动提交模式。

```rust
pub fn auto_commit(&self) -> bool
```

### 图空间管理（DDL）

#### create_space()
创建图空间。

```rust
pub fn create_space(
    &self,
    name: &str,
    config: SpaceConfig
) -> CoreResult<()>
```

```rust
use linkrs::api::core::SpaceConfig;

let config = SpaceConfig::default();
session.create_space("my_space", config)?;
```

#### drop_space()
删除图空间。

```rust
pub fn drop_space(&self, name: &str) -> CoreResult<()>
```

#### list_spaces()
列出所有图空间。

```rust
pub fn list_spaces(&self) -> CoreResult<Vec<String>>
```

### 批量操作

#### batch_inserter()
创建批量插入器。

```rust
pub fn batch_inserter(&self, batch_size: usize) -> BatchInserter<'_, S>
```

```rust
use linkrs::core::{Vertex, Value};

let mut inserter = session.batch_inserter(100);

for i in 0..1000 {
    let vertex = Vertex::with_vid(Value::Int(i));
    inserter.add_vertex(vertex);
}

let result = inserter.execute()?;
println!("插入了 {} 个顶点", result.vertices_inserted);
```

### 预编译语句

#### prepare()
预编译查询语句。

```rust
pub fn prepare(&self, query: &str) -> CoreResult<PreparedStatement<S>>
```

```rust
let mut stmt = session.prepare("MATCH (n:User {id: $id}) RETURN n")?;

stmt.bind("id", Value::Int(1))?;
let result = stmt.execute()?;

stmt.reset();
stmt.bind("id", Value::Int(2))?;
let result2 = stmt.execute()?;
```

---

