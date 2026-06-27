# P1.a 并行执行框架设计（修订版）

> 设计日期：2026-06-27
> 版本：v2.0（重大修订：基于 StreamingExecutor 而非全物化）
> 依赖基础：第 0 阶段（Executor 架构重构 + StreamingExecutor trait）
> 关键收益：LIMIT 查询 10-50 倍加速，多核利用率 70%+
> 预计工期：4-6 周

---

## ⚠️ 版本说明

**v1.0 的问题**：
- ❌ 假设基于现有全物化 DataSet
- ❌ 说"无需改动现有 executor"（实际需要依赖 StreamingExecutor）
- ❌ 与第 0 阶段的架构重构没有明确关系

**v2.0 的改进**：
- ✅ 明确依赖第 0 阶段（架构重构）完成
- ✅ 基于 StreamingExecutor 和 Box<dyn> 设计
- ✅ 与第 2 阶段（executor 迁移）形成清晰的依赖链

---

## 1. 概述

在第 0 阶段（Executor 架构重构）完成之后，引入数据分区与 Pipeline 调度器，支持 StreamingExecutor 的任务调度和并行执行。关键是利用 Pull-based 模型天然支持的中途停止（LIMIT 优化）。

### 依赖关系

```
第 0 阶段（1 周）：Executor 架构重构
  ├─ StreamingExecutor trait 定义
  ├─ StreamingBaseExecutor 实现  
  ├─ ExecutionMode 支持（Materialized vs Streaming）
  └─ PlanExecutor 支持两种模式
       ↓ (必须完成)
第 1 阶段（4-6 周）：P1.a 并行执行框架
  ├─ 数据分区（Vertex/Edge 按 ID 范围）
  ├─ Pipeline 调度器（基于 StreamingExecutor）
  └─ 与 QueryPipelineManager 集成（ExecutionMode::Streaming 路径）
```

---

## 2. 核心设计原则

### 2.1 Pull-based Pipeline

Pipeline 调度器不直接执行 executor，而是：
1. **消费者驱动**：最上层的 executor（如 Limit）调用 `next()`
2. **链式拉取**：每个 executor 从下游拉取 chunk
3. **自然支持 LIMIT**：消费者满足需求后就停止，无需特殊处理

### 2.2 与 StreamingExecutor 的协作

```
不是这样（旧设计）：
  QueryScheduler
    └─ 直接驱动旧的 ExecutorEnum
    └─ 对 executor 调用 execute()

而是这样（新设计）：
  QueryScheduler
    └─ 管理 StreamingExecutor 树的任务调度
    └─ 对 executor 调用 next()，获得 chunk
    └─ 负责数据分区的并行化
```

### 2.3 为什么不改动现有系统

**现有全物化路径保留**：
- `ExecutionMode::Materialized` 继续使用现有的 ExecutorEnum
- 完全向后兼容

**新的流式路径单独实现**：
- `ExecutionMode::Streaming` 使用 StreamingExecutor
- Pipeline 调度器仅在流式路径上工作

---

## 3. 架构设计

### 3.1 数据分区架构

#### 分区粒度

- **VertexTable**：按顶点 ID 范围分区
  - 默认分区数：`CPU 核心数 * 2`（8 核 → 16 个分区）
  - 每个分区是一个逻辑视图（不复制数据）

- **EdgeTable**：按源顶点 ID 范围分区
  - 与 VertexTable 保持一致的分区范围
  - 对于 TimeTravel 和 Simple 都要支持

#### PartitionView 设计

```rust
/// 分区视图：逻辑视图，不复制数据
pub struct PartitionView<'a> {
    pub partition_id: usize,
    pub id_range: Range<u32>,
    pub table: &'a StorageTable,  // 指向原表
}

impl<'a> PartitionView<'a> {
    /// 获取该分区内的顶点/边
    pub fn iter_ids(&self) -> impl Iterator<Item = u32> {
        self.id_range.clone()
    }
}
```

#### 实现位置

- `crates/graphdb-storage/src/vertex/vertex_table.rs` - 添加分区方法
- `crates/graphdb-storage/src/edge_table/mod.rs` - 添加分区方法
- `crates/graphdb-storage/src/partition/mod.rs` - 新增分区相关定义

