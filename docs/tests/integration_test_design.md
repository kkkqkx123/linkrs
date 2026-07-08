# GraphDB 集成测试设计方案

## 1. 概述

本文档定义 GraphDB 项目的集成测试设计规范，旨在解决当前测试验证不充分的问题，建立一套完整的集成测试体系。

## 2. 现状分析

### 2.1 当前测试基础设施

| 组件 | 文件 | 功能 | 状态 |
|------|------|------|------|
| TestStorage | `tests/common/mod.rs` | 临时存储封装 | 已完善 |
| TestScenario | `tests/common/test_scenario.rs` | 流式测试API | 已完善 |
| ValidationHelper | `tests/common/validation_helpers.rs` | 数据验证辅助 | 已完善 |
| assertions | `tests/common/assertions.rs` | 基础断言 | 需扩展 |

### 2.2 当前测试问题

1. **断言太弱** - `assert!(result.is_ok() || result.is_err())` 是永真式
2. **无状态验证** - 创建后不验证对象是否存在
3. **无数据流测试** - INSERT后不验证能否查询到
4. **重复代码** - 每个测试都重复创建 TestStorage

## 3. 测试架构设计

```
tests/
├── common/
│   ├── mod.rs                    # TestStorage 基础封装
│   ├── assertions.rs             # 扩展断言库
│   ├── data_fixtures.rs          # 测试数据集
│   ├── query_helpers.rs          # 查询辅助
│   ├── validation_helpers.rs     # 数据验证辅助
│   └── test_scenario.rs          # 流式测试场景 (核心)
├── integration_ddl.rs            # DDL 测试
├── integration_dml.rs            # DML 测试
├── integration_dql.rs            # DQL 测试
├── integration_data_flow.rs      # 数据流测试
└── test_data/                    # 预设测试数据
    ├── social_network.cypher
    └── e_commerce.cypher
```

## 4. 核心组件使用规范

### 4.1 TestScenario API

TestScenario 提供流式API，所有集成测试应优先使用：

```rust
#[test]
fn test_example() {
    TestScenario::new()
        .expect("创建测试场景失败")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(1, "Person", hashmap! {
            "name" => Value::String("Alice".into()),
            "age" => Value::Int(30)
        });
}
```

### 4.2 执行方法

| 方法 | 用途 | 返回值 |
|------|------|--------|
| `exec_ddl(query)` | 执行DDL语句 | `&mut Self` |
| `exec_dml(query)` | 执行DML语句 | `&mut Self` |
| `query(query)` | 执行查询 | `&mut Self` |
| `setup_space(name)` | 创建并使用图空间 | `&mut Self` |
| `setup_schema(ddls)` | 批量创建Schema | `&mut Self` |
| `load_data(dmls)` | 批量加载数据 | `&mut Self` |

### 4.3 断言方法

#### 4.3.1 执行状态断言

| 方法 | 用途 |
|------|------|
| `assert_success()` | 断言上次操作成功 |
| `assert_error()` | 断言上次操作失败 |

#### 4.3.2 结果内容断言

| 方法 | 用途 |
|------|------|
| `assert_result_count(expected)` | 断言结果行数 |
| `assert_result_empty()` | 断言结果为空 |
| `assert_result_columns(expected)` | 断言列名匹配 |
| `assert_result_contains(values)` | 断言包含指定行 |

#### 4.3.3 数据状态断言

| 方法 | 用途 |
|------|------|
| `assert_vertex_exists(vid, tag)` | 断言顶点存在 |
| `assert_vertex_not_exists(vid, tag)` | 断言顶点不存在 |
| `assert_vertex_props(vid, tag, props)` | 断言顶点属性 |
| `assert_edge_exists(src, dst, edge_type)` | 断言边存在 |
| `assert_edge_not_exists(src, dst, edge_type)` | 断言边不存在 |
| `assert_tag_exists(tag)` | 断言Tag存在 |
| `assert_tag_not_exists(tag)` | 断言Tag不存在 |
| `assert_vertex_count(tag, expected)` | 断言顶点数量 |
| `assert_edge_count(edge_type, expected)` | 断言边数量 |

