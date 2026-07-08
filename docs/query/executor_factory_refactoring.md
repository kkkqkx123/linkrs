# 执行器工厂重构设计方案

## 当前设计问题分析

### 1. 过度复杂的结构层次

当前的 `ExecutorFactory` 存在以下问题：

```rust
// 当前设计：过多的嵌套层次
ExecutorFactory
├── Builders (包含9个builder结构体)
│   ├── DataAccessBuilder
│   ├── DataModificationBuilder
│   ├── DataProcessingBuilder
│   ├── JoinBuilder
│   ├── SetOperationBuilder
│   ├── TraversalBuilder
│   ├── TransformationBuilder
│   ├── ControlFlowBuilder
│   └── AdminBuilder
├── RecursionDetector
├── SafetyValidator
└── 巨大的match语句（50+分支）
```

### 2. 违反开闭原则

每添加一个新的计划节点类型，必须修改：
1. `executor_factory.rs` 中的巨大 match 语句
2. 对应的 builder 结构体
3. `Builders` 集合结构体

### 3. 存储泛型污染

`<S: StorageClient + 'static>` 泛型参数贯穿整个工厂层次结构，导致：
- 代码冗余
- 编译时间增加
- 类型约束传播

### 4. 重复代码

`build_loop_executor` 和 `build_select_executor` 中重复创建临时工厂的代码。

## 改进方案：类型化注册表 + 静态分发

### 核心设计原则

1. **保持静态分发**：使用编译期 match，零运行时开销
2. **函数替代结构体**：使用无状态函数替代 builder 结构体
3. **注册表模式**：模块化注册，支持开闭原则
4. **简化类型约束**：减少泛型参数传播

### 新架构设计

```rust
// 新设计：扁平化结构
ExecutorFactoryV2
├── TypedExecutorRegistry  // 类型化注册表
│   ├── builders: Vec<(&str, BuilderFn)>  // 函数指针注册表
│   └── create() -> match语句（静态分发）
├── RecursionDetector
├── SafetyValidator
└── 简化的API

// Builder函数独立模块
builder_fns/
├── data_access.rs      // 纯函数
├── data_modification.rs
├── data_processing.rs
└── ...
```

### 关键改进点

#### 1. 函数式 Builder

将结构体-based builder 转换为纯函数：

```rust
// 旧设计：结构体 + 方法
pub struct DataAccessBuilder<S: StorageClient> {
    _phantom: PhantomData<S>,
}

impl<S: StorageClient> DataAccessBuilder<S> {
    pub fn build_scan_vertices(&self, ...) -> Result<ExecutorEnum<S>, QueryError> {
        // ...
    }
}

// 新设计：纯函数
pub fn build_scan_vertices<S: StorageClient>(
    node: &PlanNodeEnum,
    storage: Arc<Mutex<S>>,
    context: &ExecutionContext,
) -> Result<ExecutorEnum<S>, QueryError> {
    match node {
        PlanNodeEnum::ScanVertices(n) => {
            // 创建执行器
        }
        _ => Err(...)
    }
}
```

**优势**：
- 无状态，无需 `PhantomData`
- 零开销，无需结构体实例化
- 更容易测试

#### 2. 类型化注册表

```rust
pub struct TypedExecutorRegistry<S: StorageClient> {
    builders: Vec<(&'static str, BuilderFn<S>)>,
}

pub type BuilderFn<S> = fn(
    &PlanNodeEnum,
    Arc<Mutex<S>>,
    &ExecutionContext,
) -> Result<ExecutorEnum<S>, QueryError>;
```

注册方式：
```rust
impl<S: StorageClient> TypedExecutorRegistry<S> {
    fn create_default_registry() -> Self {
        let mut registry = Self::new();
        
        // 模块化注册
        registry.register("ScanVertices", build_scan_vertices);
        registry.register("ScanEdges", build_scan_edges);
        // ...
        
        registry
    }
}
```

