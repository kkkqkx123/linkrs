# GraphDB Rust 嵌入式 API 详细文档

## GraphDatabase

数据库主结构体，作为嵌入式 API 的主要入口点。

### 创建数据库

```rust
use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};
use std::time::Duration;

// 打开文件数据库
let db = GraphDatabase::open("/path/to/database")?;

// 创建内存数据库
let db = GraphDatabase::open_in_memory()?;

// 使用自定义配置
let config = DatabaseConfig::file("/path/to/database")
    .with_cache_size(128)
    .with_timeout(Duration::from_secs(60))
    .with_wal(true)
    .with_sync_mode(SyncMode::Normal);
let db = GraphDatabase::open_with_config(config)?;
```

### 方法

#### session()
创建新会话。

```rust
pub fn session(&self) -> CoreResult<Session<S>>
```

#### execute()
执行简单查询（便捷方法）。

```rust
pub fn execute(&self, query: &str) -> CoreResult<QueryResult>
```

#### execute_with_params()
执行参数化查询（便捷方法）。

```rust
pub fn execute_with_params(
    &self,
    query: &str,
    params: HashMap<String, Value>
) -> CoreResult<QueryResult>
```

#### create_space()
创建图空间（便捷方法）。

```rust
pub fn create_space(&self, name: &str, space_config: SpaceConfig) -> CoreResult<()>
```

#### drop_space()
删除图空间（便捷方法）。

```rust
pub fn drop_space(&self, name: &str) -> CoreResult<()>
```

#### list_spaces()
列出所有图空间（便捷方法）。

```rust
pub fn list_spaces(&self) -> CoreResult<Vec<String>>
```

---

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
use graphdb::core::Value;

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
use graphdb::api::core::SpaceConfig;

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
use graphdb::core::{Vertex, Value};

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

## QueryResult

查询结果结构体，封装核心层的查询结果。

### 方法

#### columns()
获取列名列表。

```rust
pub fn columns(&self) -> &[String]
```

#### len()
获取行数。

```rust
pub fn len(&self) -> usize
```

#### is_empty()
检查结果是否为空。

```rust
pub fn is_empty(&self) -> bool
```

#### get()
获取指定行。

```rust
pub fn get(&self, index: usize) -> Option<&Row>
```

#### first()
获取第一行。

```rust
pub fn first(&self) -> Option<&Row>
```

#### last()
获取最后一行。

```rust
pub fn last(&self) -> Option<&Row>
```

#### iter()
获取行迭代器。

```rust
pub fn iter(&self) -> impl Iterator<Item = &Row>
```

```rust
for row in result.iter() {
    println!("{:?}", row);
}
```

#### metadata()
获取元数据。

```rust
pub fn metadata(&self) -> &ResultMetadata
```

#### to_json()
转换为 JSON 字符串。

```rust
pub fn to_json(&self) -> CoreResult<String>
```

#### to_json_compact()
转换为紧凑格式 JSON 字符串。

```rust
pub fn to_json_compact(&self) -> CoreResult<String>
```

---

## Row

结果行结构体，封装一行数据。

### 方法

#### get()
按列名获取值。

```rust
pub fn get(&self, column: &str) -> Option<&Value>
```

```rust
if let Some(value) = row.get("name") {
    println!("Name: {:?}", value);
}
```

#### get_by_index()
按索引获取值。

```rust
pub fn get_by_index(&self, index: usize) -> Option<&Value>
```

#### has_column()
检查是否包含指定列。

```rust
pub fn has_column(&self, column: &str) -> bool
```

### 类型化获取方法

#### get_string()
获取字符串值。

```rust
pub fn get_string(&self, column: &str) -> Option<String>
```

#### get_int()
获取 i64 整数值。

```rust
pub fn get_int(&self, column: &str) -> Option<i64>
```

#### get_float()
获取 f64 浮点值。

```rust
pub fn get_float(&self, column: &str) -> Option<f64>
```

#### get_bool()
获取布尔值。

```rust
pub fn get_bool(&self, column: &str) -> Option<bool>
```

#### get_vertex()
获取顶点。

```rust
pub fn get_vertex(&self, column: &str) -> Option<&Vertex>
```

#### get_edge()
获取边。

```rust
pub fn get_edge(&self, column: &str) -> Option<&Edge>
```

#### get_path()
获取路径。

```rust
pub fn get_path(&self, column: &str) -> Option<&Path>
```

#### get_list()
获取列表。

```rust
pub fn get_list(&self, column: &str) -> Option<&List>
```

#### get_map()
获取映射。

```rust
pub fn get_map(&self, column: &str) -> Option<&HashMap<String, Value>>
```

---

## PreparedStatement

预编译语句结构体。

### 方法

#### bind()
绑定参数。

```rust
pub fn bind(&mut self, name: &str, value: Value) -> CoreResult<()>
```

```rust
stmt.bind("id", Value::Int(1))?;
stmt.bind("name", Value::String("Alice".to_string()))?;
```

