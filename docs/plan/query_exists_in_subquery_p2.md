# P2 相关子查询逐行重执行（CorrelatedApply）设计详案

> 状态：设计方案（2026-08-13）。本文是
> `docs/plan/query_exists_in_subquery_impl.md` §4 所述 P2 阶段
> （键提取失败的兜底：相关子查询逐行重执行）的**独立实现级设计文档**，
> 给出可直接实施的改动清单、数据结构与执行流程。
> P0（双侧键重构）与 P1（binder 放行 + WHERE 合取位置转换）已完成，
> 本文基于当前 main 分支的实际代码状态编写。

## 0. 结论摘要

| 主题 | 决策 |
|------|------|
| 新算子 | `ApplyOperator::CorrelatedApply` 变体，配合 `ApplySpec::CorrelatedApply`（见 D1） |
| 新计划节点 | 物理 `CorrelatedApplyNode` + 逻辑镜像 `LogicalCorrelatedApplyNode` + `PlanNodeEnum::CorrelatedApply` 变体 |
| 右子树载体 | arena 构建期将右子树递归构建为嵌套 `Arc<PhysicalPlan>`，缓存于 `ApplySpec` |
| 输入契约 | `CorrelatedApply` 为**一元输入**（仅左子树）；右子树按行重建，不进外部分片图 |
| Argument 布局 | `SourceSpec::Argument` 从单元变体改为携带 `col_names`，元数据布局 = 外层布局（见 D4） |
| 参数绑定 | 算子缓存「剥离 parameters/parameter_frame」的 `QueryBindings`，重建时逐行调用 `PhysicalPlanMaterializer::materialize`（见 D3） |
| 相关性路由 | `exists_planner::extract_keys` 不再对非等值相关报错，改为产出「相关残差条件」→ 走 CorrelatedApply；简单等值仍走 PatternApply |
| 永不 unnest | `is_simple_subquery_shape` 不含 Argument/CrossJoin，天然避开；优化器分析对 CorrelatedApply 仅需增 match arm（见 §4.9） |

## 1. 背景与触发条件

### 1.1 现状

`exists_planner.rs` 的 `extract_keys`（`exists_planner.rs:372-425`）只接受
「一侧恰引用一个子查询变量、另一侧不引用」的等值键。任何仍引用外层
变量的残差条件（非等值 `p.age > t.age`、多变量 `p.a + p.b = t.x`、
表达式形状不一致等）都会在规划期返回精确错误：

```
Correlated subquery condition `...` references outer variable(s) ...;
only equality correlation is supported (P2)
```

### 1.2 触发示例（本次要打通）

```cypher
MATCH (t:Person)
WHERE EXISTS { MATCH (p:Person) WHERE p.age > t.age }
RETURN t.name

MATCH (t:Person)
WHERE NOT EXISTS { MATCH (p:Person) WHERE p.age > t.age }

MATCH (t:Person)
WHERE t.age IN { MATCH (p:Person) WHERE p.age > t.age RETURN p.age }
```

共同点：相关条件无法拆成 PatternApply 的 `hash_keys`/`probe_keys` 双侧键，
只能「对每个外层行，把外层变量绑定进去，再重执行子查询」。

### 1.3 语义定义

对每个外层行 `row`（布局 `L`）：

```
correlated_apply(row) =
    if is_anti:  NOT EXISTS( 对 row 重执行右子树后，有任意一行满足全部相关条件 )
    else:         EXISTS(  对 row 重执行右子树后，有任意一行满足全部相关条件 )
```

EXISTS / IN / NOT EXISTS / NOT IN 统一为存在性判定（semi/anti），
与 `ApplyOperator::PatternApply` 的 `anti` 语义一致。

## 2. 计划形状

外层计划（左子树）之上叠 `CorrelatedApplyNode`；右子树是一个**自包含的
子计划树**，根为 `Argument` 源头：

```text
CorrelatedApplyNode (left=外层计划, right=右子树, anti=NOT?)
    ├─ 左子树 = 外层计划（每行触发一次右子树重执行）
    └─ 右子树 =
        Filter(correlated_conditions)            ← 引用外层变量的残差条件
            └─ CrossJoin
                ├─ Argument(col_names = 外层布局)  ← 运行时读取 correlation frame
                └─ 子查询 pattern 计划（Scan → 子查询局部 Filter → …）
```

要点：

- **Argument 只携带外层布局（不携带外层值）**；外层行值经
  `runtime.set_correlation_frame(left_layout, left_row)` 注入，运行期
  由 Argument 源头一次性取出（`source_operator.rs:344-359`）。
- 子查询 pattern 计划中**不得**再包含引用外层变量的条件——相关条件
  全部上移到 CrossJoin 之上的 Filter。子查询局部条件（仅引用子查询
  变量）留在 pattern 计划内部（现状逻辑不变）。
- `return_expr`/可选 Project 对存在性判定非必需；IN 合成的
  `left_expr = return_expr` 等值条件作为相关/残差条件一并进入 Filter，
  存在性即 `IN` 判定（semi/anti）。NULL 语义沿用现状声明（不在范围）。

