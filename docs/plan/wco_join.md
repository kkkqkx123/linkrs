# Worst-Case Optimal Join (WCO) 支持设计方案

## 背景

当前 LinkRS 的 Join 实现仅支持二元 Hash Join（两个输入，一组 join keys）。对于图查询中的三角形模式（如 `(a)->(b), (a)->(c), (b)->(c)`），二元 Join 需要将前两个边的结果物化后再与第三个边做 Join，可能产生大量中间结果。

**Worst-Case Optimal Join (WCO)** 是图数据库的关键优化：当 N 条边共享同一个公共节点时，可以通过 Intersect 操作同时处理所有边，避免中间结果的组合爆炸。

**参考实现**: Ladybug 的 WCO 实现（`ref/ladybug/src/planner/join_order/`、`ref/ladybug/src/processor/operator/intersect/`）

## 设计目标

1. 在 Planner 阶段自动检测 WCO 候选模式
2. 生成 `LogicalIntersect` 算子（N 路输入）
3. 通过排序邻接表交集高效执行
4. 与现有二元 Join 共存，由代价模型决定选择
5. 支持用户 Join Hint 指定 WCO 执行

## 整体架构

```
Planner 阶段
  ├── SubqueryGraph: bitset 编码的子图表示
  ├── JoinOrderEnumerator: DP 枚举，检测 WCO 候选
  └── 生成 LogicalIntersect

Optimizer 阶段
  └── [复用现有优化 passes，LogicalIntersect 作为普通算子处理]

Executor 阶段
  ├── IntersectBuild: 对每个 build 侧排序
  └── Intersect: 排序邻接表交集执行
```

## Phase 1: SubqueryGraph 与 DP 枚举基础

### 数据结构

**文件**: `crates/graphdb-query/src/planning/join_order/subquery_graph.rs`（新建）

```rust
/// 查询变量总数上限（bitset 位宽）
pub const MAX_NUM_QUERY_VARIABLES: usize = 64;

/// 子图表示：用 bitset 编码哪些节点和关系在子图中
pub struct SubqueryGraph {
    /// 查询图引用
    query_graph: Arc<QueryGraph>,
    /// 节点选择位集
    pub query_nodes_selector: u64,
    /// 关系选择位集
    pub query_rels_selector: u64,
}

impl SubqueryGraph {
    pub fn new(query_graph: Arc<QueryGraph>) -> Self;
    
    pub fn add_query_node(&mut self, node_pos: usize);
    pub fn add_query_rel(&mut self, rel_pos: usize);
    pub fn add_subquery_graph(&mut self, other: &SubqueryGraph);
    
    /// 总变量数 = 节点数 + 关系数
    pub fn total_num_variables(&self) -> usize;
    
    /// 查找与当前子图相邻的节点位置
    pub fn get_node_neighbor_positions(&self) -> Vec<usize>;
    /// 查找与当前子图相邻的关系位置
    pub fn get_rel_neighbor_positions(&self) -> Vec<usize>;
    
    /// 获取指定大小的邻接子图（用于 DP 枚举）
    pub fn get_neighbor_subgraphs(&self, target_size: usize) -> Vec<SubqueryGraph>;
    
    /// 获取两个子图共享的节点位置（join keys）
    pub fn get_connected_node_positions(&self, other: &SubqueryGraph) -> Vec<usize>;
}
```

### QueryGraph

**文件**: `crates/graphdb-query/src/planning/join_order/query_graph.rs`（新建）

```rust
pub struct QueryGraph {
    /// 所有查询节点
    pub query_nodes: Vec<Arc<QueryNode>>,
    /// 所有查询关系
    pub query_rels: Vec<Arc<QueryRel>>,
    /// 节点名 -> 位置索引
    pub node_name_to_pos: HashMap<String, usize>,
    /// 关系名 -> 位置索引
    pub rel_name_to_pos: HashMap<String, usize>,
}

pub struct QueryNode {
    pub name: String,
    pub variable: String,
    pub labels: Vec<String>,
}

pub struct QueryRel {
    pub name: String,
    pub variable: String,
    pub edge_types: Vec<String>,
    pub src_node_name: String,
    pub dst_node_name: String,
    pub direction: ExtendDirection,
}
```

