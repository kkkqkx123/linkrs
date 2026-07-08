# 流式 Executor 与 Pipeline 调度器设计分析

> 分析日期：2026-06-26
> 涉及改进项目：P1 阶段 - 流式 Chunk 接口与 Pipeline 调度器
> 参考基础文档：`docs/ref/linkrs_improvement_plan.md`

---

## 1. 问题背景

### 1.1 当前执行引擎的局限性

Linkrs 查询引擎采用**全物化执行模式**，存在以下问题：

| 问题 | 现状 | 影响 |
|------|------|------|
| **内存占用** | DataSet 包含全部查询结果（Vec<Vec<Value>>），加载到内存 | 大查询 OOM；LIMIT 场景浪费资源 |
| **执行阻塞** | executor.execute() 返回完整 ExecutionResult，阻塞执行 | 无法中途停止；无法动态调整资源 |
| **并行困难** | 需等待前置 executor 完成才能启动 | 无法利用多核；依赖级联长路径 |
| **内存无界** | 中间结果全在内存，无分页或溢出机制 | 无法处理超大数据集 |

改进方案目标（来自 `linkrs_improvement_plan.md` 第 7-8 章）：
- **流式 Chunk 接口**：将结果分块返回（每次返回 DataChunk 而非 DataSet）
- **Pipeline 调度器**：调度 executor 任务，支持并行与中途停止
- **预期收益**：LIMIT 查询从扫全表变为扫必要行；内存使用下降 50%+；多核利用率提升

---

## 2. 代码上下文

### 2.1 Query Package 的模块结构

```
crates/graphdb-query/src/query/
├── executor/                    # 执行器层（核心改造点）
│   ├── base/                    # Executor trait 与基础设施
│   │   ├── executor_base.rs     # Executor trait 定义 + BaseExecutor 实现
│   │   ├── execution_result.rs  # ExecutionResult 枚举（DataSet/Empty/Success/Error）
│   │   ├── executor_enum.rs     # ExecutorEnum（60+ 变体的静态分发）
│   │   ├── executor_stats.rs    # 执行统计信息
│   │   └── execution_context.rs # 执行上下文（变量、结果存储）
│   ├── impl/                    # 201 个具体 executor 实现文件
│   │   ├── scan/                # ScanVertices, ScanEdges
│   │   ├── filter/              # Filter executor
│   │   ├── projection/          # Project executor
│   │   ├── join/                # 各类 Join executor
│   │   ├── aggregate/           # 聚合 executor
│   │   └── ...                  # 其他操作
│   ├── factory/                 # ExecutorFactory（PlanNode → ExecutorEnum）
│   ├── utils/                   # 辅助工具
│   │   ├── pipeline_executors.rs# 管道相关 executor（ArgumentExecutor 等）
│   │   └── object_pool.rs       # Executor 对象池（可复用）
│   └── macros.rs                # 委托宏（match 分发到对应 executor）
├── query_pipeline_manager.rs    # Pipeline 管理器（整个执行流程协调）
├── planning/                    # 查询规划
├── optimizer/                   # 查询优化
└── parser/                      # SQL 解析
```

### 2.2 核心职责关系

#### ExecutorEnum 与 Executor Trait
- **ExecutorEnum** 是一个大型 enum，包含 60+ 种 executor 类型（ScanVertices, Filter, Join 等）
- **Executor<S> Trait** 定义统一接口：`execute()`, `open()`, `close()`, `stats()` 等
- 所有 201 个具体 executor 都实现 Executor Trait
- **静态分发**：通过 `delegate_to_executor!` 宏，ExecutorEnum 的方法调用转发到对应变体
- 优点：无虚函数开销；编译期已知所有类型；类型安全
- 缺点：ExecutorEnum 代码规模大；添加新 executor 需修改 enum；无法动态扩展

#### 执行流程（QueryPipelineManager → PlanExecutor）
1. **QueryPipelineManager.execute_query()**：入口方法
   - 解析、验证、规划、优化查询
   - 调用 `execute_plan()` 执行优化后的执行计划

2. **QueryPipelineManager.execute_plan()**：将计划转换为 executor
   - 创建 PlanExecutor
   - 调用 `PlanExecutor.execute_plan(&plan, storage, expr_ctx)`
   - 返回 ExecutionResult（包含完整 DataSet 或 Empty/Success/Error）

