# 拆分 `factorization_compute.rs` 的分析与实施计划

## 1. 现状分析

文件 `crates/graphdb-query/src/planning/plan/factorization_compute.rs` 共 1576 行，包含一个巨型 `match self` 实现 `FactorizedSchemaCompute for LogicalNodeEnum`，涵盖 **48+ 个 LogicalNodeEnum 变体**的 schema 计算逻辑。职责过多体现在：

| 类别 | 变体数 | 重复模式 |
|------|--------|----------|
| Leaf 节点 (Scan/Start/Fulltext/Vector) | 8+ | `FactorizedSchema::new()` + `create_flat_group` |
| 单输入变换 (GetVertices/Flatten/Limit/Sample/Aggregate) | 5 | clone child, 可能修改 |
| 扇出/遍历 (GetNeighbors/Traverse/Expand/ExpandAll/AppendVertices) | 5 | flatten probe + create unflat group |
| 双输入扩展 (BiExpand/BiTraverse) | 2 | merge children + flatten + create unflat |
| 二元 Join (Inner/Left/Right/Cross/FullOuter/Semi) | 6 | merge two children + flatten extras |
| Set 操作 (Union/Minus/Intersect/WcoIntersect) | 4 | merge/flatten |
| 投影/赋值 (Project/Assign) | 2 | 复杂的依赖分析 target group 选择 |
| 过滤 (Filter/Select/Loop) | 3 | FlattenAllButOne / flatten dependent |
| 排序 (Sort/TopN/Window) | 3 | FlattenAllButOne |
| 其他 (Dedup/Remove/DataCollect/Materialize/Unwind) | 5 | FlattenAll / special |

## 2. 拆分方案

### 目录结构

将 `factorization_compute.rs` (文件) 转换为 `factorization_compute/` (目录)，内含：

```
factorization_compute/
├── mod.rs                              # trait impl 主 match 分发 + resolve_id / register_output_names
├── access.rs                           # ScanVertices, ScanEdges, GetVertices, GetEdges, GetNeighbors, Start
├── operation.rs                        # Project, Filter, Aggregate, Flatten, Sort, TopN, Window, Dedup, Limit, Sample
├── join.rs                             # InnerJoin, LeftJoin, RightJoin, CrossJoin, FullOuterJoin, SemiJoin
├── traversal.rs                        # Expand, ExpandAll, Traverse, AppendVertices, BiExpand, BiTraverse
├── set_ops.rs                          # Union, Minus, Intersect, WcoIntersect
├── assign.rs                           # Assign (依赖分析与 Project 类似但独立)
├── control_flow.rs                     # Select, Loop, BeginTransaction, Commit, Rollback, PassThrough, Argument
├── unwind.rs                           # Unwind (特殊 list literal 逻辑)
└── flat_leaf.rs                        # Fulltext*, Vector*, Remove, DataCollect, Materialize 等简单 flat 节点
```

### 各文件职责

**`mod.rs`** — trait impl 入口，仅做 match 分发：
```rust
impl FactorizedSchemaCompute for LogicalNodeEnum {
    fn compute_factorized_schema(&mut self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
        match self {
            LogicalNodeEnum::ScanVertices(n) => access::scan_vertices(n),
            LogicalNodeEnum::Project(n) => operation::project(n, child_schemas),
            LogicalNodeEnum::InnerJoin(_) | ... => join::binary_join(self, child_schemas),
            // ... 所有变体分发
        }
    }
    fn compute_flat_schema(...) { ... }
}
```

保留 `resolve_id()`、`register_output_names()`、`bi_expand_schema()` 为 `pub(super)` 工具函数，供子模块使用。

**`access.rs`** — 6 个变体：`scan_vertices`, `scan_edges`, `get_vertices`, `get_edges`, `get_neighbors`, `start`

**`operation.rs`** — 10 个变体：`project`, `filter`, `aggregate`, `flatten`, `sort`, `top_n`, `window`, `dedup`, `limit`, `sample`。其中 project/filter 含 GroupDependencyAnalyzer 逻辑。

**`join.rs`** — 6 个变体：6 种 join 共享 merge-and-flatten 模式。

**`traversal.rs`** — 6 个变体：所有遍历/扩展节点，共享 flatten-probe + create-unflat 模式。

**`set_ops.rs`** — 4 个变体：`union`, `minus`, `intersect`, `wco_intersect`

**`assign.rs`** — 1 个变体：`assign`（与 Project 逻辑相似但独立，保持清晰）

**`control_flow.rs`** — 7 个变体：`select`, `loop`, `begin_transaction`, `commit`, `rollback`, `pass_through`, `argument`

**`unwind.rs`** — 1 个变体：`unwind`（list literal 特殊处理）

**`flat_leaf.rs`** — 其余简单 flat 节点：`fulltext_*`, `vector_*`, `remove`, `data_collect`, `materialize`, 默认分支

### `plan.rs` 模块声明更新

```rust
pub mod factorization_compute;  // 无需修改，Rust 自动识别目录形式的模块
```

## 3. 实施步骤

1. 创建 `factorization_compute/` 目录
2. 将原 `factorization_compute.rs` 重命名为 `factorization_compute/mod.rs`
3. 从 mod.rs 提取各组 match arm 到对应的子文件：
   - `access.rs`: ScanVertices, ScanEdges, GetVertices, GetEdges, GetNeighbors, Start
   - `operation.rs`: Project, Filter, Aggregate, Flatten, Sort, TopN, Window, Dedup, Limit, Sample
   - `join.rs`: InnerJoin..SemiJoin (6 种)
   - `traversal.rs`: Expand, ExpandAll, Traverse, AppendVertices, BiExpand, BiTraverse
   - `set_ops.rs`: Union, Minus, Intersect, WcoIntersect
   - `assign.rs`: Assign
   - `control_flow.rs`: Select, Loop, BeginTransaction..Argument
   - `unwind.rs`: Unwind
   - `flat_leaf.rs`: Fulltext*, Vector*, Remove, DataCollect, Materialize, 默认分支
4. mod.rs 中的 match arm 改为调用对应子模块的函数
5. 测试保持在 `mod.rs` 的 `#[cfg(test)] mod tests` 中（或按类别拆到各文件）
6. 运行 `cargo fmt` + `cargo test --lib` 验证

## 4. 验证

- `cargo fmt` 确保格式正确
- `cargo test --lib -p graphdb-query` 确保所有原有测试通过
- `cargo clippy -p graphdb-query` 确保无新警告
