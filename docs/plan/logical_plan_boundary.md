# LogicalPlan 与 PlanNodeEnum 职责边界重构方案

## 背景

当前系统维护了两套并行的算子枚举：
- `PlanNodeEnum`（~83 变体）：物理/执行计划，包含 IndexScan、DDL、DML、事务控制等物理实现细节
- `LogicalNodeEnum`（~50 变体）：纯逻辑计划，不含物理选择

**问题**：
1. 两套枚举之间通过 `convert_plan()` 和 `convert_logical_to_physical()` 双向转换，存在冗余
2. Heuristic 优化器直接操作 `PlanNodeEnum`（物理树），而非 `LogicalNodeEnum`（逻辑树），导致逻辑优化和物理优化的边界模糊
3. CBO 在 `LogicalPlan` 上做决策，但将结构改写应用到 `PlanNodeEnum` 上，增加了不一致性风险
4. 部分 Planner（legacy）直接生成 `PlanNodeEnum`，`LogicalPlan` 通过反向转换获得

**目标**: 明确 LogicalPlan 和 PlanNodeEnum 的职责边界，使优化流水线更加清晰。

## 当前架构分析

```
Planner
  ├── Migrated planners: BoundStatement → LogicalNodeEnum → SubPlan { logical_root, root(physical) }
  └── Legacy planners: BoundStatement → PlanNodeEnum → SubPlan { logical_root: None, root }

ExecutionPlan
  ├── logical_plan: Option<LogicalPlan>    ← CBO 决策来源
  └── root: Option<PlanNodeEnum>           ← 执行树

Optimizer
  ├── Phase 1: Heuristic (BatchOptimizer) ← 操作 PlanNodeEnum
  ├── Phase 2: CBO                         ← 决策在 LogicalPlan，改写在 PlanNodeEnum
  └── Phase 3: Partitioning
```

**核心矛盾**: Heuristic 优化器需要在逻辑层面操作（如 predicate pushdown、projection pushdown），但它操作的是 `PlanNodeEnum`（物理树），这导致：
1. 规则需要处理物理节点（如 IndexScan），但这不属于逻辑优化
2. 物理节点的引入（如 IndexScan）在 CBO 阶段完成，但 Heuristic 可能先于 CBO 运行
3. `LogicalNodeEnum` 的存在没有被充分利用

## 目标架构

```
Planner (所有 planner 生成 LogicalNodeEnum)
  └── SubPlan { logical_root: LogicalNodeEnum, root: PlanNodeEnum (1:1 mapping) }

Optimizer
  ├── Phase 1: Logical Heuristic (操作 LogicalNodeEnum)
  ├── Phase 2: CBO (在 LogicalPlan 上决策，改写 LogicalNodeEnum)
  ├── Phase 3: Physical Mapping (LogicalNodeEnum → PlanNodeEnum, 引入物理选择)
  └── Phase 4: Physical Heuristic (在 PlanNodeEnum 上做物理优化)
```

**关键变化**:
1. Heuristic 优化器分为 Logical Heuristic 和 Physical Heuristic 两个阶段
2. CBO 在 LogicalNodeEnum 上做决策和结构改写
3. Physical Mapping 在 CBO 之后执行，引入 IndexScan 等物理选择
4. Physical Heuristic 处理物理层面的优化（如内存布局、并行化）

## Phase 1: Logical Heuristic 优化器

### 目标

将现有的 heuristic rules 分为两类：
- **Logical rules**: 操作 `LogicalNodeEnum`，在逻辑层面做优化
- **Physical rules**: 操作 `PlanNodeEnum`，在物理层面做优化

### 规则分类

**Logical Rules（迁移到 LogicalNodeEnum）**:

