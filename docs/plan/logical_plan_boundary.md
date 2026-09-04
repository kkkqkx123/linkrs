# LogicalPlan 与 PlanNodeEnum 职责边界重构方案

## 背景

当前系统维护了两套并行的算子枚举：
- `PlanNodeEnum`（82 变体）：物理/执行计划，包含 IndexScan、DDL、DML、事务控制等物理实现细节
- `LogicalNodeEnum`（48 变体）：纯逻辑计划，不含物理选择

**核心问题**：
1. Heuristic 优化器直接操作 `PlanNodeEnum`（物理树），而非 `LogicalNodeEnum`（逻辑树），导致逻辑优化和物理优化的边界模糊
2. CBO 在 `LogicalPlan` 上做决策，但将结构改写应用到 `PlanNodeEnum` 上，增加了不一致性风险
3. 部分 Planner（legacy）直接生成 `PlanNodeEnum`，`LogicalPlan` 通过反向转换获得

**目标**: 明确 LogicalPlan 和 PlanNodeEnum 的职责边界，使优化流水线更加清晰。

## 已完成项

### PhysicalMapper（Phase 3 核心）

- `PhysicalMapper`（`planning/physical_mapper.rs:20`）：LogicalNodeEnum → PlanNodeEnum 转换，已集成到流水线
- `merge_physical_hints()`：合并 CBO 在物理树上做的 IndexScan limits、TopN 改动

### CBO 逻辑决策（Phase 2 部分）

- Join order、index selection、aggregate strategy 的**决策**在 `LogicalPlan` 上执行
- `LogicalScanVerticesNode.index_hint: Option<IndexHint>`（`access.rs:124`）：CBO 标记索引候选
- `LogicalScanVerticesNode.estimated_cardinality: Option<u64>`（`access.rs:125`）：基数估算

### LogicalNodeEnum 变体（Phase 1 部分）

以下变体已存在于 `LogicalNodeEnum`：Sort、Limit、Union、SemiJoin（含 `anti: bool` 字段）、TopN

### Planner 迁移（Phase 5 部分）

GoPlanner（原生逻辑）、LookupPlanner（双轨道）、MatchStatementPlanner（部分双轨道）已完成迁移。

## 当前架构

```
Planner
  ├── Migrated: BoundStatement → LogicalNodeEnum → SubPlan { logical_root, root(physical) }
  └── Legacy:   BoundStatement → PlanNodeEnum → SubPlan { logical_root: None, root }

ExecutionPlan
  ├── logical_plan: Option<LogicalPlan>    ← CBO 决策来源
  └── root: Option<PlanNodeEnum>           ← 执行树

Optimizer (engine.rs:430)
  ├── ensure_logical_plan (bridge from PlanNodeEnum)
  ├── remove_factorization
  ├── Heuristic (BatchOptimizer) ← 操作 PlanNodeEnum
  ├── CBO ← 决策在 LogicalPlan，改写在 PlanNodeEnum
  ├── factorization
  ├── Physical Mapping (LogicalNodeEnum → PlanNodeEnum)
  ├── Heuristic (BatchOptimizer) ← 第二次，仍操作 PlanNodeEnum
  └── Partitioning
```

---

## 待完成任务

### 任务 1：LogicalRule trait 与 LogicalBatchOptimizer（Phase 1）

**目标**：创建操作 `LogicalNodeEnum` 的逻辑优化器框架，与现有物理优化器并行。

#### 1.1 定义 LogicalRule trait

**新建文件**：`crates/graphdb-query/src/optimizer/heuristic/logical_rule.rs`

```rust
pub trait LogicalRule: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;
    fn apply(&self, node: &mut LogicalNodeEnum, ctx: &mut LogicalRuleContext) -> Result<bool>;
}

pub struct LogicalRuleContext {
    pub stats: Arc<StatisticsManager>,
    pub changed: bool,
}
```

与现有 `RewriteRule`（`rule.rs:41`）的区别：操作目标从 `PlanNodeEnum` 改为 `LogicalNodeEnum`。