## 5. 测试模式规范

### 5.1 AAA 模式（推荐）

```rust
#[test]
fn test_update_vertex() {
    // Arrange - 准备数据和Schema
    let mut scenario = TestScenario::new().unwrap();
    scenario
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)");
    
    // Act - 执行操作
    scenario.exec_dml("UPDATE 1 SET age = 31");
    
    // Assert - 验证结果
    scenario
        .assert_success()
        .assert_vertex_props(1, "Person", hashmap! {
            "name" => Value::String("Alice".into()),
            "age" => Value::Int(31)
        });
}
```

### 5.2 流式模式（简单场景）

```rust
#[test]
fn test_create_tag() {
    TestScenario::new()
        .expect("创建场景失败")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .assert_tag_exists("Person");
}
```

### 5.3 数据流测试模式

```rust
#[test]
fn test_complete_crud_flow() {
    TestScenario::new()
        .expect("创建场景失败")
        // 1. 创建Schema
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        // 2. 插入数据
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 2:('Bob', 25)")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        // 3. 验证插入
        .assert_vertex_count("Person", 2)
        .assert_edge_exists(1, 2, "KNOWS")
        // 4. 查询验证
        .query("MATCH (n:Person)-[KNOWS]->(m:Person) RETURN n.name, m.name")
        .assert_result_count(1)
        // 5. 更新验证
        .exec_dml("UPDATE 1 SET age = 31")
        .assert_vertex_props(1, "Person", hashmap! { "age" => Value::Int(31) })
        // 6. 删除验证
        .exec_dml("DELETE EDGE KNOWS 1 -> 2")
        .assert_edge_not_exists(1, 2, "KNOWS");
}
```

## 6. Explain/Profile 测试规范

### 6.1 查询计划验证

```rust
#[test]
fn test_query_plan_structure() {
    let mut scenario = TestScenario::new().unwrap();
    scenario
        .setup_schema(vec!["CREATE TAG Person(name STRING, age INT)"])
        .load_data(vec!["INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)"]);
    
    // 使用 EXPLAIN 验证查询计划
    scenario.query("EXPLAIN MATCH (n:Person) WHERE n.age > 25 RETURN n");
    
    // 验证计划包含预期的操作符
    if let Some(ExecutionResult::DataSet(ds)) = &scenario.last_result {
        let plan_text = format_plan(ds);
        assert!(plan_text.contains("Filter"), "计划应包含 Filter 操作符");
        assert!(plan_text.contains("ScanVertices"), "计划应包含 ScanVertices 操作符");
    }
}
```

### 6.2 性能统计验证

```rust
#[test]
fn test_query_performance_stats() {
    let mut scenario = TestScenario::new().unwrap();
    // ... 准备数据
    
    scenario.query("PROFILE MATCH (n:Person) RETURN n");
    
    if let Some(ExecutionResult::DataSet(ds)) = &scenario.last_result {
        let stats = extract_profiling_stats(ds);
        assert!(stats.rows >= 0, "应返回行数统计");
        assert!(stats.exec_time_us >= 0, "应返回执行时间");
    }
}
```

## 7. 实施阶段

### 阶段1：现有测试改造（当前阶段）

目标：将现有测试从"仅解析验证"升级到"完整执行验证"

文件清单：
- `tests/integration_ddl.rs` - DDL测试
- `tests/integration_dml.rs` - DML测试
- `tests/integration_dql.rs` - DQL测试

改造原则：
1. 保留解析测试（验证语法正确性）
2. 改造执行测试，使用 TestScenario
3. 添加状态验证断言
4. 删除无意义的永真断言

### 阶段2：数据流测试

新增文件：
- `tests/integration_data_flow.rs` - 完整CRUD流程测试

测试场景：
- 社交网络数据流
- 电商数据流
- 图遍历数据流

### 阶段3：Explain/Profile 测试

新增文件：
- `tests/integration_explain_profile.rs` - 查询计划和性能测试