## 3. 执行流程（`ApplyOperator::CorrelatedApply::next`）

```
对每个外层 chunk：
  for left_row in chunk.rows:
    1. rt.set_correlation_frame(left_chunk.layout.clone(), left_row.clone())
    2. 重建右子树 executor（fresh open）：
         (mut sub_exec, _) = PhysicalPlanMaterializer::materialize(sub_plan, stripped_bindings)?
         sub_exec.set_chunk_size(...); sub_exec.set_runtime(Some(rt.clone())); sub_exec.open()?
    3. 拉尽 sub_exec，任一行非空 ⇒ exists = true（提前 break）
    4. sub_exec.close()
    5. if exists != anti: 输出 left_row（行级输出，布局 = 左子树布局）
```

- **帧一次性语义**：`take_correlation_frame`（`runtime.rs:1153`）是
  `Mutex::take`；右子树 root 恰为单个 Argument 源头，单次消费正确。
  每次重建前 `set_correlation_frame` 覆盖上一帧，无需显式 clear。
- **共享 runtime**：`sub_exec.set_runtime(Some(rt.clone()))` 让右子树
  读取**父执行器的同一 runtime**（帧、storage、parameter_values 都从
  父 runtime 解析），并确保子计划内部不再自建独立 runtime。
- **性能**：逐行重建为 O(行数 × 子树成本)，正确性优先。按 chunk 重建 +
  算子状态重置（rewind）协议为文档化后续项（见 §7 风险表），本次不实施。

## 4. 改动点详表

### 4.1 规划层：新计划节点

**物理节点** `crates/graphdb-query/src/query/planning/plan/core/nodes/graph_operations/graph_operations_node.rs`
仿 `PatternApplyNode`（`graph_operations_node.rs:493-604`）新增：

```rust
pub struct CorrelatedApplyNode {
    id: i64,
    left_input: Box<PlanNodeEnum>,   // 外层计划
    right_input: Box<PlanNodeEnum>,  // Argument → CrossJoin → Filter 右子树
    deps: Vec<PlanNodeEnum>,
    is_anti_predicate: bool,
    output_var: Option<String>,
    col_names: Vec<String>,          // = left_input.col_names()，半连接直通
    column_types: Vec<DataType>,
}
// new(left_input, right_input, is_anti_predicate)
// 访问器：left_input() / right_input() / is_anti_predicate() / id() / col_names() …
```

**逻辑镜像** `crates/graphdb-query/src/query/planning/plan/logical/logical_nodes/graph_ops.rs`
用既有 `define_logical_join_node!` 宏（`graph_ops.rs:53-58`）仿
`LogicalPatternApplyNode` 新增 `LogicalCorrelatedApplyNode { left, right,
deps, is_anti_predicate, … }`，`deps = [left, right]`。

**枚举变体** `crates/graphdb-query/src/query/planning/plan/core/nodes/base/plan_node_enum.rs`
在 `PatternApply(RollUpApply)` 附近（`plan_node_enum.rs:182-183`）增加
`CorrelatedApply(CorrelatedApplyNode)`，并同步以下宏列表：

| 列表 | 位置（当前 PatternApply 所在行） | 新增项 |
|------|----------------------------------|--------|
| `define_enum_is_methods!` | `:291` `(PatternApply, is_pattern_apply)` | `(CorrelatedApply, is_correlated_apply)` |
| `as_*` | `:388` `(PatternApply, as_pattern_apply, PatternApplyNode)` | `(CorrelatedApply, as_correlated_apply, CorrelatedApplyNode)` |
| `as_*_mut` | `:484` | `(CorrelatedApply, as_correlated_apply_mut, CorrelatedApplyNode)` |
| 节点名 | `:584` / `:782` `(PatternApply, "PatternApply")` | `(CorrelatedApply, "CorrelatedApply")` |
| 类别 | `:683` `(PatternApply, DataProcessing)` | `(CorrelatedApply, DataProcessing)` |

### 4.2 规划层：exists_planner 路由

`crates/graphdb-query/src/query/planning/statements/clauses/exists_planner.rs`：

1. **`extract_keys` 改签名**（`exists_planner.rs:372-425`）：删除对
   外层变量残差条件的报错分支，改为照常返回 `(hash, probe, residual)`；
   由调用方按 `inner_vars` 把 `residual` 再拆成
   `inner_residual`（仅子查询变量，进子查询局部 Filter，现状不变）与
   `correlated_residual`（引用外层变量 → P2 路径）。
