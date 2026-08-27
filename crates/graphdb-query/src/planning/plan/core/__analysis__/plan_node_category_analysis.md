# PlanNode 节点分类分析文档

> 本文档与 2026-08-16 代码现状对齐（`plan_node_enum.rs`，78/81 变体、9 类）。
> 枚举 ↔ 文档的一致性由 `plan_node_registry.rs` 中的一致性测试保证，
> 新增/删除节点时请同步更新本文档与注册表。
>
> 变体计数说明：默认构建（无 `qdrant` feature）为 **78** 个变体；
> 启用 `qdrant` feature 后追加 `VectorSearch`/`VectorLookup`/`VectorMatch` 3 个变体，共 **81** 个。

## 概述

本文档描述 GraphDB 查询计划节点（PlanNode）的分类体系设计，基于功能特性和职责对节点进行分类，以提高代码可读性和可维护性。

## 当前节点清单

GraphDB 当前共有 **78** 个 PlanNode 类型（默认构建；启用 `qdrant` feature 时为 **81** 个），按功能分为 **9** 个类别（`PlanNodeCategory`）。

## 分类体系

### 1. 访问层（Access Layer）- 7 个节点

**职责**：从存储层读取数据，是执行计划的起始点。`is_leaf() = true`。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| StartNode | 起始节点，执行计划的入口 | 无 | StartNode |
| ScanVerticesNode | 全表扫描顶点 | 无 | ScanVertices |
| ScanEdgesNode | 全表扫描边 | 无 | ScanEdges |
| GetVerticesNode | 按ID/属性获取顶点 | 索引 | GetVertices |
| GetEdgesNode | 按ID/属性获取边 | 索引 | GetEdges |
| GetNeighborsNode | 获取顶点的邻居节点 | 顶点 | GetNeighbors |
| IndexScanNode | 索引扫描节点 | 索引 | IndexScan |

> **注**：`FulltextIndexScan` 在枚举中**不存在**；全文检索能力由 `DataAccess` 类的
> `FulltextSearchNode`/`FulltextLookupNode`/`MatchFulltextNode` 表达（见第 9 节）。

### 2. 操作层（Operation Layer）- 9 个节点

**职责**：对数据进行转换、过滤、聚合等操作。`supports_parallelism() = true`。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| FilterNode | 条件过滤 | 输入数据流 | Filter |
| ProjectNode | 投影/列选择 | 输入数据流 | Project |
| AggregateNode | 聚合运算（GROUP BY） | 输入数据流 | Aggregate |
| SortNode | 排序 | 输入数据流 | Sort |
| LimitNode | 限制返回行数 | 输入数据流 | Limit |
| TopNNode | Top N 排序 | 输入数据流 | TopN |
| SampleNode | 采样 | 输入数据流 | Sample |
| DedupNode | 去重 | 输入数据流 | Dedup |
| WindowNode | 窗口函数 | 输入数据流 | Window |

### 3. 连接层（Join Layer）- 6 个节点

**职责**：多数据流的连接操作（`JoinNode` trait，统一 `hash_keys`/`probe_keys` 接口）。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| InnerJoinNode | 内连接 | 两个输入流 | InnerJoin |
| LeftJoinNode | 左连接 | 两个输入流 | LeftJoin |
| RightJoinNode | 右连接 | 两个输入流 | 无（Nebula 无独立节点） |
| CrossJoinNode | 交叉连接 | 两个输入流 | CrossJoin |
| FullOuterJoinNode | 全外连接 | 两个输入流 | 无（Nebula 无独立节点） |
| SemiJoinNode | 半连接 | 两个输入流 | 无（Nebula 无独立节点） |

> **注**：逻辑层 Join 与物理 Hash 实现解耦——物理层通过 `JoinSpec::HashJoin`/`HashLeftJoin`
> 等 spec 表达执行算法，逻辑枚举中**没有** `HashInnerJoin`/`HashLeftJoin` 变体。
> 旧文档中的 `HashInnerJoinNode`/`HashLeftJoinNode`/`FulltextIndexScanNode` 均为**已删除的旧模型**，请勿再引用。

### 4. 遍历层（Traversal Layer）- 6 个节点

**职责**：图数据的遍历和扩展。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ExpandNode | 扩展边 | 顶点 | Expand |
| ExpandAllNode | 全扩展 | 顶点 | ExpandAll |
| TraverseNode | 遍历 | 顶点/边 | Traverse |
| AppendVerticesNode | 追加顶点 | 顶点/遍历结果 | AppendVertices |
| BiExpandNode | 双向扩展 | 顶点 | 无独立节点 |
| BiTraverseNode | 双向遍历 | 顶点/边 | 无独立节点 |

