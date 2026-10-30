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

