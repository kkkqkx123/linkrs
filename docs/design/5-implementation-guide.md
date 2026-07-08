# 开发者实现指南

> 更新日期：2026-06-27
> 目标读者：实际编写代码的开发者
> 包含：代码框架、测试策略、常见问题

---

## 1. 阶段 0：基础架构实现

### 1.1 StreamingExecutor Enum 定义

**文件**：`crates/graphdb-query/src/executor/streaming/executor.rs`

```rust
/// 流式执行器
/// 
/// 使用 Enum（而非 Trait）以确保类型安全和编译期检查
pub enum StreamingExecutor {
    // ============ 数据源 ============
    ScanVertices {
        table: Arc<VertexTable>,
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },
    ScanEdges {
        table: Arc<EdgeTable>,
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },
    
    // ============ 单输入 ============
    Filter {
        input: Box<StreamingExecutor>,
        condition: Expression,
        opened: bool,
    },
    Project {
        input: Box<StreamingExecutor>,
        columns: Vec<ColumnRef>,
        opened: bool,
    },
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
    },
    
    // ============ 有状态 ============
    Aggregate {
        input: Box<StreamingExecutor>,
        group_by: Vec<ColumnRef>,
        agg_funcs: Vec<AggregateFunc>,
        groups: HashMap<Vec<Value>, Vec<Value>>,
        group_iter: Option<std::collections::hash_map::IntoIter<Vec<Value>, Vec<Value>>>,
        opened: bool,
    },
}

impl StreamingExecutor {
    pub fn open(&mut self) -> DBResult<()> {
        // 递归打开所有输入 executor
        // 初始化内部状态（如聚合表）
        unimplemented!()
    }
    
    pub fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 返回下一个 chunk，或 None 表示结束
        // 对于 Scan：返回来自表的数据
        // 对于 Filter：拉取上游并过滤
        // 对于有状态：管理聚合状态和迭代
        unimplemented!()
    }
    
    pub fn stop(&mut self) -> DBResult<()> {
        // 立即停止执行（用于 LIMIT）
        // 通知所有下游停止
        unimplemented!()
    }
    
    pub fn close(&mut self) -> DBResult<()> {
        // 清理资源
        // 递归关闭所有输入 executor
        unimplemented!()
    }
}
```

### 1.2 DataChunk 定义

**文件**：`crates/graphdb-query/src/executor/streaming/chunk.rs`

```rust
use crate::executor::common::{DataRow, Schema};

/// 数据块：流式执行的基本单位
/// 
/// 典型大小：1024 行，~4MB
#[derive(Debug, Clone)]
pub struct DataChunk {
    /// 行数据
    pub rows: Vec<DataRow>,
    /// Schema（列名、类型等）
    pub schema: Arc<Schema>,
}

impl DataChunk {
    /// 创建新的数据块
    pub fn new(rows: Vec<DataRow>, schema: Arc<Schema>) -> Self {
        Self { rows, schema }
    }
    
    /// 从行推断 schema
    pub fn from_rows(rows: Vec<DataRow>) -> Self {
        let schema = if rows.is_empty() {
            Arc::new(Schema::empty())
        } else {
            Arc::new(Schema::from_row(&rows[0]))
        };
        Self { rows, schema }
    }
    
    /// chunk 的行数
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    
    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    
    /// 获取列数
    pub fn num_columns(&self) -> usize {
        self.schema.columns().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_data_chunk_creation() {
        let rows = vec![/* ... */];
        let chunk = DataChunk::from_rows(rows);
        assert!(!chunk.is_empty());
    }
    
    #[test]
    fn test_data_chunk_empty() {
        let chunk = DataChunk::from_rows(vec![]);
        assert!(chunk.is_empty());
    }
}
```

### 1.3 ExecutionMode 定义

**文件**：`crates/graphdb-query/src/executor/factory/engine.rs`

```rust
/// 执行模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// 全物化模式（现有系统）
    Materialized,
    /// 流式模式（新系统）
    Streaming,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Materialized
    }
}

impl ExecutionMode {
    pub fn is_materialized(&self) -> bool {
        *self == ExecutionMode::Materialized
    }
    
    pub fn is_streaming(&self) -> bool {
        *self == ExecutionMode::Streaming
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_execution_mode_default() {
        let mode = ExecutionMode::default();
        assert_eq!(mode, ExecutionMode::Materialized);
    }
}
```

### 1.4 PlanExecutor 改造

**文件**：`crates/graphdb-query/src/executor/factory/engine.rs`