> **注**：`BiExpand`/`BiTraverse` 为**已实现**的原生双向遍历节点（不再"暂未实现"）。
> 变量长度遍历（`[:TYPE*min..max]`）由 `RecursiveFragmentSpec`（SP/MultiSP/BFS/AllPaths 四变体）
> + `variable_length_path_planner.rs` 原生实现，不再依赖 `Loop` 控制流模拟。

### 5. 控制流层（Control Flow Layer）- 9 个节点

**职责**：执行流程控制与事务控制。`is_root() = true`。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ArgumentNode | 参数传递 | 依赖特定 | Argument |
| LoopNode | 循环执行（通用控制流） | 循环体 | Loop |
| PassThroughNode | 直通传递 | 输入流 | PassThrough |
| SelectNode | 条件选择 | 多分支 | Select |
| BeginTransactionNode | 开启事务 | 无 | （事务控制） |
| CommitNode | 提交事务 | 无 | （事务控制） |
| RollbackNode | 回滚事务 | 无 | （事务控制） |
| SavepointNode | 保存点 | 无 | （事务控制） |
| ReleaseSavepointNode | 释放保存点 | 无 | （事务控制） |

> **注**：`BeginTransaction`/`Commit`/`Rollback`/`Savepoint`/`ReleaseSavepoint` 为事务控制节点，
> 早期文档未收录。`LoopNode` 仍保留为通用控制流；路径类查询已走原生递归片段算子。

### 6. 数据处理层（Data Processing Layer）- 12 个节点

**职责**：复杂数据操作和转换。`supports_parallelism() = true`，`is_root() = true`。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| DataCollectNode | 数据收集 | 多输入流 | DataCollect |
| UnionNode | 并集操作 | 多输入流 | Union |
| MinusNode | 差集操作 | 两个输入流 | Minus |
| IntersectNode | 交集操作 | 两个输入流 | Intersect |
| UnwindNode | 展开数组 | 输入数据流 | Unwind |
| AssignNode | 变量赋值 | 输入数据流 | Assign |
| MaterializeNode | 物化中间结果 | 输入数据流 | 无 |
| ApplyNode | 应用子计划 | 输入数据流 | 无 |
| PatternApplyNode | 模式应用 | 模式匹配 | PatternApply |
| RollUpApplyNode | 上卷应用 | 聚合模式 | RollUpApply |
| CorrelatedApplyNode | 相关子查询逐行重执行 | 输入数据流 | 无 |
| RemoveNode | 移除属性 | 输入数据流 | 无 |

> **注**：`Materialize`/`Apply`/`CorrelatedApply` 为后续补充的节点，早期文档未收录。
> 其中 `CorrelatedApply` 用于相关子查询的逐行重执行。

### 7. 算法层（Algorithm Layer）- 4 个节点

**职责**：图算法执行。`supports_parallelism() = true`。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ShortestPathNode | 最短路径 | 起点/终点 | ShortestPath |
| AllPathsNode | 所有路径 | 起点/终点 | AllPaths |
| MultiShortestPathNode | 多源最短路径 | 多起点 | MultiShortestPath |
| BFSShortestNode | BFS最短路径 | 起点 | BFSShortest |

### 8. 管理/DDL层（Management Layer）- 22 个节点（7 参数化 + 11 数据修改 + 4 统计/系统）

**职责**：元数据管理、DDL 操作与数据增删改。管理节点通过**参数化子枚举**组织
（`manage_node_enums.rs`），将 90+ 变体压缩为 7 个类别变体。

#### 8.1 参数化管理子枚举（7 个）

| 子枚举变体 | 包裹的子枚举 | 子变体数 | 说明 |
|-----------|-------------|---------|------|
| `SpaceManage` | `SpaceManageNode` | 8 | Create/Drop/Desc/Show/ShowCreate/Switch/Alter/Clear Space |
| `TagManage` | `TagManageNode` | 6 | Create/Alter/Desc/Drop/Show/ShowCreate Tag |
| `EdgeManage` | `EdgeManageNode` | 6 | Create/Alter/Desc/Drop/Show/ShowCreate Edge |
| `IndexManage` | `IndexManageNode` | 12 | Tag/Edge 索引的 Create/Drop/Desc/Show/Rebuild/ShowCreate |
| `UserManage` | `UserManageNode` | 9 | Create/Alter/Drop/ChangePassword/GrantRole/RevokeRole/Describe/ShowRoles/ShowUsers |
| `FulltextManage` | `FulltextManageNode` | 5 | Create/Drop/Alter/Show/Describe Fulltext Index |
| `VectorManage` | `VectorManageNode` | 2 | Create/Drop Vector Index |

