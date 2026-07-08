# 图遍历执行器模块

本目录包含所有与图遍历相关的执行器实现。这些执行器负责处理图数据库中的遍历操作，如路径扩展、最短路径计算等。

## 文件说明

### 1. mod.rs

这是图遍历执行器模块的入口文件，定义了公共接口和可导出的类型。它包含了：

- **模块声明**：声明了 `expand`、`expand_all`、`shortest_path` 和 `traverse` 子模块
- **公共类型导出**：导出了各种执行器类型和最短路径算法枚举
- **统一trait定义**：定义了 `GraphTraversalExecutor` trait，为所有图遍历执行器提供了统一的接口
- **实现**：为各执行器实现了 `GraphTraversalExecutor` trait，提供设置边方向、边类型过滤和最大深度的方法
- **工厂模式**：提供了 `GraphTraversalExecutorFactory` 结构体，用于创建不同类型执行器的实例
- **单元测试**：包含了各执行器的基本功能测试

### 2. expand.rs

实现了 `ExpandExecutor`，这是一个单步扩展执行器，功能包括：

- 从当前节点按照指定的边类型和方向扩展一步，获取相邻节点
- 支持单向(`In`表示入边，`Out`表示出边，`Both`表示双向)边遍历
- 具备防止循环访问的机制，通过 `visited_nodes` 集合记录已访问节点
- 提供邻接关系缓存 `adjacency_cache` 以提高性能
- 继承自 `BaseExecutor`，实现 `Executor` trait
- 支持与其他执行器链接，作为流水线的一部分

典型应用场景：从某个节点开始查询其直接邻居节点。

### 3. expand_all.rs

实现了 `ExpandAllExecutor`，这是一个全路径扩展执行器，功能包括：

- 返回从当前节点出发的所有可能路径，而不是仅仅下一跳节点
- 支持递归路径扩展，使用深度优先搜索策略构建完整路径
- 实现了防止循环访问的机制，允许在特定情况下处理循环路径
- 维护路径缓存 `path_cache` 来存储发现的路径
- 支持最大深度限制以控制遍历范围
- 能够构建包含节点和边的完整路径结构

典型应用场景：探索从某个节点出发的所有可达路径。

### 4. shortest_path.rs

实现了 `ShortestPathExecutor`，这是一个最短路径执行器，功能包括：

- 计算图中两个节点之间的最短路径
- 支持多种算法：BFS（广度优先搜索）、Dijkstra（戴克斯特拉算法）和 A*（A星算法）
- 使用边的排序值作为权重评估路径成本
- 实现了路径重建功能，能够重构从源节点到目标节点的完整路径
- 维护距离映射表 `distance_map` 和前驱节点映射 `previous_map` 以进行最短路径计算
- 支持多个起点和终点的最短路径计算
- 使用队列或优先队列来管理待处理的节点

典型应用场景：社交网络中寻找两个人之间的最短关系链，或地理路线规划中的最短路径计算。

### 5. traverse.rs

实现了 `TraverseExecutor`，这是一个完整的图遍历执行器，功能包括：

- 执行完整的图遍历操作，支持多跳和条件过滤
- 结合了 `ExpandExecutor` 的功能，但提供更多高级遍历能力
- 支持在遍历过程中应用条件过滤，可以根据属性或其他标准筛选路径
- 管理多个路径的状态，通过 `current_paths` 和 `completed_paths` 跟踪遍历进度
- 提供灵活的配置选项，支持开关路径跟踪和路径生成
- 实现了防止无限循环的机制，通过访问节点集合进行跟踪
- 支持最大深度控制，限制遍历的跳跃次数

典型应用场景：复杂的关系挖掘，如查找满足特定条件的多跳关系链。

## 使用方法

每个执行器都可通过工厂模式创建：

```rust
use crate::query::executor::data_processing::graph_traversal::GraphTraversalExecutorFactory;

// 创建扩展执行器
let expand_executor = GraphTraversalExecutorFactory::create_expand_executor(
    1,                                    // ID
    storage.clone(),                      // 存储引擎
    EdgeDirection::Outgoing,              // 边方向
    Some(vec!["friend".to_string()]),     // 边类型过滤
    Some(2),                             // 最大深度
);

// 创建最短路径执行器
let shortest_path_executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
    2,                                   // ID
    storage.clone(),                     // 存储引擎
    vec![start_node_id],                 // 起始节点
    vec![end_node_id],                   // 结束节点
    EdgeDirection::Both,                 // 边方向
    Some(vec!["connect".to_string()]),   // 边类型过滤
    ShortestPathAlgorithm::BFS,          // 算法选择
);
```

## 设计理念

所有图遍历执行器遵循统一的设计理念：

- **模块化设计**：每个执行器专注于一项特定功能，易于维护和扩展
- **接口统一**：通过共同的trait保证所有执行器具有一致的外部接口
- **状态管理**：妥善管理内部状态（如访问历史、路径缓存），确保正确性
- **性能优化**：利用缓存和适当的数据结构提高遍历效率
- **错误处理**：对存储层错误和其他异常情况进行适当处理
- **资源管理**：在开启和关闭时进行适当的资源清理