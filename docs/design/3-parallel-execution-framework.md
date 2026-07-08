# 并行执行框架设计

> 更新日期：2026-06-27
> 依赖：阶段 0（StreamingExecutor 基础架构）
> 收益：LIMIT 查询 10-100x 加速，全表扫描 7x 加速

---

## 1. 数据分区架构

### 1.1 分区设计原则

```
分区目的：使 Scan executor 可以被并行调用

VertexTable（百万条记录）
  ├─ 分区 0：ID 0-62.5k      → Scan 实例 0
  ├─ 分区 1：ID 62.5k-125k  → Scan 实例 1
  ├─ 分区 2：ID 125k-187.5k → Scan 实例 2
  └─ ...（16 个分区，8 核 CPU * 2）
```

### 1.2 PartitionView 实现

```rust
/// 分区视图：逻辑视图，不复制数据，指向原表的子范围
pub struct PartitionView<'a> {
    pub partition_id: usize,
    pub id_range: Range<u32>,
    pub table: &'a VertexTable,
}

impl<'a> PartitionView<'a> {
    pub fn new(
        table: &'a VertexTable,
        partition_id: usize,
        id_range: Range<u32>,
    ) -> Self {
        Self {
            partition_id,
            id_range,
            table,
        }
    }
    
    /// 获取该分区内的所有行
    pub fn iter(&self) -> DBResult<Vec<DataRow>> {
        self.table.get_rows(self.id_range.clone())
    }
}

// VertexTable 添加分区方法
impl VertexTable {
    pub fn compute_partitions(&self, num_partitions: usize) -> Vec<PartitionView> {
        let total_ids = self.max_id();
        let partition_size = (total_ids + num_partitions as u32 - 1) / num_partitions as u32;
        
        (0..num_partitions)
            .map(|p| {
                let start = p as u32 * partition_size;
                let end = ((p as u32 + 1) * partition_size).min(total_ids);
                PartitionView::new(self, p, start..end)
            })
            .collect()
    }
}
```

### 1.3 分区粒度

```
CPU 核心数 → 分区数

4 核   → 8 分区
8 核   → 16 分区（推荐）
16 核  → 32 分区

目标：避免过度订阅（thread count < partition count），
     但分区不宜过多（导致每个分区数据太少）
```

---

## 2. Pipeline 调度器

### 2.1 核心职责

```
QueryScheduler
  │
  ├─ 1. 接收根 executor（StreamingExecutor）
  ├─ 2. 了解 executor 树的分区信息（Scan 节点知道分区）
  ├─ 3. 为不同分区创建任务
  ├─ 4. 将任务提交到线程池
  ├─ 5. 实现背压控制（缓冲大小限制）
  └─ 6. 在上层 executor 请求 next() 时返回数据
```

### 2.2 QueryScheduler 设计

```rust
pub struct QueryScheduler {
    /// 根 executor（StreamingExecutor）
    root_executor: Box<StreamingExecutor>,
    
    /// 分区信息
    num_partitions: usize,
    
    /// 线程池
    executor_pool: ThreadPool,
    
    /// 任务管理
    pending_tasks: VecDeque<PartitionTask>,
    running_tasks: HashMap<TaskId, JoinHandle<DBResult<Option<DataChunk>>>>,
    completed_chunks: VecDeque<DataChunk>,
    
    /// 背压控制
    max_buffered_chunks: usize,
    
    /// 状态
    all_partitions_submitted: bool,
    current_partition: usize,
    initialized: bool,
}

pub struct PartitionTask {
    pub id: TaskId,
    pub partition_id: usize,
}

impl QueryScheduler {
    pub fn new(
        root_executor: Box<StreamingExecutor>,
        num_partitions: usize,
    ) -> Self {
        Self {
            root_executor,
            num_partitions,
            executor_pool: ThreadPool::new(
                std::thread::available_parallelism()
                    .unwrap_or(NonZeroUsize::new(8).unwrap())
                    .get()
            ),
            pending_tasks: VecDeque::new(),
            running_tasks: HashMap::new(),
            completed_chunks: VecDeque::new(),
            max_buffered_chunks: 10,
            all_partitions_submitted: false,
            current_partition: 0,
            initialized: false,
        }
    }
    
    /// 拉取下一个 chunk
    pub fn pull_chunk(&mut self) -> DBResult<Option<DataChunk>> {
        if !self.initialized {
            self.root_executor.open()?;
            self.initialized = true;
        }
        
        // 优先返回已完成的 chunk
        if let Some(chunk) = self.completed_chunks.pop_front() {
            return Ok(Some(chunk));
        }
        
        // 尝试从根 executor 拉取
        // （根 executor 会调用下游 executor 的 next()）
        self.root_executor.next()
    }
    
    /// 停止执行
    pub fn stop(&mut self) -> DBResult<()> {
        self.root_executor.stop()?;
        self.root_executor.close()?;
        
        // 取消所有待处理的任务
        self.pending_tasks.clear();
        
        Ok(())
    }
}
```

