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

