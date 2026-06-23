# 集成测试扩展方案 - 参考信息收集

## 1. 数据库测试最佳实践（来自网络搜索）

### 1.1 集成测试通用最佳实践

来源：[Best Practices for Successful System Integration Testing & Validation](https://www.itconvergence.com/blog/best-practices-for-successful-integration-testing-and-validation/)

**关键要点**：
- 规划是集成测试和验证过程中的关键第一步
- 持续测试和验证集成系统
- 问题跟踪帮助识别模式或趋势
- 持续评估集成测试和验证过程以识别改进领域

### 1.2 图数据库测试特点

来源：[Data Validation and Testing Your Graph Data State - GraphGrid](https://graphgrid.com/blog/data-validation-and-testing-your-graph-data-state/)

**关键要点**：
- 图数据验证涉及评估数据资产质量
- 需要验证节点和关系的完整性
- 属性一致性检查

来源：[Best practices for graph DBs - Medium](https://medium.com/test-of-ideas/best-practices-fe0fd0acff55)

**关键要点**：
- 图数据库有丰富的案例研究
- 实施最佳实践可提高成功率
- 需要关注数据建模和查询优化

### 1.3 确定性集成测试模式

来源：[Best pattern for ensuring deterministic integration tests with db - Reddit](https://www.reddit.com/r/node/comments/1ankic6/best_pattern_for_ensuring_deterministic/)

**推荐模式**：
1. **事务回滚模式**：每个测试包装在事务中，拆卸时回滚
2. **独立数据库模式**：每个测试有自己的数据库
3. **租户隔离模式**：测试绑定到随机租户

### 1.4 图数据建模最佳实践

来源：[Graph data modeling best practices - Memgraph](https://memgraph.com/docs/data-modeling/best-practices)

**关键要点**：
- 优化内存使用
- 对重复值使用枚举而非字符串
- 性能测试不可忽视
- 正确索引数据

## 2. NebulaGraph测试框架

来源：[Best practices - Nebula Graph Database Manual](https://docs.nebula-graph.io/2.6.1/8.service-tuning/practice/)

**BDD-Based Integration Testing Framework**：
- NebulaGraph使用基于BDD（行为驱动开发）的集成测试框架
- 分为Part I和Part II两部分
- 支持自动化测试用例生成

## 3. 图数据库测试学术研究

来源：[Testing Graph Database Systems via Equivalent Query Rewriting (GRev)](https://joyemang33.github.io/assets/pdf/GRev.pdf)

**等效查询重写（EQR）方法**：
- 提出抽象语法图（ASG）概念
- 随机行走覆盖（RWC）算法
- 支持检测逻辑错误、性能问题和异常问题

**GRev与其他技术对比**：

| 工具 | 方法 | 支持语言 | 检测共享错误 | 测试独特功能 | 基础查询不敏感 | 检测性能错误 | 检测图相关错误 |
|------|------|----------|--------------|--------------|----------------|--------------|----------------|
| GDsmith | 差异测试 | Cypher | ✓ | ✓ | ✓ | ✗ | ✗ |
| GRev | 等效查询重写 | Cypher | ✓ | ✓ | ✓ | ✓ | ✓ |

## 4. Neo4j测试实践

来源：[Transforming Natural Language into Cypher Queries](https://medium.com/@wumingzhu1989/transforming-natural-language-into-cypher-queries-gpt-4o-as-a-bridge-to-neo4j-graph-data-072b6ff1b5ba)

**查询验证层**：
- 自动过滤包含修改关键词（如DELETE）的查询
- 确保测试安全性

来源：[PyG integration: Sample and export - Neo4j](https://neo4j.com/docs/graph-data-science-client/current/tutorials/import-sample-export-gnn/)

**数据导出验证**：
- 节点计数验证
- 关系数据流式导出
- 属性数据验证

## 5. Rust数据库测试库

通过Context7 MCP查询：

### 5.1 Database Rider
- **库ID**: /database-rider/database-rider
- **描述**: Database testing made easy!
- **代码片段**: 114
- **来源声誉**: Medium

特点：
- 简化数据库测试
- 支持多种数据库
- 数据集比较功能

### 5.2 Database Cleaner
- **库ID**: /databasecleaner/database_cleaner
- **描述**: Strategies for cleaning databases
- **代码片段**: 76
- **来源声誉**: Medium

特点：
- 确保测试状态干净
- 多种清理策略
- Ruby生态，概念可借鉴

### 5.3 Embedded Database Spring Test
- **库ID**: /zonkyio/embedded-database-spring-test
- **描述**: Creating isolated embedded databases for Spring-powered integration tests
- **代码片段**: 46
- **来源声誉**: High

特点：
- 隔离的嵌入式数据库
- 集成测试专用
- Spring生态，架构可借鉴

## 6. 项目内部Explain/Profile功能分析

### 6.1 Explain功能

**文件**: `src/query/validator/utility/explain_validator.rs`

**功能**：
```rust
pub struct ExplainValidator {
    format: ExplainFormat,           // Table, Dot格式
    inner_validator: Option<Box<Validator>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    // ...
}
```

**验证输出列**：
- `id` - 节点ID (Int)
- `name` - 节点名称 (String)
- `dependencies` - 依赖关系 (String)
- `profiling_data` - 性能数据 (String)
- `operator info` - 操作符信息 (String)

**局限性**：
- 仅分析查询结构
- 不执行实际查询
- 无法验证数据变更

### 6.2 Profile功能

**文件**: 同一文件中的 `ProfileValidator`

**功能**：
- 实际执行查询
- 收集性能统计

**可用于测试**：
- 执行时间统计
- 行数统计
- 操作符性能分析

### 6.3 PlanDescription结构

**文件**: `src/query/planning/plan/core/explain.rs`

```rust
pub struct PlanDescription {
    pub plan_node_descs: Vec<PlanNodeDescription>,
    pub node_index_map: HashMap<i64, usize>,
    pub format: String,
    pub optimize_time_in_us: i64,
}

pub struct PlanNodeDescription {
    pub name: String,
    pub id: i64,
    pub output_var: String,
    pub description: Option<Vec<Pair>>,
    pub profiles: Option<Vec<ProfilingStats>>,  // 性能统计
    pub branch_info: Option<PlanNodeBranchInfo>,
    pub dependencies: Option<Vec<i64>>,
}

pub struct ProfilingStats {
    pub rows: i64,                    // 行数
    pub exec_duration_in_us: i64,     // 执行时间
    pub total_duration_in_us: i64,    // 总时间
    pub other_stats: HashMap<String, String>,
}
```

**测试应用**：
- 验证查询计划结构
- 验证性能统计
- 验证行数估计

## 7. 执行结果类型详细分析

### 7.1 ExecutionResult枚举

**文件**: `src/query/executor/base/execution_result.rs`

```rust
pub enum ExecutionResult {
    Values(Vec<Value>),           // 通用值列表
    Vertices(Vec<Vertex>),        // 顶点列表
    Edges(Vec<Edge>),             // 边列表
    DataSet(DataSet),             // 结构化数据集
    Result(CoreResult),           // 核心结果对象
    Empty,                        // 空结果
    Success,                      // 成功无数据
    Error(String),                // 错误信息
    Count(usize),                 // 计数结果
    Paths(Vec<Path>),             // 路径列表
}
```

### 7.2 CoreResult结构

**文件**: `src/core/query_result/result.rs`

```rust
pub struct Result {
    rows: Vec<Vec<Value>>,        // 行数据
    col_names: Vec<String>,       // 列名
    meta: ResultMeta,             // 元数据
}

pub struct ResultMeta {
    pub row_count: usize,
    pub col_count: usize,
    pub state: ResultState,       // NotStarted, InProgress, Completed, Failed
    pub memory_usage: u64,
}
```

### 7.3 DataSet结构

**文件**: `src/core/value/dataset.rs`

```rust
pub struct DataSet {
    pub col_names: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

// 支持的操作
impl DataSet {
    pub fn row_count(&self) -> usize;
    pub fn col_count(&self) -> usize;
    pub fn get_col_index(&self, col_name: &str) -> Option<usize>;
    pub fn get_column(&self, col_name: &str) -> Option<Vec<Value>>;
    pub fn filter<F>(&self, predicate: F) -> DataSet;
    pub fn map<F>(&self, mapper: F) -> DataSet;
    pub fn sort_by<F>(&mut self, comparator: F);
    pub fn join(&self, other: &DataSet, on: &str) -> Result<DataSet, String>;
    pub fn group_by<F, K>(&self, key_fn: F) -> Vec<(K, DataSet)>;
}
```

## 8. 测试数据设计建议

### 8.1 社交网络数据集

```
顶点：
- Person(1, "Alice", 30)
- Person(2, "Bob", 25)
- Person(3, "Charlie", 35)
- Person(4, "David", 28)

边：
- KNOWS(1->2, since="2020-01-01")
- KNOWS(1->3, since="2021-01-01")
- KNOWS(2->3, since="2020-06-01")
- KNOWS(3->4, since="2022-01-01")
```

### 8.2 电商数据集

```
顶点：
- Customer(1, "John", "john@example.com")
- Product(101, "Laptop", 999.99)
- Product(102, "Mouse", 29.99)
- Order(1001, "2024-01-01", 1029.98)

边：
- PURCHASED(1->1001)
- CONTAINS(1001->101, quantity=1)
- CONTAINS(1001->102, quantity=1)
```

## 9. 推荐的测试模式

### 9.1 AAA模式（Arrange-Act-Assert）

```rust
#[test]
fn test_create_and_query_vertex() {
    // Arrange
    let mut scenario = TestScenario::new();
    scenario.exec_ddl("CREATE TAG Person(name STRING, age INT)");
    
    // Act
    let result = scenario.exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)");
    
    // Assert
    assert!(result.is_ok());
    scenario.assert_vertex_exists(1, "Person");
    scenario.assert_vertex_props(1, hashmap! {
        "name" => Value::String("Alice".into()),
        "age" => Value::Int(30)
    });
}
```

### 9.2 Given-When-Then模式（BDD）

```rust
#[test]
fn test_update_vertex() {
    TestScenario::new()
        .given("a Person vertex exists", |s| {
            s.exec_ddl("CREATE TAG Person(name STRING, age INT)")
             .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)");
        })
        .when("updating the vertex", |s| {
            s.exec_dml("UPDATE 1 SET age = 31");
        })
        .then("the age should be updated", |s| {
            s.assert_vertex_props(1, hashmap! {
                "age" => Value::Int(31)
            });
        });
}
```

## 10. 工具链建议

### 10.1 断言库
- 使用 `pretty_assertions` 提供更好的差异显示
- 自定义图数据库专用断言宏

### 10.2 测试组织
- 使用 `rstest` 进行参数化测试
- 使用 `test-case` 进行测试用例管理

### 10.3 性能测试
- 使用 `criterion` 进行基准测试
- 利用Profile功能收集性能数据
