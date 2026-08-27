# PlanNode 依赖关系分析文档

> 本文档与 2026-08-16 代码现状对齐（`plan_node_traits.rs` / `plan_node_enum.rs`，78/81 变体）。
> 依赖分类按当前 trait 体系（`ZeroInputNode` / `SingleInputNode` / `BinaryInputNode` /
> `MultipleInputNode`）+ 控制流特殊依赖 + 参数化管理节点 组织。
> 枚举 ↔ 本文档的一致性由 `plan_node_registry.rs` 中的一致性测试保证。

## 概述

本文档描述 GraphDB 查询计划节点（PlanNode）之间的依赖关系体系，帮助理解执行计划的拓扑结构和数据流。

## 依赖关系类型

根据节点的输入特性，PlanNode 分为以下 **4 类输入 trait + 1 类特殊控制流节点**。

### 1. 零输入节点（ZeroInputNode）- 35 个（qdrant 下 38 个）

**定义**：没有输入依赖的节点，作为执行计划的起始点/终止点。全部管理（DDL/DCL）节点与
全文/向量检索节点也归入此类——它们直接操作元数据或检索数据，不需要上游数据流。

**实现方式**：`define_plan_node!` 宏指定 `input: ZeroInputNode`，或显式 `impl ZeroInputNode for X {}`。

#### 1.1 访问层（无数据依赖）

| 节点类型 | 说明 | 文件位置 |
|---------|------|---------|
| StartNode | 执行计划入口 | `control_flow/start_node.rs` |
| GetEdgesNode | 按 ID 获取边 | `access/graph_scan_node.rs` |
| ScanVerticesNode | 全表扫描顶点 | `access/graph_scan_node.rs` |
| ScanEdgesNode | 全表扫描边 | `access/graph_scan_node.rs` |
| IndexScanNode | 索引扫描 | `access/index_scan.rs` |

> `GetVerticesNode` / `GetNeighborsNode` 为**多输入**节点（见第 4 节）：它们按上游提供的 ID 集合
> 获取顶点/邻居，属于访问层中的管道化节点。

#### 1.2 控制流与事务控制

| 节点类型 | 说明 | 文件位置 |
|---------|------|---------|
| ArgumentNode | 参数传递 | `control_flow/control_flow_node.rs` |
| PassThroughNode | 直通传递 | `control_flow/control_flow_node.rs` |
| BeginTransactionNode | 开启事务 | `control_flow/control_flow_node.rs` |
| CommitNode | 提交事务 | `control_flow/control_flow_node.rs` |
| RollbackNode | 回滚事务 | `control_flow/control_flow_node.rs` |
| SavepointNode | 建立保存点 | `control_flow/control_flow_node.rs` |
| ReleaseSavepointNode | 释放保存点 | `control_flow/control_flow_node.rs` |

> `SelectNode` / `LoopNode` 为**特殊控制流节点**（见第 5 节），不实现标准输入 trait。

#### 1.3 参数化管理节点（7 个子枚举）

`SpaceManage` / `TagManage` / `EdgeManage` / `IndexManage` / `UserManage` /
`FulltextManage` / `VectorManage`——由 `define_manage_node_enum!` 宏生成，
全部 `impl ZeroInputNode`，直接操作元数据。每个子枚举再包裹若干具体节点
（Space 8 / Tag 6 / Edge 6 / Index 12 / User 9 / Fulltext 5 / Vector 2，合计 48）。

#### 1.4 数据修改（DDL 数据）与统计/系统节点

| 节点类型 | 说明 | 输入 |
|---------|------|------|
| InsertVerticesNode | 插入顶点 | 零输入 |
| InsertEdgesNode | 插入边 | 零输入 |
| DeleteVerticesNode | 删除顶点 | 零输入 |
| DeleteEdgesNode | 删除边 | 零输入 |
| DeleteTagsNode | 删除标签属性 | 零输入 |
| DeleteIndexNode | 删除索引 | 零输入 |
| UpdateNode | 更新 | 零输入 |
| UpdateVerticesNode | 更新顶点 | 零输入 |
| UpdateEdgesNode | 更新边 | 零输入 |
| ShowStatsNode | 统计信息 | 零输入 |
| ShowConfigsNode / ShowQueriesNode / ShowSessionsNode | 系统信息 | 零输入 |

> `PipeDeleteVerticesNode` / `PipeDeleteEdgesNode` 为**单输入**（第 2 节）——它们消费上游查询结果按 ID 删除。

#### 1.5 全文 / 向量检索（DataAccess）

| 节点类型 | 输入 | feature 门控 |
|---------|------|-------------|
| FulltextSearchNode / FulltextLookupNode / MatchFulltextNode | 零输入 | 始终存在 |
| VectorSearchNode / VectorLookupNode / VectorMatchNode | 零输入 | **仅 `qdrant`** |