```rust
pub struct PlanExecutor<S: StorageClient> {
    factory: Arc<ExecutorFactory<S>>,
    execution_mode: ExecutionMode,
}

impl<S: StorageClient> PlanExecutor<S> {
    pub fn new(factory: Arc<ExecutorFactory<S>>) -> Self {
        Self {
            factory,
            execution_mode: ExecutionMode::default(),
        }
    }
    
    pub fn with_mode(mut self, mode: ExecutionMode) -> Self {
        self.execution_mode = mode;
        self
    }
    
    pub fn execute_plan(
        &mut self,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        match self.execution_mode {
            ExecutionMode::Materialized => {
                self.execute_materialized(plan)
            }
            ExecutionMode::Streaming => {
                self.execute_streaming(plan)
            }
        }
    }
    
    /// 现有的全物化执行（保持原逻辑）
    fn execute_materialized(
        &mut self,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        // ... 现有逻辑，无改动
        unimplemented!()
    }
    
    /// 新的流式执行
    fn execute_streaming(
        &mut self,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        // 1. 构建 StreamingExecutor 树
        let mut root = self.build_streaming_tree(plan)?;
        
        // 2. Pull 循环
        root.open()?;
        let mut rows = Vec::new();
        while let Some(chunk) = root.next()? {
            rows.extend(chunk.rows);
        }
        root.close()?;
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: plan.col_names().clone(),
            rows,
        }))
    }
    
    fn build_streaming_tree(
        &self,
        plan: &ExecutionPlan,
    ) -> DBResult<StreamingExecutor> {
        // 根据执行计划递归构建 executor 树
        unimplemented!()
    }
}
```

### 1.5 测试框架

**文件**：`crates/graphdb-query/src/executor/streaming/tests.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_streaming_executor_scan_vertices() {
        // 1. 准备数据
        let table = create_test_vertex_table(1000);
        let mut executor = StreamingExecutor::ScanVertices {
            table: Arc::new(table),
            partition_id: 0,
            partition_range: 0..1000,
            current_offset: 0,
        };
        
        // 2. 执行
        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        
        // 3. 验证
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 1024.min(1000));
        
        executor.close().unwrap();
    }
    
    #[test]
    fn test_streaming_executor_limit() {
        // 1. 构建执行树
        let table = create_test_vertex_table(1000);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            table: Arc::new(table),
            partition_id: 0,
            partition_range: 0..1000,
            current_offset: 0,
        });
        
        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };
        
        // 2. 执行
        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();
        
        // 3. 验证
        assert_eq!(total, 10);
    }
}
```

---

## 2. 阶段 1：并行框架实现

### 2.1 分区接口（VertexTable）

**文件**：`crates/graphdb-storage/src/vertex/vertex_table.rs`

```rust
impl VertexTable {
    /// 获取推荐的分区数
    pub fn recommended_num_partitions(&self) -> usize {
        let cores = std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(8).unwrap())
            .get();
        cores * 2
    }
    
    /// 计算分区范围
    pub fn compute_partitions(&self, num_partitions: usize) -> Vec<Range<u32>> {
        let total_ids = self.max_id();
        let partition_size = (total_ids + num_partitions as u32 - 1) / num_partitions as u32;
        
        (0..num_partitions)
            .map(|p| {
                let start = p as u32 * partition_size;
                let end = ((p as u32 + 1) * partition_size).min(total_ids);
                start..end
            })
            .collect()
    }
    
    /// 获取指定范围的顶点
    pub fn get_vertex_range(&self, range: Range<u32>) -> DBResult<Vec<Vertex>> {
        // 从存储中获取指定 ID 范围内的顶点
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_compute_partitions() {
        let table = create_test_vertex_table(1000);
        let partitions = table.compute_partitions(4);
        
        assert_eq!(partitions.len(), 4);
        // 验证覆盖所有 ID
        let total_count: u32 = partitions.iter()
            .map(|p| p.end - p.start)
            .sum();
        assert_eq!(total_count, 1000);
    }
}
```

### 2.2 Pipeline 调度器框架

**文件**：`crates/graphdb-query/src/executor/scheduler/pipeline_scheduler.rs`

