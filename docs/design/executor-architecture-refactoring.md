# Executor 架构重构设计

> 设计日期：2026-06-27
> 核心问题：201 个 executor 如何迁移到新的流式架构
> 设计原则：直接重构，避免适配层

---

## 1. 现有架构的核心问题

### 1.1 全物化执行模型

**当前执行流程**（以 Binary Join 为例）：

```
PlanExecutor.build_executor_chain()
  ├─ 创建 JoinExecutor（未执行）
  ├─ 递归构建左子树
  │  └─ 立即调用 left_executor.execute() ← 同步阻塞，返回完整 DataSet
  ├─ 将结果存入 ExecutionContext
  ├─ 递归构建右子树
  │  └─ 立即调用 right_executor.execute() ← 同步阻塞，返回完整 DataSet
  └─ 将结果存入 ExecutionContext
     （此时两个 DataSet 都在内存中）

JoinExecutor.execute()
  └─ 从 ExecutionContext 取出两个完整的 DataSet，执行 Join
     （返回新的完整 DataSet）
```

**问题**：
1. 中间结果全部物化在内存（DataFrame 模式）
2. Binary operators 无法利用 Pipeline：左表全扫描 → 右表全扫描 → Join
3. LIMIT 也要扫全表再取前 N 行
4. Pipeline 调度器难以优化（因为已经全物化）

### 1.2 执行器接口的限制

**现有三层接口**：

```rust
pub trait Executor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult>;  // 全物化
    fn open(&mut self) -> DBResult<()>;
    fn close(&mut self) -> DBResult<()>;
    fn stats(&self) -> &ExecutorStats;
}

pub trait InputExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>);      // 缓存上游 executor
    fn get_input(&self) -> Option<&ExecutorEnum<S>>;
}

pub trait HasInput<S> {
    fn get_input(&self) -> Option<&ExecutionResult>;      // 缓存 ExecutionResult
    fn set_input(&mut self, input: ExecutionResult);
}
```

**混乱**：
- `InputExecutor` 存储上游 executor（延迟执行）
- `HasInput` 存储执行结果（急切执行）
- 同一个 executor 可能同时实现两者，导致歧义

### 1.3 ExecutorEnum 的爆炸问题

当前 ExecutorEnum 包含 60+ 个 variant，每增加一个 executor 都要修改：

```rust
pub enum ExecutorEnum<S> {
    Start(StartExecutor<S>),
    Filter(FilterExecutor<S>),
    Project(ProjectExecutor<S>),
    Join(JoinExecutor<S>),
    // ... 60+ variants
}
```

**后续问题**：
- 添加 StreamingFilter variant → 需要修改所有模式匹配
- 或者 StreamingExecutorEnum 并存 → 维护两套系统

---

## 2. 正确的重构方向

### 2.1 核心洞察：Pull vs Push

**当前是 Push 模型（PlanExecutor 驱动）**：
```
PlanExecutor 决定执行顺序 → executor 被动接受
```

**需要转变为 Pull 模型（最上层 executor 驱动）**：
```
最上层 executor (Limit) 调用 .next()
  ↓
下游 executor (Filter) 调用 upstream.next()
  ↓
最下层 executor (Scan) 返回一个 chunk
  ↑ ↑ ↑ (逐层向上返回)
```

### 2.2 三层递进的重构策略

#### 第一层：定义新的流式执行接口（**最小改动**）

```rust
/// 流式执行器：Pull 模型，每次返回一个 chunk
pub trait StreamingExecutor: Send {
    /// 打开资源
    fn open(&mut self) -> DBResult<()>;
    
    /// 拉取下一个数据块
    fn next(&mut self) -> DBResult<Option<DataChunk>>;
    
    /// 关闭资源
    fn close(&mut self) -> DBResult<()>;
    
    /// 获取统计信息
    fn stats(&self) -> &ExecutorStats;
    
    /// 可选：停止执行（LIMIT 中途停止）
    fn stop(&mut self) -> DBResult<()> {
        Ok(())
    }
}

/// Single Input 操作的输入接口
pub trait StreamingInput: StreamingExecutor {
    fn set_input(&mut self, input: Box<dyn StreamingExecutor>);
}

/// Binary Input 操作的输入接口
pub trait StreamingBinaryInput: StreamingExecutor {
    fn set_left_input(&mut self, input: Box<dyn StreamingExecutor>);
    fn set_right_input(&mut self, input: Box<dyn StreamingExecutor>);
}
```

