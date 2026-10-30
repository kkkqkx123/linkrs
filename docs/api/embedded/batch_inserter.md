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

