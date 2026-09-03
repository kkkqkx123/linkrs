# 因子化模型最终架构设计

## 1. 背景与目标

LinkRS 查询引擎当前为扁平模型：各算子输出 `DataChunk`，所有列在一个批次中物化。图查询 `MATCH (a:Person)-[:Knows]->(b:Person)` 中 `Extend` 为每个 `a` 产生 `N` 个 `b`，扁平模型物化 `N×M` 行。

因子化通过嵌套存储避免物化，仅在需要时展开。参考 `ref/ladybug/src/planner/operator/schema.h` 与 `ref/ladybug/src/processor/result/factorized_table.h`。

**目标**
1. 支持嵌套/扁平混合，避免中间结果物化
2. 与现有 `DataChunk`/`SlotLayout` 执行模型兼容
3. 优化器先去因子化简化优化，再重插入因子化恢复效率
4. 与 `Logical/Physical` 分离及 `WCO Intersect` 共存
5. 单一编译产物，无条件编译分支

**最终决策：不回滚**。`296cdde6` 及其后续增量方向正确，仅存在 ID 单源与分区链等局部缺陷，修复成本 1 周，回滚重做成本 >3 周。因子化为常驻能力，通过 `FactorizationRewriter::enabled` 运行时关闭，不引入 Cargo feature。

## 2. 总体架构

```
graphdb-core::types::expr
  ExpressionId + ExpressionMeta + ContextualExpression{ id, Arc<ExpressionAnalysisContext> }  ← ID 单源

Planner 层  crates/graphdb-query/src/planning/plan/factorization.rs
  FactorizedSchema + FactorizationGroup  (≤1 unflat 不变式)
  FactorizedSchemaCompute for LogicalNodeEnum  (bottom-up child_schemas)
  LogicalFlattenNode{ group_pos }  logical/logical_nodes/flatten.rs

Optimizer 层  crates/graphdb-query/src/optimizer/
  1  RemoveFactorizationRewriter    engine.rs: apply_remove_factorization
  2  Logical Heuristic              BatchOptimizer on LogicalNodeEnum
  3  CBO on LogicalNodeEnum         join_order / index / agg
  4  FactorizationRewriter          engine.rs: apply_factorization  (FlattenAll / FlattenAllButOne)
  5  PhysicalMapping                planning/physical_planner.rs: convert_logical_to_physical
  6  Physical Heuristic + Partitioning  plan/arena_builder/partition.rs

Executor 层  crates/graphdb-query/src/executor/streaming/
  DataChunk + SlotLayout + chunk/*              ← 向量化批
  FactorizedTable / FactorizedTableUtils        ← 因子化行存储，常驻
  UnaryOperatorKind::Flatten{group_pos} → flatten_next_inner  operators/flatten.rs  SelectionVector 单行视图
```

**交互**：`LogicalNodeEnum::Flatten` → `PlanNodeEnum::Flatten` → `UnarySpec::Flatten` → `StreamingExecutor::Unary` → `flatten_next_inner`。`FactorizedTable` 通过 `factorized_table_utils::create_ftable_schema` 桥接 `FactorizedSchema` → `FactorizedTableSchema`，常驻编译，无 `#[cfg]` 分支。

**WCO 兼容** `docs/plan/wco_join.md:444`：`LogicalIntersect{ intersect_node_id, key_node_ids, inputs }` 新建单 unflat 组承载各 build 侧 payload，`FactorizationRewriter` 对其 probe/build 侧分别走 `FlattenAll`，代价模型与 `Binary Join` 择优。

## 3. 模块归属与集成边界

严格遵循 `crates/` DAG `metrics → core → config → fulltext → sync → transaction → storage → query → api → server` `AGENTS.md`。

