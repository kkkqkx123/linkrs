# Query模块优化路线图

## 1. 优化目标

基于数据流分析，制定以下优化目标：

1. **性能优化**: 消除存储访问瓶颈，提高并发处理能力
2. **内存优化**: 添加内存限制，防止OOM，支持流式处理
3. **架构优化**: 简化上下文层级，统一错误处理
4. **可维护性**: 减少样板代码，提高代码复用率

## 2. 短期优化（1-2周）

### 2.1 同步原语优化

#### 问题
`ExpressionAnalysisContext` 在编译期使用 `DashMap` 过度重量级。

#### 方案
```rust
// 优化前
pub struct ExpressionAnalysisContext {
    expressions: Arc<DashMap<ExpressionId, Arc<ExpressionMeta>>>,
    type_cache: Arc<DashMap<ExpressionId, DataType>>,
    // ...
}

// 优化后
pub struct ExpressionAnalysisContext {
    expressions: Arc<RwLock<HashMap<ExpressionId, Arc<ExpressionMeta>>>>,
    type_cache: Arc<RwLock<HashMap<ExpressionId, DataType>>>,
    // ...
}
```

#### 预期收益
- 减少编译期同步开销 20-30%
- 降低内存占用

### 2.2 上下文合并

#### 问题
`QueryResourceContext` 和 `QuerySpaceContext` 内容过少，拆分不必要。

#### 方案
```rust
// 优化前
pub struct QueryContext {
    rctx: Arc<QueryRequestContext>,
    execution_manager: QueryExecutionManager,
    resource_context: QueryResourceContext,  // 3个字段
    space_context: QuerySpaceContext,        // 4个字段
}

// 优化后
pub struct QueryContext {
    rctx: Arc<QueryRequestContext>,
    execution_manager: QueryExecutionManager,
    // 内联 resource_context 字段
    object_pool: Option<Arc<dyn ObjectPool>>,
    symbol_table: SymbolTable,
    id_generator: IdGenerator,
    // 内联 space_context 字段
    space_info: Option<SpaceInfo>,
    charset: String,
    collation: String,
}
```

#### 预期收益
- 减少访问层级: `qctx.space_context().charset()` → `qctx.charset()`
- 减少 `Arc` 开销
- 简化代码结构

### 2.3 错误类型统一

#### 问题
各模块错误类型过多，转换成本高。

#### 方案
```rust
// 新增统一的错误类型
pub enum QueryPipelineError {
    Parse { 
        source: ParseError, 
        location: SourceLocation 
    },
    Validation { 
        source: ValidationError, 
        phase: ValidationPhase 
    },
    Planning { 
        source: PlannerError, 
        stmt_type: StatementType 
    },
    Optimization { 
        source: CostError, 
        rule: Option<String> 
    },
    Execution { 
        source: ExecutionError, 
        executor: String 
    },
}

// 实现自动转换
impl From<ParseError> for QueryPipelineError {
    fn from(e: ParseError) -> Self {
        QueryPipelineError::Parse { 
            source: e, 
            location: e.location() 
        }
    }
}
```

#### 预期收益
- 统一错误处理接口
- 保留完整错误上下文
- 简化错误转换代码

## 3. 中期优化（1个月）

### 3.1 执行器工厂重构

#### 问题
手动维护68个 PlanNode 到 Executor 的映射，容易出错。

#### 方案
```rust
// 定义声明宏
macro_rules! define_executor_mapping {
    ($($plan_node:ident => $executor:ty, $builder:ident);* $(;)?) => {
        impl<S: StorageClient> ExecutorFactory<S> {
            pub fn create_executor(
                &mut self,
                plan_node: &PlanNodeEnum,
                storage: Arc<Mutex<S>>,
                context: &ExecutionContext,
            ) -> Result<ExecutorEnum<S>, QueryError> {
                match plan_node {
                    $(
                        PlanNodeEnum::$plan_node(node) => {
                            self.builders.$builder()
                                .build(node, storage, context)
                                .map(ExecutorEnum::$plan_node)
                        }
                    )*
                    _ => Err(QueryError::UnsupportedPlanNode(
                        format!("{:?}", plan_node)
                    )),
                }
            }
        }
        
        // 编译期验证数量一致
        const _: () = {
            let plan_count = <[()]>::len(&[$(stringify!($plan_node)),*]);
            let exec_count = <[()]>::len(&[$(stringify!($executor)),*]);
            assert!(plan_count == exec_count, "Count mismatch");
        };
    };
}

// 使用宏定义映射
define_executor_mapping! {
    ScanVertices => ScanVerticesExecutor, data_access;
    ScanEdges => ScanEdgesExecutor, data_access;
    GetVertices => GetVerticesExecutor, data_access;
    // ... 其他65个映射
}
```

#### 预期收益
- 消除样板代码 200+ 行
- 编译期验证映射完整性
- 新增节点只需修改一处

### 3.2 重写规则合并

#### 问题
谓词下推有12个独立规则，逻辑重复。

