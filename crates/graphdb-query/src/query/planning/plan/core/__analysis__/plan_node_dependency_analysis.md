# PlanNode 依赖关系分析文档

## 概述

本文档描述 GraphDB 查询计划节点（PlanNode）之间的依赖关系体系，帮助理解执行计划的拓扑结构和数据流。

## 依赖关系类型

根据节点的输入特性，PlanNode 分为以下几类：

### 1. 零输入节点（ZeroInputNode）- 4 个

**定义**：没有输入依赖的节点，作为执行计划的起始点。

**节点列表**：
| 节点类型 | 说明 | 依赖 | 文件位置 |
|---------|------|-----|---------|
| StartNode | 执行计划入口 | 无 | start_node.rs |
| ScanVerticesNode | 全表扫描顶点 | 无 | graph_scan_node.rs |
| ScanEdgesNode | 全表扫描边 | 无 | graph_scan_node.rs |
| EdgeIndexScanNode | 边索引扫描 | 无 | graph_scan_node.rs |

**特点**：
- 作为叶子节点出现在执行计划中
- 直接从存储层读取数据
- 可被优化器并行化

**实现方式**：
```rust
// 使用宏定义零输入节点
define_plan_node! {
    pub struct StartNode {}
    enum: Start
    input: ZeroInputNode
}
```

### 2. 单输入节点（SingleInputNode）- 19 个

**定义**：只有一个上游输入节点的节点。

**节点列表**：
| 节点类型 | 说明 | 输入依赖 | 文件位置 |
|---------|------|---------|---------|
| FilterNode | 条件过滤 | 任意单输入节点 | filter_node.rs |
| ProjectNode | 投影/列选择 | 任意单输入节点 | project_node.rs |
| AggregateNode | 聚合运算 | 任意单输入节点 | aggregate_node.rs |
| SortNode | 排序 | 任意单输入节点 | sort_node.rs |
| LimitNode | 限制返回行数 | 任意单输入节点 | sort_node.rs |
| TopNNode | Top N 排序 | 任意单输入节点 | sort_node.rs |
| SampleNode | 采样 | 任意单输入节点 | sample_node.rs |
| DedupNode | 去重 | 任意单输入节点 | data_processing_node.rs |
| ExpandNode | 边扩展 | 顶点相关节点 | traversal_node.rs |
| ExpandAllNode | 全扩展 | 顶点相关节点 | traversal_node.rs |
| TraverseNode | 遍历 | 顶点/边节点 | traversal_node.rs |
| AppendVerticesNode | 追加顶点 | 遍历结果 | traversal_node.rs |
| ArgumentNode | 参数传递 | 特定依赖 | control_flow_node.rs |
| PassThroughNode | 直通传递 | 任意单输入节点 | control_flow_node.rs |
| UnwindNode | 展开数组 | 输入数据流 | data_processing_node.rs |
| AssignNode | 变量赋值 | 输入数据流 | data_processing_node.rs |
| RollUpApplyNode | 上卷应用 | 聚合模式 | data_processing_node.rs |
| GetVerticesNode | 按ID获取顶点 | 需要输入提供ID | graph_scan_node.rs |
| GetEdgesNode | 按ID获取边 | 需要输入提供ID | graph_scan_node.rs |
| GetNeighborsNode | 获取邻居 | 需要输入提供顶点 | graph_scan_node.rs |

**特点**：
- 构成执行计划的主体
- 数据流从叶子节点流向根节点
- 支持管道化执行

**实现方式**：
```rust
// 使用宏定义单输入节点
define_plan_node_with_deps! {
    pub struct FilterNode {
        condition: Expression,
    }
    enum: Filter
    input: SingleInputNode
}
```

### 3. 双输入节点（BinaryInputNode）- 7 个

**定义**：有两个上游输入节点的节点，通常用于连接操作和集合操作。

