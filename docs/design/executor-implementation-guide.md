# Executor 重构实现指南

> 针对开发者的逐步实现说明
> 包含代码框架和关键实现点

---

## 1. 第一阶段：基础设施搭建（1 周）

### 1.1 定义 StreamingExecutor trait

**文件**：`crates/graphdb-query/src/query/executor/base/streaming_executor.rs`

```rust
use crate::query::executor::base::ExecutorStats;
use crate::core::error::DBResult;

/// 流式执行器的基本数据结构
pub struct DataChunk {
    /// 列名
    pub columns: Vec<String>,
    /// 行数据（初期保持行向量）
    pub rows: Vec<Vec<crate::core::Value>>,
    /// 实际行数
    pub size: usize,
}

/// 流式执行器的核心 trait
pub trait StreamingExecutor: Send {
    /// 打开资源
    fn open(&mut self) -> DBResult<()>;
    
    /// 拉取下一个数据块
    fn next(&mut self) -> DBResult<Option<DataChunk>>;
    
    /// 关闭资源
    fn close(&mut self) -> DBResult<()>;
    
    /// 获取执行统计信息
    fn stats(&self) -> &ExecutorStats;
    
    /// 获取可变统计信息
    fn stats_mut(&mut self) -> &mut ExecutorStats;
    
    /// 停止执行（LIMIT 中途停止时调用）
    fn stop(&mut self) -> DBResult<()> {
        Ok(())
    }
}

/// 单输入执行器的输入接口
pub trait StreamingInput: StreamingExecutor {
    fn set_input(&mut self, input: Box<dyn StreamingExecutor>);
    fn get_input(&self) -> Option<&dyn StreamingExecutor>;
}

/// 双输入执行器的输入接口
pub trait StreamingBinaryInput: StreamingExecutor {
    fn set_left_input(&mut self, input: Box<dyn StreamingExecutor>);
    fn set_right_input(&mut self, input: Box<dyn StreamingExecutor>);
}
```

### 1.2 实现 StreamingBaseExecutor

**文件**：`crates/graphdb-query/src/query/executor/base/streaming_base_executor.rs`

```rust
use std::sync::Arc;
use parking_lot::RwLock;
use crate::storage::StorageClient;
use crate::query::executor::base::ExecutorStats;
use super::streaming_executor::StreamingExecutor;

/// 流式执行器的基础实现
pub struct StreamingBaseExecutor<S> {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub storage: Option<Arc<RwLock<S>>>,
    pub context: Arc<ExecutionContext>,  // 与旧系统共享
    
    is_open: bool,
    stats: ExecutorStats,
}

impl<S: StorageClient + Send + 'static> StreamingBaseExecutor<S> {
    /// 创建基础执行器
    pub fn new(
        id: i64,
        name: String,
        storage: Arc<RwLock<S>>,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: Some(storage),
            context: Arc::new(ExecutionContext::new(...)),
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }
    
    /// 创建不需要存储的执行器
    pub fn without_storage(
        id: i64,
        name: String,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: None,
            context: Arc::new(ExecutionContext::new(...)),
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }
    
    /// 获取存储
    pub fn get_storage(&self) -> Option<&Arc<RwLock<S>>> {
        self.storage.as_ref()
    }
    
    /// 获取统计信息
    pub fn stats(&self) -> &ExecutorStats {
        &self.stats
    }
    
    /// 获取可变统计信息
    pub fn stats_mut(&mut self) -> &mut ExecutorStats {
        &mut self.stats
    }
    
    /// 标记为打开
    pub fn mark_opened(&mut self) {
        self.is_open = true;
    }
    
    /// 检查是否打开
    pub fn is_opened(&self) -> bool {
        self.is_open
    }
}
```

### 1.3 修改 PlanExecutor 支持 ExecutionMode

**文件**：`crates/graphdb-query/src/query/executor/factory/engine.rs`