#### bind_many()
绑定多个参数。

```rust
pub fn bind_many(&mut self, params: HashMap<String, Value>) -> CoreResult<()>
```

#### execute()
执行查询（返回结果集）。

```rust
pub fn execute(&mut self) -> CoreResult<QueryResult>
```

#### execute_update()
执行更新（返回影响行数）。

```rust
pub fn execute_update(&mut self) -> CoreResult<usize>
```

#### execute_batch()
批量执行。

```rust
pub fn execute_batch(
    &mut self,
    param_batches: &[HashMap<String, Value>]
) -> CoreResult<Vec<QueryResult>>
```

```rust
let batches = vec![
    {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::Int(1));
        params.insert("name".to_string(), Value::String("Alice".to_string()));
        params
    },
    {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::Int(2));
        params.insert("name".to_string(), Value::String("Bob".to_string()));
        params
    },
];

let results = stmt.execute_batch(&batches)?;
```

#### reset()
重置语句。

```rust
pub fn reset(&mut self)
```

#### clear_bindings()
清除参数绑定。

```rust
pub fn clear_bindings(&mut self)
```

#### query()
获取查询字符串。

```rust
pub fn query(&self) -> &str
```

#### parameters()
获取参数列表。

```rust
pub fn parameters(&self) -> &HashMap<String, DataType>
```

#### is_bound()
检查参数是否已绑定。

```rust
pub fn is_bound(&self, name: &str) -> bool
```

#### stats()
获取执行统计信息。

```rust
pub fn stats(&self) -> &ExecutionStats
```

---

## BatchInserter

批量插入器结构体。

### 方法

#### add_vertex()
添加顶点。

```rust
pub fn add_vertex(&mut self, vertex: Vertex) -> &mut Self
```

```rust
let vertex = Vertex::with_vid(Value::Int(1));
inserter.add_vertex(vertex);
```

#### add_edge()
添加边。

```rust
pub fn add_edge(&mut self, edge: Edge) -> &mut Self
```

#### add_vertices()
添加多个顶点。

```rust
pub fn add_vertices(&mut self, vertices: Vec<Vertex>) -> &mut Self
```

#### add_edges()
添加多个边。

```rust
pub fn add_edges(&mut self, edges: Vec<Edge>) -> &mut Self
```

#### execute()
执行批量插入。

```rust
pub fn execute(mut self) -> CoreResult<BatchResult>
```

#### buffered_vertices()
获取当前缓冲区中的顶点数量。

```rust
pub fn buffered_vertices(&self) -> usize
```

#### buffered_edges()
获取当前缓冲区中的边数量。

```rust
pub fn buffered_edges(&self) -> usize
```

#### has_buffered_data()
检查是否有缓冲的数据。

```rust
pub fn has_buffered_data(&self) -> bool
```

---

## 配置类型

### DatabaseConfig

```rust
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: Option<PathBuf>,       // 数据库路径
    pub cache_size_mb: usize,        // 缓存大小（MB）
    pub default_timeout: Duration,   // 默认超时
    pub enable_wal: bool,            // 是否启用 WAL
    pub sync_mode: SyncMode,         // 同步模式
}
```

#### 构造方法

```rust
// 内存数据库配置
let config = DatabaseConfig::memory();

// 文件数据库配置
let config = DatabaseConfig::file("/path/to/db");

// 使用路径创建配置
let config = DatabaseConfig::new("/path/to/db");
```

#### 链式配置方法

```rust
let config = DatabaseConfig::memory()
    .with_cache_size(128)
    .with_timeout(Duration::from_secs(60))
    .with_wal(false)
    .with_sync_mode(SyncMode::Full);
```

### TransactionConfig

```rust
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    pub timeout: Option<Duration>,      // 事务超时时间
    pub read_only: bool,                // 是否只读
    pub durability: DurabilityLevel,    // 持久性级别
}
```

#### 构造方法

```rust
let config = TransactionConfig::new()
    .read_only()
    .with_timeout(Duration::from_secs(60))
    .with_durability(DurabilityLevel::Relaxed);
```

### BatchConfig

```rust
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub batch_size: usize,           // 批次大小
    pub auto_commit: bool,           // 是否自动提交
    pub continue_on_error: bool,     // 是否忽略错误继续处理
    pub max_errors: Option<usize>,   // 最大错误数量
}
```

---

## 完整示例

### 基本 CRUD 操作