**节点列表**：
| 节点类型 | 说明 | 输入依赖 | 文件位置 |
|---------|------|---------|---------|
| InnerJoinNode | 内连接 | 两个输入流 | join_node.rs |
| LeftJoinNode | 左连接 | 两个输入流 | join_node.rs |
| CrossJoinNode | 交叉连接 | 两个输入流 | join_node.rs |
| HashInnerJoinNode | 哈希内连接 | 两个输入流 | join_node.rs |
| HashLeftJoinNode | 哈希左连接 | 两个输入流 | join_node.rs |
| MinusNode | 差集操作 | 两个输入流 | set_operations_node.rs |
| IntersectNode | 交集操作 | 两个输入流 | set_operations_node.rs |

**特点**：
- 需要协调两个输入流
- 可能导致数据倾斜
- 优化器需要考虑连接顺序

**实现方式**：
```rust
// 使用宏定义双输入节点
define_binary_plan_node! {
    pub struct InnerJoinNode {
        join_keys: Vec<Expression>,
    }
    enum: InnerJoin
}
```

**MinusNode 特殊依赖**：
```rust
pub struct MinusNode {
    id: i64,
    input: Option<Box<PlanNodeEnum>>,      // 主输入
    deps: Vec<Box<PlanNodeEnum>>,          // 依赖列表 [主输入, 减输入]
    // ...
}

impl MinusNode {
    pub fn minus_input(&self) -> &PlanNodeEnum {
        &self.deps[1]  // 第二个输入是要减去的集合
    }
}
```

**IntersectNode 特殊依赖**：
```rust
pub struct IntersectNode {
    id: i64,
    input: Option<Box<PlanNodeEnum>>,      // 主输入
    deps: Vec<Box<PlanNodeEnum>>,          // 依赖列表 [主输入, 交输入]
    // ...
}

impl IntersectNode {
    pub fn intersect_input(&self) -> &PlanNodeEnum {
        &self.deps[1]  // 第二个输入是求交的集合
    }
}
```

### 4. 多输入节点（MultipleInputNode）- 2 个

**定义**：有多个上游输入节点的节点。

**节点列表**：
| 节点类型 | 说明 | 输入依赖 | 文件位置 |
|---------|------|---------|---------|
| UnionNode | 并集操作 | 多输入流 | data_processing_node.rs |
| DataCollectNode | 数据收集 | 多输入流 | data_processing_node.rs |

**特点**：
- 输入数量不固定
- 需要处理不同输入的模式兼容
- 支持并行收集

**实现方式**：
```rust
// 使用宏定义多输入节点
define_plan_node_with_deps! {
    pub struct UnionNode {
        distinct: bool,
    }
    enum: Union
    input: MultipleInputNode
}
```

### 5. 特殊节点 - 6 个

**定义**：具有复杂依赖关系的特殊节点。

| 节点类型 | 说明 | 依赖特点 | 文件位置 |
|---------|------|---------|---------|
| LoopNode | 循环执行 | 包含循环体依赖 | control_flow_node.rs |
| SelectNode | 条件选择 | 包含多分支依赖 | control_flow_node.rs |
| PatternApplyNode | 模式应用 | 模式匹配依赖 | data_processing_node.rs |
| IndexScanNode | 索引扫描 | 依赖索引 | graph_scan_node.rs |
| FulltextIndexScanNode | 全文索引扫描 | 依赖索引 | graph_scan_node.rs |
| EdgeIndexScanNode | 边索引扫描 | 依赖索引 | graph_scan_node.rs |

**LoopNode 特殊依赖**：
```rust
pub struct LoopNode {
    id: i64,
    input: Option<Box<PlanNodeEnum>>,      // 循环输入
    loop_body: Box<PlanNodeEnum>,          // 循环体
    max_iterations: usize,                  // 最大迭代次数
    // ...
}
```

**SelectNode 特殊依赖**：
```rust
pub struct SelectNode {
    id: i64,
    branches: Vec<Box<PlanNodeEnum>>,      // 多分支
    condition: Expression,                  // 选择条件
    // ...
}
```

## 管理节点依赖关系

管理节点（DDL 节点）大多数是零输入节点，因为它们直接操作元数据而不需要数据流输入。