2. **`plan_subquery` 增加参数** `outer_col_names: &[String]`
   （`exists_planner.rs:144-252`）。当 `correlated_residual` 非空：
   - 构建右子树：
     ```rust
     let mut arg = ArgumentNode::new(next_node_id(), "_correlated_apply");
     arg.set_col_names(outer_col_names.to_vec());
     let cross = CrossJoinNode::new(arg.into_enum(), sub_plan.root().clone().unwrap())?;
     let filter = FilterNode::new(cross.into_enum(),
         to_contextual(and_join(&correlated_residual), &expr_context))?;
     ```
   - 返回 `PlannedSubquery { plan: 右子树, hash_keys: vec![], probe_keys: vec![], correlated: true }`
     （`PlannedSubquery` 增加 `correlated: bool` 字段，见 4.3）。
   - 否则走现状 PatternApply 路径（`correlated: false`）。
   - 嵌套 EXISTS 递归时以 `sub_plan.root().col_names()` 作为
     `outer_col_names` 传入（嵌套相关自然支持）。
3. **新增 `wrap_correlated_apply`**（仿 `wrap_pattern_apply`，
   `exists_planner.rs:258-301`）：`CorrelatedApplyNode::new(left_root,
   right_root, anti)`，物理 + 逻辑镜像同步构建。
4. **`where_clause_planner.rs::transform_clause`**（`where_clause_planner.rs:68-72`）：
   `input_plan.root().col_names()` 传入 `plan_subquery`；按
   `planned.correlated` 分支调用 `wrap_correlated_apply` 或
   `wrap_pattern_apply`。

### 4.3 spec 层：`ApplySpec::CorrelatedApply`

`crates/graphdb-query/src/query/executor/streaming/operators/spec.rs:494-508`：

```rust
pub enum ApplySpec {
    Apply { kind, correlated_columns },
    PatternApply { hash_keys, probe_keys, anti },
    CorrelatedApply {
        sub_plan: Arc<PhysicalPlan>,   // 右子树，逐行重建（见 D1）
        anti: bool,
    },
    RollUpApply { compare_columns, collect_column },
}
```

`PlannedSubquery` 的 `correlated: bool` 仅为规划层内部路由标记，不进 spec。

### 4.4 spec 层：`SourceSpec::Argument` 携带外层布局

- `specs.rs:83`：`build_source_spec` 的 Argument 分支改为
  `SourceSpec::Argument { col_names: node.col_names().to_vec() }`。
- `metadata.rs:626`：`source_output_layout` 改为
  `SourceSpec::Argument { col_names } => SlotLayout::from_names(col_names)`。
- `metadata.rs:684`：`source_explain_name` 相应加 `..`。
- `source_operator.rs:260`：`from_spec` 匹配加字段（`..` 忽略即可）；
  运行期 `next`（`source_operator.rs:344-359`）**保持**用
  `base.output_layout` 发射 chunk——此时 `base.output_layout` 即为外层
  布局，与 correlation frame 中 `(left_layout, left_row)` 的槽位序一致
  （见 D4）。

### 4.5 arena builder：conversion + metadata

**`assembler/conversion.rs`** 新增 `PlanNodeEnum::CorrelatedApply` 分支
（仿 PatternApply 分支 `conversion.rs:812-842`，但右子树**不**转成外部
fragment）：

```rust
PlanNodeEnum::CorrelatedApply(ca_node) => {
    let (left_fid, _) = Self::convert_node(ca_node.left_input(), …)?;
    let mut sub_ctx = PhysicalPlanBuildContext::from_execution_context(exec_ctx);
    sub_ctx.partition_spec = None;                 // 子计划不做分区/并行
    let sub_plan = Arc::new(PhysicalPlanBuilder::build(
        ca_node.right_input(), &mut sub_ctx, exec_ctx)?);
    let spec = ApplySpec::CorrelatedApply { sub_plan, anti: ca_node.is_anti_predicate() };
    Self::push_apply_op(…, left_fid, node.id(), spec)   // 一元 push（见下）
}
```

在 `assembler/fragment_ops.rs` 增加一元 Apply push 辅助
（仿 `push_global_unary_op`：仅挂一个 child fragment）。

**`metadata.rs`**：

- `populate_input_contracts`（`metadata.rs:139`）：对
  `OperatorKindSpec::Apply(ApplySpec::CorrelatedApply { .. })` 走
  `InputContract::UnaryInput`（仅左输入），不再归入 `BinaryInputs`。
- `infer_output_layout`：`CorrelatedApply` 与 PatternApply 相同，输出 =
  输入（外层）布局。
- `apply_explain_name`：`"CorrelatedApply"`。

### 4.6 materializer：一元输入 + 重建工厂

`crates/graphdb-query/src/query/executor/streaming/plan/materializer.rs:207-211`：

```rust
OperatorKindSpec::Apply(ApplySpec::CorrelatedApply { sub_plan, anti }) => {
    let left = take_unary_input(fragment.id, op_id, &mut inputs)?;
    let right = StreamingExecutor::Source(OperatorBase::new(0), SourceOperator::Start);
    let op = ApplyOperator::CorrelatedApply {
        sub_plan: sub_plan.clone(),
        bindings: stripped_bindings(bindings),   // 见 D3
        anti: *anti,
        right_rows: None,
        right_layout: None,
        memory_tracker: MemoryTracker::new(bindings.memory_budget.clone()),
    };
    StreamingExecutor::Apply(base, Box::new(left), Box::new(right), op)
}
```