```rust
use std::collections::VecDeque;

pub struct QueryScheduler {
    root_executor: Box<StreamingExecutor>,
    num_partitions: usize,
    executor_pool: ThreadPool,
    completed_chunks: VecDeque<DataChunk>,
    max_buffered_chunks: usize,
    initialized: bool,
}

impl QueryScheduler {
    pub fn new(
        root_executor: Box<StreamingExecutor>,
        num_partitions: usize,
    ) -> Self {
        let num_threads = std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(8).unwrap())
            .get();
        
        Self {
            root_executor,
            num_partitions,
            executor_pool: ThreadPool::new(num_threads),
            completed_chunks: VecDeque::new(),
            max_buffered_chunks: 10,
            initialized: false,
        }
    }
    
    pub fn pull_chunk(&mut self) -> DBResult<Option<DataChunk>> {
        if !self.initialized {
            self.root_executor.open()?;
            self.initialized = true;
        }
        
        // 优先返回已完成的 chunk
        if let Some(chunk) = self.completed_chunks.pop_front() {
            return Ok(Some(chunk));
        }
        
        // 从根 executor 拉取
        self.root_executor.next()
    }
    
    pub fn stop(&mut self) -> DBResult<()> {
        self.root_executor.stop()?;
        self.root_executor.close()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_query_scheduler_basic() {
        let executor = create_test_scan_executor();
        let mut scheduler = QueryScheduler::new(Box::new(executor), 4);
        
        let chunk = scheduler.pull_chunk().unwrap();
        assert!(chunk.is_some());
        
        scheduler.stop().unwrap();
    }
}
```

---

## 3. 阶段 2：关键 Executor 流式化

### 3.1 StreamingScanVertices 实现

**文件**：`crates/graphdb-query/src/executor/streaming/impl/scan.rs`

```rust
impl StreamingExecutor {
    fn scan_vertices_next(
        table: &Arc<VertexTable>,
        partition_range: &Range<u32>,
        current_offset: &mut u32,
    ) -> DBResult<Option<DataChunk>> {
        const CHUNK_SIZE: u32 = 1024;
        
        if *current_offset >= partition_range.end {
            return Ok(None);
        }
        
        let end = (*current_offset + CHUNK_SIZE).min(partition_range.end);
        let rows = table.get_rows(*current_offset..end)?;
        *current_offset = end;
        
        Ok(Some(DataChunk::from_rows(rows)))
    }
}

// 在 StreamingExecutor::next() 中
match self {
    Self::ScanVertices { 
        table, 
        partition_range, 
        current_offset, 
        .. 
    } => {
        Self::scan_vertices_next(table, partition_range, current_offset)
    }
    // ...
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_scan_vertices_chunks() {
        let table = create_test_vertex_table(2500);
        let mut executor = StreamingExecutor::ScanVertices {
            table: Arc::new(table),
            partition_id: 0,
            partition_range: 0..2500,
            current_offset: 0,
        };
        
        executor.open().unwrap();
        
        // 应该返回 3 个 chunk（1024 + 1024 + 452）
        let chunk1 = executor.next().unwrap().unwrap();
        assert_eq!(chunk1.len(), 1024);
        
        let chunk2 = executor.next().unwrap().unwrap();
        assert_eq!(chunk2.len(), 1024);
        
        let chunk3 = executor.next().unwrap().unwrap();
        assert_eq!(chunk3.len(), 452);
        
        let chunk4 = executor.next().unwrap();
        assert!(chunk4.is_none());
        
        executor.close().unwrap();
    }
}
```

### 3.2 StreamingFilter 实现

**文件**：`crates/graphdb-query/src/executor/streaming/impl/filter.rs`

```rust
impl StreamingExecutor {
    fn filter_next(
        input: &mut Box<StreamingExecutor>,
        condition: &Expression,
    ) -> DBResult<Option<DataChunk>> {
        loop {
            if let Some(chunk) = input.next()? {
                let filtered_rows: Vec<_> = chunk.rows
                    .into_iter()
                    .filter(|row| {
                        condition.evaluate(row).unwrap_or(false)
                    })
                    .collect();
                
                if !filtered_rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows(filtered_rows)));
                }
                // 如果此 chunk 全部被过滤，继续拉取下一个
            } else {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_filter_executor() {
        let scan = Box::new(create_test_scan_executor());
        let mut filter = StreamingExecutor::Filter {
            input: scan,
            condition: Expression::parse("age > 30").unwrap(),
            opened: false,
        };
        
        filter.open().unwrap();
        let chunk = filter.next().unwrap().unwrap();
        
        // 验证所有行都满足条件
        for row in &chunk.rows {
            let age = row.get_value("age").unwrap();
            assert!(age > 30);
        }
        
        filter.close().unwrap();
    }
}
```

### 3.3 StreamingLimit 实现

**文件**：`crates/graphdb-query/src/executor/streaming/impl/limit.rs`

```rust
impl StreamingExecutor {
    fn limit_next(
        input: &mut Box<StreamingExecutor>,
        limit: u32,
        consumed: &mut u32,
    ) -> DBResult<Option<DataChunk>> {
        if *consumed >= limit {
            return Ok(None);
        }
        
        if let Some(mut chunk) = input.next()? {
            let remaining = limit - *consumed;
            
            if chunk.rows.len() > remaining as usize {
                chunk.rows.truncate(remaining as usize);
            }
            
            *consumed += chunk.rows.len() as u32;
            Ok(Some(chunk))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_limit_stops_early() {
        let scan = Box::new(create_test_scan_executor_with(10000));
        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 100,
            consumed: 0,
            opened: false,
        };
        
        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();
        
        assert_eq!(total, 100);
    }
}
```