### 零输入管理节点 - 27 个

| 类别 | 节点 | 说明 |
|-----|------|-----|
| 图空间 | CreateSpaceNode, DropSpaceNode, DescSpaceNode, ShowSpacesNode | 图空间管理 |
| 标签 | CreateTagNode, AlterTagNode, DescTagNode, DropTagNode, ShowTagsNode | 标签管理 |
| 边类型 | CreateEdgeNode, AlterEdgeNode, DescEdgeNode, DropEdgeNode, ShowEdgesNode | 边类型管理 |
| 索引 | CreateTagIndexNode, DropTagIndexNode, DescTagIndexNode, ShowTagIndexesNode | 标签索引 |
| 索引 | CreateEdgeIndexNode, DropEdgeIndexNode, DescEdgeIndexNode, ShowEdgeIndexesNode | 边索引 |
| 索引 | RebuildTagIndexNode, RebuildEdgeIndexNode | 索引重建 |
| 用户 | CreateUserNode, AlterUserNode, DropUserNode, ChangePasswordNode | 用户管理 |

## 依赖关系图示

### 典型查询计划结构

```
MATCH (n) WHERE n.age > 20 RETURN n.name
│
├── ScanVerticesNode (Start) [ZeroInputNode]
│       │
│       ▼
├── FilterNode (条件过滤) [SingleInputNode]
│       │
│       ▼
├── ProjectNode (投影) [SingleInputNode]
│       │
│       ▼
└── LimitNode (结果限制) [SingleInputNode]
```

### 连接查询结构

```
MATCH (n)-[e]->(m) WHERE n.age > 20 RETURN n.name, m.name
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── ExpandNode (n → e) [SingleInputNode]
│       │
│       ▼
├── GetNeighborsNode (e → m) [SingleInputNode]
│       │
│       ▼
├── HashInnerJoinNode (合并结果) [BinaryInputNode]
│       │
│       ▼
├── FilterNode [SingleInputNode]
│       │
│       ▼
└── ProjectNode [SingleInputNode]
```

### Union 查询结构

```
MATCH (n) RETURN n UNION MATCH (m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── ProjectNode [SingleInputNode]
│       │
│       ▼
├── UnionNode [MultipleInputNode] ◄──┐
│       │                            │
│       ▼                            │
└── ProjectNode [SingleInputNode]    │
                                     │
    ScanVerticesNode (m) [ZeroInputNode]
            │
            ▼
    ProjectNode [SingleInputNode] ───┘
```

### Minus 查询结构

```
MATCH (n) RETURN n MINUS MATCH (m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── ProjectNode [SingleInputNode]
│       │
│       ▼
├── MinusNode [BinaryInputNode] ◄────┐
│       │                            │
│       └── 主输入                    │
│                                     │
│   ScanVerticesNode (m) [ZeroInputNode]
│           │
│           ▼
│   ProjectNode [SingleInputNode] ───┘
│       (减输入)
```

### Intersect 查询结构

```
MATCH (n) RETURN n INTERSECT MATCH (m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── ProjectNode [SingleInputNode]
│       │
│       ▼
├── IntersectNode [BinaryInputNode] ◄──┐
│       │                              │
│       └── 主输入                      │
│                                       │
│   ScanVerticesNode (m) [ZeroInputNode]
│           │
│           ▼
│   ProjectNode [SingleInputNode] ─────┘
│       (交输入)
```

### 循环查询结构

```
MATCH (n)-[*1..3]->(m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── LoopNode [特殊节点]
│       │
│       ├──► ExpandNode (循环体) [SingleInputNode]
│       │           │
│       │           ▼
│       └──► AppendVerticesNode [SingleInputNode]
│
└── ProjectNode [SingleInputNode]
```

## 依赖关系验证

### 规则1：类型兼容性

连接节点的两个输入必须有兼容的模式（schema）：