- 占位右输入用 `SourceOperator::Start`（子计划执行时不会触达），
  `ApplyOperator::open/close` 对 CorrelatedApply 分支跳过右子树操作。
- `stripped_bindings`：克隆 `QueryBindings` 并清空 `parameters` /
  `parameter_frame`，其余（storage、memory_budget、chunk_size、
  space_name、query_id、fulltext_manager、vector_coordinator、事务
  scope 等）保留。

### 4.7 `ApplyOperator::CorrelatedApply` 运行时循环

`crates/graphdb-query/src/query/executor/streaming/operators/apply_operator.rs`：

1. 枚举新增变体 `CorrelatedApply { sub_plan: Arc<PhysicalPlan>,
   bindings: QueryBindings, anti: bool, right_rows: Option<Vec<Vec<Value>>>,
   right_layout: Option<Arc<SlotLayout>>, memory_tracker: MemoryTracker }`
   （`apply_operator.rs:14-37`；`right_rows/right_layout` 保留仅为
   `close` 统一处理，实际不使用）。
2. `from_spec`（`apply_operator.rs:40-75`）增加分支（`bindings` 在
   materializer 构造后通过 `with_bindings`/直接构造注入——从_spec 拿不到
   `QueryBindings`，故 materializer 用字面构造而非 from_spec，见 4.6）。
3. `next`（`apply_operator.rs:97-294`）新增分支，实现 §3 流程。
4. `open/close`（`apply_operator.rs:85-95,306-341`）：CorrelatedApply 在
   close 时清理 `sub_plan` 以外的缓冲（统一走既有 `right_rows.take()`
   分支即可，`sub_plan` 保留到算子 Drop）。

### 4.8 EXPLAIN

- `planning/plan/explain/describe_visitor.rs`：仿 `visit_pattern_apply`
  （`describe_visitor.rs:326-355`）新增 `visit_correlated_apply`，标注
  `anti`，并递归展示左右子树；在宏表（`describe_visitor.rs:287` 附近）
  登记 `impl_single_input_visit!` 或手写分支。
- `executor/explain/physical_plan_explain.rs`：`apply_explain_name` 返回
  `"CorrelatedApply"`。

### 4.9 新增 match 位核对清单（`cargo check` 穷举补齐）

`PlanNodeEnum` 新增变体后，以下文件若有穷举 match/visitor 需补 arm
（编译器会逐处指出，本清单为预扫描）：

- `planning/plan/core/nodes/base/plan_node_visitor.rs`
  `plan_node_traits_impl.rs` `plan_node_operations.rs` `plan_node_children.rs`
- `planning/plan/core/nodes/plan_node_factory.rs`（若为全节点枚举宏表）
- `planning/plan/execution_plan.rs`（BatchPlanAnalyzer/算子收集）
- `planning/physical_planner.rs`（逻辑 → 物理转换）
- `optimizer/analysis/batch.rs` `reference_count.rs` `fingerprint.rs`
- `optimizer/cost/child_accessor.rs`
- `optimizer/heuristic/decorrelation.rs` 与
  `optimizer/cost_based/subquery_unnesting.rs`：`UnnestSimplePatternApplyRule`
  只匹配 `is_pattern_apply`/`is_simple_subquery_shape` 子计划，**不会**匹配
  CorrelatedApply，天然不 unnest；但若其内部对未知节点 panic/默认分支
  需核对。

## 5. 关键设计决策

### D1 右子树载体：`ApplySpec` 持有 `Arc<PhysicalPlan>`

在 arena 构建期（有 `ExecutionContext`）用 `PhysicalPlanBuilder::build`
递归构建右子树为嵌套 `Arc<PhysicalPlan>`，缓存进 `ApplySpec::CorrelatedApply`。
理由：

- `PhysicalPlan` 属执行层类型（`plan/types.rs`），spec 层引用它**不违反**
  「spec 层独立于规划层节点类型」的既有约束（违禁的是 `PlanNodeEnum`）。
- 运行期 `ApplyOperator` 持 `sub_plan` + `bindings` 即可逐行调用
  `PhysicalPlanMaterializer::materialize` 重建执行器，无需再访问
  `ExecutionContext`。
- 备选（缓存 `PlanNodeEnum` + materializer 提供重建工厂）因 materializer
  缺 `ExecutionContext` 而不可行，且引入规划层类型到执行层的反向依赖。

### D2 一元输入契约

`CorrelatedApply` 的右子树内嵌于 spec，只保留左输入。因此：
`populate_input_contracts` 对其设 `UnaryInput`；conversion 用一元 push；
materializer 用 `take_unary_input` + `SourceOperator::Start` 占位右输入。
`StreamingExecutor::Apply` 结构体保持 left/right 双子节点不变，不新增
执行器变体。

### D3 参数剥离

右子树子计划由 `PhysicalPlanBuildContext::from_execution_context` 构建，
其 `parameter_schema` 为空。`validate_bindings`（`materializer.rs:310-347`）
对「bindings 有、schema 无」的参数会报 `Unknown parameter`。故算子缓存的
`QueryBindings` 需清空 `parameters`/`parameter_frame`；运行期参数仍经父
runtime 的 `parameter_values` 解析（子执行器 `set_runtime(Some(父))`
覆盖其自建 runtime），不受影响。