**设计原则**：
- 独立定义，不改动现有 `Executor` trait
- Pull 模型天然支持 LIMIT 中途停止
- 用 `Box<dyn StreamingExecutor>` 避免 ExecutorEnum 爆炸

#### 第二层：BaseExecutor 的流式版本

```rust
pub struct StreamingBaseExecutor {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub storage: Option<Arc<RwLock<S>>>,
    pub context: ExecutionContext,
    
    // 流式执行的额外字段
    is_open: bool,
    stats: ExecutorStats,
    
    // 输入端（单输入或双输入）
    input: Option<Box<dyn StreamingExecutor>>,
    left_input: Option<Box<dyn StreamingExecutor>>,
    right_input: Option<Box<dyn StreamingExecutor>>,
}

impl<S> StreamingBaseExecutor<S> {
    /// 创建具体的 executor（无需每个都重新实现基础功能）
    pub fn new_for_executor(id: i64, name: String, storage: Arc<RwLock<S>>) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: Some(storage),
            context: ExecutionContext::new(...),
            is_open: false,
            stats: ExecutorStats::new(),
            input: None,
            left_input: None,
            right_input: None,
        }
    }
}
```

#### 第三层：具体的流式 executor 实现

**示例：StreamingScanExecutor**

```rust
pub struct StreamingScanExecutor<S: StorageClient> {
    base: StreamingBaseExecutor<S>,
    partition_id: usize,
    partition_range: Range<u32>,
    current_idx: u32,
    chunk_size: usize,
    exhausted: bool,
}

impl<S> StreamingExecutor for StreamingScanExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        if self.exhausted {
            return Ok(None);
        }
        
        // 扫描一个 chunk
        let mut rows = Vec::with_capacity(self.chunk_size);
        while self.current_idx < self.partition_range.end 
              && rows.len() < self.chunk_size {
            // 从存储读一行
            if let Some(row) = self.read_row(self.current_idx)? {
                rows.push(row);
            }
            self.current_idx += 1;
        }
        
        if self.current_idx >= self.partition_range.end {
            self.exhausted = true;
        }
        
        if rows.is_empty() {
            return Ok(None);
        }
        
        Ok(Some(DataChunk {
            columns: self.base.get_column_names(),
            rows,
            size: rows.len(),
        }))
    }
    
    fn open(&mut self) -> DBResult<()> {
        self.base.is_open = true;
        Ok(())
    }
    
    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }
    
    fn stats(&self) -> &ExecutorStats {
        &self.base.stats
    }
}
```

**示例：StreamingFilterExecutor**

```rust
pub struct StreamingFilterExecutor<S> {
    base: StreamingBaseExecutor<S>,
    condition: Expression,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl<S> StreamingInput for StreamingFilterExecutor<S> {
    fn set_input(&mut self, input: Box<dyn StreamingExecutor>) {
        self.input = Some(input);
    }
}

impl<S> StreamingExecutor for StreamingFilterExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 尝试从上游拉取 chunk
        let mut input = self.input.take().unwrap();
        
        loop {
            match input.next()? {
                Some(mut chunk) => {
                    // 过滤这个 chunk 的行
                    chunk.rows.retain(|row| {
                        self.evaluate_condition(row).unwrap_or(false)
                    });
                    chunk.size = chunk.rows.len();
                    
                    self.input = Some(input);
                    
                    // 如果过滤后还有行，返回；否则继续拉取下一个 chunk
                    if chunk.size > 0 {
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
}
```

