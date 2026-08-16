# Join 操作分析文档

## 概述

本文档详细分析了 GraphDB 项目中查询模块的 Join 操作，包括其语义、类型、实现架构、算法流程和优化策略。
本文档与 2026-08-16 代码现状对齐。

---

## 一、Join 在图数据库中的语义

在图数据库中，**Join 操作用于将两个数据集按照指定的连接键进行关联**。主要应用场景包括：

1. **MATCH 语句模式匹配**：如 `MATCH (a)-[r]->(b)` 需要将节点 a、边 r、节点 b 的结果连接起来
2. **多跳遍历关联**：GO 语句多跳后将结果与原始顶点属性关联
3. **子查询展开优化**：将 `PatternApply` 子查询转换为物理 Hash Join 提升性能
4. **可选匹配**：使用 Left Join 实现 OPTIONAL MATCH

---

## 二、Join 的类型（逻辑层 vs 物理层）

### 2.1 `JoinType` 枚举（连接语义类型）

**文件路径**: `crates/graphdb-core/src/core/types/graph_schema.rs`

```rust
pub enum JoinType {
    Inner,   // 内连接
    Left,    // 左外连接
    Right,   // 右外连接
    Full,    // 全外连接
    Cross,   // 笛卡尔积（cross join）
    Lateral, // 横向连接（correlated subquery）
}
```

> **更正**：早期文档称 "Right Join 已被移除"，实际 `JoinType::Right` 与 `Semi` 均支持，
> 逻辑枚举与物理 spec 中都存在完整的连接类型（见下）。

### 2.2 逻辑层 Join 节点（6 种，`define_join_node!` 宏）

**文件路径**: `crates/graphdb-query/src/query/planning/plan/core/nodes/join/join_node.rs`

逻辑层使用 `define_join_node!` 宏定义 6 种 Join 节点，全部实现 `BinaryInputNode`（left/right 两个输入）
与 `JoinNode`（`hash_keys` / `probe_keys` 统一接口）：

| 节点类型 | 说明 |
|---------|------|
| `InnerJoinNode` | 内连接，只返回匹配的行 |
| `LeftJoinNode` | 左外连接，保留左表所有行，未匹配填 NULL |
| `RightJoinNode` | 右外连接，保留右表所有行 |
| `CrossJoinNode` | 笛卡尔积，无连接条件 |
| `FullOuterJoinNode` | 全外连接，保留两表所有行 |
| `SemiJoinNode` | 半连接，返回左表在右表有匹配的行 |

> **重要**：逻辑枚举中**没有** `HashInnerJoinNode`/`HashLeftJoinNode` 变体（早期文档引用的
> 旧模型已删除）。"Hash" 是**物理执行算法**概念，见 2.3。

### 2.3 物理层 `JoinSpec`（9 种，物理执行算法）

**文件路径**: `crates/graphdb-query/src/query/executor/streaming/operators/spec.rs`

```rust
pub enum JoinSpec {
    InnerJoin { join_condition: Option<Expression> },
    LeftJoin  { join_condition: Option<Expression> },
    RightJoin { join_condition: Option<Expression> },
    FullOuterJoin { join_condition: Option<Expression> },
    CrossJoin,
    SemiJoin  { join_condition: Option<Expression> },
    HashJoin  { join_condition: Option<Expression>, hash_keys: Vec<Expression>, probe_keys: Vec<Expression>, build_side: BuildSide },
    HashLeftJoin { join_condition: Option<Expression>, hash_keys: Vec<Expression>, probe_keys: Vec<Expression>, build_side: BuildSide },
    NestedLoopJoin { join_condition: Option<Expression> },
}
```

**逻辑层与物理层的对应关系**：逻辑 `JoinNode`（`InnerJoinNode`/`LeftJoinNode` 等）经
`PhysicalPlanBuilder` 的 `build_join_with_keys`（`plan/arena_builder/specs.rs`）落地为物理 spec：

