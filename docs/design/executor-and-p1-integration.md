# Executor 重构与 P1 方案的融合设计

> 更新日期：2026-06-27（重大更新：引入 dyn 设计决策）
> 核心问题：executor 架构重构如何与 P1.a/P1.c 相互配合

---

## 1. 改进的系统改造结构

```
整个系统的改造分为四个维度：

┌─────────────────────────────────────────────────────────┐
│ 维度 1：执行模型的改造（第 0 阶段，必须先做）           │
│ - 从全物化 Push 改为流式 Pull                            │
│ - StreamingExecutor trait（使用 Box<dyn>）              │
│ - 关键决策：完全动态分发，虚函数开销 < 0.2%              │
│ 文档：executor-architecture-refactoring.md               │
│        streaming-executor-design-decisions.md            │
└─────────────────────────────────────────────────────────┘
                    ↓ (依赖)
┌─────────────────────────────────────────────────────────┐
│ 维度 2：并行执行框架（第 1 阶段，4-6 周）               │
│ - 数据分区（Vertex/Edge 按 ID 范围）                    │
│ - Pipeline 调度器（任务调度和背压）                     │
│ - 与 StreamingExecutor 无缝协作                         │
│ 文档：P1a-parallel-execution-framework.md               │
└─────────────────────────────────────────────────────────┘
  ↑
  └──(与第 2 阶段并行)
     ↓
┌─────────────────────────────────────────────────────────┐
│ 维度 3：关键 executor 迁移（第 2 阶段，2-3 周）         │
│ - Scan、Filter、Project、Limit 的流式化                │
│ - 无适配层，直接改造                                    │
│ - 与 Pipeline 调度器紧密配合                            │
│ 文档：executor-implementation-guide.md                   │
└─────────────────────────────────────────────────────────┘
  ↑
  └──(与第 1 阶段并行)
     ↓
┌─────────────────────────────────────────────────────────┐
│ 维度 4：存储与执行优化（第 3 阶段，2-3 周）             │
│ - 列级压缩选择器                                        │
│ - 并行 HashJoin                                        │
│ - 独立于前几个维度，可并行进行                          │
│ 文档：P1c-storage-and-execution-optimizations.md        │
└─────────────────────────────────────────────────────────┘
```

**新结构的改进**：
- 清晰的阶段划分（第 0-3 阶段）
- 明确的依赖关系（第 0 是基础，第 1-2 并行，第 3 独立）
- 关键决策明确化（使用 dyn，虚函数开销可忽略）

---

## 2. 改进点：为什么不需要 ChunkingAdapter

### 2.1 原 P1.b 设计的问题

在 `P1b-streaming-executor-migration.md` 中，我提议用 `ChunkingAdapter` 来适配旧 executor：

```rust
// 旧设计
pub struct ChunkingAdapter {
    executor: Box<dyn Executor>,  // 包装旧 executor
    input: Option<DataChunk>,
    exhausted: bool,
}

impl StreamingExecutor for ChunkingAdapter {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 第一次调用时执行完整的 executor.execute()
        // 后续调用分割 DataSet
    }
}
```

**问题**：
1. 性能开销：第一次调用会导致整个子树全部执行和物化
2. 维护负担：同时维护 Executor 和 StreamingExecutor 两套系统
3. 不清晰：上游可能是旧系统，下游是新系统，数据流不统一

### 2.2 新设计：直接改造，无适配层

根据 `executor-architecture-refactoring.md` 的分析：

```rust
// 新设计：直接用 StreamingBaseExecutor 替代
pub struct StreamingFilterExecutor<S> {
    base: StreamingBaseExecutor<S>,
    condition: Expression,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl StreamingExecutor for StreamingFilterExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 直接拉取上游 chunk，逐行过滤，返回
        // 无须先执行上游全部，再分割
    }
}
```

**优势**：
1. 清晰的 Pull 链：每个 executor 都是流式的，链条清晰
2. 无额外开销：不需要先全物化再分割
3. 统一的系统：所有 executor 都实现同一个接口

---

## 3. 融合的执行流程