| 层 | 目录 | Owner | 职责 | 禁止依赖 |
|---|---|---|---|---|
| Core 类型 | `crates/graphdb-core/src/types/expr/expression_context.rs` `contextual.rs` `expression.rs` | core | `ExpressionId` 唯一分配 `register_expression`，`ContextualExpression` 透传 | 不依赖 query |
| Schema 定义 | `crates/graphdb-query/src/planning/plan/factorization.rs` | planning/plan | `FGroupPos` `FactorizationGroup` `FactorizedSchema` `SchemaUtils` `validate_at_most_one_unflat` | 不依赖 executor |
| Schema 计算 | `crates/graphdb-query/src/planning/plan/factorization_compute.rs` | planning/plan | `impl FactorizedSchemaCompute for LogicalNodeEnum` 各算子规则，`compute_flat_schema = compute_factorized_schema(flat_children).flatten_all()` | 只读 `child_schemas`，不二次 `clone.compute(&[])` |
| 逻辑 Flatten | `crates/graphdb-query/src/planning/plan/logical/logical_nodes/flatten.rs` | planning/logical | `LogicalFlattenNode{ id, group_pos, input, deps }` `LogicalSingleInputNode` | 纯数据 |
| 物理 Flatten | `crates/graphdb-query/src/planning/plan/core/nodes/operation/flatten_node.rs` | planning/core | `FlattenNode{ group_pos }` `define_plan_node_with_deps!` | 纯数据 |
| 物理映射 | `crates/graphdb-query/src/planning/physical_planner.rs` | planning | `convert_logical_to_physical:56臂` 1:1 映射，含 `LogicalFlatten→FlattenNode` （当前为 splice 实现，见 engine.rs:501） | 不做代价决策 |
| 反向映射 | `crates/graphdb-query/src/planning/plan/logical/conversion.rs` | planning | `convert_plan: PlanNodeEnum→LogicalNodeEnum` 兜底，仅 `logical_plan.is_none()` legacy 路径 | 与正向对称 |
| 去因子化 | `crates/graphdb-query/src/optimizer/factorization/remove_factorization_rewriter.rs` | optimizer | `RemoveFactorizationRewriter{ rewrite/has_flatten/visit_operator_replace }` | 只改逻辑树 |
| 重因子化 | `crates/graphdb-query/src/optimizer/factorization/factorization_rewriter.rs` | optimizer | `FactorizationRewriter::visit_operator→FactorizedSchema` 自底上传递，`append_flattens` | 不碰物理树 |
| 决策 | `crates/graphdb-query/src/optimizer/factorization/flatten_resolver.rs` | optimizer | `FlattenAll / FlattenAllButOne::get_groups_pos_to_flatten_*` | 纯计算 |
| 依赖分析 | `crates/graphdb-query/src/optimizer/factorization/group_dependency_analyzer.rs` | optimizer | `GroupDependencyAnalyzer{ visit/visit_expression }` 覆盖 `Variable/Property/Binary/Function/Aggregate/List/Map/Case/Subscript` 及 `list_* lambda` `required_flat` | 不改 schema |
| 执行 Flatten | `crates/graphdb-query/src/executor/streaming/operators/flatten.rs` `operators/unary_operator.rs` | execution | `flatten_next_inner + prepare_flatten_buffer` `UnaryOperatorKind::Flatten{current_idx,size_to_flatten,saved_sel_vector,buffered_chunk}` 单例路径 | — |
| 执行存储 | `crates/graphdb-query/src/executor/streaming/factorized_table.rs` `factorized_table_utils.rs` | execution | `FactorizedTableSchema/ColumnSchema/OverflowValue/DataBlockCollection/FactorizedTable::append/scan/merge/flat_rows` `create_ftable_schema*` 常驻，无 cfg | — |
| 编排 | `crates/graphdb-query/src/optimizer/engine.rs` `executor/streaming/plan/arena_builder/partition.rs` `specs/graph.rs` `specs/*` | pipeline | 流水线顺序、分区链、spec 生成 | 逻辑/物理不混淆 |
| 说明 | `crates/graphdb-query/src/planning/plan/explain/description.rs` `planning/statements/dql/explain_planner.rs` | planning | `EXPLAIN` 透传 `Flatten(group=pos)` | — |

