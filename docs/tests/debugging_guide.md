# GraphDB 测试调试规范文档

## 概述

本文档描述了 GraphDB 项目中测试调试基础设施的使用规范，包括调试工具、最佳实践和调试流程。

## 调试基础设施

### 1. Debug Helpers 模块

位置：`tests/common/debug_helpers.rs`

该模块提供了查询执行分析的调试工具，所有代码都使用 `#[cfg(test)]` 条件编译，确保不会污染生产代码。

#### 主要功能

##### 1.1 查询计划格式化

```rust
use crate::common::debug_helpers::{format_query_plan, print_query_plan};

// 格式化查询计划为字符串
let plan_string = format_query_plan(&execution_plan);

// 直接打印查询计划（仅在测试模式下）
print_query_plan(&execution_plan);
```

输出示例：
```
Query Plan:
===========
[0] ProjectNode (id=5, output_var=result)
    columns: ["a", "b"]
  child[0]:
    [2] ExpandAllNode (id=4, output_var=b)
        input_var: a, edge_types: ["KNOWS"], direction: OUT
      child[0]:
        [4] ScanVerticesNode (id=1, output_var=a)
            space: test_space
```

##### 1.2 数据集格式化

```rust
use crate::common::debug_helpers::{format_dataset, print_dataset};

// 格式化数据集为字符串
let dataset_string = format_dataset(&dataset);

// 直接打印数据集（仅在测试模式下）
print_dataset(&dataset);
```

输出示例：
```
Columns: name, age
--------------------------------------------------
Row 0: 'Alice' | 30
Row 1: 'Bob' | 25
Total rows: 2
```

##### 1.3 查询执行追踪

```rust
use crate::common::debug_helpers::QueryExecutionTracer;

let mut tracer = QueryExecutionTracer::new("MATCH (a:Person) RETURN a");
tracer.add_step("Parse", "Successfully parsed query");
tracer.add_step("Plan", "Generated execution plan with 3 nodes");
tracer.add_step("Execute", "ScanVerticesNode returned 5 vertices");
tracer.print_trace();
```

输出示例：
```
Query Execution Trace for: MATCH (a:Person) RETURN a
============================================================
Step 1: Parse
  Successfully parsed query
Step 2: Plan
  Generated execution plan with 3 nodes
Step 3: Execute
  ScanVerticesNode returned 5 vertices
```

##### 1.4 调试断言宏

```rust
use crate::assert_with_debug;

assert_with_debug!(
    result.rows.len() == expected_count,
    &execution_plan,
    &result,
    "Row count mismatch"
);
```

### 2. TestScenario 调试方法

位置：`tests/common/test_scenario.rs`

`TestScenario` 提供了流式 API 用于编写集成测试，包含以下调试方法：

#### 2.1 打印查询结果

```rust
#[test]
fn test_example() {
    let scenario = TestScenario::new()
        .exec_ddl("CREATE SPACE test")
        .exec_ddl("USE test")
        .exec_ddl("CREATE TAG Person(name string, age int)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .exec_dql("MATCH (a:Person) RETURN a")
        .debug_print_result()  // 打印查询结果
        .assert_result_count(1);
}
```

输出示例：
```
=== Debug: Last Query Result ===
Columns: ["a"]
Rows (1):
  Row 0: [Vertex(Vertex { vid: Int(1), ... })]
================================
```

## 调试最佳实践

### 1. 调试流程

当测试失败时，按照以下流程进行调试：

#### 步骤 1：确认错误信息

```rust
// 使用 assert_success 确认操作是否成功
scenario
    .exec_dql("MATCH (a:Person) RETURN a")
    .assert_success();
```

#### 步骤 2：打印查询计划

```rust
// 在测试代码中获取查询计划并打印
let result = scenario.pipeline.execute_query("MATCH (a:Person) RETURN a");
if let Ok(ExecutionResult::DataSet(ds)) = result {
    // 打印数据集
    print_dataset(&ds);
}
```

#### 步骤 3：检查执行结果

```rust
scenario
    .exec_dql("MATCH (a:Person) RETURN a")
    .debug_print_result()  // 检查返回的数据
    .assert_result_count(1);
```

### 2. 常见调试场景

#### 场景 1：查询返回行数不正确

```rust
#[test]
fn test_row_count_debug() {
    let scenario = setup_scenario();
    
    // 执行查询
    let scenario = scenario.exec_dql("MATCH (a)-[:KNOWS]->(b) RETURN a, b");
    
    // 打印详细信息
    scenario.debug_print_result();
    
    // 使用断言验证
    scenario.assert_result_count(2);
}
```

#### 场景 2：查询计划分析

```rust
#[test]
fn test_plan_analysis() {
    use graphdb::query::planner::QueryPlanner;
    
    let planner = QueryPlanner::new();
    let plan = planner.plan_query("MATCH (a)-[:KNOWS]->(b) RETURN a, b").unwrap();
    
    // 打印查询计划
    print_query_plan(&plan);
    
    // 分析计划结构
    if let Some(root) = plan.root() {
        println!("Root node: {}", root.name());
        for child in root.children() {
            println!("  Child: {}", child.name());
        }
    }
}
```