### 3.1 全流程（从查询到结果）

```
QueryPipelineManager.execute_query()
  ├─ 解析、验证、规划查询 → ExecutionPlan DAG
  │
  ├─ 选择执行模式 ExecutionMode { Materialized, Streaming }
  │
  └─ 调用 PlanExecutor.execute_plan(mode)
       │
       ├─ 若 mode = Materialized（旧系统，向后兼容）
       │  └─ 现有的全物化 Push 流程
       │
       └─ 若 mode = Streaming（新系统）
          │
          ├─ 数据分区：VertexTable/EdgeTable 按 ID 范围分割
          │
          ├─ 构建 executor 树（所有都是 StreamingExecutor）
          │  └─ build_executor_chain_streaming()
          │     ├─ ScanVertices → StreamingScanExecutor
          │     ├─ Filter → StreamingFilterExecutor
          │     ├─ Limit → StreamingLimitExecutor
          │     └─ Join → StreamingHashJoinExecutor
          │
          ├─ 创建 QueryScheduler（可选，用于并行化）
          │
          └─ 执行（Pull 循环）
             ├─ root_executor.open()
             ├─ while let Some(chunk) = root_executor.next()?
             │   └─ 处理 chunk
             └─ root_executor.close()
```

### 3.2 具体示例：LIMIT 查询

```
查询：SELECT * FROM V LIMIT 10

执行树（Streaming 模式）：
  StreamingLimitExecutor(limit=10)
    └─ StreamingFilterExecutor(where_condition)
        └─ StreamingScanVertices(partition_id=0)

执行流程：
  1. LimitExecutor.open()
     ├─ FilterExecutor.open()
     │  └─ ScanExecutor.open()
     
  2. 拉取循环：
     consumed = 0
     while consumed < 10 {
       LimitExecutor.next()
         → 告诉上游"我要数据"
         → FilterExecutor.next()
              → 告诉上游"我要数据"
              → ScanExecutor.next()
                   ↓ 返回 chunk (size=1024)
              → FilterExecutor 过滤行
              ↓ 返回有效行的 chunk
         → LimitExecutor 截断到剩余配额
         ↓ 返回 chunk (size=min(1024, 10-consumed))
       consumed += chunk.size
     }
     
  3. LimitExecutor.stop()
     → 上游接收停止信号，立即返回
     → ScanExecutor 停止扫描
     
  4. 关闭资源
```

**关键点**：
- Scan 只读取了必要的行（~10 行），而非全表（百万行）
- 性能从 扫描百万行 变为 扫描 10 行
- 无需适配层，每个 executor 都遵循相同的接口

### 3.3 并行执行的融合

若启用 Pipeline 调度器：

```
QueryScheduler
  ├─ 获得执行计划的 root executor（StreamingLimitExecutor）
  │
  ├─ 数据分区：16 个分区（8 核 * 2）
  │
  ├─ 任务分解：
  │  ├─ Task: ScanVertices(partition=0) → pull next()
  │  ├─ Task: ScanVertices(partition=1) → pull next()
  │  ├─ Task: ScanVertices(partition=2) → pull next()
  │  └─ ...
  │
  ├─ 任务调度（Pull 驱动）：
  │  LimitExecutor.pull_chunk()
  │    → 若没有 chunk，分配新任务到线程池
  │    → 等待任务完成
  │    → 返回 chunk
  │
  └─ 背压控制：
     max_buffered_chunks = 10
     若缓冲满则暂停分配新任务
```

**与 StreamingExecutor 的协作**：
- QueryScheduler 不改动 executor 的接口
- 每个 executor 仍然用 `next()` 返回 chunk
- QueryScheduler 只是外层的任务协调层

---

## 4. 改造的时间线

### 4.1 并行关系图

