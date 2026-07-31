# 边属性索引查询层集成

## 背景

`EdgePropertyIndex` 的存储层已完成（`TimeTravelEdgeStore` 中的 `lookup_edges_by_property_range`），但查询执行器未接入。

## 集成路径

```
SQL 语法 → Plan Node → Spec → SourceOperator → Storage API
```

### 1. 计划节点

新增或扩展计划节点携带边属性过滤参数：

```rust
pub struct EdgePropertyIndexScanNode {
    label: LabelId,
    prop_name: String,
    lower: Option<Value>,
    upper: Option<Value>,
    edge_count_estimate: u64,
}
```

可复用的位置：`crates/graphdb-query/src/query/planning/plan/core/nodes/access/`，在 `EdgeIndexScanNode` 基础上添加过滤字段。

### 2. SourceSpec

```rust
pub enum SourceSpec {
    // ... 已有变体
    EdgePropertyIndex {
        label: LabelId,
        prop_name: String,
        prop_range: (Option<Value>, Option<Value>),
    },
}
```

### 3. SourceOperator

```rust
// in SourceOperator::open() 匹配新变体
SourceSpec::EdgePropertyIndex { label, prop_name, prop_range } => {
    let candidates = store.lookup_edges_by_property_range(
        label, &prop_name, prop_range.0, prop_range.1
    )?;
    // candidates 是 Vec<(u32, u32)> = Vec<(src, dst)>
    // 与 CSR 遍历结果取交集
    let edge_iter = CsrEdgeIter::new(csr, candidates);
    Ok(Box::new(edge_iter))
}
```

### 4. SQL 触发器

对应查询语法：

```sql
-- 语法设想
MATCH (a)-[e:KNOWS]->(b)
WHERE e.weight > 1.0 AND e.weight < 10.0
RETURN a, b, e
```

## 工作量

- 计划节点定义：0.5 天
- SourceSpec 扩展：0.5 天
- SourceOperator 分支：1 天
- SQL 解析触发：1-2 天
- 集成测试：0.5 天

总计 3.5-4.5 天。