### 参考文件

- Ladybug QueryGraph: `ref/ladybug/src/include/binder/query/query_graph.h`
- Ladybug SubqueryGraph: `ref/ladybug/src/include/binder/query/query_graph.h`

## Phase 2: JoinOrderEnumerator DP

### DP 表结构

**文件**: `crates/graphdb-query/src/planning/join_order/subplans_table.rs`（新建）

```rust
/// 每个子图的多个候选计划（不同因子化结构）
pub struct SubgraphPlans {
    /// 最大计划数
    const MAX_NUM_PLANS: usize = 10,
    /// 当前最大代价（用于剪枝）
    pub max_cost: u64,
    /// flat/unflat 编码 -> 计划索引
    pub encoded_plan_to_idx: HashMap<u64, usize>,
    /// 候选计划列表
    pub plans: Vec<LogicalPlan>,
}

/// DP 表：按子图大小组织
pub struct SubPlansTable {
    /// dp_levels[total_vars] 存储该大小的所有子图计划
    pub dp_levels: Vec<Vec<(SubqueryGraph, SubgraphPlans)>>,
}
```

### 关键设计：因子化感知的 DP

Ladybug 的一个关键创新是：DP 表中每个子图保留**多个计划**，每个计划对应不同的 flat/unflat 编码。这是因为：
- 某个变量是 flat 还是 unflat 会影响下游的 flatten 成本
- 同一个子图可能因为因子化结构不同而有不同代价

```rust
/// 将计划的因子化结构编码为 bitset
/// bit[i] = 1 表示第 i 个节点是 flat，= 0 表示 unflat
fn encode_plan(plan: &LogicalPlan, node_ids: &[ExpressionId]) -> u64 {
    let mut encoding = 0u64;
    for (i, node_id) in node_ids.iter().enumerate() {
        let group_pos = plan.schema().get_expression_group(node_id);
        let group = plan.schema().get_group(group_pos);
        if group.is_flat() {
            encoding |= 1u64 << i;
        }
    }
    encoding
}
```

### DP 枚举算法

**文件**: `crates/graphdb-query/src/planning/join_order/plan_join_order.rs`（新建）

```rust
pub struct JoinOrderEnumerator {
    context: JoinOrderEnumeratorContext,
}

pub struct JoinOrderEnumeratorContext {
    pub sub_plans_table: SubPlansTable,
    pub cardinality_estimator: CardinalityEstimator,
    pub cost_model: CostModel,
    pub max_cost: u64,
}

impl JoinOrderEnumerator {
    /// 枚举查询图的 join order
    pub fn plan_query_graph(
        query_graph: &QueryGraph,
        info: &QueryGraphPlanningInfo,
    ) -> LogicalPlan {
        let mut context = JoinOrderEnumeratorContext::new();
        
        // 1. 初始化 level-1 计划（单个节点扫描、单个关系扫描）
        self.plan_base_table_scans(query_graph, &mut context);
        
        // 2. 逐层 DP 枚举
        let max_level = query_graph.num_nodes() + query_graph.num_rels();
        for level in 2..=max_level {
            if level <= MAX_LEVEL_TO_PLAN_EXACTLY {
                self.plan_level_exactly(level, query_graph, &mut context);
            } else {
                self.plan_level_approximately(level, query_graph, &mut context);
            }
        }
        
        // 3. 从完全匹配的子图中选择最优计划
        context.sub_plans_table.get_best_plan(&full_match_subgraph)
    }
    
    /// 精确枚举：尝试所有可能的 (left, right) 分割
    fn plan_level_exactly(&mut self, level: usize, qg: &QueryGraph, ctx: &mut JoinOrderEnumeratorContext);
    
    /// 近似枚举：仅尝试 leftLevel=1 的分割
    fn plan_level_approximately(&mut self, level: usize, qg: &QueryGraph, ctx: &mut JoinOrderEnumeratorContext);
}
```

### 参考文件

- Ladybug SubPlansTable: `ref/ladybug/src/include/planner/subplans_table.h`
- Ladybug planJoinOrder: `ref/ladybug/src/planner/plan/plan_join_order.cpp`