## 4. 核心不变式与数据结构

### 4.1 类型

```rust
pub type FGroupPos = u32;
pub const INVALID_F_GROUP_POS: FGroupPos = u32::MAX;

pub struct FactorizationGroup {
    flat: bool,
    single_state: bool,
    cardinality_multiplier: f64,
    expressions: Vec<ExpressionId>,
    expression_id_to_pos: HashMap<ExpressionId, usize>,
    expression_name_to_pos: HashMap<String, usize>,
}

pub struct FactorizedSchema {
    groups: Vec<FactorizationGroup>,
    expression_to_group: HashMap<ExpressionId, FGroupPos>,
    expression_name_to_group: HashMap<String, FGroupPos>,
    expressions_in_scope: Vec<ExpressionId>,
}

pub struct SchemaUtils; // get_leading_group_pos / validate_at_most_one_unflat

pub trait FactorizedSchemaCompute {
    fn compute_factorized_schema(&mut self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema;
    fn compute_flat_schema(&mut self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema;
}
```

**不变式** `factorization.rs:385`
- 任何时刻最多一个 `unflat` 组，`validate_at_most_one_unflat` 在各分支末尾校验。
- 每个 `ExpressionId` 恰属一组，`insert_to_group_and_scope` 与 `insert_to_group_and_scope_may_repeat` 维护。
- `Flatten` 不可逆，`flatten_group(pos)` 置 `flat=true`。

### 4.2 Flatten 节点

```rust
// logical
pub struct LogicalFlattenNode { pub id: i64, pub group_pos: FGroupPos, pub input: Option<Box<LogicalNodeEnum>>, pub deps: Vec<LogicalNodeEnum>, pub output_var: Option<String>, pub col_names: Vec<String>, pub column_types: Vec<DataType> }
// physical
define_plan_node_with_deps!{ pub struct FlattenNode{ group_pos: u32, } enum: Flatten input: SingleInputNode }
```

### 4.3 FactorizedTable

```rust
pub struct ColumnSchema { pub is_unflat: bool, pub group_id: FGroupPos, pub num_bytes: u32, pub may_contain_nulls: bool }
pub struct FactorizedTableSchema { pub columns: Vec<ColumnSchema>, pub num_bytes_for_data_per_tuple: u32, pub num_bytes_for_null_map_per_tuple: u32, pub num_bytes_per_tuple: u32, pub col_offsets: Vec<u32> }
pub struct OverflowValue { pub num_elements: u64, pub values: Vec<Value> }
pub struct FactorizedTable { pub schema: FactorizedTableSchema, pub num_tuples: u64, pub flat_tuples: Vec<Vec<Value>>, pub overflow_tuples: Vec<HashMap<usize, OverflowValue>>, pub null_maps: Vec<Vec<bool>>, pub flat_data_blocks: DataBlockCollection, pub unflat_overflow_blocks: DataBlockCollection, pub in_mem_overflow_buffer: InMemOverflowBuffer }
impl FactorizedTable { pub fn append(&mut self, vectors: &[ValueVector]); pub fn scan(&self, vectors: &mut [ValueVector], start: usize, count: usize); pub fn merge(&mut self, other: Self); pub fn flat_rows(&self) -> Vec<Vec<Value>>; pub fn to_data_chunk(&self, start: usize, count: usize) -> DataChunk; }
```

依赖仅 `std::collections::HashMap` + `graphdb_core::Value`，无额外 crate，二进制增量 <10KB，未实例化时零运行时成本。开关为 `FactorizationRewriter::enabled` 运行时控制，不引入 Cargo feature。

## 5. 算子 Schema 规则