```rust
impl HashInnerJoinNode {
    pub fn new(
        left_input: PlanNodeEnum,
        right_input: PlanNodeEnum,
        join_keys: Vec<Expression>,
    ) -> Result<Self, PlannerError> {
        // 验证输入模式兼容性
        let left_schema = left_input.output_schema()?;
        let right_schema = right_input.output_schema()?;
        
        if !schemas_compatible(&left_schema, &right_schema) {
            return Err(PlannerError::SchemaMismatch(
                "Join inputs have incompatible schemas".to_string()
            ));
        }
        
        Ok(Self {
            id: -1,
            left_input: Box::new(left_input),
            right_input: Box::new(right_input),
            join_keys,
            output_var: None,
            col_names: vec![],
            cost: 0.0,
        })
    }
}
```

### 规则2：循环依赖检测

计划节点不能形成循环依赖：

```rust
pub fn detect_cycle(node: &PlanNodeEnum) -> bool {
    let mut visited = HashSet::new();
    let mut stack = HashSet::new();
    
    fn dfs(
        node: &PlanNodeEnum,
        visited: &mut HashSet<i64>,
        stack: &mut HashSet<i64>,
    ) -> bool {
        if stack.contains(&node.id()) {
            return true; // 检测到循环
        }
        
        if visited.contains(&node.id()) {
            return false;
        }
        
        visited.insert(node.id());
        stack.insert(node.id());
        
        for child in node.dependencies() {
            if dfs(child, visited, stack) {
                return true;
            }
        }
        
        stack.remove(&node.id());
        false
    }
    
    dfs(node, &mut visited, &mut stack)
}
```

### 规则3：输入数量验证

```rust
impl UnionNode {
    pub fn new(
        inputs: Vec<PlanNodeEnum>,
        distinct: bool,
    ) -> Result<Self, PlannerError> {
        if inputs.len() < 2 {
            return Err(PlannerError::InvalidInput(
                "Union requires at least 2 inputs".to_string()
            ));
        }
        
        // 验证所有输入模式兼容
        let first_schema = inputs[0].output_schema()?;
        for input in &inputs[1..] {
            if !schemas_compatible(&first_schema, &input.output_schema()?) {
                return Err(PlannerError::SchemaMismatch(
                    "Union inputs have incompatible schemas".to_string()
                ));
            }
        }
        
        Ok(Self {
            id: -1,
            deps: inputs.into_iter().map(Box::new).collect(),
            distinct,
            output_var: None,
            col_names: vec![],
            cost: 0.0,
        })
    }
}
```

### 规则4：集合操作模式兼容

```rust
impl MinusNode {
    pub fn new(
        input: PlanNodeEnum,
        minus_input: PlanNodeEnum,
    ) -> Result<Self, PlannerError> {
        // 验证两个输入的模式兼容
        let input_schema = input.output_schema()?;
        let minus_schema = minus_input.output_schema()?;
        
        if !schemas_compatible(&input_schema, &minus_schema) {
            return Err(PlannerError::SchemaMismatch(
                "Minus inputs must have compatible schemas".to_string()
            ));
        }
        
        let col_names = input.col_names().to_vec();
        
        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![Box::new(input), Box::new(minus_input)],
            output_var: None,
            col_names,
            cost: 0.0,
        })
    }
}
```

## 与 nebula-graph 的依赖关系对比

### 依赖类型支持对比

| 依赖类型 | GraphDB | nebula-graph | 说明 |
|---------|---------|-------------|------|
| 零输入 | 支持 | 支持 | 两者都支持 |
| 单输入 | 支持 | 支持 | 两者都支持 |
| 双输入 | 支持 | 支持 | 两者都支持 |
| 多输入 | 支持 | 支持 | 两者都支持 |
| 循环依赖 | 支持 | 支持 | LoopNode 实现类似 |
| 条件分支 | 支持 | 支持 | SelectNode 实现类似 |

### 依赖验证机制对比

**nebula-graph**:
- 在 C++ 中使用虚函数和继承体系
- 依赖检查分散在各个节点的构造函数中
- 运行时检查为主