**示例：StreamingLimitExecutor**

```rust
pub struct StreamingLimitExecutor<S> {
    base: StreamingBaseExecutor<S>,
    limit: usize,
    consumed: usize,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl<S> StreamingExecutor for StreamingLimitExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        if self.consumed >= self.limit {
            self.stop()?;
            return Ok(None);
        }
        
        let mut input = self.input.take().unwrap();
        
        match input.next()? {
            Some(mut chunk) => {
                let remaining = self.limit - self.consumed;
                if chunk.size > remaining {
                    // 截断 chunk
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
    
    fn stop(&mut self) -> DBResult<()> {
        if let Some(mut input) = self.input.take() {
            input.stop()?;
        }
        Ok(())
    }
}
```

---

## 3. ExecutorEnum 的演进：从 enum 到 trait object

### 3.1 问题：为什么不能继续用 enum

如果继续用 enum：
```rust
pub enum ExecutorEnum<S> {
    // ... 60 个现有 variant
    StreamingFilter(StreamingFilterExecutor<S>),
    StreamingProject(StreamingProjectExecutor<S>),
    // ... 更多 streaming variant
}

// 所有模式匹配都要更新
impl ExecutorEnum<S> {
    pub fn execute(&mut self) -> DBResult<ExecutionResult> {
        match self {
            ExecutorEnum::Filter(ref mut e) => e.execute(),
            ExecutorEnum::StreamingFilter(ref mut e) => ??? // 返回类型不同！
            // ...
        }
    }
}
```

**无法解决的矛盾**：
- 旧 executor 返回 `ExecutionResult`
- 新 executor 没有 `execute()` 方法，只有 `next()`
- 无法用单一的 match 语句处理两种接口

### 3.2 解决方案：转换为 trait object

**第一步：定义通用的 executor trait**

```rust
/// 统一的 executor trait（包含所有 executor 的共同方法）
pub trait UnifiedExecutor: Send {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
    fn stats(&self) -> &ExecutorStats;
    fn is_open(&self) -> bool;
}

/// 旧系统的 executor（全物化）
pub trait ExecutorV1: UnifiedExecutor {
    fn execute_v1(&mut self) -> DBResult<ExecutionResult>;
}

/// 新系统的 executor（流式）
pub trait ExecutorV2: UnifiedExecutor {
    fn execute_v2_streaming(&mut self) -> DBResult<Option<DataChunk>>;
    fn open(&mut self) -> DBResult<()>;
    fn close(&mut self) -> DBResult<()>;
    fn next(&mut self) -> DBResult<Option<DataChunk>>;
}
```

**第二步：PlanExecutor 支持混合执行**

```rust
pub enum ExecutorMode {
    V1(Box<dyn ExecutorV1>),  // 旧的全物化 executor
    V2(Box<dyn ExecutorV2>),  // 新的流式 executor
}

pub struct PlanExecutor<S> {
    executors: HashMap<PlanNodeId, ExecutorMode>,
    execution_mode: ExecutionMode,  // Materialized vs Streaming
}

impl<S> PlanExecutor<S> {
    pub fn execute_plan(&mut self, plan: &ExecutionPlan) -> DBResult<ExecutionResult> {
        match self.execution_mode {
            ExecutionMode::Materialized => {
                // 使用旧的全物化执行逻辑
                self.execute_materialized(plan)
            }
            ExecutionMode::Streaming => {
                // 使用新的流式执行逻辑
                self.execute_streaming(plan)
            }
        }
    }
    
    fn execute_streaming(&mut self, plan: &ExecutionPlan) -> DBResult<ExecutionResult> {
        // 获得 root executor（类型必须是 StreamingExecutor）
        let root_executor = self.executors.get_mut(&plan.root_id())
            .ok_or(DBError::execution("Root executor not found"))?;
        
        let mut results = Vec::new();
        let mut root = match root_executor {
            ExecutorMode::V2(exec) => exec,
            ExecutorMode::V1(_) => {
                return Err(DBError::execution(
                    "Streaming mode requires all executors to be V2"
                ))
            }
        };
        
        root.open()?;
        
        // Pull 循环：驱动整个执行树
        while let Some(chunk) = root.next()? {
            results.extend(chunk.rows);
        }
        
        root.close()?;
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: ...,
            rows: results,
        }))
    }
}
```

