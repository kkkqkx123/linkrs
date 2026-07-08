# Join 操作分析文档

## 概述

本文档详细分析了 GraphDB 项目中查询模块的 Join 操作，包括其语义、类型、实现架构、算法流程和优化策略。

---

## 一、Join 在图数据库中的语义

在图数据库中，**Join 操作用于将两个数据集按照指定的连接键进行关联**。主要应用场景包括：

1. **MATCH 语句模式匹配**：如 `MATCH (a)-[r]->(b)` 需要将节点 a、边 r、节点 b 的结果连接起来
2. **多跳遍历关联**：GO 语句多跳后将结果与原始顶点属性关联
3. **子查询展开优化**：将 PatternApply 子查询转换为 HashInnerJoin 提升性能
4. **可选匹配**：使用 Left Join 实现 OPTIONAL MATCH

---

## 二、Join 的类型

项目实现了 **5 种 Join 类型**：

| 类型 | 枚举值 | 说明 |
|------|--------|------|
| **Inner Join** | `JoinType::Inner` | 内连接，只返回匹配的行 |
| **Left Join** | `JoinType::Left` | 左外连接，保留左表所有行，未匹配填 NULL |
| **Cross Join** | `JoinType::Cross` | 笛卡尔积，无连接条件 |
| **Full Outer Join** | `JoinType::Full` | 全外连接，保留两表所有行 |
| **Hash Join** | 优化版本 | 基于哈希表的 Inner/Left Join 实现 |

> **注意**：Right Join 已被移除，因为可以通过交换表顺序用 Left Join 实现相同功能。

---

## 三、核心数据结构

### 3.1 JoinType 枚举

**文件路径**: `src/core/types/graph_schema.rs`

```rust
pub enum JoinType {
    Inner,   // 内连接
    Left,    // 左外连接
    Right,   // 右外连接（保留但未实现）
    Full,    // 全外连接
    Cross,   // 笛卡尔积
}
```

### 3.2 JoinConfig 配置结构

**文件路径**: `src/query/executor/data_processing/join/mod.rs`

```rust
pub struct JoinConfig {
    pub join_type: JoinType,
    pub left_var: String,              // 左输入变量名
    pub right_var: String,             // 右输入变量名
    pub left_keys: Vec<String>,        // 左表连接键
    pub right_keys: Vec<String>,       // 右表连接键
    pub output_columns: Vec<String>,   // 输出列名
    pub enable_parallel: bool,         // 是否启用并行
}
```

### 3.3 JoinKey 哈希键

**文件路径**: `src/query/executor/data_processing/join/hash_table.rs`

```rust
pub struct JoinKey {
    values: Vec<Value>,        // 多键支持
    cached_hash: u64,          // 预计算哈希值
}
```

### 3.4 Join 计划节点

**文件路径**: `src/query/planning/plan/core/nodes/join/join_node.rs`

通过 `define_join_node!` 宏定义了多种 Join 节点：
- `InnerJoinNode`
- `LeftJoinNode`
- `CrossJoinNode`
- `HashInnerJoinNode`
- `HashLeftJoinNode`
- `FullOuterJoinNode`

所有 Join 节点共享以下核心字段：

```rust
pub struct XxxJoinNode {
    id: i64,
    left: Box<PlanNodeEnum>,           // 左子树
    right: Box<PlanNodeEnum>,          // 右子树
    hash_keys: Vec<ContextualExpression>,   // 构建侧键
    probe_keys: Vec<ContextualExpression>,  // 探测侧键
    deps: Vec<PlanNodeEnum>,
    output_var: Option<String>,
    col_names: Vec<String>,
}
```

---

## 四、Join 执行器架构

### 4.1 目录结构

```
src/query/executor/data_processing/join/
├── mod.rs                  # 模块入口，定义 JoinConfig
├── base_join.rs            # 基础执行器 BaseJoinExecutor
├── inner_join.rs           # InnerJoinExecutor + HashInnerJoinExecutor
├── left_join.rs            # LeftJoinExecutor + HashLeftJoinExecutor
├── cross_join.rs           # CrossJoinExecutor
├── full_outer_join.rs      # FullOuterJoinExecutor
├── hash_table.rs           # 哈希表实现（HashTable, JoinKey）
└── join_key_evaluator.rs   # 键表达式求值器
```

### 4.2 BaseJoinExecutor 基础执行器

**文件路径**: `src/query/executor/data_processing/join/base_join.rs`

核心功能：
- **输入数据集检查**：`check_input_datasets()` - 从执行上下文获取左右输入
- **哈希表构建**：`build_single_key_hash_table()` / `build_multi_key_hash_table()`
- **哈希表探测**：`probe_single_key_hash_table()` / `probe_multi_key_hash_table()`
- **结果行构建**：`new_row()` - 合并左右两行
- **Join 顺序优化**：`optimize_join_order()` - 当左表远大于右表时交换（阈值 2 倍）

### 4.3 InnerJoinExecutor 内连接

**文件路径**: `src/query/executor/data_processing/join/inner_join.rs`

#### 算法流程