#### 8.2 数据修改节点（11 个）

| 节点类型 | 说明 |
|---------|------|
| InsertVerticesNode | 插入顶点 |
| InsertEdgesNode | 插入边 |
| DeleteVerticesNode | 删除顶点 |
| DeleteEdgesNode | 删除边 |
| DeleteTagsNode | 删除标签属性 |
| DeleteIndexNode | 删除索引 |
| PipeDeleteVerticesNode | 管道删除顶点 |
| PipeDeleteEdgesNode | 管道删除边 |
| UpdateNode | 更新（通用） |
| UpdateVerticesNode | 更新顶点 |
| UpdateEdgesNode | 更新边 |

#### 8.3 统计与系统信息节点（4 个）

| 节点类型 | 说明 |
|---------|------|
| ShowStatsNode | 显示统计信息 |
| ShowConfigsNode | 显示配置 |
| ShowQueriesNode | 显示查询 |
| ShowSessionsNode | 显示会话 |

### 9. 数据访问层（Data Access Layer）- 3 个节点（qdrant 下 +3 = 6）

**职责**：全文检索与向量检索等数据访问操作。`is_leaf() = true`。

| 节点类型 | 说明 | feature 门控 | 对应 nebula-graph |
|---------|------|-------------|------------------|
| FulltextSearchNode | 全文搜索 | 始终存在 | FulltextIndexScan |
| FulltextLookupNode | 全文查找 | 始终存在 | 无 |
| MatchFulltextNode | 全文匹配 | 始终存在 | 无 |
| VectorSearchNode | 向量搜索 | **仅 `qdrant`** | 无 |
| VectorLookupNode | 向量查找 | **仅 `qdrant`** | 无 |
| VectorMatchNode | 向量匹配 | **仅 `qdrant`** | 无 |

> **重要**：`DataAccess` 类并非"始终为 3 个节点"——向量三个节点以
> `#[cfg(feature = "qdrant")]` 门控（`plan_node_enum.rs:237-242`）。
> 默认构建的枚举形态（78 变体）与 feature 全开形态（81 变体）**不一致**，
> 讨论"全部节点"时必须以 feature 为前提，否则会产生"默认构建即含全部节点"的误解。

## 与 nebula-graph 的对比分析

### 节点数量对比

| 类别 | GraphDB | nebula-graph | 差异说明 |
|-----|---------|-------------|---------|
| 访问层 | 7 | 11 | Nebula 有 TagIndexFullScan 及多种前缀/范围扫描 |
| 操作层 | 9 | 10+ | Nebula 有更多聚合函数 |
| 连接层 | 6 | 4+ | **GraphDB 更全**：多出 RightJoin/FullOuterJoin/SemiJoin |
| 遍历层 | 6 | 8 | Nebula 有 TagIndexFullScan 系 |
| 控制流层 | 9 | 4 | GraphDB 多出 5 个事务控制节点 |
| 数据处理层 | 12 | 5+ | GraphDB 多出 Materialize/Apply/CorrelatedApply/Remove 等 |
| 算法层 | 4 | 5+ | Nebula 多 kSubgraph |
| 管理/DDL层 | 22（变体）/ 7（参数化）+ 11 + 4 | ~86 | Nebula 为分布式集群管理命令（AddHosts/Balance/Job 等），GraphDB 为单机视角 |
| 数据访问层 | 3（+3 qdrant） | 1（FulltextIndexScan） | GraphDB 多出全文/向量检索节点 |
| **总计** | **78（81 qdrant）** | **136** | GraphDB 精简了分布式管理节点 |

> Nebula 136 个节点中约 50 个为查询节点、约 86 个为管理/元数据命令（参考
> `docs/analysis/计划节点类型对比分析.md` 的核实结论）。

### nebula-graph 有但 GraphDB 未实现的节点

