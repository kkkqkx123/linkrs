# Streaming Executor 重构方案

> 分析日期：2026-07-11
> 范围：`crates/graphdb-query/src/query/executor/streaming/`
> 目标：解决枚举膨胀、样板代码重复、算子状态内联三大结构问题，同时保持向后兼容

## 一、现状与问题

当前 `StreamingExecutor` 是一个 79+ variant 的单体枚举，核心生命周期 dispatch 如下：

```rust
pub fn open(&mut self) -> Result<(), QueryError> {
    match self {
        Self::Start { .. }       => operators::access::open_start(self),
        Self::Filter { .. }      => operators::single_input::open_filter(self),
        Self::Sort { .. }        => operators::stateful::open_sort(self),
        Self::HashJoin { .. }    => operators::binary::open_hashjoin(self),
        // ... 79+ arms
    }
}
// advance() / stop() / close() 各重复 79+ arms
```

### 1.1 核心问题

| 问题 | 表现 | 量化 |
|------|------|------|
| **枚举单体膨胀** | 4 个生命周期方法各 79+ match arm，新增算子需改 8+ 位置 | executor.rs 2296 行，其中 dispatch 占 ~40% |
| **字段重复** | 每个 variant 都携带 `plan_node_id: i64`、`runtime: Option<Arc<...>>`、`opened: bool` | 79 × 3 = 237 次声明，类型一致但无法复用 |
| **状态内联** | Sort/Aggregate/Join 的状态字段（`all_rows`、`result_iter`、`hash_map`）直接嵌入枚举变体 | 无法独立测试状态机逻辑 |
| **箱式链** | 算子树通过 `Box<StreamingExecutor>` 链接，无法执行器层面做图优化 | 查询计划线性翻译为链表 |
| **无并行** | `PartitionView` 已定义但未接入执行路径 | 单线程 pull 模型无并行能力 |

### 1.2 根因分析

当前 enum-based 设计在项目初期是正确的选择——快速迭代、静态分发、调试直观。随着算子数量增长到 79+，单体枚举的成本超过了收益：

- 枚举 variant 越多，每个 match 的编译时间和代码体积越大
- 每个 `open/advance/stop/close` 的 match 必须穷举所有 variant，增加出错风险
- 算子的公共基础设施（runtime、plan_node_id、profile）与算子逻辑耦合

## 二、目标架构

```
                     StreamingExecutor (top-level enum, 6 个域)
                          /     |      |       |       |      \
                   Source  Unary  Binary  Blocking  Graph   Sink
                   enum    enum   enum    enum      enum    enum
```

**原则**：
- 保留下层算子 enum 的静态分发特性
- 在顶层拆分领域 enum，将公共基础设施提到共享位置
- 不引入 trait object，保持零动态分发开销
- 分阶段实施，每步可独立测试和 PR

## 三、第一阶段：拆分领域枚举（立即实施，无行为变化）

### 3.1 定义领域枚举

```rust
// 独立文件，每个 domain enum 与对应的 operators/ 子目录对应

pub enum SourceOperator {
    ScanVertices { buffer, ... },
    StorageScanVertices { storage, cursor, ... },
    ScanEdges { buffer, ... },
    StorageScanEdges { storage, cursor, ... },
    GetVertices { ... },
    GetEdges { ... },
    GetNeighbors { ... },
    IndexScan { ... },
    EdgeIndexScan { ... },
    Argument { ... },
    Sample { ... },
    LookupIndex { ... },
    FulltextSearch { ... },
    FulltextLookup { ... },
    VectorSearch { ... },
    VectorLookup { ... },
}

pub enum UnaryOperator {
    Filter { input, predicate, ... },
    Project { input, expressions, ... },
    Limit { input, limit, ... },
    Distinct { input, ... },
    Dedup { input, ... },
    Assign { input, assignments, ... },
    Remove { input, columns, ... },
    Unwind { input, ... },
    AppendVertices { input, ... },
    // ...
}

pub enum BinaryOperator {
    HashJoin { left, right, ... },
    HashLeftJoin { left, right, ... },
    NestedLoopJoin { left, right, ... },
    InnerJoin { left, right, ... },
    LeftJoin { left, right, ... },
    RightJoin { left, right, ... },
    FullOuterJoin { left, right, ... },
    CrossJoin { left, right, ... },
    SemiJoin { left, right, ... },
    Union { left, right, ... },
    UnionAll { left, right, ... },
    Intersect { left, right, ... },
    Except { left, right, ... },
    Minus { left, right, ... },
    Apply { left, right, ... },
    PatternApply { left, right, ... },
}

pub enum BlockingOperator {
    Sort { input, keys, ... },
    Aggregate { input, group_by, aggregates, ... },
    GroupBy { input, keys, ... },
    WindowFunction { input, partition, order, ... },
    Window { input, ... },
    TopN { input, n, ... },
    Materialize { input, ... },
    DataCollect { input, ... },
    RollUpApply { input, ... },
}

pub enum GraphOperator {
    Expand { input, ... },
    ExpandAll { input, ... },
    Traverse { input, ... },
    TraverseAll { input, ... },
    BiExpand { left, right, ... },
    BiTraverse { left, right, ... },
    ShortestPath { input, ... },
    BFSShortest { input, ... },
    AllPaths { input, ... },
    MultiShortestPath { input, ... },
    Subgraph { input, ... },
    AppendVertices { input, ... },
}

pub enum SinkOperator {
    InsertVertices { input, ... },
    InsertEdges { input, ... },
    UpdateVertices { input, ... },
    UpdateEdges { input, ... },
    DeleteVertices { input, ... },
    DeleteEdges { input, ... },
    PipeDeleteVertices { input, ... },
    PipeDeleteEdges { input, ... },
    DeleteTags { input, ... },
    SpaceManage { input, ... },
    TagManage { input, ... },
    EdgeManage { input, ... },
    IndexManage { input, ... },
    UserManage { input, ... },
    FulltextManage { input, ... },
    VectorManage { input, ... },
    BeginTransaction { input, ... },
    Commit { input, ... },
    Rollback { input, ... },
}

// 以及 StartOperator、ShowStats、Analyze、Migrate 等
pub enum MiscOperator { ... }
```

