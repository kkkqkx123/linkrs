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