| 规则 | 当前操作 | 迁移后操作 | 说明 |
|------|---------|-----------|------|
| PredicatePushdown | PlanNodeEnum | LogicalNodeEnum | 谓词下推是纯逻辑优化 |
| ProjectionPushdown | PlanNodeEnum | LogicalNodeEnum | 投影下推是纯逻辑优化 |
| Decorrelation | PlanNodeEnum | LogicalNodeEnum | 子查询去关联化是逻辑优化 |
| LimitPushdown | PlanNodeEnum | LogicalNodeEnum | LIMIT 下推是逻辑优化 |
| SortElimination | PlanNodeEnum | LogicalNodeEnum | 排序消除是逻辑优化 |
| TopNConversion | PlanNodeEnum | LogicalNodeEnum | Sort+Limit → TopN 是逻辑优化 |
| MergeConsecutiveOps | PlanNodeEnum | LogicalNodeEnum | 合并连续同类算子是逻辑优化 |

**Physical Rules（保留在 PlanNodeEnum）**:

| 规则 | 操作 | 说明 |
|------|------|------|
| IndexScanSelection | PlanNodeEnum | CBO 引入 IndexScan 后的物理优化 |
| MemoryLayoutOptimization | PlanNodeEnum | 内存布局优化是物理层面 |
| ParallelizationHints | PlanNodeEnum | 并行化提示是物理层面 |

### LogicalRule trait

```rust
pub trait LogicalRule {
    fn name(&self) -> &str;
    fn pattern(&self) -> &LogicalPattern;
    fn apply(&self, node: &mut LogicalNodeEnum, ctx: &mut RuleContext) -> Result<bool>;
    fn matches(&self, node: &LogicalNodeEnum) -> bool;
}

pub struct LogicalPattern {
    pub node_type: LogicalNodeType,
    pub children_pattern: Vec<ChildPattern>,
}

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
            result.iterations = iterations;
        }
        
        Ok(result)
    }
    
    fn apply_rule_bottom_up(
        &self,
        rule: &dyn LogicalRule,
        node: &mut LogicalNodeEnum,
    ) -> Result<bool> {
        // 自底向上应用规则
        let mut changed = false;
        match node {
            LogicalNodeEnum::Filter(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.input)?;
            }
            LogicalNodeEnum::Project(n) => {
                changed |= self.apply_rule_bottom_up(rule, &mut n.input)?;
            }
            // ... 其他算子
        }
        
        if rule.matches(node) {
            changed |= rule.apply(node, &mut RuleContext::new())?;
        }
        
        Ok(changed)
    }
}
```

### LogicalNodeEnum 扩展

当前 `LogicalNodeEnum` 需要补充一些缺失的变体以支持完整的逻辑优化：

```rust
pub enum LogicalNodeEnum {
    // ... 现有变体 ...
    
    // 补充: 支持更多逻辑优化
    Sort(LogicalSortNode),          // 排序（逻辑）
    Limit(LogicalLimitNode),        // 限制（逻辑）
    Skip(LogicalSkipNode),          // 跳过（逻辑）
    Union(LogicalUnionNode),        // 集合操作（逻辑）
    SemiJoin(LogicalSemiJoinNode),  // 半连接（逻辑，Decorrelation 产物）
    AntiJoin(LogicalAntiJoinNode),  // 反连接（逻辑，Decorrelation 产物）
    TopN(LogicalTopNNode),          // TopN（逻辑，Sort+Limit 合并产物）
}
```

### 参考文件

- Ladybug 优化器流水线: `ref/ladybug/src/optimizer/optimizer.cpp`
- Ladybug FilterPushDown: `ref/ladybug/src/optimizer/filter_push_down_optimizer.cpp`
- Ladybug ProjectionPushDown: `ref/ladybug/src/optimizer/projection_push_down_optimizer.cpp`

## Phase 2: CBO 在 LogicalNodeEnum 上操作

### 当前问题

CBO 在 `LogicalPlan` 上做决策，但将结构改写应用到 `PlanNodeEnum` 上。这导致：
1. `LogicalPlan` 和 `PlanNodeEnum` 可能不一致
2. CBO 决策和结构改写分离，增加调试难度

### 目标

CBO 在 `LogicalNodeEnum` 上做决策**和**结构改写，然后通过 Physical Mapping 生成最终的 `PlanNodeEnum`。