### 2.3 与 StreamingExecutor 的协作

**关键洞察**：调度器不需要显式管理分区的并行化

```
方案 A（复杂，原始想法）：
  QueryScheduler 显式为每个分区创建 Scan executor，
  管理线程池调度，收集结果

方案 B（简单，推荐）：
  1. StreamingScanExecutor 在构造时接收 partition_id
  2. next() 时，Scan 知道只扫描自己的分区范围
  3. 多个 Scan executor 实例被并行调用
  4. QueryScheduler 只是协调顶层的拉取
```

**推荐方案（B）的实现**：

```rust
// Scan executor 持有分区信息
pub enum StreamingExecutor {
    ScanVertices {
        table: Arc<VertexTable>,
        partition_id: usize,      // ← 关键：只扫描这个分区
        partition_range: Range<u32>,
        current_offset: u32,
    },
    // ...
}

// 在 QueryPipelineManager 中，根据是否需要并行化，
// 创建不同的 executor 树

// 简单查询（无并行化）：
let root = StreamingExecutor::Limit {
    input: Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,  // ← 单分区，顺序扫描
        partition_range: 0..u32::MAX,
    }),
    limit: 10,
};

// 复杂查询（需要并行化）：
// 创建多个 ScanVertices，每个持有不同的 partition_id
// 由 Pipeline 调度器管理它们的并行执行
```

---

## 3. 与 QueryPipelineManager 的集成

### 3.1 执行路径

```rust
impl QueryPipelineManager {
    pub fn execute_query_streaming(
        &self,
        query: &str,
    ) -> DBResult<ExecutionResult> {
        let plan = self.parse_and_plan(query)?;
        
        // 选择是否使用调度器
        let mode = if self.should_use_scheduler(&plan) {
            ExecutionMode::Streaming
        } else {
            ExecutionMode::Materialized
        };
        
        let mut executor = PlanExecutor::new(self.factory.clone());
        executor.execute_plan(&plan, mode)
    }
    
    fn should_use_scheduler(&self, plan: &ExecutionPlan) -> bool {
        // 以下情况使用 Pipeline 调度器：
        // 1. 有 Scan 节点（数据可分区）
        // 2. 没有 Sort（Sort 需要全物化）
        // 3. 有 LIMIT（受益于并行化）
        plan.has_scan() && !plan.has_sort() && plan.has_limit()
    }
}
```

### 3.2 PlanExecutor 的 execute_streaming 实现

```rust
impl<S: StorageClient> PlanExecutor<S> {
    fn execute_streaming(
        &mut self,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        // 1. 构建 executor 树
        let root = self.build_streaming_tree(plan)?;
        
        // 2. 决定是否需要 Pipeline 调度器
        let use_scheduler = self.should_use_scheduler(plan);
        
        if use_scheduler {
            // 并行执行路径
            self.execute_with_scheduler(root, plan)
        } else {
            // 简单的顺序执行路径
            self.execute_simple_streaming(root, plan)
        }
    }
    
    fn execute_simple_streaming(
        &mut self,
        mut root: Box<StreamingExecutor>,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
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
    
    fn execute_with_scheduler(
        &mut self,
        root: Box<StreamingExecutor>,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        let mut scheduler = QueryScheduler::new(root, self.num_partitions);
        
        let mut rows = Vec::new();
        while let Some(chunk) = scheduler.pull_chunk()? {
            rows.extend(chunk.rows);
        }
        scheduler.stop()?;
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: plan.col_names().clone(),
            rows,
        }))
    }
}
```

---

## 4. 执行示例

### 4.1 LIMIT 查询示例

