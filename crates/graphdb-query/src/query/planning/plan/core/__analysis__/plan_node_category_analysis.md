# PlanNode 节点分类分析文档

## 概述

本文档描述 GraphDB 查询计划节点（PlanNode）的分类体系设计，基于功能特性和职责对节点进行分类，以提高代码可读性和可维护性。

## 当前节点清单

GraphDB 当前共有 **69** 个 PlanNode 类型，按功能分为 8 个类别。

## 分类体系

### 1. 访问层（Access Layer）- 9 个节点

**职责**：从存储层读取数据，是执行计划的起始点。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| StartNode | 起始节点，执行计划的入口 | 无 | StartNode |
| ScanVerticesNode | 全表扫描顶点 | 无 | ScanVertices |
| ScanEdgesNode | 全表扫描边 | 无 | ScanEdges |
| GetVerticesNode | 按ID/属性获取顶点 | 索引 | GetVertices |
| GetEdgesNode | 按ID/属性获取边 | 索引 | GetEdges |
| GetNeighborsNode | 获取顶点的邻居节点 | 顶点 | GetNeighbors |
| IndexScan | 索引扫描节点 | 索引 | IndexScan |
| EdgeIndexScan | 边索引扫描节点 | 索引 | EdgeIndexScan |
| FulltextIndexScan | 全文索引扫描 | 索引 | FulltextIndexScan |

### 2. 操作层（Operation Layer）- 8 个节点

**职责**：对数据进行转换、过滤、聚合等操作。

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

### 3. 连接层（Join Layer）- 5 个节点

**职责**：多数据流的连接操作。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| InnerJoinNode | 内连接 | 两个输入流 | InnerJoin |
| LeftJoinNode | 左连接 | 两个输入流 | LeftJoin |
| CrossJoinNode | 交叉连接 | 两个输入流 | CrossJoin |
| HashInnerJoinNode | 哈希内连接 | 两个输入流 | HashInnerJoin |
| HashLeftJoinNode | 哈希左连接 | 两个输入流 | HashLeftJoin |

**注意**：nebula-graph 中还有 RightJoin、FullOuterJoin 等，GraphDB 暂未实现。

### 4. 遍历层（Traversal Layer）- 4 个节点

**职责**：图数据的遍历和扩展。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ExpandNode | 扩展边 | 顶点 | Expand |
| ExpandAllNode | 全扩展 | 顶点 | ExpandAll |
| TraverseNode | 遍历 | 顶点/边 | Traverse |
| AppendVerticesNode | 追加顶点 | 顶点/遍历结果 | AppendVertices |

### 5. 控制流层（Control Flow Layer）- 4 个节点

**职责**：执行流程控制。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ArgumentNode | 参数传递 | 依赖特定 | Argument |
| LoopNode | 循环执行 | 循环体 | Loop |
| PassThroughNode | 直通传递 | 输入流 | PassThrough |
| SelectNode | 条件选择 | 多分支 | Select |

### 6. 数据处理层（Data Processing Layer）- 8 个节点

**职责**：复杂数据操作和转换。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| DataCollectNode | 数据收集 | 多输入流 | DataCollect |
| UnionNode | 并集操作 | 多输入流 | Union |
| MinusNode | 差集操作 | 两个输入流 | Minus |
| IntersectNode | 交集操作 | 两个输入流 | Intersect |
| UnwindNode | 展开数组 | 输入数据流 | Unwind |
| AssignNode | 变量赋值 | 输入数据流 | Assign |
| PatternApplyNode | 模式应用 | 模式匹配 | PatternApply |
| RollUpApplyNode | 上卷应用 | 聚合模式 | RollUpApply |

### 7. 算法层（Algorithm Layer）- 4 个节点

**职责**：图算法执行。

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| ShortestPath | 最短路径 | 起点/终点 | ShortestPath |
| AllPaths | 所有路径 | 起点/终点 | AllPaths |
| MultiShortestPath | 多源最短路径 | 多起点 | MultiShortestPath |
| BFSShortest | BFS最短路径 | 起点 | BFSShortest |

### 8. 管理/DDL层（Management Layer）- 27 个节点

**职责**：元数据管理和DDL操作。

#### 8.1 图空间管理（4 个）

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| CreateSpaceNode | 创建图空间 | 无 | CreateSpace |
| DropSpaceNode | 删除图空间 | 无 | DropSpace |
| DescSpaceNode | 描述图空间 | 无 | DescSpace |
| ShowSpacesNode | 显示所有图空间 | 无 | ShowSpaces |

#### 8.2 标签管理（5 个）

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| CreateTagNode | 创建标签 | 图空间 | CreateTag |
| AlterTagNode | 修改标签 | 标签 | AlterTag |
| DescTagNode | 描述标签 | 标签 | DescTag |
| DropTagNode | 删除标签 | 标签 | DropTag |
| ShowTagsNode | 显示所有标签 | 图空间 | ShowTags |