**GraphDB**:
- 使用 Rust 的类型系统和宏
- 在编译期通过类型系统保证部分依赖安全
- 运行时检查通过 Result 类型显式处理错误

```rust
// GraphDB 的方式：类型安全 + 显式错误处理
pub trait ZeroInputNode: PlanNode {}
pub trait SingleInputNode: PlanNode {
    fn input(&self) -> &PlanNodeEnum;
    fn input_mut(&mut self) -> &mut PlanNodeEnum;
}
pub trait BinaryInputNode: PlanNode {
    fn left_input(&self) -> &PlanNodeEnum;
    fn right_input(&self) -> &PlanNodeEnum;
}
```

## 优化器依赖处理

### 1. 下推过滤

尽可能将 FilterNode 下推到访问层：

```rust
pub fn push_down_filter(plan: &mut ExecutionPlan) {
    if let Some(filter) = plan.root_mut().as_filter_mut() {
        if let Some(scan) = filter.input_mut().as_scan_vertices_mut() {
            // 将过滤条件下推到扫描节点
            scan.add_filter(filter.condition().clone());
            // 用扫描节点替换过滤节点
            *plan.root_mut() = filter.input_mut().clone();
        }
    }
}
```

### 2. 连接重排

根据代价模型重排连接顺序：

```rust
pub fn reorder_joins(plan: &mut ExecutionPlan) {
    if let Some(join) = plan.root_mut().as_hash_inner_join_mut() {
        let left_cost = estimate_cost(join.left_input());
        let right_cost = estimate_cost(join.right_input());
        
        // 代价小的表作为构建表（哈希表）
        if left_cost > right_cost {
            std::mem::swap(&mut join.left_input, &mut join.right_input);
        }
    }
}
```

### 3. 子计划合并

合并连续的同类操作：

```rust
pub fn merge_consecutive_projects(plan: &mut ExecutionPlan) {
    if let (Some(outer), Some(inner)) = (
        plan.root().as_project(),
        plan.root().input().as_project()
    ) {
        // 如果两个 Project 相邻，合并列表达式
        let merged_columns = merge_columns(
            outer.columns(),
            inner.columns()
        );
        // 用合并后的 Project 替换
    }
}
```

### 4. 管道化执行优化

单输入节点支持管道化执行：

```rust
pub fn can_pipeline(node: &PlanNodeEnum) -> bool {
    matches!(
        node,
        PlanNodeEnum::Filter(_)
            | PlanNodeEnum::Project(_)
            | PlanNodeEnum::Limit(_)
            | PlanNodeEnum::Sort(_)
    )
}
```

### 5. 集合操作优化

```rust
pub fn optimize_set_operations(plan: &mut ExecutionPlan) {
    match plan.root() {
        PlanNodeEnum::Minus(minus) => {
            // 如果减输入为空，直接返回主输入
            if is_empty_input(minus.minus_input()) {
                *plan.root_mut() = minus.input().clone();
            }
        }
        PlanNodeEnum::Intersect(intersect) => {
            // 选择较小的输入作为构建表
            let input_size = estimate_size(intersect.input());
            let intersect_size = estimate_size(intersect.intersect_input());
            
            if intersect_size < input_size {
                // 交换输入顺序以优化性能
            }
        }
        _ => {}
    }
}
```

## 依赖关系文件组织

当前节点文件按依赖类型和功能组织：