### 3.3 分阶段迁移策略

**不需要一次性修改 ExecutorEnum**，而是：

```
阶段 1：定义新接口，保留旧系统
  ├─ StreamingExecutor trait 定义
  ├─ StreamingBaseExecutor 实现
  └─ ExecutorMode enum 实现

阶段 2：迁移关键 executor（Scan, Filter, Limit）
  ├─ 新建 StreamingScanExecutor
  ├─ 新建 StreamingFilterExecutor
  ├─ 新建 StreamingLimitExecutor
  └─ ExecutorFactory 支持创建 V2 executor

阶段 3：迁移其他 executor（逐个）
  ├─ StreamingProjectExecutor
  ├─ StreamingAggregateExecutor
  ├─ StreamingJoinExecutor
  └─ 其他...

阶段 4：弃用旧 ExecutorEnum（可选，远期）
  └─ 全部使用 trait object
```

**每个阶段可以独立验证**，不需要等待全部完成。

---

## 4. PlanExecutor 的重构

### 4.1 现状：Push 模型的问题

```rust
fn build_executor_chain(&mut self, plan_node, ...) -> ExecutorEnum<S> {
    let mut executor = self.factory.create_executor(plan_node, ...)?;
    
    match plan_node.children().len() {
        0 => {} // Leaf node
        1 => {  // Single input
            let child = self.build_executor_chain(children[0], ...)?;
            executor.set_input(child);  // 存储 executor 而不执行
        }
        2 => {  // Binary input - 这是问题所在
            // 立即执行左子树
            let mut left = self.build_executor_chain(children[0], ...)?;
            let left_result = left.execute()?;  // ← 同步阻塞，全物化
            context.set_result(left_var, left_result);
            
            // 立即执行右子树
            let mut right = self.build_executor_chain(children[1], ...)?;
            let right_result = right.execute()?;  // ← 同步阻塞，全物化
            context.set_result(right_var, right_result);
        }
    }
    executor
}
```

### 4.2 新模型：Pull 模式的改造

```rust
fn build_executor_chain_streaming(
    &mut self,
    plan_node: &PlanNodeEnum,
) -> DBResult<Box<dyn StreamingExecutor>> {
    match plan_node {
        // Leaf node: Scan
        PlanNodeType::ScanVertices => {
            let exec = StreamingScanVerticesExecutor::new(...);
            Ok(Box::new(exec))
        }
        
        // Single input: Filter, Project, Limit
        PlanNodeType::Filter => {
            let mut exec = StreamingFilterExecutor::new(...);
            let child = self.build_executor_chain_streaming(children[0])?;
            exec.set_input(child);  // 存储 executor（不执行）
            Ok(Box::new(exec))
        }
        
        // Binary input: Join（关键改变）
        PlanNodeType::Join => {
            let mut exec = StreamingHashJoinExecutor::new(...);
            
            // 不立即执行，而是存储 executor
            let left = self.build_executor_chain_streaming(children[0])?;
            let right = self.build_executor_chain_streaming(children[1])?;
            
            exec.set_left_input(left);
            exec.set_right_input(right);
            
            Ok(Box::new(exec))
        }
    }
}
```

### 4.3 流式 Join 的实现