- 有效等值键（`equi_condition_from_keys` 成功）时，逻辑 `InnerJoinNode` → 物理 `JoinSpec::HashJoin`；
- `LeftJoinNode` → `JoinSpec::HashLeftJoin`；
- 无效/非等值键回退为对应类型的普通 spec（如 `NestedLoopJoin`）。

> **对应关系速记**：逻辑层的 `InnerJoin`/`LeftJoin` ↔ 物理层的 `JoinSpec::HashJoin`/`HashLeftJoin`
> （默认等值连接时）。这就是早期文档中的 "HashInnerJoin/HashLeftJoin" 在物理层的位置。

---

## 三、核心数据结构

### 3.1 逻辑层 Join 节点结构

**文件路径**: `crates/graphdb-query/src/query/planning/plan/core/nodes/join/join_node.rs`

所有 Join 节点共享以下核心字段（由 `define_join_node!` 宏生成）：

```rust
pub struct XxxJoinNode {
    id: i64,
    left: Box<PlanNodeEnum>,                // 左子树
    right: Box<PlanNodeEnum>,               // 右子树
    hash_keys: Vec<ContextualExpression>,   // 构建侧键
    probe_keys: Vec<ContextualExpression>,  // 探测侧键
    deps: Vec<PlanNodeEnum>,
    output_var: Option<String>,
    col_names: Vec<String>,
}
```

### 3.2 物理层 `BuildSide`

物理 Hash Join 需要决定**构建侧（build side）**与**探测侧（probe side）**：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildSide {
    Left,
    #[default]
    Right,
}
```

逻辑计划默认以右子树为构建侧（`BuildSide::default() = Right`）；当右子树不可哈希而左子树可哈希时，
物理转换可切换为 `BuildSide::Left`（见 `specs.rs` `build_join_with_keys` 注释）。

---

## 四、Join 执行器架构（物理算子）

### 4.1 目录结构

**文件路径**: `crates/graphdb-query/src/query/executor/streaming/operators/join_operator/`

```
join_operator/
├── hash_join.rs        # HashJoin / HashLeftJoin 执行器（build + probe 两个阶段）
├── merge_join.rs       # Merge Join 执行器（有序输入归并连接）
├── nested_loop_join.rs # NestedLoopJoin 执行器
└── cross_semi_join.rs  # CrossSemiJoin / SemiJoin 执行器
```

### 4.2 Hash Join 执行器（`hash_join.rs`）

对应 `JoinSpec::HashJoin` / `JoinSpec::HashLeftJoin`，是等值连接的默认实现。

```
1. 读取构建侧（build side）输入，逐行计算 hash_keys，插入哈希表
2. 读取探测侧（probe side）输入，逐行计算 probe_keys，在哈希表中查找
3. 对每个匹配构建结果行；HashLeftJoin 额外保留左表未匹配行（填 NULL）
```

实现要点（`join_operator/hash_join.rs`）：
- 构建/探测两阶段，bounded-memory 批量处理；
- 输出列布局按计划中的 `col_names` 对齐（含 `HashLeftJoin planned output layout` 校验）；
- 支持 `BuildSide::Left`（探测/构建侧可交换）。

### 4.3 Merge Join 执行器（`merge_join.rs`）

对应 `JoinSpec` 的归并路径：要求两输入已按连接键有序，O(N+M) 单趟归并。

### 4.4 Nested Loop Join 执行器（`nested_loop_join.rs`）

对应 `JoinSpec::NestedLoopJoin`：适用于**非等值连接条件**或无索引的小表场景，双重循环逐行匹配。

### 4.5 Cross / Semi Join 执行器（`cross_semi_join.rs`）

对应 `JoinSpec::CrossJoin` / `JoinSpec::SemiJoin`：笛卡尔积 / 半连接（存在性检测）。

---

## 五、从查询到执行的完整流程

```
1. 解析阶段（Parser）
   ↓
2. 验证阶段（Validator）
   ↓
