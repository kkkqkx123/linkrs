# Executor 模块改进分析

> 基于 `docs/issue/query_executor_analysis.md` 的深入分析与改进方案

## 一、问题根因分析

分析文档指出 10 个主要不足。这些问题的根因可归纳为三个层次：

### 1.1 执行模型层面：名义流式、实际预物化

| 问题 | 根因 |
|------|------|
| streaming 未贯穿到输出 | `execute()` 返回 `Vec<DataChunk>`，外部统一转 `DataSet` |
| scan 预读全量 | builder 中直接 scan 存储层并收集为 `Vec<Vec<Value>>` |
| 并行框架串行化 | worker 持有全局 `Mutex<HashMap>`，所有 task 抢同一把锁 |
| schema/变量丢失 | `DataChunk::from_rows()` 首行推断列名 `col_N` |

核心矛盾：**算子内部用 pull 模型（next()），但数据准备阶段是全量 eager 模式**。

### 1.2 数据表示层面：行存 Value 向量，丢失编译期信息

- `DataChunk` 使用 `Vec<Vec<Value>>`，每行独立分配，缓存不友好
- 表达式求值逐行构造 `ValueRowContext`，每次 clone row + col_names
- schema 在运行时用字符串列名查找，planner 的变量绑定信息在 executor 中丢失
- HashJoin 的 hash key 使用 `format!("{:?}", row)` 而非 slot-based key

**这些问题本质上是同一件事：缺少从 planner 到 executor 的 slot layout 传递机制。**

### 1.3 资源治理层面：无内存预算、无 spill、无取消

- Sort/Aggregate/Join/HashJoin 全量收集内存，无上限
- 长操作（scan、sort、join、traversal）内部不检查取消信号
- storage cursor 生命周期不与 executor close 绑定
- DML 不与 transaction manager 集成

这三个层面互相依赖：数据表示决定了表达式求值方式，执行模型决定了并发粒度，资源治理决定了稳定性边界。

---

## 二、改进方案

### 2.1 执行模型收敛：先单线程 pull，再考虑并行

**当前并行框架是伪并行，建议先退回到单线程 pull 模型。**

当前状态：嵌套 `Box<StreamingExecutor>` pull 树 + task/worker/registry 并行框架，两套模型共存。

改进方案：

1. **移除 WorkerPool/StreamingExecutionEngine 的多线程路径，改为单线程递归 pull**。只在 `execute()` 中对 root executor 循环调用 `next()` 收集 chunk。
2. 保留 PartitionView 和 executor id 设计，但每个 partition 创建独立的 executor 树副本，单线程依次处理。
3. 等单线程 pull 模型的正确性验证充分后，再引入真正的 morsel-driven 并行。

这样做的好处：
- 消除全局锁瓶颈（当前 worker 抢 registry mutex）
- 简化 task 调度逻辑（当前 `build_tasks()` 的依赖关系粗糙）
- 减少状态不一致风险（两个 worker 共享 executor 的 child 指针）
- 保留扩展点：partition_id 字段保持不动，未来可按 partition 分发

**影响范围**：`engine.rs`、`worker.rs`、`scheduler.rs`、`builder.rs`

### 2.2 数据表示重构：Slot-based Row Layout

**核心变更：DataChunk 从运行时推断 schema 改为携带 planner 传入的稳定 schema。**

具体步骤：

1. **定义 SlotId**：
   ```rust
   pub struct SlotLayout {
       slots: Vec<SlotInfo>,
       name_to_slot: HashMap<String, usize>,
   }
   
   pub struct SlotInfo {
       pub name: String,
       pub slot_id: usize,
       pub data_type: ColumnType,  // 枚举而非字符串
   }
   ```

2. **DataChunk 携带 Arc<SlotLayout>**：
   - 由 builder 从 planner 的 PlanNode 输出 schema 构造
   - 贯穿整棵 executor 树，Filter/Project/Join 等算子输出时产生新的 SlotLayout
   - 消除 `from_rows()` 的首行推断和 `col_N` 命名

3. **表达式求值使用 slot_id 而非列名**：
   - `ValueRowContext` 持有 `&[Value]` 引用 + `&SlotLayout`
   - 按 slot_id 索引，O(1) 访问，无需 HashMap 查找