#### 8.3 边类型管理（5 个）

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| CreateEdgeNode | 创建边类型 | 图空间 | CreateEdge |
| AlterEdgeNode | 修改边类型 | 边类型 | AlterEdge |
| DescEdgeNode | 描述边类型 | 边类型 | DescEdge |
| DropEdgeNode | 删除边类型 | 边类型 | DropEdge |
| ShowEdgesNode | 显示所有边类型 | 图空间 | ShowEdges |

#### 8.4 索引管理（10 个）

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| CreateTagIndexNode | 创建标签索引 | 标签 | CreateTagIndex |
| DropTagIndexNode | 删除标签索引 | 索引 | DropTagIndex |
| DescTagIndexNode | 描述标签索引 | 索引 | DescTagIndex |
| ShowTagIndexesNode | 显示所有标签索引 | 图空间 | ShowTagIndexes |
| CreateEdgeIndexNode | 创建边索引 | 边类型 | CreateEdgeIndex |
| DropEdgeIndexNode | 删除边索引 | 索引 | DropEdgeIndex |
| DescEdgeIndexNode | 描述边索引 | 索引 | DescEdgeIndex |
| ShowEdgeIndexesNode | 显示所有边索引 | 图空间 | ShowEdgeIndexes |
| RebuildTagIndexNode | 重建标签索引 | 索引 | RebuildTagIndex |
| RebuildEdgeIndexNode | 重建边索引 | 索引 | RebuildEdgeIndex |

#### 8.5 用户管理（4 个）

| 节点类型 | 说明 | 依赖 | 对应 nebula-graph |
|---------|------|-----|------------------|
| CreateUserNode | 创建用户 | 无 | CreateUser |
| AlterUserNode | 修改用户 | 用户 | AlterUser |
| DropUserNode | 删除用户 | 用户 | DropUser |
| ChangePasswordNode | 修改密码 | 用户 | ChangePassword |

## 与 nebula-graph 的对比分析

### 节点数量对比

| 类别 | GraphDB | nebula-graph | 差异说明 |
|-----|---------|-------------|---------|
| 访问层 | 9 | 10+ | nebula-graph 有 TagIndexFullScan |
| 操作层 | 8 | 10+ | nebula-graph 有更多聚合函数 |
| 连接层 | 5 | 8+ | nebula-graph 支持更多连接类型 |
| 遍历层 | 4 | 6+ | nebula-graph 有更复杂的遍历节点 |
| 控制流层 | 4 | 6+ | nebula-graph 有更多控制节点 |
| 数据处理层 | 8 | 8+ | 基本对齐 |
| 算法层 | 4 | 6+ | nebula-graph 支持更多图算法 |
| 管理/DDL层 | 27 | 30+ | nebula-graph 有更完整的DDL支持 |
| **总计** | **69** | **80+** | GraphDB 精简了部分节点 |

### nebula-graph 有但 GraphDB 暂未实现的节点

#### 访问层
- **TagIndexFullScan**: 标签索引全扫描

#### 连接层
- **RightJoin**: 右连接
- **FullOuterJoin**: 全外连接
- **SemiJoin**: 半连接
- **AntiJoin**: 反连接

#### 遍历层
- **BiExpand**: 双向扩展
- **BiTraverse**: 双向遍历

#### 控制流层
- **BiLeftJoin**: 双向左连接
- **BiInnerJoin**: 双向内连接

#### 算法层
- **ProduceSemiShortestPath**: 生成半最短路径
- **ConjunctPath**: 连接路径

#### 管理/DDL层
- **AddHosts**: 添加主机（分布式特性）
- **DropHosts**: 删除主机（分布式特性）
- **Balance**: 数据均衡（分布式特性）
- **SubmitJob**: 提交作业
- **ShowJobs**: 显示作业
- **StopJob**: 停止作业
- **RecoverJob**: 恢复作业
- **AddListener**: 添加监听器
- **RemoveListener**: 移除监听器
- **ShowListener**: 显示监听器
- **SignInService**: 服务登录
- **SignOutService**: 服务登出
- **Download**: 下载
- **Ingest**: 数据导入

### 设计差异分析

#### 1. 分布式特性

**nebula-graph**: 作为分布式图数据库，包含大量与分布式相关的管理节点：
- AddHosts/DropHosts: 主机管理
- Balance: 数据均衡
- 各种 Job 相关节点

**GraphDB**: 专注于单机部署，移除了所有分布式相关节点，简化了架构。

#### 2. 双向遍历支持

**nebula-graph**: 支持双向遍历节点（BiExpand, BiTraverse, BiLeftJoin, BiInnerJoin），用于优化特定查询模式。

**GraphDB**: 暂未实现双向遍历节点，使用单向遍历组合实现相同功能。