## Phase 3: WCO 候选检测

### 核心逻辑

**文件**: `crates/graphdb-query/src/planning/join_order/plan_intersect.rs`（新建）

```rust
impl JoinOrderEnumerator {
    /// WCO 候选检测与计划生成
    fn plan_wco_join(
        &mut self,
        left_level: usize,
        right_level: usize,
        qg: &QueryGraph,
        ctx: &mut JoinOrderEnumeratorContext,
    ) {
        // 遍历所有 right_level 大小的子图
        for (right_subgraph, _) in ctx.sub_plans_table.get_subgraphs(right_level) {
            // 找到与 right_subgraph 相邻的关系
            let candidates = self.populate_intersect_rel_candidates(qg, &right_subgraph);
            
            // 对每个候选公共节点，检查是否有 left_level 条关系共享该节点
            for (intersect_node_pos, rels) in &candidates {
                if rels.len() == left_level {
                    self.plan_wco_join_for_node(
                        &right_subgraph,
                        rels,
                        *intersect_node_pos,
                        qg,
                        ctx,
                    );
                }
            }
        }
    }
    
    /// 按公共节点分组的关系候选
    /// 返回: intersect_node_pos -> [rel1, rel2, ...]
    fn populate_intersect_rel_candidates(
        &self,
        qg: &QueryGraph,
        subgraph: &SubqueryGraph,
    ) -> HashMap<usize, Vec<Arc<QueryRel>>> {
        let mut candidates: HashMap<usize, Vec<Arc<QueryRel>>> = HashMap::new();
        
        for rel_pos in subgraph.get_rel_neighbor_positions() {
            let rel = &qg.query_rels[rel_pos];
            let src_pos = qg.node_name_to_pos[&rel.src_node_name];
            let dst_pos = qg.node_name_to_pos[&rel.dst_node_name];
            
            let is_src_connected = (subgraph.query_nodes_selector >> src_pos) & 1 == 1;
            let is_dst_connected = (subgraph.query_nodes_selector >> dst_pos) & 1 == 1;
            
            // 跳过两端都已连接的关系（需要 binary join）
            if is_src_connected && is_dst_connected {
                continue;
            }
            
            // 公共节点是未连接的那一端
            let intersect_node_pos = if is_src_connected { dst_pos } else { src_pos };
            candidates.entry(intersect_node_pos).or_default().push(rel.clone());
        }
        
        candidates
    }
    
    /// 为指定公共节点生成 WCO 计划
    fn plan_wco_join_for_node(
        &mut self,
        right_subgraph: &SubqueryGraph,
        rels: &[Arc<QueryRel>],
        intersect_node_pos: usize,
        qg: &QueryGraph,
        ctx: &mut JoinOrderEnumeratorContext,
    ) {
        let intersect_node = &qg.query_nodes[intersect_node_pos];
        
        // 收集每个关系的 build 计划
        let mut build_plans = Vec::new();
        let mut bound_node_ids = Vec::new();
        
        for rel in rels {
            let rel_subgraph = SubqueryGraph::single_rel(qg, rel);
            let build_plan = ctx.sub_plans_table.get_best_plan(&rel_subgraph);
            build_plans.push(build_plan);
            
            // 确定 bound node（关系中已连接到 right_subgraph 的端点）
            let src_pos = qg.node_name_to_pos[&rel.src_node_name];
            let dst_pos = qg.node_name_to_pos[&rel.dst_node_name];
            let bound_pos = if right_subgraph.query_nodes_selector[src_pos] != 0 {
                src_pos
            } else {
                dst_pos
            };
            bound_node_ids.push(qg.query_nodes[bound_pos].variable.clone());
        }
        
        // 为每个 right_subgraph 的候选计划创建 WCO 计划
        for left_plan in ctx.sub_plans_table.get_plans(right_subgraph) {
            // 跳过 intersect node 已在 scope 中的情况
            if left_plan.schema().is_in_scope(&intersect_node.variable) {
                continue;
            }
            
            let new_plan = self.append_intersect(
                &intersect_node.variable,
                &bound_node_ids,
                left_plan,
                &build_plans,
            );
            
            let mut new_subgraph = right_subgraph.clone();
            for rel in rels {
                new_subgraph.add_query_rel(qg.rel_name_to_pos[&rel.variable]);
            }
            
            ctx.sub_plans_table.add_plan(&new_subgraph, new_plan);
        }
    }
}
```