3. **PlanExecutor**（factory/engine.rs）：
   - 遍历执行计划 DAG（方向无环图）
   - 按依赖关系创建 ExecutorEnum 实例
   - 从根 executor 调用 `execute()`，链式处理输入输出
   - 整个过程是**同步、阻塞、全物化**

#### InputExecutor 与执行链
- **InputExecutor Trait**：定义 `set_input()` / `get_input()`，用于 executor 之间传递数据
- 每个处理数据的 executor 实现 InputExecutor：Filter, Project, Join, Sort 等
- 执行流程：
  - 子 executor 先执行，得到 ExecutionResult（包含 DataSet）
  - 父 executor 通过 `get_input()` 获取完整 DataSet
  - 处理后返回新的 ExecutionResult

#### ExecutionResult 与 DataSet
- **ExecutionResult** 是 enum，主要变体：
  - `DataSet(DataSet)`：存储查询结果，行集合为 `Vec<Vec<Value>>`
  - `Empty`, `Success`：无数据操作
  - `Error`：错误状态
- **DataSet** 来自 graphdb-core，完全物化在内存

### 2.3 涉及的关键类型

**在 graphdb-core 中定义**：
- `Value`：单元素值类型（数据库中的原子值）
- `DataSet`：`{ col_names: Vec<String>, rows: Vec<Vec<Value>> }`
- `DataType`：类型枚举（Int, String, Float 等）

**在 graphdb-query 中定义**：
- `Executor<S> Trait`：统一执行接口（S = StorageClient 泛型）
- `ExecutionResult`：执行结果枚举
- `ExecutorEnum<S>`：所有 executor 的并联 enum
- `ExecutorStats`：每个 executor 的性能统计

---

## 3. 流式执行的设计空间

### 3.1 需要引入的新数据结构

#### DataChunk（数据分块）
- **职责**：表示查询结果的一个部分（例如 1000 行）
- **结构**：
  - 列向量集合（Vec<ColumnVector>）
  - 选择向量（SelectionVector）：用于表示过滤后保留的行索引（可选）
  - 当前 chunk 的行数（size）
- **优点**：内存有界；支持谓词下推（选择向量延迟物化）
- **与 DataSet 的区别**：DataSet 是全量结果，DataChunk 是单个批次

#### ColumnVector（列向量）
- **职责**：表示某列的数据（按列存储，而非行存储）
- **结构**：字节数组 + 类型信息 + NULL bitmap
- **优点**：缓存友好；便于向量化计算；支持压缩
- **当前 DataSet 的局限**：按行存储，Cache miss 多；难以向量化

#### SelectionVector（选择向量）
- **职责**：表示从原始行集中选中哪些行
- **用途**：Filter 输出选择向量而非物化新 chunk，后续算子应用选择向量
- **优点**：减少内存拷贝；便于谓词下推

### 3.2 需要引入的新 Executor Trait

#### StreamingExecutor<S> Trait
- **方法**：
  - `open(&mut self) -> DBResult<()>`：初始化资源
  - `next(&mut self) -> DBResult<Option<DataChunk>>`：拉取下一个 chunk（若无更多数据返回 None）
  - `close(&mut self) -> DBResult<()>`：释放资源
- **特性**：
  - 无状态或有限状态（与 execute() 的全物化形成对比）
  - 支持中途停止（调用方可以只取前 N 个 chunk）
  - 支持内存流式处理

#### 与现有 Executor Trait 的关系
- **现有 Executor Trait**（同步）：一次性返回完整 ExecutionResult
- **StreamingExecutor Trait**（流式）：多次调用 next()，每次返回一个 DataChunk
- **设计选择**：
  - 方案 A：替代现有 Executor（高风险，需改 201 个实现）
  - **方案 B（推荐）**：并行存在，通过适配层共存；新 executor 实现 StreamingExecutor，旧 executor 通过 ChunkingAdapter 适配
  - 方案 C：仅为部分 executor 添加 streaming 变体（如 Scan, Filter, Limit）

---

## 4. Pipeline 调度器的设计空间

### 4.1 调度器的职责

#### 当前执行流程的问题
- **PlanExecutor** 按拓扑顺序同步遍历执行计划
- 子 executor 完成 → 父 executor 开始，严格的依赖链
- 无法并行：即使多个独立的子树，也会串行执行
- 无法中断：LIMIT 10 也要扫全表，再取前 10 行