```rust
pub struct StreamingHashJoinExecutor<S> {
    base: StreamingBaseExecutor<S>,
    
    // 左表流式输入
    left_input: Option<Box<dyn StreamingExecutor>>,
    
    // 右表流式输入
    right_input: Option<Box<dyn StreamingExecutor>>,
    
    // Build 阶段的状态
    build_state: BuildState,
    
    // Probe 阶段的状态
    probe_state: ProbeState,
}

pub enum BuildState {
    NotStarted,
    Building,
    BuildComplete(HashMap<JoinKey, Vec<DataChunk>>),  // 右表的 hash 表
}

pub enum ProbeState {
    NotStarted,
    Probing(Box<dyn StreamingExecutor>),  // 当前正在 probe 的 left chunk
    Complete,
}

impl StreamingExecutor for StreamingHashJoinExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 第一次调用：Build 阶段
        if matches!(self.build_state, BuildState::NotStarted) {
            self.build_phase()?;
        }
        
        // 持续进行 Probe 阶段
        self.probe_phase()
    }
    
    fn build_phase(&mut self) -> DBResult<()> {
        self.build_state = BuildState::Building;
        
        let mut right_input = self.right_input.take().unwrap();
        let mut hash_table: HashMap<JoinKey, Vec<DataChunk>> = HashMap::new();
        
        // 逐个拉取右表的 chunk，构建 hash 表
        while let Some(chunk) = right_input.next()? {
            for row in &chunk.rows {
                let key = extract_join_key(row);
                hash_table.entry(key)
                    .or_insert_with(Vec::new)
                    .push(chunk.clone());
            }
        }
        
        self.right_input = Some(right_input);
        self.build_state = BuildState::BuildComplete(hash_table);
        Ok(())
    }
    
    fn probe_phase(&mut self) -> DBResult<Option<DataChunk>> {
        // Probe 阶段：逐个拉取左表 chunk，与 hash 表 Join
        
        let hash_table = match &self.build_state {
            BuildState::BuildComplete(table) => table,
            _ => return Err(DBError::execution("Build phase not complete")),
        };
        
        let mut left_input = self.left_input.take().unwrap();
        
        loop {
            match left_input.next()? {
                Some(chunk) => {
                    // 对这个 chunk 的每一行进行 Join
                    let mut result_rows = Vec::new();
                    for left_row in &chunk.rows {
                        let key = extract_join_key(left_row);
                        if let Some(right_chunks) = hash_table.get(&key) {
                            for right_chunk in right_chunks {
                                for right_row in &right_chunk.rows {
                                    let joined = join_rows(left_row, right_row);
                                    result_rows.push(joined);
                                }
                            }
                        }
                    }
                    
                    self.left_input = Some(left_input);
                    
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk {
                            rows: result_rows,
                            size: result_rows.len(),
                            ..chunk
                        }));
                    }
                    // 如果没有 join 结果，继续拉取下一个 chunk
                }
                None => {
                    self.left_input = Some(left_input);
                    return Ok(None);
                }
            }
        }
    }
}
```

---

## 5. 现有 executor 的改造路线

### 5.1 按优先级分类

#### 优先级 1（必须改造）- 数据源和消费端

| Executor | 原因 | 改造难度 |
|----------|------|--------|
| ScanVertices | 数据源，无依赖 | 低 |
| ScanEdges | 数据源，无依赖 | 低 |
| Limit | 消费端，LIMIT 优化的关键 | 低 |

#### 优先级 2（核心路径）- 单输入算子

| Executor | 原因 | 改造难度 |
|----------|------|--------|
| Filter | 高频，谓词下推的基础 | 低 |
| Project | 高频，列投影 | 低 |
| Aggregate | 高频，但有状态管理 | 中 |
| Sort | 常见，但可能内存大 | 中 |

#### 优先级 3（可选）- 双输入算子

| Executor | 原因 | 改造难度 |
|----------|------|--------|
| HashInnerJoin | 常见，但复杂 | 高 |
| HashLeftJoin | 常见，但复杂 | 高 |
| Union | 不常见，简单 | 低 |

#### 优先级 4（图专用）- 图遍历

| Executor | 原因 | 改造难度 |
|----------|------|--------|
| Expand | 图特有，但需谨慎 | 高 |
| ShortestPath | 算法复杂 | 高 |