### WCO 触发条件

1. **左子图包含至少 2 条关系**（`leftLevel > 1`）：这是 WCO 的前提条件，否则退化为 binary join
2. **N 条关系共享恰好 1 个公共未连接节点**：这是 Intersect 的核心条件
3. **公共节点不在 probe 侧的 scope 中**：正确性约束

### 参考文件

- Ladybug planWCOJoin: `ref/ladybug/src/planner/plan/plan_join_order.cpp`
- Ladybug populateIntersectRelCandidates: `ref/ladybug/src/planner/plan/plan_join_order.cpp`

## Phase 4: Binary Join 与 WCO Join 共存

### 代价模型

**文件**: `crates/graphdb-query/src/planning/join_order/cost_model.rs`（新建）

```rust
pub struct CostModel;

impl CostModel {
    /// Hash Join 代价
    pub fn compute_hash_join_cost(
        join_node_ids: &[ExpressionId],
        probe_plan: &LogicalPlan,
        build_plan: &LogicalPlan,
    ) -> u64 {
        let mut cost = probe_plan.cost() + build_plan.cost();
        cost += probe_plan.cardinality(); // probe 侧扫描
        cost += BUILD_PENALTY * Self::get_join_keys_flat_cardinality(join_node_ids, build_plan);
        cost
    }
    
    /// Intersect (WCO) 代价
    pub fn compute_intersect_cost(
        probe_plan: &LogicalPlan,
        build_plans: &[LogicalPlan],
    ) -> u64 {
        let mut cost = probe_plan.cost();
        cost += probe_plan.cardinality();
        for build_plan in build_plans {
            cost += build_plan.cost();
        }
        cost
    }
    
    /// Extend 代价
    pub fn compute_extend_cost(child_plan: &LogicalPlan) -> u64 {
        child_plan.cost() + child_plan.cardinality()
    }
}
```

### Join 策略选择

在 `planLevelExactly` 中，同时尝试 Binary Join 和 WCO Join：

```rust
fn plan_level_exactly(&mut self, level: usize, qg: &QueryGraph, ctx: &mut JoinOrderEnumeratorContext) {
    let max_left = level / 2;
    
    for left_level in 1..=max_left {
        let right_level = level - left_level;
        
        // WCO Join: 仅当 leftLevel > 1 时尝试
        if left_level > 1 {
            self.plan_wco_join(left_level, right_level, qg, ctx);
        }
        
        // Binary Join
        self.plan_inner_join(left_level, right_level, qg, ctx);
    }
}
```

### Binary Join（现有实现）

```rust
fn plan_inner_join(
    &mut self,
    left_level: usize,
    right_level: usize,
    qg: &QueryGraph,
    ctx: &mut JoinOrderEnumeratorContext,
) {
    for (right_subgraph, _) in ctx.sub_plans_table.get_subgraphs(right_level) {
        for nbr_subgraph in right_subgraph.get_neighbor_subgraphs(left_level) {
            let join_node_positions = right_subgraph.get_connected_node_positions(&nbr_subgraph);
            
            // 优先尝试 Index-Nested-Loop Join
            if self.try_plan_inl_join(&right_subgraph, &nbr_subgraph, &join_node_positions, ctx) {
                continue;
            }
            
            // 回退到 Hash Join
            self.plan_inner_hash_join(&right_subgraph, &nbr_subgraph, &join_node_positions, ctx);
        }
    }
}
```

### 参考文件

- Ladybug CostModel: `ref/ladybug/src/planner/join_order/cost_model.cpp`
- Ladybug planInnerJoin: `ref/ladybug/src/planner/plan/plan_join_order.cpp`

## Phase 5: LogicalIntersect 算子

### 逻辑算子

**文件**: `crates/graphdb-query/src/planning/plan/logical/logical_node_enum.rs`