#### 方案
```rust
// 定义通用谓词下推规则
pub struct PredicatePushdownRule {
    target_nodes: Vec<NodeCategory>,
}

impl RewriteRule for PredicatePushdownRule {
    fn name(&self) -> &'static str {
        "PredicatePushdown"
    }
    
    fn apply(&self, plan: ExecutionPlan, ctx: &RewriteContext) -> RewriteResult {
        plan.transform(|node| {
            if let PlanNodeEnum::Filter(filter) = node {
                if let Some(child) = filter.child() {
                    if self.can_pushdown(child) {
                        return self.pushdown(filter, child);
                    }
                }
            }
            node
        })
    }
    
    fn can_pushdown(&self, node: &PlanNodeEnum) -> bool {
        self.target_nodes.iter().any(|cat| cat.matches(node))
    }
}

// 配置不同场景
lazy_static! {
    static ref SCAN_PREDICATE_PUSHDOWN: PredicatePushdownRule = 
        PredicatePushdownRule {
            target_nodes: vec![
                NodeCategory::Scan,
                NodeCategory::IndexScan,
            ],
        };
    
    static ref JOIN_PREDICATE_PUSHDOWN: PredicatePushdownRule = 
        PredicatePushdownRule {
            target_nodes: vec![
                NodeCategory::Join,
                NodeCategory::HashJoin,
            ],
        };
}
```

#### 预期收益
- 减少代码量 60%+
- 统一逻辑，易于维护
- 显式配置优先级

### 3.3 计划缓存改进

#### 问题
缓存键使用完整查询文本，没有规范化。

#### 方案
```rust
// 优化前
pub struct PlanCacheKey {
    pub query_template: String,
    pub param_count: usize,
}

// 优化后
pub struct PlanCacheKey {
    pub query_hash: u64,  // 使用哈希而非完整文本
    pub param_count: usize,
}

// 查询规范化
pub fn normalize_query(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(", ", ",")
}

// 实现LRU淘汰
pub struct QueryPlanCache {
    cache: LruCache<PlanCacheKey, CachedPlan>,
    // ...
}
```

#### 预期收益
- 提高缓存命中率 20-40%
- 减少内存占用
- 支持更多查询复用

## 4. 长期优化（2-3个月）

### 4.1 存储访问优化

#### 问题
`Arc<Mutex<S>>` 成为全局瓶颈。

#### 方案
```rust
// 连接池模式
pub struct StorageConnectionPool<S: StorageClient> {
    connections: ArrayQueue<Arc<S>>,
    max_connections: usize,
    semaphore: Semaphore,
}

impl<S: StorageClient> StorageConnectionPool<S> {
    pub async fn acquire(&self) -> Result<PooledConnection<S>, PoolError> {
        let permit = self.semaphore.acquire().await?;
        let conn = self.connections.pop()
            .ok_or_else(|| PoolError::Exhausted)?;
        Ok(PooledConnection {
            connection: conn,
            permit,
            pool: self,
        })
    }
}

// 修改执行器工厂
pub struct ExecutorFactory<S: StorageClient> {
    connection_pool: Arc<StorageConnectionPool<S>>,
    // ...
}

impl<S: StorageClient> ExecutorFactory<S> {
    pub async fn create_executor(
        &self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let conn = self.connection_pool.acquire().await?;
        // 使用连接创建执行器
    }
}
```

#### 预期收益
- 支持并发存储访问
- 提高吞吐量 50%+
- 更好的资源管理

### 4.2 流式执行

#### 问题
所有结果需要完全物化，内存压力大。

#### 方案
```rust
// 定义流式结果
pub enum ExecutionResult {
    Empty,
    Values(Vec<Value>),
    Rows(Vec<Row>),
    Stream(StreamResult),  // 新增
}

pub struct StreamResult {
    receiver: mpsc::Receiver<Result<Row, ExecutionError>>,
    schema: Schema,
}

impl StreamResult {
    pub async fn next(&mut self) -> Option<Result<Row, ExecutionError>> {
        self.receiver.recv().await
    }
}

// 修改执行器支持流式
pub trait StreamingExecutor<S: StorageClient>: Executor<S> {
    fn execute_stream(
        &mut self,
    ) -> Result<StreamResult, ExecutionError>;
}

// 示例：流式Scan
impl<S: StorageClient> StreamingExecutor<S> for ScanVerticesExecutor<S> {
    fn execute_stream(&mut self) -> Result<StreamResult, ExecutionError> {
        let (tx, rx) = mpsc::channel(1000);  // 缓冲区大小
        
        tokio::spawn(async move {
            let storage = self.storage.lock();
            let mut stream = storage.scan_vertices_stream(&self.tag);
            
            while let Some(vertex) = stream.next().await {
                if tx.send(Ok(vertex)).await.is_err() {
                    break;  // 接收端关闭
                }
            }
        });
        
        Ok(StreamResult {
            receiver: rx,
            schema: self.schema.clone(),
        })
    }
}
```

#### 预期收益
- 支持大数据集处理
- 降低内存峰值 70%+
- 提高响应速度（首行返回时间）

### 4.3 查询管道重构

#### 问题
`QueryPipelineManager` 职责过重。