#### 1.2 定义 LogicalBatchOptimizer

**新建文件**：`crates/graphdb-query/src/optimizer/heuristic/logical_batch.rs`

```rust
pub struct LogicalBatchOptimizer {
    rules: Vec<Box<dyn LogicalRule>>,
    max_iterations: usize,
}

impl LogicalBatchOptimizer {
    pub fn optimize(&self, plan: &mut LogicalNodeEnum) -> Result<OptimizationResult> {
        let mut result = OptimizationResult::default();
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < self.max_iterations {
            changed = false;
            for rule in &self.rules {
                let rule_changed = self.apply_rule_bottom_up(rule.as_ref(), plan)?;
                changed = changed || rule_changed;
            }
            iterations += 1;
        }
        result.iterations = iterations;
        Ok(result)
    }

    fn apply_rule_bottom_up(&self, rule: &dyn LogicalRule, node: &mut LogicalNodeEnum) -> Result<bool> {
        let mut changed = false;
        match node {
            LogicalNodeEnum::Filter(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::Project(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::InnerJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::LeftJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::RightJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::CrossJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::SemiJoin(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.left)?;
                changed |= self.apply_rule_bottom_up(rule, &mut n.right)?;
            }
            LogicalNodeEnum::Sort(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::Limit(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::TopN(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::Aggregate(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::Expand(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::ExpandAll(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::Traverse(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::AppendVertices(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::BiExpand(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            LogicalNodeEnum::BiTraverse(n) => { changed |= self.apply_rule_bottom_up(rule, &mut n.input)?; }
            // ... 其他单输入算子递归子节点
            _ => {}
        }
        if rule.apply(node, &mut LogicalRuleContext::new())? {
            changed = true;
        }
        Ok(changed)
    }
}
```

#### 1.3 集成到 OptimizerEngine

**修改文件**：`crates/graphdb-query/src/optimizer/engine.rs`

在 `OptimizerEngine` struct 中新增字段：

```rust
pub struct OptimizerEngine {
    // ... existing fields ...
    logical_heuristic: LogicalBatchOptimizer,  // 新增
}
```

在 `optimize_with_layout`（line 430）中，将第一次 heuristic 调用改为在逻辑树上操作：

```rust
// Phase 1: Logical Heuristic (操作 LogicalNodeEnum)
if self.enable_heuristic {
    if let Some(ref mut logical) = current_plan.logical_plan {
        self.logical_heuristic.optimize(&mut logical.root)?;
    }
}
```

#### 1.4 迁移规则的逐步策略

不一次性迁移所有规则，而是按以下顺序逐步迁移：

1. **第一批（逻辑特征明确）**：PredicatePushdown（`predicate_pushdown/`）、ProjectionPushdown（`projection_pushdown/`）
2. **第二批（排序/限制相关）**：LimitPushdown（`limit_pushdown/`）、SortElimination（`elimination/eliminate_sort.rs`）、TopNConversion（`limit_pushdown/convert_sort_limit_to_topn.rs`）
3. **第三批（复杂逻辑）**：Decorrelation（`decorrelation.rs`）、MergeConsecutiveOps（`merge/`）

每批迁移后运行 `cargo test -p graphdb-query --lib` 验证正确性。

---

### 任务 2：CBO 结构改写迁移到逻辑树（Phase 2）

**目标**：将 CBO 的结构改写从 `PlanNodeEnum` 迁移到 `LogicalNodeEnum`，使决策和改写在同一棵树上完成。

#### 2.1 Join reorder 改写在逻辑树上

**修改文件**：`crates/graphdb-query/src/optimizer/cost_based/join_order_rewriter/`

当前 `walk_and_optimize_joins_with_decisions()` 操作 `PlanNodeEnum`。需新建逻辑树版本：