### 3.2 顶层枚举转为薄 dispatch 层

```rust
// streaming/executor.rs → 仅做一级 dispatch

pub struct OperatorBase {
    pub plan_node_id: i64,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub opened: bool,
}

pub enum StreamingExecutor {
    Source(OperatorBase, SourceOperator),
    Unary(OperatorBase, Box<StreamingExecutor>, UnaryOperator),
    Binary(OperatorBase, Box<StreamingExecutor>, Box<StreamingExecutor>, BinaryOperator),
    Blocking(OperatorBase, Box<StreamingExecutor>, BlockingOperator),
    Graph(OperatorBase, Box<StreamingExecutor>, GraphOperator),
    GraphBinary(OperatorBase, Box<StreamingExecutor>, Box<StreamingExecutor>, GraphOperator),
    Sink(OperatorBase, Box<StreamingExecutor>, SinkOperator),
    Misc(OperatorBase, MiscOperator),
}
```

生命周期方法简化为 6-7 个 match arm，而不是 79 个：

```rust
impl StreamingExecutor {
    pub fn open(&mut self) -> Result<(), QueryError> {
        let (base, has_input) = match self {
            Self::Source(base, op)   => return op.open(base),
            Self::Unary(base, _, op) => return op.open(base),
            Self::Binary(base, ..)   => ...
            Self::Blocking(base, _, op) => ...
            Self::Graph(base, _, op) => ...
            Self::GraphBinary(base, ..) => ...
            Self::Sink(base, _, op)  => ...
            Self::Misc(base, op)     => return op.open(base),
        };
    }
}
```

### 3.3 共享 OperatorBase

将 `plan_node_id`、`runtime`、`opened` 等公共字段抽离为 `OperatorBase`：

```rust
// streaming/executor/base.rs 或 streaming/operator_base.rs

pub struct OperatorBase {
    pub plan_node_id: i64,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub opened: bool,
}

impl OperatorBase {
    pub fn set_runtime(&mut self, rt: Option<Arc<ExecutionRuntime>>) {
        self.runtime = rt;
    }

    pub fn plan_node_id(&self) -> i64 {
        self.plan_node_id
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        if let Some(rt) = &self.runtime {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    pub fn record_profile_timing(&self, phase: &str, elapsed_us: u64) {
        // ...
    }

    pub fn record_profile_rows(&self, count: u64) {
        // ...
    }

    pub fn register_resource<F>(&self, f: F) where F: FnOnce() + Send + 'static {
        if let Some(rt) = &self.runtime {
            rt.on_cleanup(f);
        }
    }
}
```

### 3.4 领域算子实现

每个领域枚举有自己的 `open/next/stop/close`：

```rust
impl UnaryOperator {
    pub fn open(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        base.ensure_not_cancelled()?;
        let start = Instant::now();
        match self {
            Self::Filter { input, predicate, .. } => input.open(),
            Self::Project { input, .. } => input.open(),
            Self::Limit { input, .. } => input.open(),
            // ...
        }
        base.record_profile_timing("open", start.elapsed().as_micros() as u64);
        Ok(())
    }

    pub fn next(&mut self, base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        base.ensure_not_cancelled()?;
        match self {
            Self::Filter { input, predicate, .. } => {
                // filter-specific logic
            }
            Self::Limit { input, limit, consumed, .. } => {
                // limit-specific logic
            }
            // ...
        }
    }
}
```