#### Pipeline 调度器的改进目标
1. **任务分解**：将执行计划 DAG 分解为独立的 task
   - 例如：Scan task、Filter task、Aggregate task 各为一个 task
   - 每个 task 处理一个 chunk

2. **并行执行**：
   - 数据分区后，可以在多个线程/核心上并行处理
   - 例如：Scan partition-0、Scan partition-1 可并行

3. **流式触发**：
   - 不等待所有输入，有 chunk 就处理
   - Scan 产生第一个 chunk → Filter 立即处理 → Aggregate 接收
   - 减少内存占用，提升缓存命中率

4. **资源管理**：
   - 跟踪每个 task 的内存占用
   - 动态调整 chunk 大小
   - 背压（backpressure）：上游快速，下游慢，减速上游

5. **可中断执行**：
   - LIMIT 算子收够 10 行后，发送停止信号
   - 上游 task 停止生成数据
   - Scan 停止扫描

### 4.2 调度器的模型

#### Push vs Pull 模型
- **Pull 模型（推荐）**：
  - 消费者（如 Limit）驱动生产者（如 Scan）
  - `Limit.next()` → 调用 `Filter.next()` → 调用 `Scan.next()`
  - 自然支持中途停止，内存有界
  
- **Push 模型**：
  - 生产者驱动消费者
  - `Scan` 产生 chunk → 推给 Filter → 推给 Limit
  - 需要缓冲队列，复杂但支持并行

#### PipelineScheduler 核心接口设想
```
PipelineScheduler {
    plan: ExecutionPlan,
    executor_map: HashMap<PlanNodeId, Box<dyn StreamingExecutor>>,
    task_queue: VecDeque<Task>,
    ...
}

Task {
    node_id: PlanNodeId,
    chunk_index: usize,
    status: TaskStatus, // Pending, Running, Done, Error
}

pub fn schedule(&mut self) -> DBResult<ExecutionResult> {
    while !task_queue.is_empty() {
        // 按依赖关系取出 task
        // 执行 task（调用 executor.next()）
        // 收集 chunk，直到 LIMIT 满足或无更多数据
    }
}
```

### 4.3 与现有系统的集成点

#### QueryPipelineManager 的改造
- 当前 `execute_plan()` 调用 `PlanExecutor.execute_plan()`
- 改造后可选择：
  - **传统路径**：继续用全物化 Executor（向后兼容）
  - **流式路径**：创建 PipelineScheduler，调用 `schedule()`

#### ExecutorFactory 的改造
- 当前为 PlanNode → ExecutorEnum 的 1:1 转换
- 改造后可生成：
  - ExecutorEnum（实现旧 Executor Trait）
  - 或 StreamingExecutor 实现（逐步迁移）

---

## 5. 关键设计决策

### 5.1 待讨论的问题

#### Q1: DataChunk 的设计细节
- **Chunk 大小**如何确定？
  - 固定大小（如 1024 行、4MB）？
  - 动态大小（根据内存压力调整）？
  - 由 executor 自行决定？

- **列向量的编码**是否需要与现有压缩对接？
  - 当前有 ColumnEncoding（FSST、Bitpack 等）
  - DataChunk 中的 ColumnVector 是否复用这些编码？

#### Q2: StreamingExecutor 的异步特性
- 是否使用 async/await？
  - async 可支持 IO 等待（未来可能需要）
  - 但增加复杂度，当前 executor 多数是 CPU-bound
- 建议：保持同步，但设计可中断的接口

#### Q3: 与现有 201 个 executor 的兼容性
- 全部迁移为 StreamingExecutor？（大工程，多周时间）
- 逐步迁移关键 executor？（Scan、Filter、Limit、Aggregate）
- 提供自动适配层？（执行性能损失）

#### Q4: 内存管理与分页
- Pipeline 调度器是否应负责内存限制？
- 是否需要配合 BufferManager（P2 改进项目）？
- 目前先假设内存足够（后续加入 mmap/分页）

#### Q5: 并行度与调度策略
- 默认并行度是多少？（CPU 核心数？）
- 是否支持动态调整？
- 如何处理 CPU-bound 与 IO-bound 的混合工作负载？

### 5.2 实现路径建议

#### 阶段 1：基础设施（2-3 周）
- 在 graphdb-query 中创建 `executor/streaming/` 子模块
- 定义 DataChunk、ColumnVector、SelectionVector 数据结构
- 定义 StreamingExecutor trait
- 为 BaseExecutor 提供流式版本（StreamingBaseExecutor）