### D4 Argument 布局元数据 = 外层布局

`take_correlation_frame` 返回 `(left_layout, left_row)`，但运行期 Argument
用 `base.output_layout` 发射 chunk，而 CrossJoin/Filter 的输出布局由
元数据推断（`infer_output_layout`）。因此 **`SourceSpec::Argument` 的
`col_names` 必须 = 外层计划 `col_names`**，且规划期 `ArgumentNode::set_col_names`
用 `outer_col_names` 填充。该值与左子树运行时布局同名同序，槽位对齐。

### D5 永不 unnest

CorrelatedApply 由 exists_planner 直接构建（不经 PatternApply），
`is_simple_subquery_shape` 不含 Argument/CrossJoin，unnest 规则天然不匹配。
`BatchPlanAnalyzer` 对含外层变量的 Filter 做确定性分析无碍（仅需补
§4.9 的 match arm，不参与 unnest 判定）。

## 6. 测试与验证

### 6.1 单元测试

| 层级 | 用例 |
|------|------|
| `exists_planner` | `extract_keys` 对 `p.age > t.age` 返回 correlated_residual 而非报错；`IN` 合成等值落入相关残差；纯子查询局部条件仍走 residual |
| `exists_planner` | `wrap_correlated_apply` 构建的右子树结构断言：Argument(col_names=外层) → CrossJoin → Filter |
| `apply_operator` | `CorrelatedApply` 单测：semi（存在即出）/anti（不存在即出）；`take_correlation_frame` 单次消费正确；逐行重建结果与手算一致 |
| `metadata` | `source_output_layout(Argument { col_names })` 返回外层布局 |
| `materializer` | `stripped_bindings` 后嵌套 `materialize` 不报 Unknown parameter |

### 6.2 e2e（`tests/e2e/subquery.rs` 追加）

- 相关非等值 EXISTS：`WHERE EXISTS { MATCH (p:Person) WHERE p.age > t.age }`
- 相关非等值 NOT EXISTS
- 相关非等值 IN / NOT IN
- 多变量相关（`p.a + p.b > t.x`）
- 嵌套相关（相关 EXISTS 的子查询内再嵌套相关 EXISTS）
- EXPLAIN 断言 `CorrelatedApply` 节点及 `anti` 标注

### 6.3 回归

```shell
cargo test -p graphdb-query --lib                 # 基线 1450
cargo test --test integration_e2e subquery        # 基线 9
cargo test --test '*'                             # 全量 integration
cargo clippy -p graphdb-query --all-targets       # 零新增警告
cargo fmt
```

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| `take_correlation_frame` 一次性语义在嵌套重建下被提前消费 | 每次重建前 `set_correlation_frame` 覆盖，单 Argument 消费；若未来右子树多源头需扩展为计数式帧（文档化） |
| 子计划重建成本 O(行数 × 子树成本) | 正确性优先；优化项：按 chunk 重建 + 算子状态重置（rewind）协议，文档化暂不实施 |
| 嵌套 `materialize` 报 Unknown parameter | D3 参数剥离；单测锁定 |
| Argument 布局与左布局槽位不一致 | D4 强制 col_names = 外层 col_names；e2e 覆盖 |
| `PlanNodeEnum` 新增变体的漏网 match | §4.9 清单 + `cargo check`/`cargo clippy` 穷举 |
| `SourceSpec::Argument` 变体形状变更波及 `source_operator.rs`/`metadata.rs`/`state.rs` | 全部为同一仓库内小改动，`cargo check` 覆盖 |
| IN 的 NULL 语义 | 沿用 `Value::contains` 现状，概览文档已声明不在范围 |

## 8. 实施步骤（任务分解，可追踪/可交接）

> 顺序依赖：步骤 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9。
> 每个步骤包含三部分：**任务清单**（具体到文件与改动点）、**验证**（完成判据）、**状态**。
> 状态用 `[ ]`（未开始）/ `[x]`（完成）/ `[~]`（进行中）勾选，交接时按行核对。

### 进度总览

| 步骤 | 主题 | 涉及文件 | 状态 |
|------|------|----------|------|
| 0 | 基线确认 | —（命令） | [ ] |
| 1 | 新计划节点 + 枚举变体 + match 位 | 规划层 10 文件（见 8.1） | [ ] |
| 2 | Argument 布局元数据 | spec/specs/metadata/source_operator/state | [ ] |
| 3 | ApplySpec + arena 构建（一元 push + 嵌套子计划） | spec/conversion/fragment_ops/metadata | [x] |
| 4 | materializer（一元输入 + 参数剥离） + 算子骨架 | materializer/apply_operator | [x] |
| 5 | ApplyOperator 运行时循环 | apply_operator | [x] |
| 6 | exists_planner 相关路由 | exists_planner/where_clause_planner | [x] |
| 7 | EXPLAIN 双路径 | describe_visitor/physical_plan_explain | [x] |
| 8 | 单测 + e2e | 各测试文件 + tests/e2e/subquery.rs | [x] |
| 9 | 全量回归 + 文档收尾 | docs/plan/README.md | [ ] |