### CBO 改写逻辑

```rust
pub struct CostBasedOptimizer {
    statistics_manager: Arc<StatisticsManager>,
    cost_calculator: CostCalculator,
    selectivity_estimator: SelectivityEstimator,
}

impl CostBasedOptimizer {
    pub fn optimize(&self, plan: &mut LogicalPlan) -> Result<()> {
        // 1. Join Order 优化（在 LogicalNodeEnum 上改写）
        self.optimize_join_order(&mut plan.root)?;
        
        // 2. Index Selection（在 LogicalNodeEnum 上标记，Physical Mapping 时转换）
        self.mark_index_candidates(&mut plan.root)?;
        
        // 3. Aggregate Strategy（在 LogicalNodeEnum 上标记）
        self.mark_aggregate_strategy(&mut plan.root)?;
        
        // 4. Subquery Unnesting（在 LogicalNodeEnum 上改写）
        self.unnest_subqueries(&mut plan.root)?;
        
        // 5. 更新基数估算
        self.update_cardinalities(&mut plan.root)?;
        
        Ok(())
    }
    
    fn optimize_join_order(&self, plan: &mut LogicalNodeEnum) -> Result<()> {
        // 在 LogicalNodeEnum 上重新排列 Join 子节点
        // 而非在 PlanNodeEnum 上操作
        match plan {
            LogicalNodeEnum::InnerJoin(n) => {
                // 基于代价选择 build/probe 顺序
                let left_cost = self.estimate_cost(&n.left_input);
                let right_cost = self.estimate_cost(&n.right_input);
                if right_cost < left_cost {
                    std::mem::swap(&mut n.left_input, &mut n.right_input);
                }
                // 递归优化子节点
                self.optimize_join_order(&mut n.left_input)?;
                self.optimize_join_order(&mut n.right_input)?;
            }
            _ => {
                // 递归处理其他算子
            }
        }
        Ok(())
    }
    
    fn mark_index_candidates(&self, plan: &mut LogicalNodeEnum) -> Result<()> {
        // 在 ScanVertices 节点上标记可用的索引
        // Physical Mapping 阶段根据标记决定是否转换为 IndexScan
        match plan {
            LogicalNodeEnum::ScanVertices(n) => {
                if let Some(index) = self.find_best_index(n) {
                    n.index_hint = Some(index);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
```

### LogicalNodeEnum 增强

```rust
pub struct LogicalScanVerticesNode {
    pub id: i64,
    pub variable: String,
    pub labels: Vec<String>,
    pub properties: Vec<String>,
    /// CBO 标记: 最佳索引（Physical Mapping 时转换为 IndexScan）
    pub index_hint: Option<IndexHint>,
    /// CBO 标记: 基数估算
    pub estimated_cardinality: Option<u64>,
    pub input: Option<Box<LogicalNodeEnum>>,
}

pub struct LogicalInnerJoinNode {
    pub id: i64,
    pub join_type: JoinType,
    pub hash_keys: Vec<ExpressionId>,
    pub probe_keys: Vec<ExpressionId>,
    pub left_input: Box<LogicalNodeEnum>,
    pub right_input: Box<LogicalNodeEnum>,
    /// CBO 标记: 推荐的 join 算法
    pub recommended_algorithm: Option<JoinAlgorithm>,
    /// CBO 标记: 基数估算
    pub estimated_cardinality: Option<u64>,
    /// 因子化信息
    pub factorization_info: Option<JoinFactorizationInfo>,
}
```

### 参考文件

- Ladybug CBO 决策: `ref/ladybug/src/planner/plan/plan_join_order.cpp`
- Ladybug IndexSelection: `ref/ladybug/src/planner/index_selector.cpp`

## Phase 3: Physical Mapping

### 目标

将优化后的 `LogicalNodeEnum` 转换为 `PlanNodeEnum`，引入物理实现选择。

### PhysicalMapper