### 2. 单输入节点（SingleInputNode）- 23 个

**定义**：只有一个上游输入节点的节点。**特点**：
- 构成执行计划的主体，数据流从叶子节点流向根节点；
- 支持管道化执行。

**实现方式**：`define_plan_node_with_deps!` 宏（`input: SingleInputNode`，内部为
`input: Option<Box>` + `deps: Vec<Box>` 结构，支持 `deps[1..]` 附加输入），或显式实现 trait。

| 节点类型 | 说明 | 文件位置 |
|---------|------|---------|
| ProjectNode | 投影/列选择 | `operation/project_node.rs` |
| FilterNode | 条件过滤 | `operation/filter_node.rs` |
| SortNode / LimitNode / TopNNode | 排序/限制/TopN | `operation/sort_node.rs` |
| SampleNode | 采样 | `operation/sample_node.rs` |
| AggregateNode | 聚合运算 | `graph_operations/aggregate_node.rs` |
| WindowNode | 窗口函数 | `graph_operations/window_node.rs` |
| DedupNode | 去重 | `graph_operations/graph_operations_node.rs` |
| TraverseNode | 图遍历 | `traversal/traversal_node.rs` |
| UnionNode | 并集（`deps[1]` = union_input） | `graph_operations/graph_operations_node.rs` |
| UnwindNode | 展开数组 | `graph_operations/graph_operations_node.rs` |
| MinusNode | 差集（`deps[1]` = minus_input） | `graph_operations/set_operations_node.rs` |
| IntersectNode | 交集（`deps[1]` = intersect_input） | `graph_operations/set_operations_node.rs` |
| DataCollectNode | 数据收集 | `graph_operations/graph_operations_node.rs` |
| AssignNode | 变量赋值 | `graph_operations/graph_operations_node.rs` |
| MaterializeNode | 物化 | `graph_operations/graph_operations_node.rs` |
| PatternApplyNode | 模式应用子计划 | `graph_operations/graph_operations_node.rs` |
| RollUpApplyNode | 上卷应用 | `graph_operations/graph_operations_node.rs` |
| CorrelatedApplyNode | 相关子查询逐行重执行 | `graph_operations/graph_operations_node.rs` |
| RemoveNode | 移除属性 | `graph_operations/graph_operations_node.rs` |
| PipeDeleteVerticesNode / PipeDeleteEdgesNode | 管道删除 | `data_modification/delete_nodes.rs` |

> **集合操作（Union/Minus/Intersect）使用 `SingleInputNode` + `deps` 附加输入模型**：
> 主输入经 `input` 字段、附加输入经 `deps[1]`（`union_input()` / `minus_input()` /
> `intersect_input()`）。物理上为双流合并，逻辑建模统一为单输入 + 依赖列表，
> 与多输入 trait（第 4 节）不同。

### 3. 双输入节点（BinaryInputNode）- 13 个

**定义**：有两个上游输入节点的节点，用于连接操作、双向遍历与图算法。

**实现方式**：`define_join_node!` / `define_binary_input_node!` 宏
（内部 `left: Box` + `right: Box` + `deps: Vec`，并生成 `BinaryInputNode` 实现），
或显式实现 trait。

| 节点类型 | 说明 | 文件位置 |
|---------|------|---------|
| InnerJoinNode | 内连接 | `join/join_node.rs` |
| LeftJoinNode | 左连接 | `join/join_node.rs` |
| RightJoinNode | 右连接 | `join/join_node.rs` |
| CrossJoinNode | 交叉连接 | `join/join_node.rs` |
| FullOuterJoinNode | 全外连接 | `join/join_node.rs` |
| SemiJoinNode | 半连接 | `join/join_node.rs` |
| BiExpandNode | 双向扩展 | `traversal/traversal_node.rs` |
| BiTraverseNode | 双向遍历 | `traversal/traversal_node.rs` |
| MultiShortestPathNode | 多源最短路径 | `traversal/path_algorithms.rs` |
| BFSShortestNode | BFS 最短路径 | `traversal/path_algorithms.rs` |
| AllPathsNode | 所有路径 | `traversal/path_algorithms.rs` |
| ShortestPathNode | 最短路径 | `traversal/path_algorithms.rs` |
| ApplyNode | 应用子计划 | `graph_operations/graph_operations_node.rs` |

> 6 个 Join 节点额外实现 `JoinNode` trait（`hash_keys` / `probe_keys`）——
> 从 `define_join_node!` 宏生成，物理层统一映射为 `JoinSpec::HashJoin` 等。