### 5.2 改造的通用模式

**Pattern 1: 无状态的单输入算子（Filter, Project）**

```rust
pub struct StreamingFilterExecutor<S> {
    base: StreamingBaseExecutor<S>,
    condition: Expression,
    input: Option<Box<dyn StreamingExecutor>>,
}

impl StreamingExecutor for StreamingFilterExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 拉取上游 chunk，过滤行，返回
        // 逻辑与全物化版本完全相同，只是作用在单个 chunk 上
    }
}
```

**Pattern 2: 有状态的单输入算子（Aggregate）**

```rust
pub struct StreamingAggregateExecutor<S> {
    base: StreamingBaseExecutor<S>,
    agg_specs: Vec<AggregateFunctionSpec>,
    input: Option<Box<dyn StreamingExecutor>>,
    
    // 聚合状态（跨多个 chunk 累积）
    state: AggregateState,
    all_chunks_consumed: bool,
}

impl StreamingExecutor for StreamingAggregateExecutor<S> {
    fn next(&mut self) -> DBResult<Option<DataChunk>> {
        // 消费所有上游 chunk，累积到 state
        while let Some(chunk) = input.next()? {
            for row in chunk.rows {
                self.state.update(&row);
            }
        }
        
        // 返回最终的聚合结果（单行 chunk）
        if !self.all_chunks_consumed {
            self.all_chunks_consumed = true;
            return Ok(Some(self.state.finalize()?));
        }
        
        Ok(None)  // 后续调用返回 None
    }
}
```

**Pattern 3: 双输入算子（Join）**

```rust
pub struct StreamingHashJoinExecutor<S> {
    base: StreamingBaseExecutor<S>,
    left_input: Option<Box<dyn StreamingExecutor>>,
    right_input: Option<Box<dyn StreamingExecutor>>,
    
    build_state: BuildState,
    probe_state: ProbeState,
}

// 如 4.3 所示的实现
```

---

## 6. 具体迁移步骤

### 6.1 准备阶段（1 周）

1. 定义 `StreamingExecutor` trait 和相关接口
2. 实现 `StreamingBaseExecutor`
3. 定义 `DataChunk` 数据结构
4. 修改 `ExecutorFactory` 支持 `ExecutorMode` enum
5. 修改 `PlanExecutor` 支持两种执行模式

### 6.2 第一批：数据源和消费端（2 周）

**并行进行**：
- Developer A：StreamingScanVertices + StreamingScanEdges
- Developer B：StreamingLimit
- Developer C：编写基准测试和集成测试

### 6.3 第二批：单输入算子（2.5 周）

**顺序**（因为有依赖）：
1. StreamingFilter（week 1）
2. StreamingProject（week 1）
3. StreamingAggregate（week 1.5）
4. StreamingSort（week 1）

### 6.4 第三批：双输入算子（2-3 周）

1. StreamingUnion（简单，week 0.5）
2. StreamingHashJoin（复杂，week 1.5-2）
3. 其他 Join variant（week 0.5-1）

### 6.5 第四批：图遍历（根据需求）

- StreamingExpand（week 2-3）
- StreamingTraverse（week 2-3）

---

## 7. 向后兼容性

### 7.1 保留旧系统

- 现有的 `Executor` trait 继续存在
- `ExecutorEnum` 保持不变
- 旧的 `execute()` 方法继续工作
- 通过 `ExecutionMode::Materialized` 启用

### 7.2 过渡期

```rust
pub enum ExecutionMode {
    Materialized,           // 旧系统：全物化
    Streaming,              // 新系统：流式（要求所有 executor 都是 V2）
    HybridAutomatic,        // 混合：自动检测，某些 executor 用 V1，某些用 V2
}
```

### 7.3 配置控制