#### 方案
```rust
// 拆分职责到独立组件
pub struct QueryPipelineManager<S: StorageClient> {
    parser: Arc<Parser>,
    validator: Arc<Validator>,
    planner: Arc<Planner>,
    optimizer: Arc<OptimizerEngine>,
    executor_factory: Arc<ExecutorFactory<S>>,
    cache_manager: Arc<CacheManager>,
}

// 各阶段独立接口
#[async_trait]
pub trait QueryStage<Input, Output> {
    async fn process(&self, input: Input, ctx: Arc<QueryContext>) -> Result<Output, StageError>;
}

// 解析阶段
pub struct ParseStage;
#[async_trait]
impl QueryStage<String, Arc<Ast>> for ParseStage {
    async fn process(&self, query: String, ctx: Arc<QueryContext>) -> Result<Arc<Ast>, StageError> {
        // ...
    }
}

// 验证阶段
pub struct ValidateStage;
#[async_trait]
impl QueryStage<Arc<Ast>, ValidatedStatement> for ValidateStage {
    async fn process(&self, ast: Arc<Ast>, ctx: Arc<QueryContext>) -> Result<ValidatedStatement, StageError> {
        // ...
    }
}

// 使用管道组合
pub struct QueryPipeline<S: StorageClient> {
    stages: Vec<Box<dyn QueryStage<..., ...>>>,
}

impl<S: StorageClient> QueryPipeline<S> {
    pub async fn execute(&self, query: String, ctx: Arc<QueryContext>) -> Result<ExecutionResult, PipelineError> {
        let ast = self.parser.process(query, ctx.clone()).await?;
        let validated = self.validator.process(ast, ctx.clone()).await?;
        let plan = self.planner.process(validated, ctx.clone()).await?;
        let optimized = self.optimizer.process(plan, ctx.clone()).await?;
        let result = self.executor.process(optimized, ctx).await?;
        Ok(result)
    }
}
```

#### 预期收益
- 单一职责，易于测试
- 支持阶段间异步处理
- 便于添加监控和重试

## 5. 实施计划

### 第一阶段（第1-2周）

| 任务 | 负责人 | 预计工时 | 依赖 |
|-----|-------|---------|-----|
| 同步原语优化 | TBD | 16h | 无 |
| 上下文合并 | TBD | 12h | 无 |
| 错误类型统一 | TBD | 20h | 无 |
| 单元测试 | TBD | 16h | 上述任务 |

### 第二阶段（第3-6周）

| 任务 | 负责人 | 预计工时 | 依赖 |
|-----|-------|---------|-----|
| 执行器工厂重构 | TBD | 24h | 无 |
| 重写规则合并 | TBD | 32h | 无 |
| 计划缓存改进 | TBD | 16h | 无 |
| 集成测试 | TBD | 24h | 上述任务 |

### 第三阶段（第7-12周）

| 任务 | 负责人 | 预计工时 | 依赖 |
|-----|-------|---------|-----|
| 存储访问优化 | TBD | 40h | 第二阶段完成 |
| 流式执行 | TBD | 48h | 第二阶段完成 |
| 查询管道重构 | TBD | 40h | 流式执行完成 |
| 性能测试 | TBD | 32h | 上述任务 |

## 6. 风险评估

### 6.1 技术风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|-----|-------|-----|---------|
| 流式执行引入并发bug | 中 | 高 | 充分测试，逐步上线 |
| 存储连接池配置不当 | 中 | 中 | 提供默认配置，监控指标 |
| 缓存改进导致命中率下降 | 低 | 中 | A/B测试，可回滚 |

### 6.2 进度风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|-----|-------|-----|---------|
| 重构范围超出预期 | 中 | 中 | 分阶段交付，及时调整 |
| 测试覆盖率不足 | 中 | 高 | 强制代码覆盖率检查 |

## 7. 成功指标

### 7.1 性能指标

| 指标 | 当前 | 目标 | 测量方法 |
|-----|-----|-----|---------|
| 并发查询QPS | X | X * 1.5 | sysbench |
| 平均查询延迟 | Y | Y * 0.7 | 监控统计 |
| 内存峰值 | Z | Z * 0.6 | 压力测试 |
| 缓存命中率 | 30% | 50% | 监控统计 |

### 7.2 代码质量指标

| 指标 | 当前 | 目标 | 测量方法 |
|-----|-----|-----|---------|
| 代码重复率 | 15% | < 10% | clippy |
| 平均函数行数 | 50 | < 40 | clippy |
| 测试覆盖率 | 60% | > 80% | cargo-tarpaulin |
| 文档覆盖率 | 40% | > 70% | cargo-doc |

## 8. 总结

本优化路线图分为三个阶段：

1. **短期（1-2周）**: 修复明显的同步原语和上下文问题，统一错误处理
2. **中期（1个月）**: 重构执行器工厂和重写规则，改进计划缓存
3. **长期（2-3个月）**: 引入连接池和流式执行，重构查询管道

通过逐步实施这些优化，可以显著提高查询模块的性能、可维护性和可扩展性。