```
1. 检查输入数据集
2. 判断单键还是多键连接（use_multi_key = hash_keys.len() > 1）
3. 优化 Join 顺序（选择较小表作为构建表）
4. 构建阶段：遍历构建表，计算哈希键值，插入哈希表
5. 探测阶段：遍历探测表，计算探测键值，在哈希表中查找匹配
6. 对每个匹配，构建结果行并添加到结果集
```

#### 单键连接核心代码

```rust
fn execute_single_key_join(
    &mut self,
    left_dataset: &DataSet,
    right_dataset: &DataSet,
) -> Result<DataSet, QueryError> {
    // 优化 Join 顺序
    self.base_executor.optimize_join_order(left_dataset, right_dataset);
    
    // 构建哈希表（选择较小表）
    for row in &build_dataset.rows {
        let key = ExpressionEvaluator::evaluate(&hash_key, &mut context)?;
        hash_table.entry(key).or_default().push(row.to_vec());
    }
    
    // 探测哈希表
    for probe_row in &probe_dataset.rows {
        let probe_key_val = ExpressionEvaluator::evaluate(&probe_key, &mut probe_context)?;
        if let Some(matching_rows) = hash_table.get(&probe_key_val) {
            for build_row in matching_rows {
                result.rows.push(Self::build_join_result_row(...));
            }
        }
    }
}
```

### 4.4 LeftJoinExecutor 左外连接

**文件路径**: `src/query/executor/data_processing/join/left_join.rs`

与 Inner Join 的区别：
- 左表始终是探测表（驱动表），右表构建哈希表
- 对于未匹配的左表行，用 NULL 填充右表列
- 使用 `HashSet` 跟踪已匹配的左表行

```rust
// 处理未匹配的行（填充 NULL）
for left_row in &left_dataset.rows {
    if !matched_rows.contains(left_row) {
        let mut new_row = left_row.to_vec();
        for _ in 0..self.right_col_size {
            new_row.push(Value::Null(NullType::Null));
        }
        result.rows.push(new_row);
    }
}
```

### 4.5 CrossJoinExecutor 笛卡尔积

**文件路径**: `src/query/executor/data_processing/join/cross_join.rs`

支持两表和多表笛卡尔积：
- 两表：双重循环
- 多表：递归生成

```rust
fn generate_cartesian_product_recursive(
    &self,
    datasets: &[DataSet],
    current_index: usize,
    current_row: Vec<Value>,
    result: &mut DataSet,
) {
    if current_index >= datasets.len() {
        result.rows.push(current_row);
        return;
    }
    for row in &datasets[current_index].rows {
        let mut new_row = current_row.clone();
        new_row.extend(row.clone());
        self.generate_cartesian_product_recursive(...);
    }
}
```

---

## 五、从查询到执行的完整流程

```
1. 解析阶段（Parser）
   ↓
2. 验证阶段（Validator）
   ↓
3. 规划阶段（Planner）
   - MatchStatementPlanner 使用 SegmentsConnector 创建 Join 节点
   - connector.rs: inner_join() / left_join() / cross_join()
   ↓
4. 优化阶段（Optimizer）
   - JoinOrderOptimizer: 基于成本优化 Join 顺序
   - 启发式规则：PushFilterDownJoin, JoinElimination 等
   - SubqueryUnnesting: PatternApply → HashInnerJoin
   ↓
5. 执行阶段（Executor）
   - JoinBuilder 从 PlanNode 创建 Executor
   - ExecutorEnum 分发到具体执行器
   - 执行 Hash Join 算法
```

### 5.1 关键代码路径

#### 规划器创建 Join 节点

**文件路径**: `src/query/planning/connector.rs`

```rust
pub fn inner_join(
    _qctx: &QueryContext,
    left: SubPlan,
    right: SubPlan,
    _inter_aliases: HashSet<&str>,
) -> Result<SubPlan, PlannerError> {
    let join_node = PlanNodeEnum::InnerJoin(
        InnerJoinNode::new(left_root, right_root, hash_keys, probe_keys)?
    );
    Ok(SubPlan { root: Some(join_node), ... })
}
```

#### 执行器工厂构建

**文件路径**: `src/query/executor/factory/builders/join_builder.rs`

```rust
pub fn build_inner_join(
    node: &InnerJoinNode,
    storage: Arc<Mutex<S>>,
    context: &ExecutionContext,
) -> Result<ExecutorEnum<S>, QueryError> {
    let (left_var, right_var) = Self::extract_join_vars(node);
    let config = InnerJoinConfig {
        id,
        hash_keys,
        probe_keys,
        left_var,
        right_var,
        col_names,
    };
    let executor = InnerJoinExecutor::new(storage, context.expression_context(), config);
    Ok(ExecutorEnum::InnerJoin(executor))
}
```

---

## 六、Join 优化策略

### 6.1 基于成本的 Join 顺序优化

**文件路径**: `src/query/optimizer/cost_based/join_order.rs`

使用动态规划（类 DPccp 算法）选择最优 Join 顺序：