---

### 3.2 Pipeline 调度器

#### 核心职责

Pipeline 调度器的作用：

```
┌─────────────────────────────────────────────┐
│          QueryScheduler                      │
│  ─────────────────────────────────────     │
│  1. 接收 StreamingExecutor 树                 │
│  2. 按分区将查询分解为任务                    │
│  3. 将任务分配到线程池执行                    │
│  4. 实现背压控制（内存管理）                  │
│  5. 支持 LIMIT 中途停止                      │
└─────────────────────────────────────────────┘
           ↓
    执行流程
    ─────────────
    Partition-0 → Task → StreamingScan.next()
    Partition-1 → Task → StreamingScan.next()
    Partition-2 → Task → StreamingScan.next()
           ↓
    [合并结果]
           ↓
    返回 chunk 给上层 executor
```

#### 数据结构

```rust
/// Pipeline 调度器：协调 StreamingExecutor 的分区并行执行
pub struct QueryScheduler {
    /// 根 executor（StreamingExecutor）
    root_executor: Box<dyn StreamingExecutor>,
    
    /// 分区信息
    num_partitions: usize,
    partition_ranges: Vec<Range<u32>>,
    
    /// 线程池
    executor_pool: ThreadPool,
    
    /// 任务队列和状态
    pending_tasks: VecDeque<Task>,
    running_tasks: HashSet<TaskId>,
    completed_tasks: HashMap<TaskId, DataChunk>,
    
    /// 内存管理（背压）
    active_chunks: VecDeque<DataChunk>,
    max_buffered_chunks: usize,
    
    /// 统计信息
    stats: ExecutionStats,
}

pub struct Task {
    pub id: TaskId,
    pub partition_id: usize,
    pub status: TaskStatus,
}

pub enum TaskStatus {
    Pending,
    Running(tokio::task::JoinHandle<DBResult<Option<DataChunk>>>),
    Completed(Option<DataChunk>),
}
```

#### Pull 接口

```rust
impl QueryScheduler {
    /// 创建调度器
    pub fn new(
        root_executor: Box<dyn StreamingExecutor>,
        num_partitions: usize,
    ) -> Self {
        let num_threads = std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(8).unwrap())
            .get();
        
        Self {
            root_executor,
            num_partitions,
            partition_ranges: Self::compute_ranges(num_partitions),
            executor_pool: ThreadPool::new(num_threads),
            pending_tasks: VecDeque::new(),
            running_tasks: HashSet::new(),
            completed_tasks: HashMap::new(),
            active_chunks: VecDeque::new(),
            max_buffered_chunks: 10,
            stats: ExecutionStats::new(),
        }
    }
    
    /// 拉取下一个 chunk（主入口）
    pub fn pull_chunk(&mut self) -> DBResult<Option<DataChunk>> {
        // 第一次调用：初始化并打开 root executor
        if !self.initialized {
            self.root_executor.open()?;
            self.initialized = true;
        }
        
        // 从 root executor 拉取 chunk
        // root executor 会在内部使用分区逻辑
        self.root_executor.next()
    }
    
    /// 停止执行（LIMIT 时调用）
    pub fn stop(&mut self) -> DBResult<()> {
        self.root_executor.stop()?;
        self.root_executor.close()?;
        Ok(())
    }
}
```

#### 与 StreamingExecutor 的交互

**关键洞察**：Pipeline 调度器不需要单独管理 Scan executor 的并行化，因为：

1. **分区信息传递给 Scan**
   ```rust
   // StreamingScanExecutor 在构造时接收分区信息
   let mut scan = StreamingScanVerticesExecutor::new(
       id,
       storage,
       partition_id,     // 告诉 Scan 只扫描这个分区
       partition_range,
   );
   ```

2. **Scan 内部处理并行扫描**
   ```rust
   impl StreamingExecutor for StreamingScanVerticesExecutor {
       fn next(&mut self) -> DBResult<Option<DataChunk>> {
           // 只扫描自己的分区范围
           // 多个分区的 Scan executor 会被并行调用
       }
   }
   ```