4. **HashJoin 使用 SlotLayout 中的 join key slot id**：
   - hash key 由指定 slot 的值计算，而非整个 row 的 Debug 字符串
   - probe 时比较对应 slot 的值

**依赖关系**：需要在 planner 中稳定输出每个 PlanNode 的 schema 信息，并传递到 builder。

### 2.3 Scan 节点改为游标拉取

当前 ScanVertices/ScanEdges 在 builder 中全量收集数据。改为：

```rust
ScanVerticesCursor {
    storage: StorageRef,
    scan_handle: ScanHandle,       // 存储层游标
    chunk_size: usize,             // 每次 next() 拉取行数
    partition_range: Option<Range>, // 可选分区范围
}
```

`open()` 创建游标，`next()` 最多拉取 chunk_size 行，支持 limit/filter pushdown。

**前提条件**：存储层需要支持游标式迭代器（而非全量返回 `Vec<>`）。需要验证存储层现有接口是否支持。

### 2.4 阻塞算子内存预算

当前 Sort/Aggregate/Join/HashJoin/WindowFunction/Intersect 等算子全量收集。

改进方向：

1. **估算接口**：每个阻塞算子在 `open()` 时估算最大内存
   ```rust
   fn estimate_memory(&self) -> usize;
   ```
2. **QueryMemoryBudget**：从 optimizer 传入每个 query 的内存上限
3. **上限检查**：`next()` 收集数据前检查 `estimate_memory() > budget`，超限返回错误
4. **预留 spill 接口**（当前不做实现，但预留 trait）：
   - `ExternalSort` trait
   - `SpillableHashAggregate` trait
   - `SpillableHashJoin` trait

### 2.5 Join 改进：基于 Slot 的 Equi-Hash-Join

1. Join key 改为 slot_id 列表（从 planner 传入）
2. Build side hash 改为 `HashMap<Vec<Value>, Vec<Vec<Value>>>`，不再用 Debug 字符串
3. Probe 时直接比较 slot 值
4. 增加 build side 内存上限检查
5. 清理冗余 variant：`InnerJoin`、`LeftJoin`、`RightJoin`、`CrossJoin` 与 `HashJoin`/`NestedLoopJoin` 语义重叠，建议统一

### 2.6 表达式执行优化

1. **消除逐行 clone**：ValueRowContext 改为引用 `&[Value]`（从 DataChunk 中借）
2. **预编译表达式为 slot-based 操作序列**：在 builder 中编译 Expression 为 `Vec<ExprOp>`，避免运行时递归求值
3. **统一错误处理**：表达式求值失败时返回明确错误（区分类型错误、字段未找到、函数参数不匹配），不静默吞掉

### 2.7 枚举拆分策略（可选重构）

当前 79 variant 的 StreamingExecutor enum + 4*79 = 316 match arms 是代码维护痛点。

**建议保持当前 enum 架构，但建立支持矩阵文档**（`docs/dev/operator_matrix.md`），明确每个 variant 的实现状态和测试覆盖。等所有 operator 稳定后，再考虑按业务域拆分为多个 enum（例如 `StreamingSource`、`StreamingTransform`、`StreamingSink`）。

### 2.8 统计与 PROFILE 基础设施

当前 `NodeExecutionStats` 等结构已定义，但 streaming operator 未记录。

改进：在 `next()` 调用中嵌入统计：

```rust
pub struct OperatorStats {
    pub open_time: Duration,
    pub next_calls: u64,
    pub rows_input: u64,
    pub rows_output: u64,
    pub peak_memory: usize,
    pub storage_read_rows: u64,
}
```

Stats 通过 `Arc<Mutex<OperatorStats>>` 共享，EXPLAIN ANALYZE 执行后读取。

### 2.9 取消与资源治理

1. 在 executor 的 `next()` 循环中插入 `check_cancelled()` 检查点
2. `stop()` 语义贯彻到阻塞算子：正在 collect 的算子收到 stop 后停止收集
3. Storage cursor 生命周期管理：`close()` 确保 cursor drop

---

## 三、阶段实施计划

### Phase 0：执行模型收敛（2-3 天）

**目标**：移除伪并行，建立稳定的单线程 pull 执行路径