```rust
pub fn optimize_join_order(
    &self,
    tables: &[TableInfo],
    conditions: &[JoinCondition],
) -> JoinOrderResult {
    // 单表初始解
    // 迭代选择最优 Join 对
    // 计算成本 = 构建成本 + 探测成本 + 输出成本
}
```

#### Join 算法选择策略

```rust
fn choose_join_algorithm(...) -> JoinAlgorithm {
    // 策略1: 有索引且数据量适中 → IndexJoin
    // 策略2: 数据量小 → NestedLoopJoin
    // 策略3: 默认 → HashJoin（选较小表做构建侧）
}
```

### 6.2 启发式优化规则

**文件路径**: `src/query/optimizer/heuristic/rule_enum.rs`

| 规则 | 说明 |
|------|------|
| `PushFilterDownInnerJoin` | 下推过滤条件到 Inner Join 下方 |
| `PushFilterDownHashInnerJoin` | 下推过滤条件到 Hash Inner Join |
| `PushFilterDownLeftJoin` | 下推过滤条件到 Left Join |
| `LeftJoinToInnerJoin` | 当右表非空时将 Left Join 转换为 Inner Join |
| `JoinElimination` | 消除不必要的 Join |
| `JoinConditionSimplify` | 简化 Join 条件 |
| `JoinToExpand` | 将 Join 转换为 Expand 操作 |
| `JoinToAppendVertices` | 将 Join 转换为 AppendVertices |

### 6.3 子查询展开

**文件路径**: `src/query/optimizer/cost_based/subquery_unnesting.rs`

将 `PatternApply` 子查询转换为 `HashInnerJoin`：

```rust
fn transform(&self, plan: &PlanNodeEnum) -> Result<Option<PlanNodeEnum>, OptimizerError> {
    // 检查是否适合转换
    // 比较 PatternApply vs HashJoin 的成本
    // 如果 HashJoin 更优，创建 HashInnerJoinNode
}
```

---

## 七、关键文件路径汇总

| 类别 | 文件路径 |
|------|----------|
| **类型定义** | `src/core/types/graph_schema.rs` |
| **Join 节点** | `src/query/planning/plan/core/nodes/join/join_node.rs` |
| **Join 宏** | `src/query/planning/plan/core/nodes/join/macros.rs` |
| **连接器** | `src/query/planning/connector.rs` |
| **Join 执行器模块** | `src/query/executor/data_processing/join/mod.rs` |
| **基础执行器** | `src/query/executor/data_processing/join/base_join.rs` |
| **内连接** | `src/query/executor/data_processing/join/inner_join.rs` |
| **左连接** | `src/query/executor/data_processing/join/left_join.rs` |
| **笛卡尔积** | `src/query/executor/data_processing/join/cross_join.rs` |
| **全外连接** | `src/query/executor/data_processing/join/full_outer_join.rs` |
| **哈希表** | `src/query/executor/data_processing/join/hash_table.rs` |
| **执行器构建器** | `src/query/executor/factory/builders/join_builder.rs` |
| **执行器枚举** | `src/query/executor/executor_enum.rs` |
| **Join 顺序优化** | `src/query/optimizer/cost_based/join_order.rs` |
| **子查询展开** | `src/query/optimizer/cost_based/subquery_unnesting.rs` |
| **启发式规则枚举** | `src/query/optimizer/heuristic/rule_enum.rs` |
| **MATCH 规划器** | `src/query/planning/statements/match_statement_planner.rs` |

---

## 八、总结

该 GraphDB 项目的 Join 实现具有以下特点：

1. **基于哈希的 Join 算法**：所有等值 Join 都使用哈希表实现，时间复杂度 O(N+M)
2. **多键支持**：支持复合键 Join（`JoinKey` 包含 `Vec<Value>`）
3. **表达式求值**：连接键通过 `ExpressionEvaluator` 动态求值，支持复杂表达式
4. **自动优化**：自动选择较小表作为构建侧，支持 Join 顺序优化
5. **模块化设计**：规划节点、执行器、优化器清晰分离
6. **图数据库特有优化**：支持将 Join 转换为 Expand/AppendVertices 等图专用操作

---

## 附录：Join 操作在图查询中的典型应用

### 示例 1：MATCH 语句

```cypher
MATCH (a:Person)-[r:KNOWS]->(b:Person)
RETURN a.name, b.name
```

执行流程：
1. 扫描所有 `Person` 节点作为 `a`
2. 扫描所有 `KNOWS` 边作为 `r`
3. 扫描所有 `Person` 节点作为 `b`
4. 使用 Join 操作将 `a.src_id = r.src` 和 `r.dst = b.dst_id` 关联起来

### 示例 2：OPTIONAL MATCH

```cypher
MATCH (a:Person)
OPTIONAL MATCH (a)-[r:KNOWS]->(b:Person)
RETURN a.name, b.name
```

执行流程：
1. 扫描所有 `Person` 节点作为 `a`
2. 使用 Left Join 关联边和目标节点
3. 对于没有 `KNOWS` 关系的人，`b.name` 返回 NULL

---

**文档创建时间**: 2026-04-11  
**分析基于项目版本**: GraphDB Rust 实现