3. **Pipeline 调度器只是协调**
   ```rust
   // 并行化发生在：
   // - StreamingScan 的不同分区实例被并行调用
   // - 而不是调度器显式管理线程
   ```

---

### 3.3 与 QueryPipelineManager 的集成

#### ExecutionMode::Streaming 路径

```rust
impl QueryPipelineManager {
    pub fn execute_query_streaming(&self, query: &str) -> DBResult<ExecutionResult> {
        let plan = self.parse_and_plan(query)?;
        
        // 使用 PlanExecutor 的 Streaming 模式
        let mut plan_executor = PlanExecutor::new(factory)
            .with_mode(ExecutionMode::Streaming);
        
        plan_executor.execute_plan(&plan, storage, context)
    }
}

// PlanExecutor 的 execute_streaming 方法
impl<S> PlanExecutor<S> {
    fn execute_streaming(&mut self, plan: &ExecutionPlan) -> DBResult<ExecutionResult> {
        // 1. 构建 StreamingExecutor 树
        let root_executor = self.build_streaming_tree(plan)?;
        
        // 2. 创建 Pipeline 调度器（可选，用于并行化）
        // 注意：对于简单的单线程执行，可以不用调度器
        // 对于需要并行化的查询，创建调度器
        
        if self.should_use_scheduler(&plan) {
            let mut scheduler = QueryScheduler::new(root_executor, num_partitions);
            self.execute_with_scheduler(&mut scheduler)
        } else {
            // 简单的 Pull 执行（不使用调度器）
            self.execute_simple_streaming(root_executor)
        }
    }
    
    /// 简单的流式执行（无调度器）
    fn execute_simple_streaming(
        &mut self,
        mut root: Box<dyn StreamingExecutor>,
    ) -> DBResult<ExecutionResult> {
        root.open()?;
        let mut rows = Vec::new();
        while let Some(chunk) = root.next()? {
            rows.extend(chunk.rows);
        }
        root.close()?;
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: ...,
            rows,
        }))
    }
    
    /// 带调度器的并行流式执行
    fn execute_with_scheduler(
        &mut self,
        scheduler: &mut QueryScheduler,
    ) -> DBResult<ExecutionResult> {
        let mut rows = Vec::new();
        while let Some(chunk) = scheduler.pull_chunk()? {
            rows.extend(chunk.rows);
        }
        scheduler.stop()?;
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: ...,
            rows,
        }))
    }
    
    /// 决定是否使用调度器
    fn should_use_scheduler(&self, plan: &ExecutionPlan) -> bool {
        // 只在以下情况使用：
        // 1. 有 Scan 节点（数据源可分区）
        // 2. 没有 Sort 节点（Sort 需要全物化）
        // 3. 有 LIMIT 节点（受益于并行化）
        plan.has_scan() && !plan.has_sort() && plan.has_limit()
    }
}
```

---

## 4. 重要的改进说明

### 4.1 与第 2 阶段（executor 迁移）的关系

```
第 0 阶段：Executor 架构重构
  └─ 定义 StreamingExecutor trait
  └─ 完成 ExecutionMode 支持

      ↓ (依赖)

第 1 阶段：Pipeline 调度器 (P1.a)
  └─ 可以在基础的 StreamingExecutor 上工作
  └─ 即使只有 Scan 被流式化，也能工作

      ↓ (与第 1 并行)

第 2 阶段：关键 executor 迁移
  └─ StreamingScanVerticesExecutor
  └─ StreamingFilterExecutor
  └─ StreamingLimitExecutor
  └─ 随着迁移完成，Pipeline 调度器的效果更好
```

### 4.2 向后兼容性

**旧系统（Materialized）完全保留**：
```rust
// 现有的查询执行路径保持不变
QueryPipelineManager.execute_query(query)
  → PlanExecutor with ExecutionMode::Materialized
  → 使用现有的 ExecutorEnum
  → 全物化执行
```