| 算子 | 规则 | 依赖实现 |
|---|---|---|
| ScanVertices/ScanEdges | 单 flat 组，`single_state` 若主键扫描；有 `expression: Option<ContextualExpression>` 则 `insert(resolve_id(expr))`，否则空 flat 组 | `factorization_compute.rs:23` |
| GetVertices | 拷贝 child，若空则单 flat 组 | `factorization_compute.rs:53` |
| GetNeighbors/Traverse/Expand*/Bi* | 拷贝 child，`flatten` 既有 unflat，再 `create_group()` 新建 unflat 空组（邻居列由 `DataChunk` `col_names` 解析，不预插桩） | `factorization_compute.rs:63,184` |
| Flatten | `flatten_group(group_pos)` | `factorization_compute.rs:95` |
| Project | 对每列 `GroupDependencyAnalyzer::with_expr_store` 求 `dependent/required_flat`，`required_flat` 先 flatten，再 `get_leading_group_pos` 决定 alias 落组 | `factorization_compute.rs:104` |
| Filter | 依赖组 `FlattenAll` | `factorization_compute.rs:170` |
| Aggregate | 新 flat 单组，`group_key_exprs: Vec<ContextualExpression>` 逐 `resolve_id` 入组，`group_keys.is_empty()` 全局聚合不 flatten，`child` 回落单 flat | `factorization_compute.rs:177` `operation.rs:68` |
| Sort/TopN/Limit | `Sort`/`TopN` 依赖 `order keys` 时 `FlattenAllButOne`，`Limit` 无 flatten（显式） | `factorization_compute.rs: Range 190` |
| Dedup/Window | `Dedup` `FlattenAll(groups_in_scope)`，`Window` 对 `partition_by/order_by` `FlattenAllButOne` | 同上 |
| Inner/Left/Right/Cross/FullOuter/Semi Join | `merge_groups_from(right)` 重映射 `expression_to_group`，超一 unflat 则保留首个 unflat 其余 flatten；`probe` 侧 `FlattenAll(key_groups)`，`build` 侧 `FlattenAllButOne(key_groups)`，`Right/Cross/Semi` 镜像 | `factorization_compute.rs:144` `factorization_rewriter.rs:224` |
| Intersect (WCO) | 拷贝 probe，新 unflat 组放 `intersect_node_id` + 各 build 侧 payload | `wco_join.md:471` |
| Fulltext/Vector* | 叶 flat 组；全图搜索后续建 unflat 组（当前 pass-through） | `factorization_compute.rs:335` |
| Union/Minus/Intersect/Unwind 等 | `child_schemas[0].clone()` 或 `child_schemas.first()`，`Unwind` 单输入 | `factorization_compute.rs:200+` |

## 6. 优化器集成

执行顺序 `engine.rs:410` 对应 `docs/plan/logical_plan_boundary.md:406` 目标五段：

```
1  RemoveFactorizationRewriter       flatten_all + strip LogicalFlatten
2  Logical Heuristic                  Predicate/Projection/Limit/SortElimination/TopN/MergeConsecutiveOps on LogicalNodeEnum
3  CBO on LogicalNodeEnum             join_order / index_selection / aggregate_strategy / subquery unnest
4  FactorizationRewriter              per-type getGroupsPosToFlatten → append_flatten
5  PhysicalMapping                    LogicalNodeEnum → PlanNodeEnum  (IndexScan 选择在此)
6  Physical Heuristic + Partitioning  内存布局/并行化 + 决定 local/global 链
```

`FactorizationRewriter` 算法 `factorization_rewriter.cpp`：
```
visitOperator(op):
  for child in children: visitOperator(child) // 自底向上，返回 FactorizedSchema
  visitOperatorSwitch(op) // FlattenAll / FlattenAllButOne per type
  op.computeFactorizedSchema(child_schemas)
  append_flattens(child, groups_to_flatten, child_schema) // sorted 去重
```
`has_flatten` 与 `visit_operator_replace` 均需递归 `Loop.body/Select.if/else/Apply.left/right/Assign.deps` 等全部 `LogicalNodeEnum` 变体，`Loop` 体保留结构但校验深层。

## 7. 执行层

### 7.1 向量化批 DataChunk