```rust
use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};
use graphdb::core::Value;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 创建内存数据库
    let db = GraphDatabase::open_in_memory()?;

    // 创建会话
    let mut session = db.session()?;

    // 创建图空间
    session.create_space("test_space", Default::default())?;
    session.use_space("test_space")?;

    // 创建标签
    session.execute("CREATE TAG user(name string, age int)")?;

    // 插入数据
    session.execute(r#"INSERT VERTEX user(name, age) VALUES "1":("Alice", 25)"#)?;
    session.execute(r#"INSERT VERTEX user(name, age) VALUES "2":("Bob", 30)"#)?;

    // 查询数据
    let result = session.execute("MATCH (n:user) RETURN n")?;
    println!("找到 {} 个用户", result.len());

    for row in result.iter() {
        if let Some(vertex) = row.get_vertex("n") {
            println!("顶点: {:?}", vertex);
        }
    }

    // 使用参数化查询
    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::String("Alice".to_string()));

    let result = session.execute_with_params(
        "MATCH (n:user {name: $name}) RETURN n",
        params
    )?;

    println!("查询结果: {:?}", result.to_json()?);

    Ok(())
}
```

### 事务使用示例

```rust
use graphdb::api::embedded::{GraphDatabase, TransactionConfig};
use std::time::Duration;

fn transaction_example() -> Result<(), Box<dyn std::error::Error>> {
    let db = GraphDatabase::open_in_memory()?;
    let session = db.session()?;

    // 基本事务
    let txn = session.begin_transaction()?;
    txn.execute("CREATE TAG product(name string, price double)")?;
    txn.execute(r#"INSERT VERTEX product(name, price) VALUES "1":("Laptop", 999.99)"#)?;
    txn.commit()?;

    // 带保存点的事务
    let txn = session.begin_transaction()?;

    let sp1 = txn.create_savepoint(Some("after_create".to_string()))?;
    txn.execute("CREATE TAG order(id int)")?;

    let sp2 = txn.create_savepoint(Some("after_insert".to_string()))?;
    txn.execute(r#"INSERT VERTEX order(id) VALUES "1":(1001)"#)?;

    // 回滚到第一个保存点
    txn.rollback_to_savepoint(sp1)?;

    // 提交事务（只包含 CREATE TAG，不包含 INSERT）
    txn.commit()?;

    // 使用 with_transaction
    let result = session.with_transaction(|txn| {
        txn.execute("CREATE TAG customer(name string)")?;
        txn.execute(r#"INSERT VERTEX customer(name) VALUES "1":("John")"#)?;
        Ok("事务成功")
    })?;

    println!("{}", result);

    Ok(())
}
```

### 批量插入示例

```rust
use graphdb::api::embedded::GraphDatabase;
use graphdb::core::{Vertex, Edge, Value};
use std::collections::HashMap;

fn batch_insert_example() -> Result<(), Box<dyn std::error::Error>> {
    let db = GraphDatabase::open_in_memory()?;
    let session = db.session()?;
    session.use_space("test_space")?;

    // 创建批量插入器，每100条自动刷新
    let mut inserter = session.batch_inserter(100);

    // 批量添加顶点
    for i in 0..1000 {
        let mut vertex = Vertex::with_vid(Value::Int(i));
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String(format!("User{}", i)));
        props.insert("age".to_string(), Value::Int(20 + (i % 50) as i64));

        let tag = graphdb::core::vertex_edge_path::Tag::new(
            "user".to_string(),
            props
        );
        vertex.add_tag(tag);
        inserter.add_vertex(vertex);
    }

    // 批量添加边
    for i in 0..999 {
        let edge = Edge::new(
            Value::Int(i),
            Value::Int(i + 1),
            "follows".to_string(),
            0,
            HashMap::new(),
        );
        inserter.add_edge(edge);
    }

    // 执行批量插入
    let result = inserter.execute()?;

    println!("插入顶点: {}", result.vertices_inserted);
    println!("插入边: {}", result.edges_inserted);

    if result.has_errors() {
        println!("错误数量: {}", result.error_count());
        for error in &result.errors {
            println!("错误: {:?} - {}", error.item_type, error.error);
        }
    }

    Ok(())
}
```

### 预编译语句示例

```rust
use graphdb::api::embedded::GraphDatabase;
use graphdb::core::Value;
use std::collections::HashMap;

fn prepared_statement_example() -> Result<(), Box<dyn std::error::Error>> {
    let db = GraphDatabase::open_in_memory()?;
    let session = db.session()?;
    session.use_space("test_space")?;

    // 预编译查询
    let mut stmt = session.prepare("MATCH (n:User {id: $id}) RETURN n.name, n.age")?;

    // 第一次执行
    stmt.bind("id", Value::Int(1))?;
    let result1 = stmt.execute()?;
    println!("结果1: {:?}", result1.to_json()?);

    // 重置并重新绑定
    stmt.reset();
    stmt.bind("id", Value::Int(2))?;
    let result2 = stmt.execute()?;
    println!("结果2: {:?}", result2.to_json()?);

    // 查看执行统计
    let stats = stmt.stats();
    println!("执行次数: {}", stats.execution_count);
    println!("平均执行时间: {:?}", stats.average_execution_time());

    // 批量执行
    let batches: Vec<HashMap<String, Value>> = (1..=10)
        .map(|i| {
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::Int(i));
            params
        })
        .collect();

    let results = stmt.execute_batch(&batches)?;
    println!("批量执行完成，共 {} 个结果", results.len());

    Ok(())
}
```