#### 管理/DDL层（分布式特性，单机场景暂无对应执行价值）
- **AddHosts / DropHosts**: 主机管理（分布式特性）
- **Balance**: 数据均衡（分布式特性）
- **SubmitJob / ShowJobs / StopJob / RecoverJob**: 作业管理
- **AddListener / RemoveListener / ShowListener**: 监听器管理
- **SignInService / SignOutService**: 服务登录/登出
- **Download / Ingest**: 下载与数据导入

#### 查询层
- **TagIndexFullScan** 及 `kTagIndexPrefixScan`/`kTagIndexRangeScan` 等：由 `IndexScanNode` 统一覆盖
- **Subgraph**: 子图查询（`kSubgraph`），暂未实现

### 设计差异分析

#### 1. 分布式特性

**nebula-graph**: 作为分布式图数据库，包含大量与分布式相关的管理节点：
- AddHosts/DropHosts: 主机管理
- Balance: 数据均衡
- 各种 Job 相关节点

**GraphDB**: 专注于单机部署，移除了所有分布式相关节点，简化了架构。
未来若推进分布式，管理面需要**扩充**（AddHosts/Balance/SubmitJob/Snapshot 等），而非缩减。

#### 2. 双向遍历支持

**nebula-graph**: 无独立双向遍历节点（靠 kTraverse + 筛选实现）。

**GraphDB**: **已实现**原生双向遍历节点（`BiExpand`/`BiTraverse`），用于优化特定查询模式。

#### 3. 集合操作

**nebula-graph**: 支持完整的集合操作（Union, Minus, Intersect）。

**GraphDB**: 已实现完整的集合操作（Union, Minus, Intersect）。

#### 4. 连接类型

**nebula-graph**: 支持 Inner/Left/Cross + Hash 变体；**无** Right/FullOuter/Semi 独立节点。

**GraphDB**: **更全**——实现 Inner/Left/Right/Cross/FullOuter/Semi 全部 6 种逻辑连接；
物理层经 `JoinSpec` 落地（`HashJoin`/`HashLeftJoin`/`NestedLoopJoin`/`CrossSemiJoin` 等）。

## 节点分类使用示例

### 节点分类识别

分类通过 `PlanNodeEnum::category()` 提供（由宏生成），无需手写 match：

```rust
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

fn describe(node: &PlanNodeEnum) -> &'static str {
    match node.category() {
        PlanNodeCategory::Access => "Access layer - reads data from the storage layer",
        PlanNodeCategory::Operation => "Operational Layer - Data Conversion and Filtering",
        PlanNodeCategory::Join => "Connection Layer - Multi-Stream Connectivity",
        PlanNodeCategory::Traversal => "Traversal Layer - Graph Traversal and Extension",
        PlanNodeCategory::ControlFlow => "Control Flow Layer - Performs process control",
        PlanNodeCategory::DataProcessing => "Data Processing Layer - Complex Data Manipulation",
        PlanNodeCategory::Algorithm => "Algorithm Layer - Graph Algorithm Execution",
        PlanNodeCategory::Management => "Management/DDL Layer - Metadata Management",
        PlanNodeCategory::DataAccess => "Data access layer - full text search and other data access operations",
    }
}
```

### 类别语义方法

`PlanNodeCategory` 提供三类语义方法，直接服务于优化器决策：

| 方法 | 返回 true 的类别 | 用途 |
|------|-----------------|------|
| `is_leaf()` | Access、DataAccess | 叶子节点（无数据依赖），作为计划起点 |
| `is_root()` | ControlFlow、DataProcessing | 根节点（无下游依赖），作为计划出口 |
| `supports_parallelism()` | Operation、DataProcessing、Algorithm | 支持并行执行的节点 |

### 优化器使用场景

1. **下推过滤**：操作层节点优先于访问层节点
2. **连接重排**：连接层节点根据代价模型重排
3. **索引使用**：访问层节点优先使用索引
4. **并行执行**：`supports_parallelism()` 为 true 的节点可并行

## 命名规范

### 统一命名规则

| 分类 | 前缀 | 示例 |
|-----|------|-----|
| 访问层 | Scan/Get | ScanVertices, GetNeighbors |
| 操作层 | Filter/Project/Aggregate | Filter, Project, Aggregate |
| 连接层 | Join | InnerJoin, LeftJoin |
| 遍历层 | Expand/Traverse | Expand, Traverse |
| 控制流 | Loop/Select/Argument | Loop, Select |
| 数据处理 | Union/Minus/Intersect | Union, Minus, Intersect |
| 算法层 | ShortestPath/AllPaths | ShortestPath, AllPaths |
| 管理/DDL | Create/Drop/Alter/Show | CreateSpace, DropTag |
| 数据访问 | Search/Lookup/Match | FulltextSearch, VectorLookup |