```
LogicalFlatten{group_pos} --physical_planner:1077--> FlattenNode{group_pos}
  --specs/graph.rs:224--> UnarySpec::Flatten{group_pos}
  --assembler/conversion--> StreamingExecutor::Unary
  --unary_operator.rs:726--> flatten_next_inner  flatten.rs:16
    saved_sel_vector = visible_indices(chunk)
    take_selection → set flat mode
    per sel_pos emit DataChunk::new_with_layout(vec![row[sel_pos]], layout) + gather_typed_column
```

约束 `chunk/selection.rs:58` `visible_indices/take_selection/materialize_selection`，`partition.rs:972` `Flatten` 归 local 链，`partition.rs:213` `build_partition_local_fragments` 需显式 `Flatten` 分支，`group_pos` 透传不参与布局校验（校验在 EXPLAIN）。

### 7.2 因子化存储 FactorizedTable

`FactorizedTable` / `FactorizedTableUtils` 常驻编译，无 `#[cfg]`。`flat_rows:590` 为跨积展开基准，`to_data_chunk` 为 `FactorizedTable → DataChunk` 桥接，`scan:422` 要求 unflat 时 `count==1`，`DataBlockCollection` 按 `schema.num_bytes_per_tuple` 预分配，`InMemOverflowBuffer` 处理变长。

运行时是否使用由上游 `FactorizedSchema` 决定，非 Cargo feature。关闭因子化仅需 `FactorizationRewriter::disabled()` 使所有组保持 flat，此时 `FactorizedTable` 退化为单 flat 块，与 `DataChunk` 等价。

## 8. 桥接 Schema → FactorizedTableSchema

```rust
pub fn create_ftable_schema(expressions: &[ExpressionId], schema: &FactorizedSchema) -> FactorizedTableSchema {
    for eid in expressions {
        let pos = schema.get_group_pos(eid).expect("in scope");
        let flat = schema.get_group(pos).map(|g| g.is_flat()).unwrap_or(true);
        columns.push(ColumnSchema{ is_unflat: !flat, group_id: pos, num_bytes: if flat { row_layout_size(typ) } else { size_of::<OverflowValue>() as u32 }, may_contain_nulls: true });
    }
}
pub fn create_ftable_schema_for_logical(node: &mut LogicalNodeEnum, child_schemas: &[FactorizedSchema]) -> FactorizedTableSchema;
// 优先 get_group_pos(&eid)，fallback get_group_pos_by_name 仅 #[cfg(test)]
```

常驻调用，不由 feature 门控。

## 9. 数据流示例

`MATCH (a:Person)-[:Knows]->(b:Person) RETURN a.name, b.name`

```
Planning  Scan(a) Group0 flat {a.id,a.name}
          GetNeighbors(a->b) Group0 flat {a} + Group1 unflat {b}
          Project  dependent {a.name:0,b.name:1} leading=0 无需 flatten

Rewriter  FlattenAllButOne → 无 flatten 插入

Execution Scan a → DataChunk[a1,a2]
          GetNeighbors → FactorizedTable Row0: a1 + overflow[b1,b2], Row1: a2 + overflow[b3]
                        或 DataChunk 扁平批（两者镜像，FactorizedTable 节省物化）
          Flatten(next) → SelectionVector 逐行暴露 (a1,b1)(a1,b2)(a2,b3)
          Project → 输出 (a.name,b.name)
```

FactorizedTable 布局：
```
Row0: a1.name=flat  b=overflow[b1,b2]  → flat_rows 展开 (a1,b1)(a1,b2)
Row1: a2.name=flat  b=overflow[b3]     → flat_rows 展开 (a2,b3)
```

## 10. 边界与集成点清单

