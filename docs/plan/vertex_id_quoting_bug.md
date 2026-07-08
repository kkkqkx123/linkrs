# Vertex ID 引号问题修复分析

## 根因分析

`INSERT EDGE` 报 `Vertex not found` 的根因是 `VertexId::to_string()` 对字符串类型的 ID 会自动添加双引号。

### 问题链路

1. `VertexId::from_string("person00001")` 存储原始字节 `person00001`（无引号）
2. `VertexId::to_string()` 输出 `"person00001"`（带引号）—— 实现了 `Display` trait
3. `insert_vertex_at_timestamp` 调用 `vertex.vid.to_string()` 获取 ID 字符串
4. 该字符串传入 `ctx.insert_vertex(label_id, &id_str, ...)` → `table.insert(external_id, ...)`
5. `table.insert` 使用 `IdKey::Text(external_id.to_string())` 作为索引键
6. 因此顶点被索引为 `IdKey::Text("\"person00001\"")`（带引号）

### 边插入时的查找

1. `INSERT EDGE works_at VALUES "person00001" -> "comp064"` 解析后
2. `edge.src` 是 `VertexId::from_string("person00001")`（无引号）
3. `resolve_internal_id_from_str` 调用 `id_indexer.get_index(&IdKey::Text("person00001"))`
4. 查找键是 `IdKey::Text("person00001")`（无引号）
5. 与索引中的 `IdKey::Text("\"person00001\"")` 不匹配 → 返回 `None` → `Vertex not found`

## 修复方案

在 `insert_vertex_at_timestamp` 中，优先使用 `vertex.vid.as_str()` 获取无引号的原始字符串，而不是 `vertex.vid.to_string()`。

```rust
// Before:
let id_str = vertex.vid.to_string();

// After:
if let Some(id_str) = vertex.vid.as_str() {
    ctx.insert_vertex(label_id, id_str, &props, ts)?;
} else {
    let id_str = vertex.vid.to_string();
    ctx.insert_vertex(label_id, &id_str, &props, ts)?;
}
```

## 额外修复：MVCC 时间戳

修复过程中还发现 `get_write_timestamp()` 不递增 `write_ts`，导致所有操作使用相同时间戳（ts=1）。

修复：在 `VersionManager` 中添加 `next_write_timestamp()` 方法，使用 `fetch_add(1)` 原子递增。

```rust
pub fn next_write_timestamp(&self) -> Timestamp {
    self.write_ts.fetch_add(1, Ordering::SeqCst)
}
```

并在 `GraphStorageContext::get_write_timestamp()` 中调用它。

## 测试结果

修复后：
- 48 个测试通过（之前 14 个）
- 20 个测试失败（之前更多）
- `INSERT EDGE works_at` 不再报 `Vertex not found`
- 剩余失败均为 pre-existing 问题（DATE 解析、VECTOR 解析、fulltext 等）