### 4. 多输入节点（MultipleInputNode）- 4 个

**定义**：输入数量不固定（≥1）的节点。

| 节点类型 | 说明 | 文件位置 |
|---------|------|---------|
| GetVerticesNode | 按上游 ID 列表获取顶点 | `access/graph_scan_node.rs` |
| GetNeighborsNode | 按上游顶点取邻居 | `access/graph_scan_node.rs` |
| ExpandNode | 边扩展 | `traversal/traversal_node.rs` |
| AppendVerticesNode | 追加顶点 | `traversal/traversal_node.rs` |

### 5. 特殊控制流节点 - 3 个

**定义**：具有复杂依赖关系、**不实现标准输入 trait** 的手写控制流节点。

| 节点类型 | 说明 | 依赖结构 | 文件位置 |
|---------|------|---------|---------|
| SelectNode | 运行时选择 if/else 分支 | `condition` + `if_branch: Option<Box>` + `else_branch: Option<Box>` | `control_flow/control_flow_node.rs` |
| LoopNode | 循环执行 | `input` + `loop_body: Box`（循环体）+ `max_iterations` | `control_flow/control_flow_node.rs` |
| ExpandAllNode | 全扩展（多源/批次输入） | `deps: Vec` + `src_vids` + `input_var` + `join_input` | `traversal/traversal_node.rs` |

> `LoopNode` 保留为**通用控制流**；变量长度遍历（`[:TYPE*min..max]`）、最短/所有路径已由原生
> `RecursiveFragmentSpec`（SP/MultiSP/BFS/AllPaths 四变体）+ `variable_length_path_planner.rs`
> 实现，不再需要 Loop 模拟。早期文档中"变量长度遍历依赖 Loop 控制流模拟"的结论**已过时**。

---

## 依赖分布统计（按枚举变体，2026-08-16 核实）

| 输入类别 | 变体数（默认） | 变体数（qdrant） | 说明 |
|---------|--------------|-----------------|------|
| ZeroInputNode | 35 | 38 | 访问入口 + 控制/事务 + 管理 + 全文/向量检索 |
| SingleInputNode | 23 | 23 | 操作 + 集合 + 管道删除等 |
| BinaryInputNode | 13 | 13 | 连接 + 双向遍历 + 路径算法 + Apply |
| MultipleInputNode | 4 | 4 | GetVertices/GetNeighbors/Expand/AppendVertices |
| 特殊控制流（Select/Loop/ExpandAll） | 3 | 3 | 不实现标准输入 trait |
| **合计** | **78** | **81** | 与 `plan_node_enum.rs` 变体数一致 |

---

## 管理节点依赖关系

管理节点（DDL/DCL）绝大多数是**零输入**节点：它们直接操作元数据而不需要数据流输入。
两类例外是**单输入**的管道删除节点（`PipeDeleteVertices`/`PipeDeleteEdges`，消费上游查询结果）。

### 参数化管理子枚举（全部零输入）

| 子枚举 | 包裹的具体节点 | 数量 |
|-------|--------------|------|
| `SpaceManageNode` | Create/Drop/Desc/Show/ShowCreate/Switch/Alter/Clear SpaceNode | 8 |
| `TagManageNode` | Create/Alter/Desc/Drop/Show/ShowCreate TagNode | 6 |
| `EdgeManageNode` | Create/Alter/Desc/Drop/Show/ShowCreate EdgeNode | 6 |
| `IndexManageNode` | Tag/Edge 索引 Create/Drop/Desc/Show/Rebuild/ShowCreate IndexNode | 12 |
| `UserManageNode` | Create/Alter/Drop/ChangePassword/GrantRole/RevokeRole/Describe/ShowRoles/ShowUsersNode | 9 |
| `FulltextManageNode` | Create/Drop/Alter/Show/Describe FulltextIndexNode | 5 |
| `VectorManageNode` | Create/Drop VectorIndexNode | 2 |

---

## 依赖关系图示