```
第 1 阶段（4-6 周）：Executor 架构重构
  ├─ StreamingExecutor trait 定义 ✓
  ├─ StreamingBaseExecutor 实现 ✓
  ├─ 数据分区设计 ✓
  ├─ PlanExecutor 支持 ExecutionMode ✓
  └─ 基准测试框架搭建 ✓

         ↓

第 2 阶段（3-4 周）：关键 executor 迁移
  ├─ StreamingScanVertices/Edges ✓
  ├─ StreamingFilter ✓
  ├─ StreamingProject ✓
  ├─ StreamingLimit ✓
  └─ 集成测试 ✓

         ↓ (同时进行)

第 3 阶段（2-3 周）：Pipeline 调度器
  ├─ 任务调度框架 ✓
  ├─ 背压机制 ✓
  ├─ 并行执行验证 ✓
  └─ 性能基准 ✓

总周期：9 周（需要 3 人团队并行）
```

### 4.2 与原 P1 方案的差异

| 方面 | 原 P1.b 设计 | 新设计 |
|------|-----------|--------|
| 适配层 | 需要 ChunkingAdapter | 无适配层，直接改造 |
| Executor 改造 | 逐步，可选 | 需要改造到新系统 |
| DataChunk 复杂度 | 保守（行向量） | 保守（行向量）✓ |
| ExecutorEnum | 继续保留 | 保留，增加 ExecutorMode |
| PlanExecutor | 小改动 | 有改动，支持两种模式 |

---

## 5. 详细的改造流程

### 5.1 Executor 架构重构（第 1 阶段）

**文件结构**：

```
crates/graphdb-query/src/query/executor/
├── base/
│   ├── executor_base.rs          (保留，旧系统)
│   ├── executor_enum.rs          (保留，旧系统)
│   ├── streaming_executor.rs     ← 新增
│   ├── streaming_base_executor.rs ← 新增
│   ├── execution_result.rs       (保留)
│   └── execution_context.rs      (保留)
│
├── streaming/                     ← 新增目录
│   ├── mod.rs
│   ├── data_chunk.rs             ← 新增
│   └── impl/
│       ├── mod.rs
│       ├── scan.rs               ← 新增
│       ├── filter.rs             ← 新增
│       ├── project.rs            ← 新增
│       ├── limit.rs              ← 新增
│       └── ...
│
├── impl/                          (旧 executor，保留)
│   ├── scan/
│   ├── filter/
│   └── ...
│
└── factory/
    ├── engine.rs                  (修改：支持 ExecutionMode)
    └── executor_factory.rs        (修改：创建 StreamingExecutor)
```

**核心改动**：

1. **定义 StreamingExecutor trait**（`streaming_executor.rs`）
2. **实现 StreamingBaseExecutor**（`streaming_base_executor.rs`）
3. **修改 PlanExecutor**（`factory/engine.rs`）
   - 新增 `ExecutionMode` enum
   - 新增 `execute_streaming()` 方法
   - 保留旧的 `execute_materialized()` 方法

### 5.2 关键 Executor 迁移（第 2 阶段）

**优先级顺序**：

1. **StreamingScanVertices** + **StreamingScanEdges**
   - 原因：数据源，无依赖
   - 难度：低
   - 文件：`streaming/impl/scan.rs`

2. **StreamingFilter**
   - 原因：高频，单输入，无状态
   - 难度：低
   - 文件：`streaming/impl/filter.rs`

3. **StreamingProject**
   - 原因：常见，单输入，无状态
   - 难度：低
   - 文件：`streaming/impl/project.rs`

4. **StreamingLimit**
   - 原因：LIMIT 优化的关键
   - 难度：低
   - 文件：`streaming/impl/limit.rs`

5. **StreamingAggregate**（可选，复杂）
   - 原因：有状态，需要消费所有输入
   - 难度：中
   - 文件：`streaming/impl/aggregate.rs`

### 5.3 Pipeline 调度器集成（第 3 阶段）

QueryScheduler 与 StreamingExecutor 的协作：