#### 场景 3：执行过程追踪

```rust
#[test]
fn test_execution_trace() {
    let mut tracer = QueryExecutionTracer::new("复杂查询");
    
    tracer.add_step("初始化", "创建存储和管道");
    
    let scenario = TestScenario::new().unwrap();
    tracer.add_step("创建场景", "TestScenario 创建成功");
    
    let scenario = scenario.exec_ddl("CREATE SPACE test");
    tracer.add_step("DDL", "创建空间成功");
    
    let scenario = scenario.exec_dql("MATCH (a:Person) RETURN a");
    tracer.add_step("查询", "执行 MATCH 查询");
    
    // 打印完整追踪信息
    tracer.print_trace();
    
    scenario.assert_success();
}
```

### 3. 调试代码规范

#### 3.1 条件编译

所有调试代码必须使用 `#[cfg(test)]` 属性，确保不会编译到生产代码中：

```rust
#[cfg(test)]
pub fn debug_function() {
    // 调试代码
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_with_debug() {
        // 测试代码
    }
}
```

#### 3.2 调试输出格式

调试输出应该清晰、结构化，便于阅读：

```rust
#[cfg(test)]
pub fn print_dataset(dataset: &DataSet) {
    eprintln!("\n=== Debug: Dataset ===");
    eprintln!("Columns: {:?}", dataset.col_names);
    eprintln!("Rows ({}):", dataset.rows.len());
    for (i, row) in dataset.rows.iter().enumerate() {
        eprintln!("  Row {}: {:?}", i, row);
    }
    eprintln!("======================\n");
}
```

#### 3.3 错误信息规范

调试断言应该提供清晰的错误信息：

```rust
// 好的做法
assert_eq!(
    actual_count, expected_count,
    "查询 '{}' 期望返回 {} 行，但实际返回 {} 行",
    query, expected_count, actual_count
);

// 使用调试宏
assert_with_debug!(
    result.rows.len() == expected,
    &plan,
    &result,
    &format!("行数不匹配: 期望 {}, 实际 {}", expected, result.rows.len())
);
```

## 调试工具使用示例

### 示例 1：完整的调试测试

```rust
#[test]
fn test_complex_query_with_debug() {
    use crate::common::debug_helpers::*;
    
    // 设置测试场景
    let scenario = TestScenario::new()
        .unwrap()
        .exec_ddl("CREATE SPACE social_network")
        .exec_ddl("USE social_network")
        .exec_ddl("CREATE TAG Person(name string)")
        .exec_ddl("CREATE EDGE KNOWS()")
        .load_data(vec![
            "INSERT VERTEX Person(name) VALUES 1:('Alice')",
            "INSERT VERTEX Person(name) VALUES 2:('Bob')",
            "INSERT EDGE KNOWS() VALUES 1->2:()",
        ]);
    
    // 执行复杂查询
    let scenario = scenario.exec_dql(
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name"
    );
    
    // 调试输出
    scenario.debug_print_result();
    
    // 验证结果
    scenario
        .assert_success()
        .assert_result_count(1);
}
```

### 示例 2：查询计划分析

```rust
#[test]
fn test_analyze_query_plan() {
    use graphdb::query::planner::QueryPlanner;
    use crate::common::debug_helpers::format_query_plan;
    
    let planner = QueryPlanner::new();
    let query = "MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person) RETURN a, b, c";
    
    match planner.plan_query(query) {
        Ok(plan) => {
            // 格式化并打印计划
            let plan_str = format_query_plan(&plan);
            eprintln!("{}", plan_str);
            
            // 分析计划节点
            if let Some(root) = plan.root() {
                assert_eq!(root.name(), "ProjectNode");
                assert_eq!(root.children().len(), 1);
            }
        }
        Err(e) => {
            panic!("查询计划生成失败: {:?}", e);
        }
    }
}
```

### 示例 3：性能调试

```rust
#[test]
fn test_performance_debug() {
    use std::time::Instant;
    
    let scenario = setup_large_dataset();
    
    let start = Instant::now();
    let scenario = scenario.exec_dql("MATCH (a)-[:KNOWS*1..3]->(b) RETURN a, b");
    let duration = start.elapsed();
    
    eprintln!("查询执行时间: {:?}", duration);
    scenario.debug_print_result();
    
    // 验证性能要求
    assert!(duration.as_secs() < 5, "查询执行时间过长");
}
```

## 注意事项

1. **不要在生产代码中使用调试函数**：所有调试代码必须使用 `#[cfg(test)]` 保护
2. **及时清理调试代码**：测试通过后，应该移除不必要的调试输出
3. **使用 eprintln 而不是 println**：调试输出应该使用标准错误流，避免干扰正常输出
4. **保持输出格式一致**：遵循项目中定义的调试输出格式规范

## 相关文件

- `tests/common/debug_helpers.rs` - 调试工具函数
- `tests/common/test_scenario.rs` - 测试场景和调试方法
- `tests/common/mod.rs` - 公共测试模块

## 参考文档

- `docs/test/integration_test_design.md` - 集成测试设计文档
- `docs/test/integration_test_analysis.md` - 集成测试分析文档