### 8.0 步骤 0：基线确认（前置，不改代码）

**任务清单**：记录当前验证基线，作为步骤 8/9 的对比基准。

```shell
cargo test -p graphdb-query --lib                 # 预期 1450 passed
cargo test --test integration_e2e subquery        # 预期 9 passed
cargo clippy -p graphdb-query --all-targets       # 预期零新增警告
```

**验证**：三条命令全部通过，并记录输出。若基线不符，先修复既有问题再开工。

**状态**：- [ ]

### 8.1 步骤 1：规划层新计划节点 + 枚举变体 + match 位补齐

> 对应设计：§2 计划形状、§4.1、§4.9、D1/D5。本步骤仅「建类型」，无执行行为。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 1.1 | `planning/plan/core/nodes/graph_operations/graph_operations_node.rs` | 新增 `CorrelatedApplyNode`（仿 `PatternApplyNode`，`graph_operations_node.rs:493-604`）：字段 `left_input/right_input/deps/is_anti_predicate/col_names=left_input.col_names()`；构造 `new(left, right, anti)`；访问器 `left_input()/right_input()/is_anti_predicate()/col_names()/…`；实现 `PlanNode`/`PlanNodeClonable`/`MemoryEstimatable` |
| 1.2 | `planning/plan/logical/logical_nodes/graph_ops.rs` | 用 `define_logical_join_node!`（`graph_ops.rs:53-58`）新增 `LogicalCorrelatedApplyNode { left, right, deps=[left,right], is_anti_predicate, … }` |
| 1.3 | `planning/plan/core/nodes/base/plan_node_enum.rs` | 变体声明（`plan_node_enum.rs:182` 附近）`CorrelatedApply(CorrelatedApplyNode)`；6 处宏列表各加一行：`is_*`(`:291`)、`as_*`(`:388`)、`as_*_mut`(`:484`)、节点名(`:584`、`:782`)、类别(`:683`) |
| 1.4 | §4.9 列出的 match 位文件 | 逐一补 `CorrelatedApply` arm：`plan_node_visitor.rs`、`plan_node_traits_impl.rs`、`plan_node_operations.rs`、`plan_node_children.rs`、`plan_node_factory.rs`(如有全节点枚举)、`planning/plan/execution_plan.rs`、`physical_planner.rs`、`optimizer/analysis/{batch,reference_count,fingerprint}.rs`、`optimizer/cost/child_accessor.rs`；`decorrelation.rs`/`subquery_unnesting.rs` 核对未知节点默认分支不 panic |

**验证**：`cargo check -p graphdb-query` 通过（编译器穷举未补 arm 的点）；`cargo clippy -p graphdb-query --all-targets` 零新增警告。

**状态**：- [ ]

### 8.2 步骤 2：`SourceSpec::Argument` 携带外层布局

> 对应设计：§4.4、D4。让 Argument 的元数据布局 = 外层布局，运行期 chunk 槽位才与 correlation frame 对齐。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 2.1 | `executor/streaming/operators/spec.rs` | `SourceSpec::Argument` 从单元变体改为 `Argument { col_names: Vec<String> }` |
| 2.2 | `executor/streaming/plan/arena_builder/specs.rs` | `build_source_spec`（`specs.rs:83`）：`SourceSpec::Argument { col_names: node.col_names().to_vec() }` |
| 2.3 | `executor/streaming/plan/arena_builder/metadata.rs` | `source_output_layout`（`metadata.rs:626`）：`Argument { col_names } => SlotLayout::from_names(col_names)`；`source_explain_name`（`:684`）匹配加 `{ .. }` |
| 2.4 | `executor/streaming/operators/source_operator.rs` | `from_spec`（`source_operator.rs:260`）匹配 `Argument { .. }`；运行期 `next`（`:344-359`）**逻辑不变**（仍用 `base.output_layout`） |
| 2.5 | `executor/streaming/operators/state.rs` 等 | 编译期补齐所有 `SourceSpec::Argument` 穷举匹配点（`state.rs:109`、`metadata.rs:327/424`、`spec.rs:951`） |

**验证**：新增 metadata 单测：`source_output_layout(Argument { col_names: ["t","t.name"] })` 返回含同名槽位的布局；`cargo check -p graphdb-query` 通过。

**状态**：- [ ]

### 8.3 步骤 3：`ApplySpec::CorrelatedApply` + arena 构建