```rust
pub struct QueryScheduler<S: StorageClient> {
    root_executor: Box<dyn StreamingExecutor>,
    data_partitions: Vec<PartitionView>,
    task_queue: VecDeque<Task>,
    thread_pool: ThreadPool,
}

impl<S> QueryScheduler<S> {
    pub fn execute_plan(mut self) -> DBResult<ExecutionResult> {
        self.root_executor.open()?;
        
        let mut all_rows = Vec::new();
        
        // Pull 循环：消费 executor 的所有 chunk
        while let Some(chunk) = self.root_executor.next()? {
            all_rows.extend(chunk.rows);
        }
        
        self.root_executor.close()?;
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: ...,
            rows: all_rows,
        }))
    }
}
```

---

## 6. 向后兼容性策略

### 6.1 现有代码的保护

```rust
// 旧系统（Materialized 模式）
pub struct PlanExecutor<S> {
    factory: ExecutorFactory<S>,
}

impl<S> PlanExecutor<S> {
    pub fn execute_plan(
        &mut self,
        plan: &ExecutionPlan,
        mode: ExecutionMode,
    ) -> DBResult<ExecutionResult> {
        match mode {
            ExecutionMode::Materialized => {
                // 现有的完整执行逻辑，无改动
                self.execute_materialized(plan)
            }
            ExecutionMode::Streaming => {
                // 新的流式执行逻辑
                self.execute_streaming(plan)
            }
        }
    }
}
```

### 6.2 迁移期的模式选择

```rust
pub enum ExecutionMode {
    /// 全物化模式（默认，向后兼容）
    Materialized,
    
    /// 流式模式（新系统，所有 executor 必须是 StreamingExecutor）
    Streaming,
    
    /// 混合模式（如果某个 executor 尚未迁移，自动降级到全物化）
    /// 实现复杂，暂不支持
    // Hybrid,
}
```

### 6.3 用户可见的 API

```rust
// QueryPipelineManager（高层 API）
impl QueryPipelineManager {
    pub fn execute_query(&self, query: &str) -> DBResult<ExecutionResult> {
        // 默认行为：选择 Materialized 模式
        self.execute_query_with_mode(query, ExecutionMode::Materialized)
    }
    
    pub fn execute_query_with_mode(
        &self,
        query: &str,
        mode: ExecutionMode,
    ) -> DBResult<ExecutionResult> {
        // ...
    }
}

// 配置方式
// 方式 1：环境变量
let mode = std::env::var("LINKRS_EXECUTION_MODE")
    .map(|v| {
        match v.as_str() {
            "streaming" => ExecutionMode::Streaming,
            _ => ExecutionMode::Materialized,
        }
    })
    .unwrap_or(ExecutionMode::Materialized);

// 方式 2：查询 Hint（后续支持）
// SELECT /*+ EXECUTION_MODE(streaming) */ * FROM V LIMIT 10
```

---

## 7. 实施的关键决策点

### 7.1 何时改造 Executor

**立即改造（第 2 阶段）**：
- ScanVertices, ScanEdges（数据源）
- Filter, Project, Limit（高频，简单）

**可延后改造**：
- Aggregate（有状态，复杂）
- Join（双输入，非常复杂）
- Sort（可能内存大）

**优先级低，可保留旧系统**：
- 图遍历算子（Expand, Traverse）
- 不常用的操作符

### 7.2 是否保留旧的 Executor trait

**建议：保留**
- 某些复杂 executor 短期内难以流式化
- 可以用 ExecutionMode::Materialized 继续使用
- 避免一次性改造所有 201 个 executor

**长期（2-3 年后）**：
- 全部迁移完成
- 可考虑删除旧系统

### 7.3 ExecutorEnum 是否改动

**答案：最小改动**

当前：
```rust
pub enum ExecutorEnum<S> {
    Filter(FilterExecutor<S>),
    // ...
}
```

不改为：
```rust
pub enum ExecutorEnum<S> {
    Filter(FilterExecutor<S>),
    StreamingFilter(StreamingFilterExecutor<S>),  // ✗ 导致爆炸
}
```

而是引入 ExecutorMode：
```rust
pub enum ExecutorMode<S> {
    V1(Box<dyn Executor<S>>),        // 旧系统
    V2(Box<dyn StreamingExecutor>),   // 新系统
}
```

这样 ExecutorEnum 本身无需改动。

---

## 8. 总结对比表