### 典型查询计划结构（单输入管道）

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
├── GetNeighborsNode (n → e → m) [MultipleInputNode]
│       │
│       ▼
├── TraverseNode (e → m 遍历) [SingleInputNode]
│       │
│       ▼
├── InnerJoinNode (合并结果) [BinaryInputNode]
│       │
│       ▼
└── ProjectNode [SingleInputNode]
```

### Union 查询结构（单输入 + deps 模型）

```
MATCH (n) RETURN n UNION MATCH (m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── ProjectNode [SingleInputNode]
│       │
│       ▼
├── UnionNode [SingleInputNode + deps[1]=union_input] ◄──────┐
│       │                                                  │
│       ▼                                                  │
└── ProjectNode [SingleInputNode]                          │
                                                            │
    ScanVerticesNode (m) [ZeroInputNode]                   │
            │                                               │
            ▼                                               │
    ProjectNode [SingleInputNode] ─────────────────────────┘
                        (union_input, 经 deps[1])
```

### 循环查询结构（已由原生递归算子取代）

早期文档用 `LoopNode` 包裹 `ExpandNode` 模拟变量长度遍历，图形如下（示意）：
```
MATCH (n)-[*1..3]->(m) RETURN m
│
├── ScanVerticesNode (n) [ZeroInputNode]
│       │
│       ▼
├── LoopNode [特殊控制流]   ← 通用控制流，非路径查询专用
│       │
│       ├──► ExpandNode (循环体) [MultipleInputNode]
│       │
└── ProjectNode [SingleInputNode]
```

> **已过时说明**：当前 `[:TYPE*1..3]` 由 `variable_length_path_planner.rs` 规划为
> `VariableLengthPathPlan` IQM（`RecursiveFragmentSpec::*` 四变体），由原生
> `recursive_fragment_operator.rs` 执行（frontier/visited-set/路径前驱），
> **不再生成 Loop 结构**。见 `docs/analysis/计划节点类型对比分析.md` 修订。

---

## 与 nebula-graph 的依赖关系对比

### 依赖类型支持对比

| 依赖类型 | GraphDB | nebula-graph | 说明 |
|---------|---------|-------------|------|
| 零输入 | 支持（35/38 变体） | 支持 | DDL 与管理节点归零输入 |
| 单输入 | 支持（23 变体，含 deps 附加输入） | 支持 | 集合操作走 Single+deps 模型 |
| 双输入 | 支持（13 变体） | 支持 | + `JoinNode`（hash_keys/probe_keys） |
| 多输入 | 支持（4 变体） | 支持 | 输入数量不固定 |
| 循环/条件分支 | 支持（Loop/Select 特殊节点） | 支持 | 与 Nebula 同类设计 |

### trait 建模差异

**nebula-graph**: 基类 `dependencies_` 向量 + `SingleDependencyNode`/`SingleInputNode`/
`BinaryInputNode`/`VariableDependencyNode` 派生（运行时 `dynamic_cast`/`DCHECK` 校验，非穷尽）。

**GraphDB**: Rust trait 体系 `ZeroInputNode`/`SingleInputNode`/`BinaryInputNode`/`MultipleInputNode`
（编译期受穷尽性保证）。`define_plan_node_with_deps!` 为单输入节点叠加 `deps: Vec<Box>`，
统一表达"主输入 + 附加输入"。

```rust
// GraphDB 的方式：类型安全 + 穷尽 match
pub trait ZeroInputNode: PlanNode { fn input_count(&self) -> usize { 0 } }
pub trait SingleInputNode: PlanNode {
    fn input(&self) -> &PlanNodeEnum;
    fn input_mut(&mut self) -> &mut PlanNodeEnum;
}
pub trait BinaryInputNode: PlanNode {
    fn left_input(&self) -> &PlanNodeEnum;
    fn right_input(&self) -> &PlanNodeEnum;
}
pub trait MultipleInputNode: PlanNode {
    fn inputs(&self) -> &[PlanNodeEnum];
}
```

---

## 依赖关系验证（编译器保证 + 运行时检查边界）

- **编译期**：穷尽 match（`is_*`/`as_*`/`category`/`type_name`/`describe`）保证新增变体不会漏分支；
- **运行时**：仅系统边界校验（如连接输入 schema 兼容性由规划器 `PlannerError` 显式处理），
  内部通过类型系统保证依赖安全；
- **一致性**：枚举变体 ↔ 本文档 ↔ `plan_node_registry.rs` 由一致性测试门禁，
  防止 docs 漂移后再现早期"69 节点/8 类/HashInnerJoin 旧模型"的失联问题。

---

## 总结

GraphDB 的 PlanNode 依赖体系设计遵循以下原则：

1. **类型安全**：Rust trait 体系 + 穷尽 match，编译期保证；
2. **单一模型**：单输入节点用 `SingleInputNode + deps` 统一表达"主输入 + 附加输入"；
3. **管理归零**：DDL/DCL 全部零输入，参数化子枚举压缩变体数（90+ → 7）；
4. **控制流独立**：Loop/Select/ExpandAll 为特殊节点，不强行套输入 trait；
5. **文档同步**：与 `plan_node_enum.rs`（78/81 变体）、`plan_node_registry.rs` 三方一致。

当前 78（81 qdrant）个节点按 4 类输入 trait + 3 特殊控制流组织，覆盖查询执行、数据处理、
事务控制、全文/向量检索和元数据管理的全部流向。