```rust
pub fn walk_and_optimize_joins_logical(
    plan: &mut LogicalNodeEnum,
    decisions: &[JoinOrderDecision],
) -> Result<()> {
    match plan {
        LogicalNodeEnum::InnerJoin(n) => {
            if let Some(decision) = decisions.iter().find(|d| d.node_id == n.id) {
                if decision.swap_children {
                    std::mem::swap(&mut n.left, &mut n.right);
                }
            }
            walk_and_optimize_joins_logical(&mut n.left, decisions)?;
            walk_and_optimize_joins_logical(&mut n.right, decisions)?;
        }
        LogicalNodeEnum::LeftJoin(n) => {
            walk_and_optimize_joins_logical(&mut n.left, decisions)?;
            walk_and_optimize_joins_logical(&mut n.right, decisions)?;
        }
        LogicalNodeEnum::RightJoin(n) => {
            walk_and_optimize_joins_logical(&mut n.left, decisions)?;
            walk_and_optimize_joins_logical(&mut n.right, decisions)?;
        }
        LogicalNodeEnum::CrossJoin(n) => {
            walk_and_optimize_joins_logical(&mut n.left, decisions)?;
            walk_and_optimize_joins_logical(&mut n.right, decisions)?;
        }
        // ... 其他算子递归
    }
    Ok(())
}
```

#### 2.2 Index selection 改写在逻辑树上

**修改文件**：`crates/graphdb-query/src/optimizer/cost_based/index_selection.rs`

当前 `rewrite_index_scans()`（line 42）操作 `PlanNodeEnum`。`rewrite_index_scans_logical()`（line 356）已存在但仅 stamp hints，不改写结构。需扩展为完整逻辑树改写：

- 逻辑树上的 `Filter → ScanVertices` 在有 `index_hint` 时标记为需要转换
- Physical Mapping 阶段根据标记生成 `IndexScan`

#### 2.3 新增 LogicalInnerJoinNode.recommended_algorithm 字段

**修改文件**：`crates/graphdb-query/src/planning/plan/logical/logical_nodes/join.rs`

当前 `LogicalInnerJoinNode` 由宏 `define_logical_join_node!` 生成（`join.rs:7`），字段仅有 `id`、`left`、`right`、`hash_keys`、`probe_keys`、`deps` 等标准字段。宏已支持 `$($field:ident: $type:ty),*` 语法（`logical_macros.rs:275`），可直接添加字段：

```rust
define_logical_join_node! {
    pub struct LogicalInnerJoinNode {
        join_condition: Option<ContextualExpression>,
        recommended_algorithm: Option<JoinAlgorithm>,  // 新增
    }
    enum: InnerJoin
}
```

同时需要在 `define_logical_join_node!` 宏的 `Clone` impl（`logical_macros.rs:294`）和 `new()` 方法中确保新字段被正确克隆和初始化。

#### 2.4 修改 engine.rs 中 apply_cost_based

**修改文件**：`crates/graphdb-query/src/optimizer/engine.rs:656`

将 `apply_cost_based` 中的结构改写从物理树改为逻辑树：

```rust
fn apply_cost_based(&self, plan: ExecutionPlan, space: Option<&str>) -> OptimizeResult<ExecutionPlan> {
    let mut plan = plan;
    let stats = StatsView::new(&self.stats_manager, space);
    if let Some(ref mut logical) = plan.logical_plan {
        // 1. Join order 优化（逻辑树改写）
        self.optimize_join_order_logical(&mut logical.root, &stats)?;
        // 2. Index selection（逻辑树 stamp hints）
        self.mark_index_candidates_logical(&mut logical.root, &stats)?;
        // 3. TopN conversion（逻辑树改写）
        self.apply_topn_wiring_logical(&mut logical.root)?;
        // 4. Subquery unnesting（逻辑树改写）
        self.subquery_unnesting_optimizer.optimize_logical(&mut logical.root)?;
    }
    // Physical Mapping 阶段将逻辑树转为物理树（含 index hints → IndexScan）
    Ok(plan)
}
```

需要同步修改以下文件，使结构改写在逻辑树上执行：
- `optimizer/cost_based/join_order_rewriter/`：新增 `walk_and_optimize_joins_logical()`
- `optimizer/cost_based/topn_wiring.rs`：新增逻辑树版本的 Sort+Limit → TopN 改写
- `optimizer/cost_based/subquery_unnesting.rs`：新增逻辑树版本的 PatternApply → InnerJoin 改写