```rust
use crate::query::executor::base::ExecutionResult;

/// 执行模式
pub enum ExecutionMode {
    /// 全物化模式（现有系统）
    Materialized,
    /// 流式模式（新系统）
    Streaming,
}

pub struct PlanExecutor<S: StorageClient + Send + 'static> {
    factory: ExecutorFactory<S>,
    object_pool: Option<Arc<ThreadSafeExecutorPool<S>>>,
    mode: ExecutionMode,
}

impl<S: StorageClient + Send + 'static> PlanExecutor<S> {
    pub fn new(factory: ExecutorFactory<S>) -> Self {
        Self {
            factory,
            object_pool: None,
            mode: ExecutionMode::Materialized,  // 默认向后兼容
        }
    }
    
    /// 设置执行模式
    pub fn with_mode(mut self, mode: ExecutionMode) -> Self {
        self.mode = mode;
        self
    }
    
    /// 执行计划
    pub fn execute_plan(
        &mut self,
        plan: &ExecutionPlan,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> DBResult<ExecutionResult> {
        match self.mode {
            ExecutionMode::Materialized => {
                // 现有的执行逻辑，保持不变
                self.execute_materialized(plan, storage, context)
            }
            ExecutionMode::Streaming => {
                // 新的流式执行逻辑
                self.execute_streaming(plan, storage, context)
            }
        }
    }
    
    /// 全物化执行（现有逻辑）
    fn execute_materialized(
        &mut self,
        plan: &ExecutionPlan,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> DBResult<ExecutionResult> {
        // 复制现有的 execute_plan 逻辑
        // build_executor_chain → execute
        // ...
        todo!()
    }
    
    /// 流式执行（新逻辑）
    fn execute_streaming(
        &mut self,
        plan: &ExecutionPlan,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> DBResult<ExecutionResult> {
        // 1. 构建流式执行器树
        let mut root_executor = self.build_streaming_executor_chain(
            &plan.root(),
            storage,
            context,
        )?;
        
        // 2. 执行 open
        root_executor.open()?;
        
        // 3. Pull 循环：逐个 chunk 拉取
        let mut all_rows = Vec::new();
        while let Some(chunk) = root_executor.next()? {
            all_rows.extend(chunk.rows);
        }
        
        // 4. 关闭资源
        root_executor.close()?;
        
        // 5. 返回结果
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: plan.output_columns(),
            rows: all_rows,
        }))
    }
    
    /// 构建流式执行器树
    fn build_streaming_executor_chain(
        &mut self,
        plan_node: &PlanNodeEnum,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> DBResult<Box<dyn StreamingExecutor>> {
        match plan_node.node_type() {
            // 扫描操作
            PlanNodeType::ScanVertices => {
                let executor = StreamingScanVerticesExecutor::new(
                    plan_node.id(),
                    storage,
                )?;
                Ok(Box::new(executor))
            }
            
            // 单输入操作
            PlanNodeType::Filter => {
                let mut executor = StreamingFilterExecutor::new(
                    plan_node.id(),
                    storage.clone(),
                    plan_node.get_condition()?,
                )?;
                
                let child = self.build_streaming_executor_chain(
                    &plan_node.children()[0],
                    storage,
                    context,
                )?;
                executor.set_input(child);
                
                Ok(Box::new(executor))
            }
            
            PlanNodeType::Limit => {
                let mut executor = StreamingLimitExecutor::new(
                    plan_node.id(),
                    plan_node.get_limit()?,
                )?;
                
                let child = self.build_streaming_executor_chain(
                    &plan_node.children()[0],
                    storage,
                    context,
                )?;
                executor.set_input(child);
                
                Ok(Box::new(executor))
            }
            
            // 其他操作...
            _ => Err(DBError::execution(
                format!("Unsupported executor type in streaming mode: {}", 
                    plan_node.node_type())
            ))
        }
    }
}
```

---

## 2. 第二阶段：关键 Executor 迁移（2 周）

### 2.1 StreamingScanExecutor