#### 3. 保持静态分发

```rust
// 编译期生成的match语句，确保类型安全
pub fn create(&self, node: &PlanNodeEnum, ...) -> Result<ExecutorEnum<S>, QueryError> {
    match node {
        PlanNodeEnum::ScanVertices(n) => {
            self.call_registered("ScanVertices", node, storage, context)
        }
        PlanNodeEnum::ScanEdges(n) => {
            self.call_registered("ScanEdges", node, storage, context)
        }
        // ... 所有分支在编译期确定
    }
}
```

**优势**：
- 编译期检查确保所有类型被处理
- 零运行时开销
- 更好的IDE支持

#### 4. 特殊处理优化

对于 `Loop` 和 `Select` 这类需要递归构建的执行器：

```rust
// 使用注册表自身进行递归构建，无需创建临时工厂
PlanNodeEnum::Loop(n) => self.build_loop_executor(n, storage, context),

fn build_loop_executor(&self, node: &LoopNode, ...) -> Result<ExecutorEnum<S>, QueryError> {
    let body = node.body().ok_or(...)?;
    let body_executor = self.create(body, storage.clone(), context)?; // 递归调用
    // ...
}
```

### 实施步骤

#### 阶段1：创建基础设施（已完成）

1. ✅ 创建 `typed_registry.rs` - 类型化注册表
2. ✅ 创建 `builder_fns/` 模块 - 函数式builder
3. ✅ 创建 `executor_factory_v2.rs` - 新工厂实现

#### 阶段2：迁移 Builder 函数

按类别迁移 builder 函数：

```rust
// 每个文件独立迁移
builder_fns/
├── data_access.rs      // 7个函数
├── data_modification.rs // 3个函数
├── data_processing.rs  // 8个函数
├── join.rs            // 6个函数
├── set_operation.rs   // 3个函数
├── graph_traversal.rs // 7个函数
├── transformation.rs  // 6个函数
├── control_flow.rs    // 5个函数（Loop和Select特殊处理）
└── admin.rs           // 30+个函数
```

#### 阶段3：验证和切换

1. 保持旧工厂运行，并行测试新工厂
2. 逐步替换使用点
3. 最终移除旧工厂代码

### 代码示例

#### 新工厂使用方式

```rust
// 创建工厂
let factory = ExecutorFactoryV2::with_storage(storage);

// 分析计划
factory.analyze_plan_lifecycle(&plan)?;

// 创建执行器
let executor = factory.create_executor(&plan_node, storage, context)?;
```

#### 注册新的执行器类型

```rust
// 1. 在对应模块添加builder函数
pub fn build_new_executor<S: StorageClient>(
    node: &PlanNodeEnum,
    storage: Arc<Mutex<S>>,
    context: &ExecutionContext,
) -> Result<ExecutorEnum<S>, QueryError> {
    match node {
        PlanNodeEnum::NewNode(n) => {
            // 创建执行器
        }
        _ => Err(...)
    }
}

// 2. 在注册表中注册
registry.register("NewNode", build_new_executor);

// 3. 在match语句中添加分支（编译期检查）
PlanNodeEnum::NewNode(n) => {
    self.call_registered("NewNode", node, storage, context)
}
```

### 预期收益

1. **代码量减少**：消除 9 个 builder 结构体的样板代码
2. **编译时间**：减少泛型实例化
3. **可维护性**：模块化注册，开闭原则
4. **性能**：保持静态分发，零运行时开销
5. **测试性**：纯函数更容易单元测试

### 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| 大规模重构引入bug | 保持旧工厂并行运行，逐步切换 |
| 类型不匹配 | 编译期match确保类型安全 |
| 学习成本 | 完善的文档和示例 |

## 结论

该重构方案在保持静态分发优势的同时，解决了当前设计的结构复杂性和维护性问题。通过函数式builder和注册表模式，实现了更好的模块化和可扩展性。