```rust
// QueryPipelineManager
pub fn execute_query_with_mode(
    query: &str,
    mode: ExecutionMode,
) -> DBResult<ExecutionResult> {
    // ...
}

// 默认向后兼容
let default_mode = ExecutionMode::Materialized;
```

---

## 8. 实现的关键细节

### 8.1 DataChunk 的设计

```rust
pub struct DataChunk {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,  // 初期保持行向量
    pub size: usize,
    pub stats: Option<ChunkStats>,
}
```

**为什么不是列向量**：
- 列向量化需要大量转换代码
- 初期用行向量与全物化 DataSet 兼容
- 后期优化时再迁移到列向量

### 8.2 ExecutorStats 的流式版本

```rust
pub struct ExecutorStats {
    pub id: i64,
    pub name: String,
    pub rows_processed: u64,
    pub rows_filtered: u64,
    pub elapsed_ms: u64,
    pub memory_bytes: u64,
}

// StreamingExecutor 每次 next() 调用时增量更新这些统计
```

### 8.3 Error Handling

```rust
// 流式执行中可能的错误
pub enum ExecutionError {
    DataError(String),
    ComputeError(String),
    InputExhausted,  // 上游没有更多数据
    Interrupted,     // LIMIT 中途停止
}
```

---

## 9. 风险与缓解

| 风险 | 概率 | 缓解 |
|------|------|------|
| 旧系统和新系统混在一起，维护复杂 | 中 | 清晰的代码组织；新建 `executor/streaming/` 目录 |
| 某些 executor 难以流式化（如排序、聚合） | 中 | 允许这些 executor 先用全物化模式，后期优化 |
| 性能反而下降（context switch 开销） | 低 | 充分的基准测试；识别瓶颈 |
| 现有代码大量改动，引入 bug | 中 | 充分的单元和集成测试；逐步迁移 |

---

## 10. 文件组织

```
crates/graphdb-query/src/query/executor/
├── base/
│   ├── executor_base.rs           (保留，旧系统)
│   ├── executor_enum.rs           (保留，旧系统)
│   ├── streaming_executor.rs      (新增，新 trait)
│   ├── streaming_base.rs          (新增，新基类)
│   └── ...
├── impl/
│   └── ... (旧系统的 executor，保留)
├── streaming/                      (新增目录)
│   ├── mod.rs
│   ├── executor.rs                (StreamingExecutor trait)
│   ├── base.rs                    (StreamingBaseExecutor)
│   ├── data_chunk.rs              (DataChunk)
│   └── impl/
│       ├── mod.rs
│       ├── scan.rs                (StreamingScanVertices, StreamingScanEdges)
│       ├── filter.rs              (StreamingFilterExecutor)
│       ├── project.rs             (StreamingProjectExecutor)
│       ├── limit.rs               (StreamingLimitExecutor)
│       ├── aggregate.rs           (StreamingAggregateExecutor)
│       ├── join.rs                (StreamingHashJoinExecutor)
│       └── ...
├── factory/
│   ├── engine.rs                  (修改，支持 ExecutionMode)
│   ├── executor_factory.rs        (修改，创建 V2 executor)
│   └── ...
└── ...
```

---

## 11. 总结

### 核心改造原则

1. **不改动 Executor trait**，而是新增 StreamingExecutor trait
2. **不改动 ExecutorEnum**，而是通过 ExecutorMode 支持新旧并存
3. **PlanExecutor 支持两种模式**，通过开关选择
4. **逐步迁移 executor**，按优先级和依赖关系进行

### 最终效果

- 旧系统继续工作（向后兼容）
- 新系统逐步上线（低风险）
- 最终所有 executor 都迁移到新系统（可选）
- 内存占用从 O(N) 变为 O(chunk_size)
- LIMIT 查询性能提升 10-50 倍

### 关键优势

- **无适配层开销**：直接重构，清晰的执行路径
- **可增量迁移**：不必一次性改造所有 executor
- **向后兼容**：旧代码继续工作，无感知
- **性能可控**：通过 ExecutionMode 选择，避免性能回归