**文件**：`crates/graphdb-query/src/query/executor/streaming/impl/scan.rs`

```rust
use std::sync::Arc;
use parking_lot::RwLock;
use crate::storage::{StorageClient, StorageReader};
use crate::query::executor::base::streaming_executor::{
    StreamingExecutor, DataChunk,
};
use crate::query::executor::base::streaming_base_executor::StreamingBaseExecutor;

pub struct StreamingScanVerticesExecutor<S: StorageClient> {
    base: StreamingBaseExecutor<S>,
    partition_range: std::ops::Range<u32>,
    current_idx: u32,
    chunk_size: usize,
    exhausted: bool,
}

impl<S: StorageClient + Send + 'static> StreamingScanVerticesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
    ) -> DBResult<Self> {
        let base = StreamingBaseExecutor::new(id, "ScanVertices".to_string(), storage);
        
        // 获取顶点总数
        let total_vertices = 0u32;  // 从 storage 读取
        
        Ok(Self {
            base,
            partition_range: 0..total_vertices,
            current_idx: 0,
            chunk_size: 1024,  // 每个 chunk 返回 1024 行
            exhausted: false,
        })
    }
    
    /// 从指定行索引读取一行
    fn read_vertex(&self, idx: u32) -> DBResult<Option<Vec<Value>>> {
        let storage = self.base.get_storage().unwrap();
        let reader = storage.read();
        
        // 从存储读取顶点
        if let Some(vertex) = reader.get_vertex(idx)? {
            let row = vec![
                Value::Int(vertex.id()),
                Value::String(vertex.label()),
                // ... 其他列
            ];
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }
}

impl<S: StorageClient + Send + 'static> StreamingExecutor for StreamingScanVerticesExecutor<S> {
    fn open(&mut self) -> DBResult<()> {
        self.base.mark_opened();
        Ok(())
    }
    
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        if self.exhausted {
            return Ok(None);
        }
        
        let mut rows = Vec::with_capacity(self.chunk_size);
        let mut read_count = 0;
        
        // 读取最多 chunk_size 行
        while self.current_idx < self.partition_range.end 
              && read_count < self.chunk_size {
            if let Some(row) = self.read_vertex(self.current_idx)? {
                rows.push(row);
                read_count += 1;
            }
            self.current_idx += 1;
        }
        
        // 检查是否已到末尾
        if self.current_idx >= self.partition_range.end {
            self.exhausted = true;
        }
        
        if rows.is_empty() {
            return Ok(None);
        }
        
        self.base.stats_mut().add_rows(rows.len());
        
        Ok(Some(DataChunk {
            columns: vec!["id".to_string(), "label".to_string()],
            size: rows.len(),
            rows,
        }))
    }
    
    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }
    
    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }
    
    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}
```

### 2.2 StreamingFilterExecutor

**文件**：`crates/graphdb-query/src/query/executor/streaming/impl/filter.rs`