**新系统（Streaming）单独实现**：
```rust
// 新的查询执行路径
QueryPipelineManager.execute_query_streaming(query)
  → PlanExecutor with ExecutionMode::Streaming
  → 使用 StreamingExecutor
  → 流式或并行执行（使用 Pipeline 调度器）
```

---

## 5. 性能目标与验证

### 5.1 预期收益

| 场景 | 现状 | 改进后 | 加速比 |
|------|------|--------|--------|
| LIMIT 10（百万行，无并行） | 500ms | 50ms | **10x** |
| LIMIT 10（百万行，8 核并行） | 500ms | 5ms | **100x** |
| 全表扫描（8 核并行） | 500ms | 70ms | **7x** |

### 5.2 验证计划

```bash
# 基准测试
cargo bench --bench limit_query          # LIMIT 性能
cargo bench --bench parallel_scan        # 并行扫描
cargo bench --bench memory_usage         # 内存占用

# 集成测试
cargo test --test scheduler_integration  # 调度器集成
cargo test --test streaming_correctness  # 结果正确性
```

---

## 6. 实现顺序

### 6.1 前置条件

**必须先完成第 0 阶段**：
- [ ] StreamingExecutor trait 定义
- [ ] StreamingBaseExecutor 实现
- [ ] PlanExecutor 支持 ExecutionMode
- [ ] 基准测试框架

### 6.2 第 1 阶段的实现步骤

1. **周 1**：数据分区架构
   - VertexTable/EdgeTable 添加分区方法
   - PartitionView 实现

2. **周 2-3**：Pipeline 调度器框架
   - QueryScheduler 基本结构
   - 线程池和背压机制

3. **周 4**：与 PlanExecutor 集成
   - ExecutionMode::Streaming 路径完成
   - 简单的流式执行（无调度器）

4. **周 5-6**：优化和验证
   - 调度器性能优化
   - 基准测试和集成测试

---

## 7. 文件变更清单

### 新增文件
- `crates/graphdb-storage/src/partition/mod.rs`
- `crates/graphdb-query/src/executor/scheduler/mod.rs`
- `crates/graphdb-query/src/executor/scheduler/pipeline_scheduler.rs`

### 修改文件
- `crates/graphdb-storage/src/vertex/vertex_table.rs` - 添加分区方法
- `crates/graphdb-storage/src/edge_table/mod.rs` - 添加分区方法
- `crates/graphdb-query/src/executor/factory/engine.rs` - ExecutionMode::Streaming 实现
- `crates/graphdb-query/src/query_pipeline_manager.rs` - 添加流式执行入口

---

## 8. 关键决策

| 问题 | 决策 | 理由 |
|------|------|------|
| **依赖第 0 阶段？** | 是 | 需要 StreamingExecutor trait 和 ExecutionMode |
| **改动旧系统？** | 否 | Materialized 模式保留，完全兼容 |
| **何时启用调度器？** | 可选 | 简单查询无需调度器，复杂查询才启用 |
| **分区方式？** | 按 ID 范围 | 简单、均匀、与现有数据结构兼容 |
| **线程数？** | CPU 核心数 | 避免过度订阅 |

---

## 9. 与旧版本的区别总结

| 方面 | v1.0（旧） | v2.0（新） |
|------|-----------|-----------|
| 基础 | 全物化 DataSet | StreamingExecutor + ExecutionMode |
| 依赖 | P0（独立） | 第 0 阶段（架构重构） |
| 对现有系统的改动 | 有改动 | 无改动（完全隔离） |
| 何时可用 | 第 0 周 | 第 1 周（等待第 0 阶段完成） |
| Pull 循环 | QueryScheduler 内部 | PlanExecutor 的 execute_streaming 方法 |

---

## 总结

**P1.a v2.0 的核心改进**：
1. ✅ 明确依赖第 0 阶段（架构重构）
2. ✅ 基于 StreamingExecutor 设计，而非全物化
3. ✅ 无需改动现有系统（Materialized 模式保留）
4. ✅ 与第 2 阶段（executor 迁移）形成清晰的依赖链
5. ✅ 可选的 Pipeline 调度器（简单查询无需）

这是一个更加清晰、架构更合理的设计。