```
src/query/planner/plan/core/nodes/
├── mod.rs                    # 模块导出
├── macros.rs                 # 节点定义宏
├── plan_node_traits.rs       # 依赖 trait 定义
├── plan_node_enum.rs         # 节点枚举
│
├── start_node.rs             # ZeroInputNode: Start
├── graph_scan_node.rs        # ZeroInputNode: ScanVertices, ScanEdges, EdgeIndexScan
│                              # SingleInputNode: GetVertices, GetEdges, GetNeighbors
│                              # Special: IndexScan, FulltextIndexScan
├── space_nodes.rs            # ZeroInputNode: 4 个空间管理节点
├── tag_nodes.rs              # ZeroInputNode: 5 个标签管理节点
├── edge_nodes.rs             # ZeroInputNode: 5 个边类型管理节点
├── index_nodes.rs            # ZeroInputNode: 10 个索引管理节点
├── user_nodes.rs             # ZeroInputNode: 4 个用户管理节点
│
├── filter_node.rs            # SingleInputNode: Filter
├── project_node.rs           # SingleInputNode: Project
├── aggregate_node.rs         # SingleInputNode: Aggregate
├── sort_node.rs              # SingleInputNode: Sort, Limit, TopN
├── sample_node.rs            # SingleInputNode: Sample
├── traversal_node.rs         # SingleInputNode: Expand, ExpandAll, Traverse, AppendVertices
├── control_flow_node.rs      # SingleInputNode: Argument, PassThrough
│                              # 特殊: Loop, Select
├── data_processing_node.rs   # SingleInputNode: Dedup, Unwind, Assign, RollUpApply
│                              # MultipleInputNode: Union, DataCollect
│                              # 特殊: PatternApply
├── join_node.rs              # BinaryInputNode: 5 个连接节点
├── set_operations_node.rs    # BinaryInputNode: Minus, Intersect
│
└── algorithms/               # 算法节点
    ├── mod.rs
    └── path_algorithms.rs    # ShortestPath, AllPaths, MultiShortestPath, BFSShortest
```

## 依赖关系统计

| 依赖类型 | 节点数量 | 占比 | 主要用途 |
|---------|---------|-----|---------|
| ZeroInputNode | 31 | 44.9% | 数据访问起点、DDL操作 |
| SingleInputNode | 19 | 27.5% | 数据转换、过滤、排序 |
| BinaryInputNode | 7 | 10.1% | 连接操作、集合操作 |
| MultipleInputNode | 2 | 2.9% | 并集、数据收集 |
| 特殊节点 | 10 | 14.5% | 控制流、索引扫描、算法 |
| **总计** | **69** | **100%** | - |

## 未来优化建议

### 1. 依赖图可视化

建议实现执行计划的可视化工具，展示节点间的依赖关系：

```rust
pub fn to_dot_format(plan: &ExecutionPlan) -> String {
    let mut dot = String::from("digraph Plan {\n");
    // 遍历节点并生成 Graphviz DOT 格式
    dot.push_str("}\n");
    dot
}
```

### 2. 依赖缓存

对于复杂的查询计划，可以缓存依赖关系避免重复计算：

```rust
pub struct DependencyCache {
    cache: HashMap<i64, Vec<i64>>, // 节点 ID -> 依赖节点 IDs
}
```

### 3. 并行依赖分析

对于多输入节点，可以并行分析各输入分支：

```rust
pub fn analyze_dependencies_parallel(node: &PlanNodeEnum) -> DependencyGraph {
    match node {
        PlanNodeEnum::Union(union) => {
            // 并行分析所有输入分支
            union.deps().par_iter().map(analyze).collect()
        }
        // ...
    }
}
```

### 4. 集合操作优化

针对 Minus 和 Intersect 节点，可以实现更多优化策略：

```rust
pub fn optimize_minus(node: &MinusNode) -> PlanNodeEnum {
    // 如果减输入是空集，直接返回主输入
    if is_empty(node.minus_input()) {
        return node.input().clone();
    }
    
    // 如果两个输入相同，返回空集
    if node.input() == node.minus_input() {
        return PlanNodeEnum::Start(StartNode::new());
    }
    
    PlanNodeEnum::Minus(node.clone())
}

pub fn optimize_intersect(node: &IntersectNode) -> PlanNodeEnum {
    // 如果交输入是空集，返回空集
    if is_empty(node.intersect_input()) {
        return PlanNodeEnum::Start(StartNode::new());
    }
    
    // 如果两个输入相同，返回任意一个
    if node.input() == node.intersect_input() {
        return node.input().clone();
    }
    
    PlanNodeEnum::Intersect(node.clone())
}
```