```
查询：SELECT * FROM V LIMIT 10

执行计划（DAG）：
  Limit(10)
    └─ Scan(V)

ExecutionMode::Streaming 路径：

1. 构建 executor 树：
   StreamingExecutor::Limit {
       limit: 10,
       input: Box::new(StreamingExecutor::ScanVertices {
           partition_id: 0,  // 单分区顺序执行
           partition_range: 0..u32::MAX,
       })
   }

2. 执行流程：
   Limit.open()
     └─ Scan.open()

   consumed = 0
   while consumed < 10 {
       Limit.next()
         └─ Scan.next()
              └─ 返回 chunk (size=1024)
         └─ 截断到剩余配额
           └─ 返回 chunk (size=min(1024, 10-consumed))
       consumed += returned_chunk.size
   }

   Limit.stop()
     └─ Scan 停止扫描

   Limit.close()
     └─ Scan.close()

结果：
  - 扫描行数：~10（而非百万）
  - 时间：5ms（而非 500ms）
  - 加速比：100x
```

### 4.2 并行化的 LIMIT 查询示例

```
查询：SELECT * FROM V WHERE prop > 100 LIMIT 1000

执行计划（DAG）：
  Limit(1000)
    └─ Filter(prop > 100)
       └─ Scan(V)

ExecutionMode::Streaming with Pipeline Scheduler：

1. 决策：has_scan=true, !has_sort=true, has_limit=true
        → 使用 Pipeline 调度器

2. 为 16 个分区创建 16 个 Scan executor：
   StreamingExecutor::Limit {
       limit: 1000,
       input: Box::new(StreamingExecutor::Filter {
           condition: prop > 100,
           input: Box::new(StreamingExecutor::ScanVertices {
               partition_id: [0..16],  // ← 并行
               partition_range: [各自的范围],
           })
       })
   }

3. Pipeline 调度器协调：
   thread_pool.execute(Partition-0)
   thread_pool.execute(Partition-1)
   ...
   thread_pool.execute(Partition-15)
   
   ↓ 等待结果 ↓
   
   Limit 从 Filter 拉取数据
   Filter 从 Scan 拉取数据
   Scan 的 16 个实例并行运行

4. 背压控制：
   max_buffered_chunks = 10
   如果缓冲区满，暂停提交新任务

结果：
  - 有效扫描行数：~1000-2000（16 个分区并行）
  - 加速比：10-50x（相比无并行）
```

---

## 5. 性能目标

| 查询场景 | 单线程 | 8 核并行 | 加速比 |
|---------|--------|---------|--------|
| SELECT * LIMIT 10（百万行） | 500ms | 5ms | **100x** |
| SELECT * WHERE x>100 LIMIT 1000 | 800ms | 50ms | **16x** |
| SELECT * （百万行） | 500ms | 70ms | **7x** |

---

## 6. 关键设计决策

| 决策 | 选项 | 原因 |
|------|------|------|
| **何时使用调度器** | 有 Scan 且有 LIMIT | 只有这些查询才受益 |
| **分区方式** | 按 ID 范围 | 简单、均匀、兼容现有结构 |
| **分区数量** | CPU 核心数 * 2 | 避免过度订阅，最大化吞吐 |
| **Build side 选择** | 右表 | 通常较小 |
| **背压阈值** | max 10 chunks | 平衡内存和吞吐 |

---

## 7. 实现清单

### 依赖

- [x] 阶段 0（StreamingExecutor 基础）

### 任务

- [ ] VertexTable/EdgeTable 添加分区方法
- [ ] PartitionView 实现
- [ ] QueryScheduler 框架
- [ ] 线程池集成
- [ ] 背压机制
- [ ] QueryPipelineManager 集成
- [ ] 基准测试（LIMIT、全表、WHERE 过滤）
- [ ] 集成测试

### 验收标准

- [ ] LIMIT 查询：100x 加速
- [ ] 全表扫描：7x 加速
- [ ] 内存占用恒定（~4MB/chunk）
- [ ] 所有并行测试通过

---

## 总结

- ✅ **分区架构**：逻辑分区，不复制数据
- ✅ **调度器简单**：只协调顶层拉取，不显式管理分区线程
- ✅ **自然并行化**：多个 Scan executor 实例被并行调用
- ✅ **向后兼容**：简单查询无需调度器