```rust
/// WCO Intersect: N 路输入，共享一个公共节点
LogicalIntersect(LogicalIntersectNode),

pub struct LogicalIntersectNode {
    pub id: i64,
    /// 公共节点 ID
    pub intersect_node_id: ExpressionId,
    /// 每个 build 侧的 bound node ID
    pub key_node_ids: Vec<ExpressionId>,
    /// children[0] = probe 侧
    /// children[1..N] = N 个 build 侧
    pub inputs: Vec<LogicalNodeEnum>,
}
```

### Schema 计算

```rust
impl LogicalIntersectNode {
    /// 因子化 Schema: 创建新组存储 build 侧 payload
    fn compute_factorized_schema(&self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
        let mut schema = child_schemas[0].clone(); // 复制 probe 侧 Schema
        let out_group_pos = schema.create_group();
        
        // 公共节点放入新组
        schema.insert_to_group_and_scope(&self.intersect_node_id, out_group_pos);
        
        // 每个 build 侧的 payload 放入新组
        for build_schema in &child_schemas[1..] {
            for expr in build_schema.expressions_in_scope() {
                schema.insert_to_group_and_scope(&expr, out_group_pos);
            }
        }
        
        schema
    }
    
    /// Probe 侧需要 flatten 的组: 所有 key node 组
    fn get_groups_to_flatten_on_probe_side(&self) -> Vec<FGroupPos> {
        self.key_node_ids.iter()
            .map(|key| self.schema.get_expression_group(key))
            .collect()
    }
    
    /// Build 侧需要 flatten 的组: bound node 组
    fn get_groups_to_flatten_on_build_side(&self, build_idx: usize) -> Vec<FGroupPos> {
        vec![self.schema.get_expression_group(&self.key_node_ids[build_idx])]
    }
}
```

### 参考文件

- Ladybug LogicalIntersect: `ref/ladybug/src/include/planner/operator/logical_intersect.h`

## Phase 6: 物理执行

### IntersectBuild 算子

**文件**: `crates/graphdb-query/src/executor/streaming/operators/intersect_build.rs`（新建）

```rust
/// 为 Intersect 的每个 build 侧构建排序哈希表
pub struct IntersectBuild {
    /// 哈希表：bound_node_id -> payload 列表
    hash_table: JoinHashTable,
    /// 排序键位置
    key_pos: DataChunkPos,
}

impl IntersectBuild {
    /// 追加数据时按键排序（为后续 merge-style intersect 准备）
    fn append_vectors(&mut self, key_vectors: &[ValueVector], payload_vectors: &[ValueVector]) {
        self.hash_table.append_vector_with_sorting(key_vectors, payload_vectors);
    }
}
```

### Intersect Probe 算子

**文件**: `crates/graphdb-query/src/executor/streaming/operators/intersect.rs`（新建）

```rust
/// WCO Intersect: 排序邻接表交集执行
pub struct Intersect {
    /// 输出位置
    output_pos: DataChunkPos,
    /// 每个 build 侧的数据信息
    intersect_data_infos: Vec<IntersectDataInfo>,
    /// 共享状态（每个 build 侧的哈希表）
    shared_states: Vec<Arc<Mutex<IntersectSharedState>>>,
    /// probe 侧子算子
    probe_child: Box<dyn StreamingExecutor>,
}

pub struct IntersectDataInfo {
    /// bound node ID 在 probe 侧的位置
    pub bound_node_pos: DataChunkPos,
    /// 输出 payload 列的位置
    pub output_payload_pos: Vec<DataChunkPos>,
}

pub struct IntersectSharedState {
    pub hash_table: JoinHashTable,
}
```

### 核心执行算法