> 对应设计：§4.3、§4.5、D1/D2。本步骤打通「规划节点 → 物理计划（含嵌套子计划）」的构建链路。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 3.1 | `executor/streaming/operators/spec.rs` | `ApplySpec`（`spec.rs:494-508`）新增 `CorrelatedApply { sub_plan: Arc<PhysicalPlan>, anti: bool }` |
| 3.2 | `executor/streaming/plan/arena_builder/assembler/fragment_ops.rs` | 新增一元 push 辅助（仿 `push_global_unary_op`），仅挂一个 child fragment，用于 Apply |
| 3.3 | `executor/streaming/plan/arena_builder/assembler/conversion.rs` | 新增 `PlanNodeEnum::CorrelatedApply` 分支：左子树 `convert_node` → 右子树用 `PhysicalPlanBuildContext::from_execution_context(exec_ctx)`（`partition_spec = None`）递归 `PhysicalPlanBuilder::build` 得 `Arc<PhysicalPlan>` → `ApplySpec::CorrelatedApply` → 一元 push |
| 3.4 | `executor/streaming/plan/arena_builder/metadata.rs` | `populate_input_contracts`（`metadata.rs:139`）对 `CorrelatedApply` 走 `InputContract::UnaryInput`（排除出 `BinaryInputs` 分支）；`infer_output_layout`（`:513`）加入 `CorrelatedApply { .. } => input`；`apply_explain_name`（`:785`）返回 `"CorrelatedApply"` |
| 3.5 | `executor/streaming/plan/types.rs` | 核对 `OperatorKindSpec`/`FragmentSpec` 无新增需求（沿用既有字段） |

**验证**：新增计划级单测：给定 `CorrelatedApplyNode(Scan, Argument→CrossJoin→Filter)`，断言构建出的 `PhysicalPlan` 含 CorrelatedApply 算子、其 `sub_plan` 存在且 root 为 Argument；`cargo check` 通过。

**状态**：- [x]

### 8.4 步骤 4：materializer（一元输入 + 参数剥离）+ 算子骨架

> 对应设计：§4.6、§4.7 第 1/2 点、D3。本步骤让 CorrelatedApply 算子能被实例化（运行循环为空壳，下一步实现）。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 4.1 | `executor/streaming/plan/materializer.rs` | `OperatorKindSpec::Apply` 分支（`materializer.rs:207-211`）内分叉：`CorrelatedApply` → `take_unary_input` + 右输入 `StreamingExecutor::Source(OperatorBase::new(0), SourceOperator::Start)` 占位 + `ApplyOperator` 字面构造（携带 `sub_plan/anti/stripped_bindings`） |
| 4.2 | `executor/streaming/plan/materializer.rs` | 新增 `stripped_bindings(&QueryBindings) -> QueryBindings`：克隆后清空 `parameters`/`parameter_frame`，其余字段保留 |
| 4.3 | `executor/streaming/operators/apply_operator.rs` | 枚举（`apply_operator.rs:14-37`）新增 `CorrelatedApply { sub_plan, bindings, anti, right_rows: None, right_layout: None, memory_tracker }`；`from_spec`（`:40-75`）补 arm；`close`（`:306-341`）并入统一清理分支；`memory_tracker()`（`:77-83`）补 arm |
| 4.4 | `executor/streaming/executor.rs` | 核对 `StreamingExecutor::set_runtime`/`set_chunk_size` 递归行为无需改动（`Apply` 双子节点已覆盖） |

**验证**：新增 materializer 单测：CorrelatedApply 片段材料化成功，`take_unary_input` 取左输入、右输入为 Start 占位；`stripped_bindings` 后嵌套 `materialize` 不再报 `Unknown parameter`；`cargo check` 通过。

**状态**：- [x]

### 8.5 步骤 5：`ApplyOperator::CorrelatedApply` 运行时循环

> 对应设计：§3 执行流程、§4.7 第 3 点、D3。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 5.1 | `executor/streaming/operators/apply_operator.rs` | `next`（`apply_operator.rs:97-294`）新增 `CorrelatedApply` 分支，实现 §3 伪代码：取 `base.runtime` → 对每个 `left_row`：`set_correlation_frame(left_chunk.layout.clone(), left_row.clone())` → `PhysicalPlanMaterializer::materialize(sub_plan, &bindings)` → `sub_exec.set_chunk_size(bindings.chunk_size)` → `sub_exec.set_runtime(Some(rt.clone()))` → `open()` → 拉尽到任一行非空即 exists=true（提前 break）→ `close()` → `if exists != anti` 输出 left_row |
| 5.2 | `executor/streaming/operators/apply_operator.rs` | `open`（`:85-95`）对 CorrelatedApply 分支不打开占位右输入（保持左打开即可） |
| 5.3 | `executor/streaming/operators/apply_operator.rs` | `use` 补齐：`PhysicalPlanMaterializer`、`QueryBindings`、`ExecutionRuntime` 引用路径 |

**验证**：新增 apply_operator 单测（仿既有 `execute_apply` 辅助，`apply_operator.rs:427-450`）：构造 `sub_plan = Argument → Scan(固定行)` 的嵌套计划，断言 semi（右非空出左行）/anti（右空出左行）/逐行帧正确（两行左输入取到不同帧值）；`cargo check` 通过。

**状态**：- [x]

### 8.6 步骤 6：exists_planner 相关路由