---

### 任务 3：LogicalNodeEnum 变体补充（Phase 1）

#### 3.1 新增 Skip 变体

**修改文件**：`crates/graphdb-query/src/planning/plan/logical/logical_node_enum.rs`

在 `LogicalNodeEnum` 中添加：

```rust
pub enum LogicalNodeEnum {
    // ... existing ...
    Skip(LogicalSkipNode),  // 新增
}
```

**新建文件**：`crates/graphdb-query/src/planning/plan/logical/logical_nodes/operation.rs` 中追加：

```rust
define_logical_plan_node! {
    pub struct LogicalSkipNode {
        offset: i64,
    }
    enum: Skip
    input: SingleInputNode
}
```

同时需要在 `convert_plan()`（`conversion.rs:47`）中添加 `PlanNodeEnum` 中对应的 Skip/Offset 反向转换支持。

#### 3.2 AntiJoin 处理

当前 `LogicalSemiJoinNode` 已有 `anti: bool` 字段（`join.rs:35`），可区分半连接和反连接。**无需新增独立 AntiJoin 变体**，这是已确认的设计决策。

---

### 任务 4：PhysicalHeuristicOptimizer 独立化（Phase 3）

**目标**：将物理启发式优化从 `BatchOptimizer` 复用中解耦为独立 struct。

**新建文件**：`crates/graphdb-query/src/optimizer/heuristic/physical_heuristic.rs`

```rust
pub struct PhysicalHeuristicOptimizer {
    batch: BatchOptimizer,  // 复用现有规则集
}

impl PhysicalHeuristicOptimizer {
    pub fn optimize(&self, plan: PlanNodeEnum) -> RewriteResult<OptimizationResult> {
        self.batch.optimize(plan)
    }
}
```

**修改文件**：`crates/graphdb-query/src/optimizer/engine.rs:488`

将第二次 heuristic 调用改为使用 `PhysicalHeuristicOptimizer`：

```rust
// Phase 4: Physical Heuristic
if self.enable_heuristic {
    if let Some(root) = current_plan.root.clone() {
        let result = self.physical_heuristic.optimize(root)?;
        current_plan.set_root(result.optimized_plan);
    }
}
```

---

### 任务 5：Legacy Planner 迁移（Phase 5）

**目标**：6 个 legacy planner 迁移为生成 `LogicalNodeEnum`。

#### 迁移策略

每个 legacy planner 需要：
1. 新增 `LogicalNodeEnum` 生成路径
2. 将 `SubPlan::new(physical_root, physical_tail)` 改为 `SubPlan::from_logical_root(logical_root)`
3. 删除物理树直接生成代码

#### 新增逻辑 DML 变体

**修改文件**：`crates/graphdb-query/src/planning/plan/logical/logical_node_enum.rs`

```rust
pub enum LogicalNodeEnum {
    // ... existing ...
    InsertVertices(LogicalInsertVerticesNode),  // 新增
    InsertEdges(LogicalInsertEdgesNode),        // 新增
    Update(LogicalUpdateNode),                  // 新增
}
```

**新建文件**：`crates/graphdb-query/src/planning/plan/logical/logical_nodes/dml.rs`

```rust
define_logical_plan_node! {
    pub struct LogicalInsertVerticesNode {
        space_id: u64,
        space_name: String,
        vertices: Vec<VertexData>,
    }
    enum: InsertVertices
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalInsertEdgesNode {
        space_id: u64,
        space_name: String,
        edges: Vec<EdgeData>,
    }
    enum: InsertEdges
    input: ZeroInputNode
}
```

#### 迁移优先级与方案