| 方面 | Executor 架构重构 | P1.a (并行框架) | P1.b (流式迁移) |
|------|-----------------|---------------|---------------|
| 时间线 | 阶段 1 (4-6 周) | 阶段 1-2 (4-6 周) | 阶段 2-3 (3-4 周) |
| 依赖关系 | 独立 | 依赖重构 | 依赖重构 |
| 改动范围 | executor/base, executor/factory | executor/scheduler, storage/vertex | executor/streaming |
| 关键概念 | StreamingExecutor trait | Pipeline 调度器 | 逐步迁移 executor |
| 是否必须 | 是 | 是（用于并行化） | 否（用于流式化） |
| 向后兼容 | 是（ExecutionMode） | 是（可选启用） | 部分（ExecutionMode） |

---

## 9. 建议的实施路线

### 完整的改造流程

```
Week 1-3: Executor 架构重构（必须先做）
  ├─ W1: StreamingExecutor trait + StreamingBaseExecutor
  ├─ W2: 修改 PlanExecutor + ExecutionMode
  ├─ W3: 测试框架 + 基准搭建
  └─ Milestone: 架构就位，准备迁移具体 executor

Week 4-6: 关键 executor 迁移 + Pipeline 调度器设计
  ├─ W4: StreamingScan + StreamingFilter + StreamingLimit (并行)
  ├─ W5: 集成测试 + 性能验证
  ├─ W5-6: Pipeline 调度器框架搭建
  └─ Milestone: LIMIT 查询 100x 加速

Week 7-9: 更多 executor 迁移 + 存储优化
  ├─ W7: StreamingProject + StreamingAggregate
  ├─ W8: 列级压缩 + 并行 Join 设计
  ├─ W9: 性能优化 + 文档
  └─ Milestone: 完整的流式系统上线

Week 10+: 后续优化
  ├─ 图遍历算子流式化（按需）
  ├─ BufferManager + PageCache（P2）
  └─ GDS 框架（P2）
```

---

## 10. 修改的关键文件清单

### 第 1 阶段（架构重构）

**新增文件**：
- `crates/graphdb-query/src/query/executor/base/streaming_executor.rs`
- `crates/graphdb-query/src/query/executor/base/streaming_base_executor.rs`
- `crates/graphdb-query/src/query/executor/streaming/mod.rs`
- `crates/graphdb-query/src/query/executor/streaming/data_chunk.rs`

**修改文件**：
- `crates/graphdb-query/src/query/executor/factory/engine.rs`
  - 新增 `ExecutionMode` enum
  - 新增 `execute_streaming()` 方法
  - 修改 `execute_plan()` 方法签名

### 第 2 阶段（executor 迁移）

**新增文件**：
- `crates/graphdb-query/src/query/executor/streaming/impl/scan.rs`
- `crates/graphdb-query/src/query/executor/streaming/impl/filter.rs`
- `crates/graphdb-query/src/query/executor/streaming/impl/limit.rs`
- `crates/graphdb-query/src/query/executor/streaming/impl/project.rs`

**修改文件**：
- `crates/graphdb-query/src/query/executor/factory/executor_factory.rs`
  - 新增方法 `create_streaming_executor()`

### 第 3 阶段（Pipeline 调度器）

**新增文件**：
- `crates/graphdb-query/src/executor/scheduler/mod.rs`
- `crates/graphdb-query/src/executor/scheduler/pipeline_scheduler.rs`
- `crates/graphdb-query/src/executor/scheduler/task.rs`

---

## 结论

通过结合 **Executor 架构重构**、**P1.a 并行框架**、**P1.b 流式迁移**，可以实现：

1. **清晰的技术方向**：Pull-based 流式执行，而非适配器补丁
2. **渐进式改造**：第 1 阶段架构，第 2 阶段迁移关键 executor，第 3 阶段实现并行
3. **向后兼容**：旧系统通过 ExecutionMode 继续可用
4. **性能目标**：LIMIT 查询 10-50 倍加速，多核利用率 70%+，内存占用恒定

这是一个**清晰、无适配层、可增量实施**的架构重构方案。