测试内容：
- 查询计划结构验证
- 索引使用验证
- 性能基准测试

## 8. 测试数据规范

### 8.1 社交网络数据集

```cypher
// 顶点
INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX Person(name, age) VALUES 2:('Bob', 25);
INSERT VERTEX Person(name, age) VALUES 3:('Charlie', 35);
INSERT VERTEX Person(name, age) VALUES 4:('David', 28);

// 边
INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01');
INSERT EDGE KNOWS(since) VALUES 1 -> 3:('2021-01-01');
INSERT EDGE KNOWS(since) VALUES 2 -> 3:('2020-06-01');
INSERT EDGE KNOWS(since) VALUES 3 -> 4:('2022-01-01');
```

### 8.2 电商数据集

```cypher
// 顶点
INSERT VERTEX Customer(name, email) VALUES 1:('John', 'john@example.com');
INSERT VERTEX Product(name, price) VALUES 101:('Laptop', 999.99);
INSERT VERTEX Product(name, price) VALUES 102:('Mouse', 29.99);
INSERT VERTEX Order(order_date, total) VALUES 1001:('2024-01-01', 1029.98);

// 边
INSERT EDGE PURCHASED() VALUES 1 -> 1001;
INSERT EDGE CONTAINS(quantity) VALUES 1001 -> 101:(1);
INSERT EDGE CONTAINS(quantity) VALUES 1001 -> 102:(1);
```

## 9. 代码规范

### 9.1 命名规范

| 类型 | 命名方式 | 示例 |
|------|----------|------|
| 测试函数 | `test_<feature>_<scenario>` | `test_create_tag_basic` |
| 辅助函数 | `snake_case` | `assert_vertex_exists` |
| 测试数据 | `SCREAMING_SNAKE_CASE` | `SOCIAL_NETWORK_DATA` |

### 9.2 注释规范

```rust
/// Test creating TAG with single property
/// 
/// # Verification Points
/// - Parsing succeeds
/// - Execution succeeds
/// - Tag exists after creation
/// - Tag properties match definition
#[test]
fn test_create_tag_single_property() {
    // ...
}
```

### 9.3 错误信息规范

断言应提供清晰的错误信息：

```rust
assert!(
    result.count() > 0,
    "Expected vertex {} with tag {} to exist, but not found",
    vid, tag
);
```

## 10. 参考文件

| 文件 | 说明 |
|------|------|
| `tests/common/test_scenario.rs` | TestScenario 实现 |
| `tests/common/validation_helpers.rs` | 验证辅助函数 |
| `src/query/executor/base/execution_result.rs` | ExecutionResult 类型 |
| `src/query/validator/utility/explain_validator.rs` | Explain/Profile 实现 |
| `src/core/query_result/result.rs` | CoreResult 类型 |
| `src/core/value/dataset.rs` | DataSet 类型 |

## 11. 附录

### 11.1 禁用模式

以下模式应被禁止：

```rust
// ❌ 永真断言
assert!(result.is_ok() || result.is_err());

// ❌ 仅打印不验证
println!("结果: {:?}", result);

// ❌ 无状态验证的执行测试
let result = pipeline.execute_query(query);
assert!(result.is_ok());
// 缺少：验证数据是否实际写入

// ❌ 重复创建 TestStorage（每个测试应使用 TestScenario）
let test_storage = TestStorage::new().expect("...");
let storage = test_storage.storage();
let stats_manager = Arc::new(StatsManager::new());
let mut pipeline_manager = QueryPipelineManager::with_optimizer(...);
```

### 11.2 推荐模式

```rust
// ✅ 使用 TestScenario
TestScenario::new()
    .expect("创建场景失败")
    .exec_ddl("CREATE TAG Person(name STRING)")
    .assert_success()
    .assert_tag_exists("Person");

// ✅ 完整验证
scenario
    .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
    .assert_success()
    .assert_vertex_exists(1, "Person")
    .assert_vertex_props(1, "Person", hashmap! { "name" => Value::String("Alice".into()) });
```