| Planner | 文件 | 迁移方案 |
|---------|------|---------|
| **InsertPlanner** | `statements/dml/insert_planner.rs` | 使用新增 `LogicalInsertVerticesNode` / `LogicalInsertEdgesNode`，planner 生成逻辑树 |
| **CreatePlanner** | `statements/dml/create_planner.rs` | 同 InsertPlanner，复用相同的逻辑 DML 变体 |
| **MergePlanner** | `statements/dml/merge_planner.rs` | 拆分为 PatternMatch + Insert/Update 逻辑子树 |
| **UserManagementPlanner** | `statements/ddl/user_management_planner.rs` | DDL 无逻辑优化需求，保持 legacy + `from_plan_node()` bridge |
| **MaintainPlanner** | `statements/ddl/maintain_planner.rs` | 同 UserManagementPlanner |
| **UsePlanner** | `statements/ddl/use_planner.rs` | 同 UserManagementPlanner |

DDL 类 planner（UserManagement、Maintain、Use）无逻辑优化需求，建议保持 legacy + bridge 方案。仅 DML 类（Insert、Create、Merge）需要迁移以支持 CBO 优化。

#### convert_plan() 补充

**修改文件**：`crates/graphdb-query/src/planning/plan/logical/conversion.rs:47`

为新的逻辑 DML 变体添加反向转换支持。

---

### 任务 6：优化流水线重构（Phase 4）

**目标**：将当前流水线调整为目标架构。

**修改文件**：`crates/graphdb-query/src/optimizer/engine.rs:430`

目标 `optimize_with_layout` 流程：

```rust
pub fn optimize_with_layout(&self, plan: ExecutionPlan, space: Option<&str>, layout: &PartitioningLayoutInfo) -> OptimizeResult<ExecutionPlan> {
    let mut plan = plan;
    self.maybe_apply_feedback();
    plan = self.ensure_logical_plan(plan);
    plan = self.apply_remove_factorization(plan);

    // Phase 1: Logical Heuristic (操作 LogicalNodeEnum)
    if self.enable_heuristic {
        if let Some(ref mut logical) = plan.logical_plan {
            self.logical_heuristic.optimize(&mut logical.root)?;
        }
    }

    // Phase 2: CBO (决策和改写都在 LogicalNodeEnum)
    plan = self.apply_cost_based(plan, space)?;

    plan = self.apply_factorization(plan);
    plan = self.apply_intersect_to_join_rewrite(plan, space);

    // Phase 3: Physical Mapping (LogicalNodeEnum → PlanNodeEnum)
    plan = self.apply_physical_mapping(plan);

    // Phase 4: Physical Heuristic (操作 PlanNodeEnum)
    if self.enable_heuristic {
        plan = self.apply_physical_heuristic(plan)?;
    }

    // Phase 5: Partitioning
    plan = self.apply_partitioning_selection(plan, space, layout);

    Ok(plan)
}
```

---

## 任务依赖与执行顺序

```
Task 1 (LogicalRule + LogicalBatchOptimizer)
  ↓
Task 2 (CBO 改写迁移) + Task 3 (Skip 变体)  ← 可并行
  ↓
Task 4 (PhysicalHeuristicOptimizer) + Task 5 (DML Planner 迁移)  ← 可并行
  ↓
Task 6 (流水线重构，依赖上述全部)
```

## 验证方式

```bash
# 运行查询计划测试
cargo test -p graphdb-query --lib -- --nocapture

# 对比 EXPLAIN 输出
cargo test --test explain_output -- --nocapture

# 回归测试
cargo test --lib
```

## 风险与缓解

1. **复杂度风险**: 两套优化器增加维护成本
   - 缓解: Logical Heuristic 和 Physical Heuristic 使用统一的 Rule trait，仅分发目标不同

2. **兼容性风险**: Legacy Planner 迁移可能引入回归
   - 缓解: 通过 `LogicalPlan::from_plan_node()` 桥接，逐步迁移

3. **性能风险**: 多一层 LogicalNodeEnum 转换增加开销
   - 缓解: Physical Mapping 是简单的 1:1 转换，开销可忽略

4. **正确性风险**: 优化规则在 LogicalNodeEnum 上可能遗漏物理约束
   - 缓解: Physical Heuristic 阶段处理物理约束，Logical 阶段专注逻辑优化
