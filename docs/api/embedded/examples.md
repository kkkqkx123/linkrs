## 完整示例

### 基本 CRUD 操作

```rust
use linkrs::api::embedded::{GraphDatabase, DatabaseConfig};
use linkrs::core::Value;
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
use linkrs::api::embedded::{GraphDatabase, TransactionConfig};
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
use linkrs::api::embedded::GraphDatabase;
use linkrs::core::{Vertex, Edge, Value};
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

        let tag = linkrs::core::vertex_edge_path::Tag::new(
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
use linkrs::api::embedded::GraphDatabase;
use linkrs::core::Value;
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