3. 规划阶段（Planner）
   - connector.rs: inner_join() / left_join() / cross_join() 创建逻辑 JoinNode
   - middleware/connector 由 MATCH 各个 match 分支拼接 Join 节点
   ↓
4. 优化阶段（Optimizer）
   - 启发式规则（heuristic/）：PushFilterDownInnerJoin、JoinElimination、JoinToExpand 等
   - CBO（cost_based/join_order.rs）：基于成本重排连接顺序
   - 子查询去关联（cost_based/subquery_unnesting.rs + heuristic/decorrelation.rs）
   ↓
5. 物理规划阶段（physical_planner.rs）
   - 逻辑 JoinNode → 物理 JoinSpec（build_join_with_keys：等值键 → HashJoin/HashLeftJoin）
   ↓
6. 执行阶段（Executor）
   - factory.rs 按 JoinSpec 分发到 join_operator/ 下具体执行器
```

### 5.1 关键代码路径

#### 规划器创建 Join 节点

**文件路径**: `crates/graphdb-query/src/query/planning/connector.rs`

```rust
pub fn inner_join(
    _qctx: &QueryContext,
    left: SubPlan,
    right: SubPlan,
    _inter_aliases: HashSet<&str>,
) -> Result<SubPlan, PlannerError> {
    // 计算 hash_keys / probe_keys
    let join_node = PlanNodeEnum::InnerJoin(
        InnerJoinNode::new(left_root, right_root, hash_keys, probe_keys)?
    );
    Ok(SubPlan { root: Some(join_node), ... })
}
```

#### 物理映射：逻辑 JoinNode → JoinSpec

**文件路径**: `crates/graphdb-query/src/query/planning/physical_planner.rs`（约 :303 处 InnerJoin）+
`crates/graphdb-query/src/query/executor/streaming/plan/arena_builder/specs.rs`

```rust
pub(super) fn build_join_with_keys(
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
    default: JoinSpec,
) -> Result<JoinSpec, PlanBuildError> {
    match equi_condition_from_keys(hash_keys, probe_keys)? {
        Some(_) => match default {
            JoinSpec::InnerJoin { .. } => Ok(JoinSpec::HashJoin { join_condition: None, ... }),
            JoinSpec::LeftJoin { .. }  => Ok(JoinSpec::HashLeftJoin { join_condition: None, ... }),
            _ => Ok(default),
        },
        None => Ok(default),
    }
}
```

---

## 六、Join 优化策略

### 6.1 连接重排（cost_based/join_order.rs → heuristic/join_optimization/join_reorder.rs）

基于代价模型（类 DP／启发式）重排多表连接顺序，选择较小的表作为构建侧以降低哈希表内存。

### 6.2 启发式优化规则（heuristic/ + rule_enum.rs）

| 规则 | 说明 |
|------|------|
| `PushFilterDownInnerJoin` | 下推过滤条件到 Inner Join 下方（`predicate_pushdown/push_filter_down_inner_join.rs`） |
| `JoinElimination` | 消除不必要的 Join |
| `JoinConditionSimplify` | 简化 Join 条件 |
| `JoinToExpand` | 将图连接转换为 Expand 操作 |
| `JoinToAppendVertices` | 将 Join 转换为 AppendVertices |

### 6.3 子查询去关联

**文件路径**: `cost_based/subquery_unnesting.rs` + `heuristic/decorrelation.rs`

将 `PatternApply` / `CorrelatedApply` 子查询转换为等价的 Join 形式（`Lateral`/相关子查询等价改写），
在物理层以 Hash Join 执行。

---

## 七、关键文件路径汇总（2026-08-16 核实）

| 类别 | 文件路径 |
|------|----------|
| **JoinType 枚举** | `crates/graphdb-core/src/core/types/graph_schema.rs` |
| **逻辑 Join 节点** | `crates/graphdb-query/src/query/planning/plan/core/nodes/join/join_node.rs` |
| **Join 宏** | `crates/graphdb-query/src/query/planning/plan/core/nodes/base/macros/binary_input.rs`（`define_join_node!`） |
| **连接器** | `crates/graphdb-query/src/query/planning/connector.rs` |
| **物理 JoinSpec** | `crates/graphdb-query/src/query/executor/streaming/operators/spec.rs` |
| **Join 执行器模块** | `crates/graphdb-query/src/query/executor/streaming/operators/join_operator/` |
| **哈希连接** | `.../join_operator/hash_join.rs` |
| **归并连接** | `.../join_operator/merge_join.rs` |
| **嵌套循环** | `.../join_operator/nested_loop_join.rs` |
| **交叉/半连接** | `.../join_operator/cross_semi_join.rs` |
| **物理映射（等值键 → Hash）** | `.../streaming/plan/arena_builder/specs.rs` |
| **物理规划器** | `crates/graphdb-query/src/query/planning/physical_planner.rs` |
| **连接重排（启发式）** | `.../optimizer/heuristic/join_optimization/join_reorder.rs` |
| **连接重排（CBO）** | `.../optimizer/cost_based/join_order.rs` |
| **子查询去关联** | `.../optimizer/cost_based/subquery_unnesting.rs`、`.../optimizer/heuristic/decorrelation.rs` |
| **启发式规则枚举** | `.../optimizer/heuristic/rule_enum.rs` |

> **更正**：早期文档引用的旧路径 `src/query/executor/data_processing/join/`（含
> `base_join.rs`/`inner_join.rs`/`left_join.rs`/`cross_join.rs`/`full_outer_join.rs`/
> `hash_table.rs`/`join_key_evaluator.rs`）与 `src/query/executor/factory/builders/join_builder.rs`
> 均为**已重构的旧模型**，当前实现在 `executor/streaming/operators/join_operator/` 与
> `streaming/factory.rs` 中。

---

## 八、总结

该 GraphDB 项目的 Join 实现具有以下特点：

1. **逻辑/物理分离**：逻辑层 6 种 Join 节点（`InnerJoinNode` 等）与物理层 `JoinSpec`（
   `HashJoin`/`HashLeftJoin`/`NestedLoopJoin`/`CrossJoin`/`SemiJoin` 等）解耦，等值连接默认映射到 Hash Join；
2. **多种物理算法**：Hash Join（默认等值）、Merge Join（有序输入）、Nested Loop（非等值/小表）、
   Cross/Semi；
3. **构建侧可切换**：`BuildSide::Left`/`Right`，右子树不可哈希时可交换构建/探测侧；
4. **统一接口**：`JoinNode` trait 统一 `hash_keys`/`probe_keys`，规划器与优化器一致处理；
5. **多种优化**：启发式下推、CBO 连接重排、子查询去关联（PatternApply/CorrelatedApply → Join）；
6. **图数据库特有优化**：支持将 Join 转换为 Expand/AppendVertices 等图专用操作。

---

## 附录：Join 操作在图查询中的典型应用

### 示例 1：MATCH 语句

```rust
MATCH (a:Person)-[r:KNOWS]->(b:Person)
RETURN a.name, b.name
```

执行流程：
1. 扫描所有 `Person` 节点作为 `a`
2. 扫描所有 `KNOWS` 边作为 `r`
3. 扫描所有 `Person` 节点作为 `b`
4. 使用 Join 操作将 `a.src_id = r.src` 和 `r.dst = b.dst_id` 关联起来（物理上通常被改写为遍历算子）

### 示例 2：OPTIONAL MATCH

```rust
MATCH (a:Person)
OPTIONAL MATCH (a)-[r:KNOWS]->(b:Person)
RETURN a.name, b.name
```

执行流程：
1. 扫描所有 `Person` 节点作为 `a`
2. 使用 Left Join（物理 `JoinSpec::HashLeftJoin`）关联边和目标节点
3. 对于没有 `KNOWS` 关系的人，`b.name` 返回 NULL

---

**文档更新日期**: 2026-08-16  
**分析基于项目版本**: GraphDB Rust 实现（linkrs）