## 文件组织

### 节点文件分布

| 文件 | 包含节点 | 数量 |
|-----|---------|-----|
| `control_flow/start_node.rs` | StartNode | 1 |
| `access/graph_scan_node.rs` | ScanVerticesNode, ScanEdgesNode, GetVerticesNode, GetEdgesNode, GetNeighborsNode | 5 |
| `access/index_scan.rs` | IndexScanNode | 1 |
| `operation/filter_node.rs` | FilterNode | 1 |
| `operation/project_node.rs` | ProjectNode | 1 |
| `operation/sort_node.rs` | SortNode, LimitNode, TopNNode | 3 |
| `operation/sample_node.rs` | SampleNode | 1 |
| `graph_operations/aggregate_node.rs` | AggregateNode | 1 |
| `graph_operations/window_node.rs` | WindowNode | 1 |
| `graph_operations/graph_operations_node.rs` | DataCollectNode, DedupNode, UnionNode, UnwindNode, AssignNode, MaterializeNode, ApplyNode, PatternApplyNode, RollUpApplyNode, CorrelatedApplyNode, RemoveNode | 11 |
| `graph_operations/set_operations_node.rs` | MinusNode, IntersectNode | 2 |
| `join/join_node.rs` | InnerJoinNode, LeftJoinNode, RightJoinNode, CrossJoinNode, FullOuterJoinNode, SemiJoinNode | 6 |
| `traversal/traversal_node.rs` | ExpandNode, ExpandAllNode, TraverseNode, AppendVerticesNode, BiExpandNode, BiTraverseNode | 6 |
| `traversal/path_algorithms.rs` | ShortestPathNode, AllPathsNode, MultiShortestPathNode, BFSShortestNode | 4 |
| `control_flow/control_flow_node.rs` | ArgumentNode, LoopNode, PassThroughNode, SelectNode, BeginTransactionNode, CommitNode, RollbackNode, SavepointNode, ReleaseSavepointNode | 9 |
| `management/manage_node_enums.rs` | SpaceManageNode, TagManageNode, EdgeManageNode, IndexManageNode, UserManageNode, FulltextManageNode, VectorManageNode | 7 |
| `data_modification/insert_nodes.rs` | InsertVerticesNode, InsertEdgesNode | 2 |
| `data_modification/delete_nodes.rs` | DeleteVerticesNode, DeleteEdgesNode, DeleteTagsNode, DeleteIndexNode, PipeDeleteVerticesNode, PipeDeleteEdgesNode | 6 |
| `data_modification/update_nodes.rs` | UpdateNode, UpdateVerticesNode, UpdateEdgesNode | 3 |
| `management/stats_nodes.rs` | ShowStatsNode | 1 |
| `management/system_nodes.rs` | ShowConfigsNode, ShowQueriesNode, ShowSessionsNode | 3 |
| `search/fulltext/data_access.rs` | FulltextSearchNode, FulltextLookupNode, MatchFulltextNode | 3 |
| `search/vector/data_access.rs` | VectorSearchNode, VectorLookupNode, VectorMatchNode（qdrant） | 3 |
| `management/*/`（子枚举内） | 各管理子枚举包裹的具体节点（见 8.1） | 48 |

> 具体管理节点文件分布：`management/space_nodes.rs`（8）、`tag_nodes.rs`（6）、
> `edge_nodes.rs`（6）、`index_nodes.rs`（12）、`user_nodes.rs`（9）、
> `search/fulltext/management.rs`（5）、`search/vector/management.rs`（2）。

## 总结

GraphDB 的 PlanNode 分类体系设计遵循以下原则：

1. **职责单一**：每个节点只负责一种操作
2. **分类清晰**：按功能分为 9 个层次
3. **命名统一**：遵循统一的命名规范
4. **文件分离**：按功能分组到不同文件
5. **与 nebula-graph 对齐**：保持与原始设计的兼容性（查询层）；管理层以参数化子枚举压缩变体数
6. **文档同步**：枚举 ↔ 本文档 ↔ `plan_node_registry.rs` 三方一致，由一致性测试门禁

当前 78（81 qdrant）个节点覆盖了查询执行、数据处理、事务控制、全文/向量检索和元数据管理的
主要场景；分布式管理节点（AddHosts/Balance/Job 等）留待未来分布式演进时扩充。