```rust
use crate::core::Expression;
use crate::query::executor::base::streaming_executor::{
    StreamingExecutor, StreamingInput, DataChunk,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;

pub struct StreamingFilterExecutor<S> {
    base: StreamingBaseExecutor<S>,
    condition: Expression,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl<S: StorageClient + Send + 'static> StreamingFilterExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        condition: Expression,
    ) -> DBResult<Self> {
        let base = StreamingBaseExecutor::new(id, "Filter".to_string(), storage);
        
        Ok(Self {
            base,
            condition,
            input: None,
        })
    }
}

impl<S: StorageClient + Send + 'static> StreamingInput for StreamingFilterExecutor<S> {
    fn set_input(&mut self, input: Box<dyn StreamingExecutor>) {
        self.input = Some(input);
    }
    
    fn get_input(&self) -> Option<&dyn StreamingExecutor> {
        self.input.as_ref().map(|b| b.as_ref())
    }
}

impl<S: StorageClient + Send + 'static> StreamingExecutor for StreamingFilterExecutor<S> {
    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input {
            input.open()?;
        }
        self.base.mark_opened();
        Ok(())
    }
    
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        let mut input = self.input.take().unwrap();
        
        loop {
            match input.next()? {
                Some(mut chunk) => {
                    // 过滤 chunk 中的行
                    let mut filtered_rows = Vec::new();
                    
                    for row in chunk.rows {
                        // 为行创建表达式上下文
                        let mut context = DefaultExpressionContext::new();
                        
                        // 设置列变量
                        for (i, col_name) in chunk.columns.iter().enumerate() {
                            if i < row.len() {
                                context.set_variable(col_name.clone(), row[i].clone());
                            }
                        }
                        
                        // 评估过滤条件
                        match ExpressionEvaluator::evaluate(&self.condition, &mut context) {
                            Ok(Value::Bool(true)) => {
                                filtered_rows.push(row);
                            }
                            _ => {}
                        }
                    }
                    
                    chunk.rows = filtered_rows;
                    chunk.size = chunk.rows.len();
                    
                    self.input = Some(input);
                    
                    // 如果有过滤结果，返回；否则继续拉取下一个 chunk
                    if chunk.size > 0 {
                        self.base.stats_mut().add_rows(chunk.size);
                        return Ok(Some(chunk));
                    }
                }
                None => {
                    self.input = Some(input);
                    return Ok(None);
                }
            }
        }
    }
    
    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input {
            input.close()?;
        }
        Ok(())
    }
    
    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }
    
    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}
```

### 2.3 StreamingLimitExecutor

**文件**：`crates/graphdb-query/src/query/executor/streaming/impl/limit.rs`

```rust
pub struct StreamingLimitExecutor<S> {
    base: StreamingBaseExecutor<S>,
    limit: usize,
    consumed: usize,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl<S: StorageClient + Send + 'static> StreamingLimitExecutor<S> {
    pub fn new(id: i64, limit: usize) -> DBResult<Self> {
        let base = StreamingBaseExecutor::without_storage(id, "Limit".to_string());
        
        Ok(Self {
            base,
            limit,
            consumed: 0,
            input: None,
        })
    }
}

impl<S: StorageClient + Send + 'static> StreamingInput for StreamingLimitExecutor<S> {
    fn set_input(&mut self, input: Box<dyn StreamingExecutor>) {
        self.input = Some(input);
    }
    
    fn get_input(&self) -> Option<&dyn StreamingExecutor> {
        self.input.as_ref().map(|b| b.as_ref())
    }
}

impl<S: StorageClient + Send + 'static> StreamingExecutor for StreamingLimitExecutor<S> {
    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input {
            input.open()?;
        }
        self.base.mark_opened();
        Ok(())
    }
    
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 已达到 limit，不再返回数据
        if self.consumed >= self.limit {
            self.stop()?;
            return Ok(None);
        }
        
        let mut input = self.input.take().unwrap();
        
        match input.next()? {
            Some(mut chunk) => {
                let remaining = self.limit - self.consumed;
                
                // 如果 chunk 超过剩余配额，截断
                if chunk.size > remaining {
                    chunk.rows.truncate(remaining);
                    chunk.size = remaining;
                }
                
                self.consumed += chunk.size;
                self.input = Some(input);
                
                Ok(Some(chunk))
            }
            None => {
                self.input = Some(input);
                Ok(None)
            }
        }
    }
    
    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input {
            input.close()?;
        }
        Ok(())
    }
    
    fn stop(&mut self) -> DBResult<()> {
        // 停止上游执行
        if let Some(ref mut input) = self.input {
            input.stop()?;
        }
        Ok(())
    }
    
    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }
    
    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}
```

---

## 3. 开发者检查清单

### 3.1 代码质量检查