### 3.5 第一阶段影响范围

| 文件 | 变更 |
|------|------|
| `streaming/streaming_executor.rs` → 拆分 | 拆分前备份，新建各领域枚举文件 |
| `streaming/operator_base.rs` | 新增：OperatorBase 结构体 + 共享方法 |
| `streaming/executor.rs` | 从 2296 行缩减为 ~200 行 dispatch 层 |
| `streaming/driver.rs` | 简化 `extract_operator_name`，按领域枚举匹配 |
| `streaming/engine.rs` | 无变化（仍通过 StreamingExecutor 顶层枚举调用） |
| `streaming/builder.rs` | 调整构造调用，创建领域枚举实例 |
| `streaming/executor/operators/*.rs` | 每个 operators/ 文件改为 impl 对应领域枚举 |

### 3.6 第一阶段风险

- **PR 体积大**：拆分影响 ~20 个文件，建议 1-2 天内完成，防止长期分支冲突
- **功能不变**：重构前后运行已有测试确保行为一致
- **Builder 兼容**：builder 创建算子的代码只需变构造路径，不涉及行为

## 四、第二阶段：阻塞算子状态抽离（可选，在第一阶段后）

当前 `Sort`、`Aggregate`、`HashJoin` 等阻塞算子将状态（`all_rows`、`result_iter`、`hash_map`）直接嵌入枚举变体。第二阶段将其提取为独立的状态结构体：

```rust
// 阻塞算子之前
Sort {
    input: Box<StreamingExecutor>,
    sort_expressions: Vec<Expression>,
    sort_directions: Vec<SortDirection>,
    all_rows: Vec<Vec<Value>>,
    row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    opened: bool,
    memory_tracker: MemoryTracker,
    plan_node_id: i64,
    runtime: Option<Arc<ExecutionRuntime>>,
}

// 提取后
struct SortState {
    all_rows: Vec<Vec<Value>>,
    row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    memory_tracker: MemoryTracker,
}

impl BlockingOperator {
    Sort {
        input: Box<StreamingExecutor>,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        state: Option<SortState>,  // None until open()
    },
}
```

这么做的好处：
- 状态结构体可以独立测试
- `Spillable` 实现可以操作 `SortState` 而不是整个算子
- `close()` 只需 `state = None`，无需逐个清理字段

## 五、第三阶段：并行基础（长远规划）

| 步骤 | 内容 | 前置条件 |
|------|------|---------|
| 3.1 | PartitionView 接入 builder，为 source operator 创建分区副本 | 领域枚举拆分完成 |
| 3.2 | 单线程依次处理每个分区的数据（不并行，但验证分区语义正确） | 3.1 |
| 3.3 | `OperatorBase` 增加 `is_global` 标记，区分 local/global operator state | 3.2 |
| 3.4 | 引入线程池，按 morsel 分发任务 | 3.3、Pipeline breaker 完善 |

> 第三阶段依赖 pipeline breaker 机制成熟，详见 `pipeline_refactor.md`

## 六、迁移策略

### 6.1 过渡期兼容

拆分期间 `StreamingExecutor` 保留原枚举定义，新增领域枚举作为内部实现细节。提供 `From`/`Into` 转换：

```rust
impl From<SourceOperator> for StreamingExecutor {
    fn from(op: SourceOperator) -> Self {
        // 转换为旧枚举变体用于过渡
    }
}
```

过渡期结束后（所有路径使用新结构），删除旧枚举变体。

### 6.2 拆分顺序

```
Step 1: 定义 OperatorBase + SourceOperator（影响最小，最快验证）
Step 2: 定义 UnaryOperator（单输入，涉及 20+ variant，关键）
Step 3: 定义 BinaryOperator（双输入，需要处理 left/right）
Step 4: 定义 BlockingOperator（状态管理复杂，先抽离状态）
Step 5: 定义 GraphOperator（图遍历算子，独立语义）
Step 6: 定义 SinkOperator（副作用的 DML/DDL）
Step 7: 顶层枚举改为 6-way dispatch
Step 8: 删除过渡兼容代码
```

## 七、验证标准

每个阶段完成后的检查项：

- [ ] `cargo clippy --all-targets --all-features` 无新增 warning
- [ ] `cargo test --lib` 所有已有测试通过，无 behavior change
- [ ] 4 个生命周期 dispatch 的 match arm 数量从 79 降至 ≤7
- [ ] 新增算子的代码修改点从 8 处降至 ≤3 处
- [ ] `extract_operator_name` 不再需要 79+ arms（使用领域枚举的 Display）