```rust
impl Intersect {
    fn next_tuples(&mut self) -> Option<DataChunk> {
        loop {
            // 1. 从 probe 侧获取下一个公共节点 ID
            let probe_chunk = self.probe_child.next_chunk()?;
            let intersect_node_id = probe_chunk.get_node_id(self.intersect_node_pos);
            
            // 2. 在每个 build 侧的哈希表中查找邻接列表
            let mut probed_lists: Vec<Vec<NodeId>> = Vec::new();
            for (i, shared_state) in self.shared_states.iter().enumerate() {
                let list = shared_state.lock().hash_table.lookup(intersect_node_id);
                if list.is_empty() {
                    continue; // 任何一侧为空，跳过
                }
                probed_lists.push(list);
            }
            
            if probed_lists.len() < self.shared_states.len() {
                continue; // 某侧无匹配
            }
            
            // 3. 排序：将最小列表放在前面（优化交集性能）
            probed_lists.sort_by_key(|list| list.len());
            
            // 4. 增量两两交集
            let result = self.intersect_lists(&probed_lists);
            
            if !result.is_empty() {
                return Some(self.build_output_chunk(&result));
            }
        }
    }
    
    /// 增量两两交集（merge-style）
    fn intersect_lists(&self, lists: &[Vec<NodeId>]) -> Vec<NodeId> {
        let mut result = lists[0].clone();
        let mut sel_vector = SelectionVector::unfiltered(result.len());
        
        for i in 1..lists.len() {
            self.two_way_intersect(&mut result, &mut sel_vector, &lists[i]);
            // slice 所有之前的 selection vector 保持同步
        }
        
        result
    }
    
    /// 两路排序交集（O(n+m) merge-style）
    fn two_way_intersect(
        &self,
        left: &mut Vec<NodeId>,
        left_sel: &mut SelectionVector,
        right: &[NodeId],
    ) {
        let mut left_pos = 0;
        let mut right_pos = 0;
        let mut output_pos = 0;
        let left_data = &left[left_sel.as_slice()];
        let right_data = right;
        
        while left_pos < left_data.len() && right_pos < right_data.len() {
            if left_data[left_pos] < right_data[right_pos] {
                left_pos += 1;
            } else if left_data[left_pos] > right_data[right_pos] {
                right_pos += 1;
            } else {
                // 匹配
                left_sel.set(output_pos, left_pos);
                output_pos += 1;
                left_pos += 1;
                right_pos += 1;
            }
        }
        
        left_sel.set_size(output_pos);
    }
}
```

### 参考文件

- Ladybug IntersectBuild: `ref/ladybug/src/processor/operator/intersect/intersect_build.cpp`
- Ladybug Intersect: `ref/ladybug/src/processor/operator/intersect/intersect.cpp`

## Phase 7: 代价估算

### 基数估算

**文件**: `crates/graphdb-query/src/planning/join_order/cardinality_estimator.rs`（新建）

```rust
impl CardinalityEstimator {
    /// Intersect 基数估算
    fn estimate_intersect(
        &self,
        join_node_ids: &[ExpressionId],
        probe_op: &LogicalOperator,
        build_ops: &[&LogicalOperator],
    ) -> u64 {
        // 方法 1: 保守过滤估算
        let est1 = (probe_op.cardinality() as f64 * NON_EQUALITY_PREDICATE_SELECTIVITY) as u64;
        
        // 方法 2: 独立性假设
        let mut numerator = probe_op.cardinality();
        for build_op in build_ops {
            numerator *= build_op.cardinality();
        }
        let mut denominator = 1u64;
        for join_node_id in join_node_ids {
            denominator *= self.get_node_id_domain(join_node_id);
        }
        let est2 = numerator / denominator.max(1);
        
        est1.min(est2).max(1)
    }
}
```

### 参考文件

- Ladybug CardinalityEstimator: `ref/ladybug/src/planner/join_order/cardinality_estimator.cpp`

## Phase 8: Join Hint 支持

### 语法扩展

```cypher
-- 用户可以通过 JOIN HINT 指定 WCO 执行
MATCH (a)-[e1]->(b), (a)-[e2]->(c), (b)-[e3]->(c)
JOIN HINT (a, e1, e2, e3)
RETURN a, b, c
```

### Hint 解析

**文件**: `crates/graphdb-query/src/planner/join_order/join_tree.rs`（新建）