```rust
pub struct PhysicalMapper;

impl PhysicalMapper {
    pub fn map(logical: LogicalNodeEnum) -> PlanNodeEnum {
        match logical {
            // ScanVertices + index_hint → IndexScan 或 ScanVertices
            LogicalNodeEnum::ScanVertices(n) => {
                if let Some(index_hint) = n.index_hint {
                    PlanNodeEnum::IndexScan(IndexScanNode {
                        id: n.id,
                        variable: n.variable,
                        index_info: index_hint.into(),
                        // ...
                    })
                } else {
                    PlanNodeEnum::ScanVertices(ScanVerticesNode {
                        id: n.id,
                        variable: n.variable,
                        labels: n.labels,
                        properties: n.properties,
                        input: n.input.map(Self::map).map(Box::new),
                    })
                }
            }
            
            // InnerJoin → InnerJoin 或 HashJoin（根据 recommended_algorithm）
            LogicalNodeEnum::InnerJoin(n) => {
                match n.recommended_algorithm {
                    Some(JoinAlgorithm::HashJoin) => {
                        PlanNodeEnum::InnerJoin(InnerJoinNode {
                            id: n.id,
                            hash_keys: n.hash_keys,
                            probe_keys: n.probe_keys,
                            left_input: Box::new(Self::map(*n.left_input)),
                            right_input: Box::new(Self::map(*n.right_input)),
                        })
                    }
                    Some(JoinAlgorithm::IndexNestedLoop) => {
                        // 转换为 Expand + Filter 模式
                        todo!()
                    }
                    _ => {
                        // 默认: Hash Join
                        PlanNodeEnum::InnerJoin(InnerJoinNode {
                            id: n.id,
                            hash_keys: n.hash_keys,
                            probe_keys: n.probe_keys,
                            left_input: Box::new(Self::map(*n.left_input)),
                            right_input: Box::new(Self::map(*n.right_input)),
                        })
                    }
                }
            }
            
            // 1:1 映射
            LogicalNodeEnum::Filter(n) => PlanNodeEnum::Filter(FilterNode {
                id: n.id,
                condition: n.condition,
                input: Box::new(Self::map(*n.input)),
            }),
            
            // ... 其他算子
        }
    }
}
```

### Physical Heuristic 优化器

在 Physical Mapping 之后，还可以运行一些物理层面的优化：

```rust
pub struct PhysicalHeuristicOptimizer;

impl PhysicalHeuristicOptimizer {
    pub fn optimize(&self, plan: &mut PlanNodeEnum) -> Result<()> {
        // 1. 内存布局优化
        self.optimize_memory_layout(plan)?;
        
        // 2. 并行化提示
        self.add_parallelization_hints(plan)?;
        
        // 3. 物理算子合并（如 Filter + Scan → FilteredScan）
        self.merge_physical_operators(plan)?;
        
        Ok(())
    }
}
```

### 参考文件

- Ladybug PlanMapper: `ref/ladybug/src/processor/map/` 目录

## Phase 4: 优化流水线重构

### 当前流水线

```
Phase 1: Heuristic (操作 PlanNodeEnum)
Phase 2: CBO (决策在 LogicalPlan, 改写在 PlanNodeEnum)
Phase 3: Partitioning
```

### 目标流水线

```
Phase 1: Logical Heuristic (操作 LogicalNodeEnum)
Phase 2: CBO (决策和改写都在 LogicalNodeEnum)
Phase 3: Physical Mapping (LogicalNodeEnum → PlanNodeEnum)
Phase 4: Physical Heuristic (操作 PlanNodeEnum)
Phase 5: Partitioning
```

### OptimizerEngine 重构