#### 阶段 2：关键 executor 流式化（3-4 周）
- 实现 StreamingScanExecutor（顶点/边扫描）
- 实现 StreamingFilterExecutor（谓词过滤）
- 实现 StreamingProjectExecutor（列投影）
- 实现 StreamingLimitExecutor（截断）

#### 阶段 3：PipelineScheduler 框架（2-3 周）
- 创建 `executor/scheduler/` 子模块
- 实现 PipelineScheduler，支持 pull-based 任务调度
- 集成到 QueryPipelineManager（作为可选路径）
- 单元测试：验证流式执行与全物化等价

#### 阶段 4：验证与优化（1-2 周）
- 性能基准测试（LIMIT 查询应快 10+ 倍）
- 内存占用测试（应降低 50%+）
- 集成测试（确保兼容性）

---

## 6. 依赖关系与风险

### 6.1 模块依赖
```
graphdb-core
  ↑
graphdb-query
  ├─ streaming/ (新增)
  │  ├─ data_chunk.rs      (DataChunk, ColumnVector, SelectionVector)
  │  ├─ streaming_executor.rs (StreamingExecutor trait)
  │  └─ impl/              (具体 executor 实现)
  ├─ executor/
  │  ├─ scheduler/ (新增) (PipelineScheduler)
  │  └─ base/              (修改现有接口)
  └─ query_pipeline_manager.rs (修改)
```

### 6.2 主要风险
| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 201 个 executor 改造工作量大 | 高 | 项目周期延长 | 逐步迁移 + 适配层 |
| 流式模型引入性能回归 | 中 | 某些查询变慢 | 对小结果集提供快速路径 |
| ExecutorEnum 复杂度进一步增加 | 中 | 维护困难 | 考虑拆分 enum 或迁移至 trait object |
| 与 P2 项目冲突（BufferManager） | 低 | 架构返工 | 提前讨论集成方案 |

---

## 7. 设计思路总结

### 核心原则
1. **向后兼容**：现有系统继续工作，新系统并行存在
2. **渐进式改造**：关键路径优先，逐步扩展覆盖
3. **自包含**：流式模块与调度器相对独立，便于单独开发测试
4. **明确的接口边界**：DataChunk、StreamingExecutor、PipelineScheduler 各司其职

### 建议的下一步
1. **与协作者讨论本文档**，确认设计决策（Q1-Q5）
2. **制定实现优先级**（优先改造 Scan、Filter、Limit？）
3. **预留时间**与 P2 BufferManager 的协调设计
4. **建立性能指标**（LIMIT 查询加速比、内存占用降低比例）

### 预期收益（第 7 章改进方案）
- **LIMIT 查询**：从 O(N) 扫描降至 O(K)，加速 10-50 倍
- **内存占用**：中间结果流式处理，峰值内存从全数据集降至单 chunk，降低 50%+
- **并行执行**：pipeline 任务可在多核上并行，多核利用率 70%+（现状 <20%）
- **系统吞吐量**：多个查询并发时，调度器支持任务交错，减少等待

---

## 附录：文件修改清单

**需要创建的新文件**：
- `crates/graphdb-query/src/query/executor/streaming/mod.rs`
- `crates/graphdb-query/src/query/executor/streaming/data_chunk.rs`
- `crates/graphdb-query/src/query/executor/streaming/streaming_executor.rs`
- `crates/graphdb-query/src/query/executor/streaming/impl/scan.rs`
- `crates/graphdb-query/src/query/executor/streaming/impl/filter.rs`
- `crates/graphdb-query/src/query/executor/scheduler/mod.rs`
- `crates/graphdb-query/src/query/executor/scheduler/pipeline_scheduler.rs`

**需要修改的现有文件**：
- `crates/graphdb-query/src/query/executor/base/executor_base.rs`（添加 StreamingBaseExecutor）
- `crates/graphdb-query/src/query/query_pipeline_manager.rs`（添加流式执行路径）
- `crates/graphdb-query/src/query/executor/factory/engine.rs`（支持流式 executor 创建）

**参考现有代码**：
- `crates/graphdb-query/src/query/executor/impl/scan/` - 扫描 executor 参考
- `crates/graphdb-query/src/query/executor/utils/pipeline_executors.rs` - 管道 executor 参考

---

**制作人**：Claude (Haiku 4.5)  
**审核状态**：待讨论