---

## 4. 测试策略

### 4.1 单元测试

```rust
// 针对每个 executor 类型的单独测试
#[cfg(test)]
mod executor_tests {
    #[test]
    fn test_executor_open_close() { /* ... */ }
    
    #[test]
    fn test_executor_next_returns_chunks() { /* ... */ }
    
    #[test]
    fn test_executor_stop_cancels_execution() { /* ... */ }
}
```

### 4.2 集成测试

**文件**：`tests/streaming_executor_integration.rs`

```rust
#[test]
fn test_limit_query_streaming() {
    let db = create_test_database_with(1_000_000);
    
    let query = "SELECT * FROM V LIMIT 10";
    let result = db.execute_streaming(query).unwrap();
    
    assert_eq!(result.rows.len(), 10);
}

#[test]
fn test_filter_limit_query() {
    let db = create_test_database();
    
    let query = "SELECT * FROM V WHERE age > 30 LIMIT 100";
    let result = db.execute_streaming(query).unwrap();
    
    assert!(result.rows.len() <= 100);
    for row in &result.rows {
        assert!(row.get_value("age").unwrap() > 30);
    }
}
```

### 4.3 性能基准测试

**文件**：`benches/streaming_executor_bench.rs`

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_limit_query(c: &mut Criterion) {
    let db = create_test_database_with(1_000_000);
    
    c.bench_function("limit_10", |b| {
        b.iter(|| {
            db.execute_streaming(black_box("SELECT * FROM V LIMIT 10"))
        });
    });
    
    c.bench_function("limit_1000", |b| {
        b.iter(|| {
            db.execute_streaming(black_box("SELECT * FROM V LIMIT 1000"))
        });
    });
}

criterion_group!(benches, bench_limit_query);
criterion_main!(benches);
```

---

## 5. 常见问题与解决方案

### Q1：StreamingExecutor 与现有 Executor 如何共存？

**A**：通过 `ExecutionMode` 完全隔离：
- `ExecutionMode::Materialized`：使用现有的 ExecutorEnum
- `ExecutionMode::Streaming`：使用新的 StreamingExecutor enum

两者独立运行，互不影响。

### Q2：如何处理执行树中有些 executor 还未流式化？

**A**：暂时仅支持两种极端：
- 全部使用 Materialized（现有系统）
- 全部使用 Streaming（新系统）

不支持混合。这简化了实现，后续可扩展。

### Q3：Error handling 如何做？

**A**：统一使用 `DBResult<T>`：
```rust
pub type DBResult<T> = Result<T, DBError>;

impl StreamingExecutor {
    pub fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 任何错误都返回 Err
        // None 表示数据结束，不是错误
    }
}
```

### Q4：内存占用如何保证恒定？

**A**：
1. Chunk 大小固定（1024 行）
2. 有状态算子（如 Aggregate）在第一次调用时消费全部输入，但之后迭代返回
3. 不缓存未处理的 chunk

### Q5：如何测试 ExecutionMode 的切换？

**A**：
```rust
#[test]
fn test_execution_mode_switch() {
    let executor = PlanExecutor::new(factory);
    
    let result_materialized = executor
        .with_mode(ExecutionMode::Materialized)
        .execute_plan(&plan)?;
    
    let result_streaming = executor
        .with_mode(ExecutionMode::Streaming)
        .execute_plan(&plan)?;
    
    assert_eq!(result_materialized, result_streaming);
}
```

---

## 6. 代码质量检查清单

在提交 PR 前，确保：

- [ ] 编译通过，无 warnings：`cargo clippy --all-targets`
- [ ] 单元测试全部通过：`cargo test --lib`
- [ ] 集成测试全部通过：`cargo test --test '*'`
- [ ] 性能基准通过：`cargo bench`
- [ ] 代码格式正确：`cargo fmt`
- [ ] 文档完整：`cargo doc --no-deps`
- [ ] 注释清晰，包括 `///` 文档注释
- [ ] 没有 `unwrap()`（除了测试和确实无法失败的情况）
- [ ] 错误信息有意义
- [ ] 新增 public API 有文档注释

---

## 总结

- ✅ **清晰的代码框架**：每个阶段的实现路径明确
- ✅ **完整的测试策略**：单元 + 集成 + 基准
- ✅ **丰富的示例代码**：可直接参考
- ✅ **常见问题解答**：减少开发时的困惑