```rust
pub struct OptimizerEngine {
    logical_heuristic: LogicalBatchOptimizer,
    cost_based: CostBasedOptimizer,
    physical_heuristic: PhysicalHeuristicOptimizer,
    partitioning: PartitioningPlanner,
    stats: Arc<StatisticsManager>,
}

impl OptimizerEngine {
    pub fn optimize(&self, plan: &mut ExecutionPlan) -> Result<()> {
        // 确保有 LogicalPlan
        if plan.logical_plan.is_none() {
            // Legacy path: 从 PlanNodeEnum 反向生成 LogicalPlan
            plan.logical_plan = Some(LogicalPlan::from_plan_node(plan.root.as_ref().unwrap()));
        }
        
        let logical_plan = plan.logical_plan.as_mut().unwrap();
        
        // Phase 1: Logical Heuristic
        self.logical_heuristic.optimize(&mut logical_plan.root)?;
        
        // Phase 2: CBO
        self.cost_based.optimize(logical_plan)?;
        
        // Phase 3: Physical Mapping
        let physical_root = PhysicalMapper::map(logical_plan.root.clone());
        plan.root = Some(physical_root);
        
        // Phase 4: Physical Heuristic
        if let Some(ref mut root) = plan.root {
            self.physical_heuristic.optimize(root)?;
        }
        
        // Phase 5: Partitioning
        if let Some(ref mut root) = plan.root {
            self.partitioning.plan(root)?;
        }
        
        Ok(())
    }
}
```

### Pipeline 整合

**文件**: `crates/graphdb-query/src/pipeline/compiler.rs`

```rust
impl<S: QueryStorage> QueryPipelineManager<S> {
    pub fn optimize_execution_plan(
        &self,
        plan: ExecutionPlan,
        space: &str,
    ) -> Result<ExecutionPlan, QueryPipelineError> {
        let mut plan = plan;
        
        // 1. 确保有 LogicalPlan
        if plan.logical_plan.is_none() {
            plan.logical_plan = Some(LogicalPlan::from_plan_node(
                plan.root.as_ref().ok_or_else(|| /* error */)?
            ));
        }
        
        // 2. 运行优化流水线
        self.optimizer_engine.optimize(&mut plan)?;
        
        // 3. 验证
        PhysicalPlanValidator::validate(plan.root.as_ref().unwrap())?;
        
        Ok(plan)
    }
}
```

## Phase 5: Legacy Planner 迁移

### 目标

所有 Planner 都生成 `LogicalNodeEnum`，消除 legacy `PlanNodeEnum` 直接生成路径。

### 迁移策略

1. **新 Planner**: 直接生成 `LogicalNodeEnum`（已完成的 migrated planners）
2. **Legacy Planner**: 通过 `LogicalPlan::from_plan_node()` 桥接（当前 fallback）
3. **最终目标**: 移除 `LogicalPlan::from_plan_node()` 反向转换

### 迁移优先级

| Planner | 当前状态 | 迁移难度 |
|---------|---------|---------|
| GoPlanner | 已迁移 | - |
| LookupPlanner | 已迁移 | - |
| MatchStatementPlanner | 部分迁移 | 中 |
| InsertPlanner | Legacy | 中 |
| CreatePlanner | Legacy | 中 |
| MergePlanner | Legacy | 中 |
| DDL Planners | Legacy | 低 |

### 参考文件

- 当前迁移状态: `docs/plan/query_module_refactoring.md`

## 实现优先级

| 阶段 | 内容 | 复杂度 | 依赖 |
|------|------|--------|------|
| Phase 1 | Logical Heuristic 优化器 | 高 | 无 |
| Phase 2 | CBO 在 LogicalNodeEnum 上操作 | 高 | Phase 1 |
| Phase 3 | Physical Mapping + Physical Heuristic | 中 | Phase 2 |
| Phase 4 | 优化流水线重构 | 中 | Phase 1-3 |
| Phase 5 | Legacy Planner 迁移 | 中 | Phase 4 |

## 验证方式

1. **单元测试**: 每个 LogicalRule 的正确性验证
2. **集成测试**: 完整查询计划的 EXPLAIN 输出对比
3. **回归测试**: 现有测试用例在新架构下通过
4. **性能测试**: 优化后的查询计划性能不低于当前版本

```bash
# 运行查询计划测试
cargo test -p graphdb-query --lib -- --nocapture

# 对比 EXPLAIN 输出
cargo test --test explain_output -- --nocapture
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