> 对应设计：§4.2、D4。本步骤让非等值相关条件从「报错」改为「走 CorrelatedApply」。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 6.1 | `planning/statements/clauses/exists_planner.rs` | `extract_keys`（`:372-425`）删除「残差条件引用外层变量即报错」分支，改为照常返回 `(hash, probe, residual)` |
| 6.2 | `planning/statements/clauses/exists_planner.rs` | 新增 `split_correlated(&residual, &inner_vars) -> (inner_residual, correlated_residual)`：按变量是否属于 `inner_vars` 拆分 |
| 6.3 | `planning/statements/clauses/exists_planner.rs` | `plan_subquery`（`:144-252`）增加参数 `outer_col_names: &[String]`；`PlannedSubquery`（`:49-56`）增加字段 `correlated: bool`；`correlated_residual` 非空时构建右子树（`ArgumentNode::new(next_node_id(), "_correlated_apply").set_col_names(outer_col_names)` → `CrossJoinNode` → `FilterNode(and_join(correlated_residual))`），返回 `correlated: true`；否则走现状 PatternApply 路径；嵌套递归以 `sub_plan.root().col_names()` 为外层名传入 |
| 6.4 | `planning/statements/clauses/exists_planner.rs` | 新增 `wrap_correlated_apply(left, planned, anti) -> SubPlan`（仿 `wrap_pattern_apply`，`:258-301`）：物理 `CorrelatedApplyNode::new(left_root, right_root, anti)` + 逻辑镜像 `LogicalCorrelatedApplyNode` |
| 6.5 | `planning/statements/clauses/where_clause_planner.rs` | `transform_clause`（`:68-72`）：`plan_subquery(spec, …, input_plan.root().col_names())`；按 `planned.correlated` 分支调 `wrap_correlated_apply` 或 `wrap_pattern_apply` |

**验证**：更新 `extract_keys`/`plan_subquery` 既有单测（`exists_planner.rs:560-569` 的 `rejects_outer_reference_in_residual` 改为「返回 correlated_residual」）；新增：相关非等值 EXISTS 计划树断言（root=CorrelatedApply，右子树 Filter 引用外层变量、Argument.col_names=外层名）、IN 相关残差、嵌套相关；`cargo check` 通过。

**状态**：- [x]

### 8.7 步骤 7：EXPLAIN 双路径

> 对应设计：§4.8。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 7.1 | `planning/plan/explain/describe_visitor.rs` | 新增 `visit_correlated_apply`（仿 `visit_pattern_apply`，`:326-355`）：递归访问左右子树、依赖 id、`anti` 标注；在 `impl_single_input_visit!` 宏表（`:287` 附近）登记 |
| 7.2 | `executor/explain/physical_plan_explain.rs` | 核对 Apply 算子解释路径输出 `apply_explain_name`（`metadata.rs:785` 已在 3.4 返回 `"CorrelatedApply"`）并展示 `anti`/子计划信息 |

**验证**：EXPLAIN 单测断言 `CorrelatedApply` 描述行含 `anti`；`cargo check` 通过。

**状态**：- [x]

### 8.8 步骤 8：单测 + e2e

> 对应设计：§6.1/§6.2。

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 8.1 | `planning/statements/clauses/exists_planner.rs` | 补齐 6.6 列出的单测 |
| 8.2 | `executor/streaming/operators/apply_operator.rs` | 补齐 5.3 列出的运行时单测 |
| 8.3 | `tests/e2e/subquery.rs` | 追加 e2e：相关非等值 EXISTS / NOT EXISTS / IN / NOT IN、多变量相关、嵌套相关、EXPLAIN 断言 `CorrelatedApply` |

**验证**：`cargo test -p graphdb-query --lib` 全绿；`cargo test --test integration_e2e subquery` 新增用例全绿（含手算结果比对）。

**状态**：- [x]

### 8.9 步骤 9：全量回归 + 文档收尾

**任务清单**：

| # | 文件 | 改动 |
|---|------|------|
| 9.1 | — | `cargo test -p graphdb-query --lib`、`cargo test --test integration_e2e subquery`、`cargo test --test '*'`、`cargo clippy -p graphdb-query --all-targets`、`cargo fmt` 全部通过 |
| 9.2 | `docs/plan/README.md` | §2.1 补 P2 条目；§2.2 #1 标记完成；§5 基线数字更新；本文档状态行「待实施 → 已完成」 |

**验证**：步骤 8 基线数字无回退；clippy 零新增警告；README 与本文档状态一致。

**状态**：- [ ]

## 9. 与其他计划的衔接

- 承接 `query_exists_in_subquery_impl.md` §4；P2 完成后 2.2 表中仅剩 P3
  （表达式级兜底/精确报错）与全量回归。
- CBO/反馈闭环（`apply_rows`/`join_rows`）与 CorrelatedApply 无直接耦合，
  不参与本次改动。
- 后续性能优化（chunk 级重建、rewind 协议）如实施，接口预留于
  `ApplyOperator::CorrelatedApply` 与 `stripped_bindings` 的构造点。
