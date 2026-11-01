# 递归查询操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的递归查询操作：变长路径、最短路径算法与
`WITH RECURSIVE` 不动点（fixpoint）迭代的算子化实现。
本文档与 2026-11-01 代码现状对齐。

递归类查询共享同一物理算子 `RecursiveFragmentOperator`：以显式递归片段
（recursive fragment）的形式反复执行子计划，直到产出收敛或达到迭代上限。

---

## 一、语义类型（RecursiveFragmentSpec）

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/recursive.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::RecursiveFragmentSpec`）

| 变体 | 说明 | 关键字段 |
|------|------|----------|
| `ShortestPath` | 起点→目标点的**双向 BFS 最短路径**；指定 `weight` 边属性时退化为 Dijkstra 加权最短路（`algorithms::weighted_shortest_path`） | `edge_types`、`direction`、`max_depth`、`start/target_vertices`、`weight` |
| `MultiShortestPath` | 多源多目标最短路（双向 BFS） | `left/right_vertex_column`、`single_shortest` |
| `BFSShortest` | 可配置深度与环路策略的 BFS 遍历 | `allow_loops`、`max_depth` |
| `AllPaths` | 枚举起点→目标点间所有路径 | `min/max_depth`、`path_uniqueness`（路径唯一性语义）、`limit`/`offset` |
| `Fixpoint` | **递归 CTE 不动点迭代**：`WITH [RECURSIVE] name AS (anchor [UNION ALL step])` | `cte_name`（混淆后标签）、`anchor`（锚点子计划，执行一次）、`step`（迭代子计划，可为 None 即非递归内联视图）、`max_iterations` |

公共约定：frontier（波前）、visited-set（去重集）、路径前驱、结果队列的内存全部计入
查询内存池；每轮迭代与批量扩展均检查取消令牌（cancellation token），支持中断。
`vertex_tag` 用于路径顶点的物化标签。

---

## 二、RecursiveFragmentOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/recursive_fragment_operator.rs`

`RecursiveFragmentOperator` 由 `from_spec` 实例化，状态枚举
`RecursiveFragmentOperatorKind` 与五种 spec 变体对应，
不动点状态由 `FixpointState` 管理（`:91`）。

### 2.1 Fixpoint 迭代流程

```
1. anchor 子计划执行一次，结果发布为 CTE 工作表（初始 delta）
2. run_fixpoint 循环：
   a. 以混淆后的 cte_name 发布上一轮 delta 为工作表
   b. step 子计划经 CteScan 源读取工作表（见 scan_operation.md 的 CteScan 变体），
      重新执行（materialize_child / child_bindings）
   c. 产出新 delta；无新行或达到 max_iterations（超限报错）则终止
3. collect_all 汇总输出（V1 输出为单列，col_names）
```

### 2.2 关键方法

| 方法 | 位置 | 作用 |
|------|------|------|
| `next` | `:241` | 主循环：非递归变体执行一次；Fixpoint 走 `next_fixpoint` |
| `run_fixpoint` | `:790` | 不动点迭代主逻辑 |
| `materialize_child` | `:853` | 物化子计划一轮的输出 |
| `collect_all` | `:868` | 汇总结果行 |
| `bind_runtime` | `:700` | 子计划注入运行时 |

---

## 三、与遍历算子的关系

- **短跳遍历**（有限步数、无路径枚举语义）由 Expand/Traverse 算子族直接完成
  （见 expand_operation.md），不需要递归片段；
- **路径算法与 CTE**（最短路、全路径枚举、递归 CTE）跨多轮迭代且轮次不可静态展开，
  统一走 `RecursiveFragmentOperator`；
- 逻辑层对应节点：`MultiShortestPathNode`、`BFSShortestNode`
  （`planning/plan/core/nodes/traversal/path_algorithms.rs`），
  路径唯一性语义 `PathUniqueness` 亦定义于该文件。

---

## 四、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **递归执行算子** | `crates/linkrs-query/src/executor/streaming/operators/recursive_fragment_operator.rs` |
| **物理 RecursiveFragmentSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/recursive.rs` |
| **路径算法节点/唯一性语义** | `crates/linkrs-query/src/planning/plan/core/nodes/traversal/path_algorithms.rs` |
| **CTE 工作表扫描** | `crates/linkrs-query/src/executor/streaming/operators/source_operator.rs`（`CteScan`） |
| **CTE 名称混淆** | `crate::cte::mangle_cte_name`（见 spec/recursive.rs 注释） |
| **加权最短路** | `crates/linkrs-query/src/executor/streaming/operators/algorithms.rs`（`weighted_shortest_path`） |

---

## 五、总结

1. **一个算子覆盖五类递归**：双向 BFS 最短路、多源最短路、可配置 BFS、全路径枚举、
   递归 CTE 不动点统一由 RecursiveFragmentOperator 执行；
2. **Fixpoint 语义完整**：锚点一次 + 步进迭代 + delta 发布 + 迭代上限报错，
   非递归 CTE 退化为内联视图（`step: None`）；
3. **资源可控可中断**：迭代内存计入查询内存池，每轮检查取消令牌，`max_iterations`
   防止不收敛查询拖垮系统；
4. **加权与唯一性显式化**：Dijkstra 加权与 `PathUniqueness` 路径语义作为 spec 字段
   显式声明，无隐式约定。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