| 步骤 | 内容 | 影响文件 |
|------|------|---------|
| 0.1 | 简化 `execute()` 为单线程递归 pull root executor | `engine.rs` |
| 0.2 | `WorkerPool` 标记为 `#[cfg(test)]` 或独立模块 | `worker.rs` |
| 0.3 | `PipelineScheduler` 直接返回 root executor 的 task | `scheduler.rs` |
| 0.4 | 清理 `executor_registry` 和 `task_to_executor_id` | `engine.rs`、`builder.rs` |
| 0.5 | 验证已有测试全部通过 | 现有 test |

### Phase 1：Slot Layout 建立（3-5 天）

**目标**：planner 的输出 schema 稳定传递到 executor，DataChunk 不再首行推断

| 步骤 | 内容 | 前置 |
|------|------|------|
| 1.1 | 定义 `SlotLayout` 结构，替换 schema 的字符串列名 | - |
| 1.2 | Planner 每个 PlanNode 输出时生成 SlotLayout | - |
| 1.3 | Builder 传入 SlotLayout 到 DataChunk | Phase 1.1 |
| 1.4 | 表达式求值改为 slot_id 访问 | Phase 1.1 |
| 1.5 | Join 算子使用 slot_id 做 hash key | Phase 1.4 |
| 1.6 | 删除 `DataChunk::from_rows()` 的首行推断 | Phase 1.3 |

### Phase 2：Scan 游标化（2-3 天）

**目标**：scan 算子不再预读全部数据

| 步骤 | 内容 | 前置 |
|------|------|------|
| 2.1 | 确认存储层游标接口 | - |
| 2.2 | 新增 `ScanVerticesCursor` variant（保留原 ScanVertices） | Phase 0 |
| 2.3 | Builder 根据存储接口选择 buffer 或 cursor 模式 | Phase 2.2 |
| 2.4 | 后续删除旧 buffer 模式 | Phase 2.3 稳定后 |

### Phase 3：内存预算与 Join 改进（3-4 天）

**目标**：阻塞算子可估算内存，Join 使用 slot-based key

| 步骤 | 内容 | 前置 |
|------|------|------|
| 3.1 | 实现阻塞算子的 `estimate_memory()` | Phase 1 |
| 3.2 | QueryMemoryBudget 从 optimizer 传入 executor | - |
| 3.3 | HashJoin 使用 slot_id hash key | Phase 1.5 |
| 3.4 | 清理重叠 Join variant | Phase 3.3 |

### Phase 4：可观测性（2-3 天）

**目标**：EXPLAIN ANALYZE 展示真实统计

| 步骤 | 内容 | 前置 |
|------|------|------|
| 4.1 | OperatorStats 嵌入每个算子 | Phase 0 |
| 4.2 | EXPLAIN ANALYZE 路由到 streaming executor | - |
| 4.3 | 展示 input/output rows、time、peak memory | Phase 4.1-4.2 |

### Phase 5：表达式优化与取消治理（2-3 天）

**目标**：消除逐行 clone，统一错误处理，取消链路完整

| 步骤 | 内容 | 前置 |
|------|------|------|
| 5.1 | ValueRowContext 改为引用 | Phase 1 |
| 5.2 | 统一表达式错误处理策略 | - |
| 5.3 | 阻塞算子加入 check_cancelled() | Phase 0 |
| 5.4 | Storage cursor 生命周期绑定 close() | Phase 2 |

---

## 四、与现有计划的依赖关系

| 现有计划 | 与本方案的关系 |
|----------|---------------|
| `executor_cleanup_plan.md` Phase 2（补充实现） | 建议先在单线程 pull 模型下补充实现，不要在本阶段引入并行 |
| `executor_cleanup_plan.md` Phase 3（删除旧模块） | 独立，可并行执行 |
| `streaming_operator_verify_plan.md` Phase B（存储集成） | 建议在 Phase 2 之后做，scan 游标化是存储集成的基础 |
| `phase3b_and_phase4_plan.md`（dead code） | 独立，可并行执行 |

**关键路径**：Phase 0 → Phase 1 → Phase 3 → Phase 4，其余可并行。

核心原则：**先做正确，再做快；先单线程，再并行。**