#### 3. 集合操作

**nebula-graph**: 支持完整的集合操作（Union, Minus, Intersect）。

**GraphDB**: 已实现完整的集合操作（Union, Minus, Intersect）。

#### 4. 连接类型

**nebula-graph**: 支持完整的 SQL 连接类型（Inner, Left, Right, Full Outer, Semi, Anti）。

**GraphDB**: 目前只实现了 Inner、Left 和 Cross 连接，以及对应的 Hash 连接变体。

## 节点分类使用示例

### 节点分类识别

```rust
use crate::query::planner::plan::core::nodes::PlanNodeCategory;

impl PlanNodeEnum {
    /// 获取节点所属分类
    pub fn category(&self) -> PlanNodeCategory {
        match self {
            // 访问层
            PlanNodeEnum::Start(_) => PlanNodeCategory::Access,
            PlanNodeEnum::ScanVertices(_) => PlanNodeCategory::Access,
            PlanNodeEnum::ScanEdges(_) => PlanNodeCategory::Access,
            PlanNodeEnum::GetVertices(_) => PlanNodeCategory::Access,
            PlanNodeEnum::GetEdges(_) => PlanNodeCategory::Access,
            PlanNodeEnum::GetNeighbors(_) => PlanNodeCategory::Access,
            PlanNodeEnum::IndexScan(_) => PlanNodeCategory::Access,
            PlanNodeEnum::EdgeIndexScan(_) => PlanNodeCategory::Access,
            PlanNodeEnum::FulltextIndexScan(_) => PlanNodeCategory::Access,

            // 操作层
            PlanNodeEnum::Filter(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Project(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Aggregate(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Sort(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Limit(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::TopN(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Sample(_) => PlanNodeCategory::Operation,
            PlanNodeEnum::Dedup(_) => PlanNodeCategory::Operation,

            // ... 其他分类
        }
    }
}
```

### 优化器使用场景

1. **下推过滤**：操作层节点优先于访问层节点
2. **连接重排**：连接层节点根据代价模型重排
3. **索引使用**：访问层节点优先使用索引
4. **并行执行**：数据处理层节点可并行

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

## 文件组织

### 节点文件分布

| 文件 | 包含节点 | 数量 |
|-----|---------|-----|
| start_node.rs | StartNode | 1 |
| graph_scan_node.rs | ScanVerticesNode, ScanEdgesNode, GetVerticesNode, GetEdgesNode, GetNeighborsNode, IndexScanNode, EdgeIndexScanNode, FulltextIndexScanNode | 8 |
| filter_node.rs | FilterNode | 1 |
| project_node.rs | ProjectNode | 1 |
| aggregate_node.rs | AggregateNode | 1 |
| sort_node.rs | SortNode, LimitNode, TopNNode | 3 |
| sample_node.rs | SampleNode | 1 |
| join_node.rs | InnerJoinNode, LeftJoinNode, CrossJoinNode, HashInnerJoinNode, HashLeftJoinNode | 5 |
| traversal_node.rs | ExpandNode, ExpandAllNode, TraverseNode, AppendVerticesNode | 4 |
| control_flow_node.rs | ArgumentNode, LoopNode, PassThroughNode, SelectNode | 4 |
| data_processing_node.rs | DataCollectNode, UnionNode, UnwindNode, AssignNode, PatternApplyNode, RollUpApplyNode | 6 |
| set_operations_node.rs | MinusNode, IntersectNode | 2 |
| space_nodes.rs | CreateSpaceNode, DropSpaceNode, DescSpaceNode, ShowSpacesNode | 4 |
| tag_nodes.rs | CreateTagNode, AlterTagNode, DescTagNode, DropTagNode, ShowTagsNode | 5 |
| edge_nodes.rs | CreateEdgeNode, AlterEdgeNode, DescEdgeNode, DropEdgeNode, ShowEdgesNode | 5 |
| index_nodes.rs | CreateTagIndexNode, DropTagIndexNode, DescTagIndexNode, ShowTagIndexesNode, CreateEdgeIndexNode, DropEdgeIndexNode, DescEdgeIndexNode, ShowEdgeIndexesNode, RebuildTagIndexNode, RebuildEdgeIndexNode | 10 |
| user_nodes.rs | CreateUserNode, AlterUserNode, DropUserNode, ChangePasswordNode | 4 |

## 总结

GraphDB 的 PlanNode 分类体系设计遵循以下原则：

1. **职责单一**：每个节点只负责一种操作
2. **分类清晰**：按功能分为 8 个层次
3. **命名统一**：遵循统一的命名规范
4. **文件分离**：按功能分组到不同文件
5. **与 nebula-graph 对齐**：保持与原始设计的兼容性

当前 69 个节点覆盖了查询执行、数据处理和元数据管理的主要场景，能够满足大部分图数据库查询需求。