- [ ] 所有 StreamingExecutor 实现都实现了全部必需的方法
- [ ] 错误处理完整（使用 `?` 传播或显式处理）
- [ ] 统计信息正确更新（`stats_mut().add_rows()`）
- [ ] 资源正确打开/关闭（open/close 配对）
- [ ] 文件编码：UTF-8，Rust formatting (`cargo fmt`)

### 3.2 测试检查

- [ ] 单元测试：每个 executor 的 `next()` 方法
  ```rust
  #[test]
  fn test_filter_executor() {
      let executor = StreamingFilterExecutor::new(...);
      executor.open().unwrap();
      while let Some(chunk) = executor.next().unwrap() {
          // 验证过滤结果
      }
      executor.close().unwrap();
  }
  ```

- [ ] 集成测试：多个 executor 组合
  ```rust
  #[test]
  fn test_filter_limit_chain() {
      // Limit → Filter → Scan
  }
  ```

- [ ] 性能测试：基准测试
  ```rust
  #[bench]
  fn bench_scan(b: &mut Bencher) {
      // 测试扫描速度
  }
  ```

### 3.3 代码审查检查

- [ ] 类型安全：没有 unwrap() 或 panic!()
- [ ] 线程安全：Send + Sync 约束是否正确
- [ ] 生命周期：Arc/RwLock 使用是否正确
- [ ] API 文档：每个 public 方法都有 doc comment
- [ ] 与现有系统的集成：不破坏旧代码

---

## 4. 常见问题解答

### Q1: 为什么 DataChunk 使用行向量而不是列向量？

A: 为了兼容现有的 DataSet 和最小化改动。初期用行向量，后期可优化为列向量。

### Q2: 如何处理有状态的 executor（如 Aggregate）？

A: Aggregate 需要消费所有输入才能产生结果。实现方式：
```rust
fn next(&mut self) -> DBResult<Option<DataChunk>> {
    // 首次调用：消费所有输入
    if !self.consumed_all {
        while let Some(chunk) = input.next()? {
            self.state.update(&chunk);
        }
        self.consumed_all = true;
        return Ok(Some(self.state.finalize()?));
    }
    // 后续调用：返回 None
    Ok(None)
}
```

### Q3: 如何处理双输入 executor（Join）？

A: 使用 StreamingBinaryInput trait，实现 Build-Probe 两个阶段：
```rust
fn next(&mut self) -> DBResult<Option<DataChunk>> {
    if !self.build_complete {
        self.build_phase()?;  // 消费右表
    }
    self.probe_phase()  // Probe 左表
}
```

### Q4: 如何在 ExecutorFactory 中创建 StreamingExecutor？

A: 添加新方法 `create_streaming_executor()`：
```rust
pub fn create_streaming_executor(
    plan_node: &PlanNodeEnum,
    storage: Arc<RwLock<S>>,
) -> DBResult<Box<dyn StreamingExecutor>> {
    match plan_node.node_type() {
        PlanNodeType::ScanVertices => Ok(Box::new(...)),
        // ...
    }
}
```

---

## 5. 关键实现细节

### 5.1 chunk_size 的选择

```rust
// 建议值
let chunk_size = 4096;  // 行数

// 内存大小估算
let chunk_memory_bytes = chunk_size * avg_row_size;  // 通常 4-16MB
```

### 5.2 统计信息更新

```rust
// 在 next() 返回前更新
self.base.stats_mut().add_rows(chunk.size);
self.base.stats_mut().add_elapsed(elapsed_ms);
```

### 5.3 错误处理

```rust
// 好的做法
match executor.next()? {
    Some(chunk) => { /* ... */ }
    None => { /* 无更多数据 */ }
}

// 不推荐
let chunk = executor.next().unwrap()?;  // ✗ 混合 unwrap 和 ?
```

---

## 总结

通过按照这个指南逐步实现，可以：
1. 周 1-3：完成基础设施
2. 周 4-5：完成关键 executor（Scan, Filter, Limit）
3. 周 6-9：完成其他 executor 和优化

最终达到清晰、可维护的流式执行系统。