```rust
pub enum TreeNodeType {
    NodeScan,
    RelScan,
    BinaryJoin,
    MultiwayJoin,  // WCO
}

pub struct JoinTreeNode {
    pub node_type: TreeNodeType,
    pub extra_info: JoinTreeExtraInfo,
    pub children: Vec<JoinTreeNode>,
}

pub struct JoinTreeExtraInfo {
    pub join_nodes: Vec<ExpressionId>,
}

/// 从用户 hint 构建 JoinTree
pub struct JoinTreeConstructor;

impl JoinTreeConstructor {
    pub fn construct(query_graph: &QueryGraph, hint: &BoundJoinHint) -> JoinTree {
        let root = Self::construct_tree_node(query_graph, &hint.root);
        JoinTree { root }
    }
    
    fn construct_tree_node(qg: &QueryGraph, hint_node: &BoundJoinHintNode) -> JoinTreeNode {
        if hint_node.is_leaf() {
            // 叶节点: NODE_SCAN 或 REL_SCAN
            Self::construct_scan_node(qg, hint_node)
        } else if hint_node.is_binary() {
            // 二元: BINARY_JOIN
            let left = Self::construct_tree_node(qg, &hint_node.children[0]);
            let right = Self::construct_tree_node(qg, &hint_node.children[1]);
            JoinTreeNode::binary_join(left, right)
        } else {
            // 多元: MULTIWAY_JOIN (WCO)
            Self::construct_multiway_join(qg, hint_node)
        }
    }
    
    fn construct_multiway_join(qg: &QueryGraph, hint_node: &BoundJoinHintNode) -> JoinTreeNode {
        let probe = Self::construct_tree_node(qg, &hint_node.children[0]);
        let mut builds = Vec::new();
        let mut build_subgraphs = Vec::new();
        
        for child in &hint_node.children[1..] {
            let build = Self::construct_tree_node(qg, child);
            builds.push(build);
            build_subgraphs.push(Self::get_subgraph(qg, child));
        }
        
        // 找到公共节点: 所有 build 子图的邻居交集
        let intersect_node = Self::get_intersect_node(qg, &build_subgraphs);
        
        JoinTreeNode::multiway_join(probe, builds, intersect_node)
    }
}
```

### 参考文件

- Ladybug JoinTree: `ref/ladybug/src/planner/join_order/join_tree.h`
- Ladybug JoinTreeConstructor: `ref/ladybug/src/planner/join_order/join_tree_constructor.cpp`
- Ladybug JoinPlanSolver: `ref/ladybug/src/planner/join_order/join_plan_solver.cpp`

## 实现优先级

| 阶段 | 内容 | 复杂度 | 依赖 |
|------|------|--------|------|
| Phase 1 | SubqueryGraph + QueryGraph | 中 | 无 |
| Phase 2 | SubPlansTable + DP 框架 | 高 | Phase 1 |
| Phase 3 | WCO 候选检测 | 中 | Phase 2 |
| Phase 4 | Binary/WCO 代价模型 | 低 | Phase 2, 3 |
| Phase 5 | LogicalIntersect 算子 | 中 | Phase 3 |
| Phase 6 | IntersectBuild + Intersect 物理执行 | 高 | Phase 5 |
| Phase 7 | 基数估算 | 中 | Phase 2 |
| Phase 8 | Join Hint 支持 | 中 | Phase 1 |

## 验证方式

1. **单元测试**: SubqueryGraph 操作、DP 表操作、代价计算
2. **集成测试**: 完整查询计划验证（EXPLAIN 输出）
3. **性能测试**: 三角形查询 vs 二元 Join 的性能对比
4. **正确性测试**: WCO 结果与二元 Join 结果一致性

```bash
# 运行查询计划测试
cargo test -p graphdb-query --lib -- --nocapture

# 运行集成测试
cargo test --test query_plan -- --nocapture
```

## 风险与缓解

1. **复杂度风险**: DP 枚举的组合爆炸
   - 缓解: 精确枚举限制在 level ≤ 7，超过使用近似枚举

2. **正确性风险**: Intersect 的排序交集逻辑
   - 缓解: 参考 Ladybug 的 merge-style 两路交集实现，充分测试边界情况

3. **性能风险**: 排序开销可能在小数据集上不划算
   - 缓解: 由代价模型自动选择，WCO 仅在代价低于 Binary Join 时启用

4. **与现有代码兼容性**: 现有 Planner 不生成 LogicalIntersect
   - 缓解: 现有 Planner 保持不变，WCO 仅在新的 JoinOrderEnumerator 中启用