| 集成点 | 文件:行 | 目标 |
|---|---|---|
| ID 单源透传 | `planning/plan/factorization_compute.rs:11` `planning/statements/*` `plan/logical/conversion.rs:249` | 复用 `BoundStatement` 的 `ExpressionAnalysisContext` 透传 `ContextualExpression`，禁止 `DefaultHasher` 与现场 `Arc::new(Context)`；`transform` 遗留路径例外需 `// TODO(plan_bound)` 标记，`plan_bound` 路径必须透传 |
| Schema 全覆盖 | `factorization_compute.rs:335` | 叶搜索类按 `Scan` 建组，`Unwind/Union` 显式规则 |
| 反向映射 | `plan/logical/conversion.rs:412` | 补 `Flatten(group_pos)` 及遍历/控制流臂，与 `physical_planner:34` 对称 |
| 流水线顺序 | `optimizer/engine.rs:943,956,487,520,450` | 按 `logical_plan_boundary.md:406` 五段重排，`Remove→LogicalHeuristic→CBO→Factorization→PhysicalMapping→PhysicalHeuristic→Partitioning` |
| 分区链 | `plan/arena_builder/partition.rs:969,213,820` | 增 `Flatten` local/global 臂 `push_unary_op(build_flatten_spec)` |
| 执行 | `operators/unary_operator.rs:726` `operators/flatten.rs:16` | 单例 `flatten_next_inner`，同时处理 `columns` 与 `typed_columns` |
| 存储 | `executor/streaming/factorized_table.rs:1` `factorized_table_utils.rs:1` | 常驻，无 cfg，`Cargo.toml` 无 `factorization` feature |
| WCO | `planning/join_order/*` `logical_node_enum.rs` | `LogicalIntersect` 按 `wco_join.md:471` 规则参与因子化与代价择优 |

## 11. 验证

- 单元：`cargo test --lib -- --nocapture` 覆盖 `FactorizedSchema` 各算子 `validate_at_most_one_unflat`；`cargo test flatten -- --nocapture` 覆盖 `single_batch/selection/empty/typed_columns_preserved/reset`；`cargo test --lib factorized_table` 覆盖 `append/scan/merge/flat_rows`
- 集成：`tests/factorization_e2e.rs` 三例 `EXPLAIN MATCH (a)-[:Knows]->(b) RETURN / WHERE b.age>30 RETURN count(*) / UNION` 对比 `FactorizationRewriter::disabled()` 结果一致，`EXPLAIN` 快照含 `Flatten(group=1)`
- 门禁：`grep -rn DefaultHasher crates/graphdb-query/src/planning/plan/factorization* crates/graphdb-query/src/optimizer/factorization/` 零命中；`grep -rn FlattenOperator` 零命中（单路径）
- 回退：`FactorizationRewriter::disabled()` 一键扁平，查询语义不变，无需重新编译

## 12. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 复杂度增加 | 五段流水线每段独立测试，Phase1-3 稳定后 Phase4-5 |
| Unflat 在小数据集劣于扁平 | `disabled()` 运行时回退 |
| SelectionVector 操控错误 | 对齐 `SelVectorOverWriter`，`chunk/selection.rs` 契约单测 |
| 与现有算子 flat 假设不兼容 | `Remove` 后 `compute_flat_schema` 提供扁平视图 |
| 编译产物增大 | 无新增依赖，增量 <10KB，零运行时成本 |

## 附录

- Ladybug 引用：`ref/ladybug/src/include/planner/operator/schema.h` `ref/ladybug/src/processor/result/factorized_table.*` `ref/ladybug/src/optimizer/factorization_rewriter.cpp` `ref/ladybug/src/planner/operator/factorization/flatten_resolver.cpp` `ref/ladybug/src/processor/operator/flatten.cpp`
- 交付：`planning/plan/factorization.rs:630` `factorization_compute.rs:545` `logical/logical_nodes/flatten.rs:117` `core/nodes/operation/flatten_node.rs:31` `executor/streaming/factorized_table.rs:781` `factorized_table_utils.rs:249` `operators/flatten.rs:240` `operators/unary_operator.rs:1013` `optimizer/factorization/*:4 files` `physical_planner.rs:1093` `arena_builder/specs/graph.rs:224` `partition.rs:969`
