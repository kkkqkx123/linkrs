# ColdSnapshot 查询引擎集成

ColdSnapshot 当前是独立的数据结构，未接入查询引擎。后续需要实现 `StorageReader` trait 使其可作为只读数据源参与查询。

## StorageReader trait

现有 trait 位于 `graphdb-storage/src/storage/traits.rs`，约 30 个方法。ColdSnapshot 只需实现边缘查询子集：

```rust
impl StorageReader for ColdSnapshot {
    // 核心方法
    fn get_out_edges(&self, src: u32, label: LabelId) -> Vec<EdgeRef>;
    fn get_in_edges(&self, dst: u32, label: LabelId) -> Vec<EdgeRef>;
    fn get_edge(&self, src: u32, dst: VertexId, label: LabelId) -> Option<EdgeRef>;
    fn degree(&self, src: u32, label: LabelId) -> usize;

    // 元数据
    fn has_label(&self, label: LabelId) -> bool;
    fn vertex_capacity(&self, label: LabelId) -> usize;
}
```

## 查询引擎集成

在 `graphdb-storage` 的 `GraphStorage` 中检测 ColdSnapshot：

```rust
pub struct GraphStorage {
    active: Arc<EdgeStore>,       // 热数据
    cold: Option<ColdSnapshot>,   // 冷快照
    ...
}

impl StorageReader for GraphStorage {
    fn get_out_edges(&self, src: u32, label: LabelId) -> Vec<EdgeRef> {
        let mut results = self.active.get_out_edges(src, label);
        if let Some(ref cold) = self.cold {
            if cold.label() == label {
                results.extend(cold.get_out_edges(src));
            }
        }
        results
    }
}
```

## 注意事项

- **只读约束**：ColdSnapshot 是只读的，所有写入/修改操作必须路由到 `active`。
- **快照切换**：正在使用的 ColdSnapshot 不能被丢弃，需引用计数追踪。
- **flush 跳过**：当 cold 快照存在时，flush/checkpoint 应跳过 cold 部分。
