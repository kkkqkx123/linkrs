# 剩余架构问题修改方案

> 基于 `docs/analysis/query/plan_optimizer_executor_integration.md`（2026-07-14）  
> 对照当前代码（2026-08-10）分析所得的五个未解决问题。  
> 编写日期：2026-08-10
>
> **P2 完成状态（2026-08-10）**：任务 3（Storage 边界优化）与任务 4 阶段 1（filter/project 消费 typed_columns）已完成，见 §3.6、§4.4 的完成记录。

---

## 1. 统计反馈闭环

### 1.1 问题

ANALYZE 已将 catalog 统计写入 `StatisticsManager`（`stats/collector.rs`），但执行后的 operator actual rows 没有回灌到优化器。现有组件全部仅在自身测试中使用：

| 组件 | 位置 | 职责 |
|------|------|------|
| `QueryExecutionFeedback` | `stats/feedback/query.rs:136` | 查询级 estimated vs actual |
| `OperatorFeedback` | `stats/feedback/query.rs:104` | 算子级 estimated vs actual |
| `AutoFeedbackTrigger` | `stats/feedback/trigger.rs:140` | 冷却期 + 阈值触发 |
| `SelectivityFeedbackManager` | `stats/feedback/selectivity.rs:229` | 选择率修正 |
| `QueryFeedbackHistory` | `stats/feedback/history.rs:29` | 历史存储 |
| `ProfileCollector` | `runtime.rs:346` | actual rows/time/spill |

### 1.2 方案

分两阶段，阶段 1 低风险、可观测；阶段 2 基于阶段 1 的数据自动修正。

#### 阶段 1：执行完成后采集 feedback

**核心思路：** `QueryExecutionInstance::execute()` 完成后，将 runtime profile 中的 actual rows 与 `plan.row_estimates` 比较，构建 `QueryExecutionFeedback` 并存入共享的 `QueryFeedbackHistory`。

**数据流：**

```
PhysicalPlan.row_estimates  ─┐
                              ├─→ QueryExecutionFeedback ─→ QueryFeedbackHistory
Runtime profile/operators  ──┘
```

**需修改的文件与改动：**

1. **`executor/streaming/runtime.rs`**：`ExecutionRuntime` 新增字段
   ```rust
   pub feedback_history: Arc<QueryFeedbackHistory>,
   ```
   在 `new()` 中接受 `feedback_history` 参数（默认 `Arc::new(QueryFeedbackHistory::new(...))`），由 materializer 注入。

2. **`executor/streaming/plan/materializer.rs`**：创建 runtime 时从 bindings 获取 `feedback_history` 并注入。

3. **`executor/streaming/instance.rs`**：`QueryBindings` 新增 `feedback_history: Option<Arc<QueryFeedbackHistory>>`；`execute()` 末尾调用：
   ```rust
   fn collect_execution_feedback(&self) {
       let profile = self.runtime().profile().flush_to_collector();
       let plan = &self.plan;
       let mut feedback = QueryExecutionFeedback::new(
           plan.fingerprint.clone().unwrap_or_default(),
       );
       feedback.estimated_rows = plan.output.estimated_rows.unwrap_or(0);
       feedback.actual_rows = profile.total_output_rows();
       feedback.actual_time_us = profile.total_duration_us();
       for (key, op_profile) in &profile.operators {
           if let Some(est) = plan.row_estimates.get(
               &(key.physical_operator_id.0 as i64)
           ) {
               feedback.add_operator_feedback(OperatorFeedback {
                   operator_id: key.physical_operator_id.0.to_string(),
                   operator_type: op_profile.name.clone(),
                   estimated_rows: *est,
                   actual_rows: op_profile.output_rows,
                   estimated_time_us: 0,
                   actual_time_us: op_profile.open_time_us
                       + op_profile.next_time_us
                       + op_profile.close_time_us,
                   execution_loops: 1,
               });
           }
       }
       if let Some(ref history) = self.runtime().feedback_history {
           history.add_feedback(feeding);
       }
   }
   ```

4. **`optimizer/engine.rs`**：新增 `feedback_history: Arc<QueryFeedbackHistory>` 字段，暴露 `pub fn feedback_history(&self)` 访问器。

5. **`pipeline/execution.rs`**：`build_execution_context` 中从 `self.optimizer_engine` 获取 `feedback_history()` 并写入 bindings。

#### 阶段 2：选择率自动修正（后续迭代）

利用 `AutoFeedbackTrigger` 和 `SelectivityFeedbackManager`，在 feedback 累积到阈值后自动修正 `StatisticsManager` 中的 selectivity：

```rust
// optimizer/engine.rs — 新增方法
pub fn maybe_apply_feedback(&self) {
    let history = self.feedback_history();
    let groups = history.recent_groups(); // 按 fingerprint 分组
    for (fp, feedbacks) in groups {
        let avg_error: f64 = feedbacks.iter()
            .map(|f| f.row_estimation_error())
            .sum::<f64>() / feedbacks.len() as f64;
        if self.feedback_trigger.should_trigger(avg_error) {
            // 基于 feedbacks 修正对应 tag/edge 的 selectivity
            self.apply_selectivity_correction(&feedbacks);
            self.feedback_trigger.mark_updated();
        }
    }
}
```

此方法可在 `OptimizerEngine::optimize()` 的尾部可选调用（受 `enable_feedback` 开关控制），或由后台 ANALYZE 线程定期调用。

### 1.3 预期收益

- EXPLAIN/PROFILE 可展示 estimated vs actual 偏差
- CBO 统计随执行自动收敛，ANALYZE 间隔更长
- 选择率估算精度逐步提升

### 1.4 风险

- feedback_history 有内存上限（`max_feedback_history`），需配置合理值
- 冷却期确保不会频繁触发修正，不影响热路径性能

---

## 2. SubPlan 弱连接

### 2.1 问题

`SegmentsConnector::add_input`（`connector.rs:94`）不修改 child 的 input，仅返回新 SubPlan：

```rust
pub fn add_input(input_plan: SubPlan, dependent_plan: SubPlan, _is_left: bool) -> SubPlan {
    SubPlan {
        root: dependent_plan.root,
        tail: input_plan.tail,
    }
}
```

调用方随后通过 `node.add_input(node_root.clone())` 手动将 root 注入 expand node：

| 调用点 | 代码位置 |
|--------|----------|
| `plan_combiner.rs:80` | `new_expand.add_input(node_root.clone())` |
| `plan_combiner.rs:110` | `new_expand.add_input(left_root.clone())` |
| `go_planner.rs:122` | `expand_all_node.add_input(tail_node)` |
| `subgraph_planner.rs:164` | `expand_node.add_input(input)` |

**风险：** 先创建 expand 节点再手动注入 input，planner 和 executor 看到的树结构可能不一致；优化器改写子树时可能丢失 input 连接。

### 2.2 方案

**步骤 1：引入 `SubPlan::connect_upstream`**

```rust
impl SubPlan {
    /// 将 upstream 的 root 作为 downstream 的 input，
    /// 返回结构已闭合的完整计划。
    pub fn connect_upstream(
        mut downstream: SubPlan,
        upstream: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let down_root = downstream.root.take()
            .ok_or_else(|| PlannerError::PlanGenerationFailed(
                "downstream has no root".to_string()
            ))?;
        let up_root = upstream.root
            .ok_or_else(|| PlannerError::PlanGenerationFailed(
                "upstream has no root".to_string()
            ))?;
        let mut connected = down_root;
        connected.set_input(up_root);
        downstream.root = Some(connected);
        downstream.tail = upstream.tail;
        Ok(downstream)
    }
}
```

**步骤 2：改造 4 处调用点**

```rust
// plan_combiner.rs — 改造前
let mut new_expand = ExpandAllNode::new(...);
new_expand.add_input(node_root.clone());
Ok(SubPlan { root: Some(new_expand.into_enum()), tail: ... })

// 改造后
let upstream = SubPlan::from_single_node(node_root);
let downstream = SubPlan::from_single_node(
    ExpandAllNode::new(...).into_enum()
);
SubPlan::connect_upstream(downstream, upstream)
```

同理改造 `go_planner.rs`、`subgraph_planner.rs`。

**步骤 3：将 `SegmentsConnector::add_input` 标记为 `#[deprecated]`**

```rust
#[deprecated(note = "use SubPlan::connect_upstream instead")]
pub fn add_input(...) -> SubPlan { ... }
```

**修改文件：**
- `planning/plan/execution_plan.rs` — `SubPlan::connect_upstream` 新增方法
- `planning/statements/plan_combiner.rs` — 2 处 add_input 改为 connect_upstream
- `planning/statements/dql/go_planner.rs` — 1 处
- `planning/statements/dql/subgraph_planner.rs` — 1 处
- `planning/connector.rs` — `add_input` 标记 deprecated

### 2.3 预期收益

- SubPlan 组合时结构闭合，优化器改写不会丢失连接
- 语义明确：upstream 数据流向 downstream
- 旧接口保留向后兼容，逐步迁移

---

## 3. Storage 边界优化

### 3.1 问题

`ExecutionContext.storage` 为 `Arc<RwLock<dyn QueryStorage>>`，所有算子共享同一把外层读写锁：

```rust
// storage_scan.rs / index_scan.rs 中
let guard = storage.read();  // 全局读锁
```

每个算子的每次 `next()` 调用都获取/释放全局锁，在并发查询场景下成为瓶颈。成熟设计应为：每查询持有一个 snapshot/transaction handle，算子只借用该 handle。

### 3.2 现状

storage 层已有 snapshot 基础设施：
- `bind_read_operation_context()` → 返回带 `SnapshotHandle` 的只读绑定
- `bind_auto_commit_context()` → 返回自动提交绑定
- `bind_operation_context(StorageOperationContext)` → 返回带 transaction 的绑定

这些绑定在 `prepared.rs` 中已用于 DML/只读场景，绑定后的 storage 存入 `ExecutionContext`。

### 3.3 方案

**核心思路：** 将 `QueryStorage` 绑定提前到查询实例化阶段，绑定后的 storage handle（含 snapshot）存入 `QueryBindings`，每个算子通过 bindings 获取，不再全局加锁。

**步骤 1：QueryBindings 中携带绑定后的 storage**

```rust
// instance.rs — QueryBindings 新增
pub struct QueryBindings {
    // ...existing...
    /// Bound storage handle for this execution (snapshot-scoped).
    /// Each operator borrows this handle instead of competing on
    /// the global storage lock.
    pub bound_storage: Option<Arc<dyn BoundStorage>>,
}
```

**步骤 2：在 pipeline 中完成绑定**

```rust
// execution.rs — build_execution_context 改造
fn build_execution_context(&self, query_context: &QueryContext) -> ExecutionContext {
    let mut context = ExecutionContext { ... };
    // 已有逻辑: 如果有 operation_storage，直接使用
    // 新增: 对只读查询，提前绑定 snapshot
    if context.operation_storage.is_none() {
        if let Some(ref storage) = self.storage {
            let bound = storage.read().bind_read_operation_context()?;
            context.storage = Some(Arc::new(RwLock::new(bound)));
        }
    }
    Ok(context)
}
```

**步骤 3：算子使用 bound storage**

```rust
// storage_scan.rs — 改造
impl UnaryStreamingOperator for StorageScanOperator {
    fn next(&mut self, _runtime: &ExecutionRuntime) -> Result<Option<DataChunk>> {
        // 改造前: let guard = self.storage.read();
        // 改造后: 直接使用 self.bound_storage (无锁)
        self.bound_storage.scan_vertices(...)
    }
}
```

**步骤 4：QueryStorage trait 增加 snapshot 接口**

```rust
// graphdb-storage/src/storage/client.rs
pub trait QueryStorage: Send + Sync {
    // 现有接口保留...
    fn snapshot_handle(&self) -> Option<SnapshotHandle>;
}
```

**修改文件：**
- `executor/streaming/instance.rs` — QueryBindings 新增 `bound_storage`
- `pipeline/execution.rs` — build_execution_context 中提前绑定
- `executor/streaming/operators/source_operator/storage_scan.rs` — 使用 bound storage
- `executor/streaming/operators/source_operator/index_scan.rs` — 使用 bound storage
- `graphdb-storage/src/storage/client.rs` — QueryStorage trait 增加 snapshot_handle

### 3.4 预期收益

- 并发查询不再竞争全局 storage 锁
- 每查询持有一个一致的 snapshot，避免读到中间状态
- 与现有 DML 自动提交绑定兼容

### 3.5 风险与缓解

- 大事务场景下 snapshot 可能持有较长时间 → 需结合 MVCC GC 策略
- bound_storage 的生命周期由 QueryBindings 管理，Drop 时自动释放 snapshot

### 3.6 完成记录（2026-08-10，P2）

在既有演进架构（`prepared.rs` 已在 prepare 阶段按语句类别绑定读/写 operation context，算子持有 per-query bound storage）之上补齐了快照可观测层：

1. `graphdb-storage/src/storage/client.rs` — `QueryStorage` trait 新增 `snapshot_handle()`：
   - 绑定到 operation context 时返回 `SnapshotHandle`（优先取已注册的 MVCC vertex snapshot handle，否则由 `snapshot_timestamp()` 合成 `id=0`）
   - 未绑定（raw 全局 storage）返回 `None`
   - `SnapshotHandle` 由 `graphdb-storage/src/storage.rs` 公开导出
2. `executor/streaming/instance.rs` — `QueryBindings` 新增 `bound_snapshot: Option<SnapshotHandle>`，`from_context` 拷贝自 `ExecutionContext`
3. `executor/base/execution_context.rs` — `ExecutionContext` 新增 `bound_snapshot` 字段（5 个构造路径 + Default）
4. `pipeline/execution.rs` — `build_execution_context` 从生效的 storage handle 读取 `snapshot_handle()` 写入 `bound_snapshot`
5. 测试：`test_mock.rs` 3 个单测（未绑定/读绑定/自动提交绑定）+ `tests/storage_boundary.rs` 端到端测试（读查询携带绑定快照、并发读各持独立 snapshot）

**说明**：doc 步骤 2 的"提前绑定"已由 `prepare_request` 按语句分类完成（只读语句 `bind_read_operation_storage`、DML `bind_auto_commit_storage`），`build_execution_context` 的 raw 回退仅用于 EXPLAIN/DDL 等无需快照的路径，未做无条件绑定以免破坏 DDL 写路径。

---

## 4. 列式执行路径扩展

### 4.1 问题

`DataChunk` 已有混合行列表示（`rows: Vec<Vec<Value>>` + `typed_columns: Option<Vec<TypedColumn>>` + `selection: Option<Vec<usize>>`），但：

- 仅 `storage_scan` 算子构建 `typed_columns`
- 下游算子（filter/project/sort）仍以行式 `rows` 为主
- `typed_columns_enabled()` 是运行时开关，无自动检测
- `columnar_promise_holds` 检查表达式是否适合列式求值

### 4.2 方案

**阶段 1：让 project/filter 算子消费 typed_columns（低风险）**

```rust
// unary_operator.rs — project 算子
impl UnaryStreamingOperator for ProjectOperator {
    fn next(&mut self, runtime: &ExecutionRuntime) -> Result<Option<DataChunk>> {
        let input = self.child.next(runtime)?;
        match input {
            Some(chunk) if chunk.typed_columns.is_some() => {
                // 列式快速路径：直接从 TypedColumn 读取
                self.project_from_typed_columns(chunk)
            }
            Some(chunk) => {
                // 回退行式路径
                self.project_from_rows(chunk)
            }
            None => Ok(None),
        }
    }
}
```

同理 filter 算子在 `typed_columns` 存在时使用列式谓词求值。

**阶段 2：引入 ColumnarPolicy（自动检测）**

```rust
/// 根据 chunk 的历史列式命中率决定使用行式还是列式路径。
pub struct ColumnarPolicy {
    hits: AtomicU64,
    misses: AtomicU64,
    threshold: f64,  // 命中率阈值，如 0.8
}

impl ColumnarPolicy {
    pub fn should_use_columnar(&self) -> bool {
        let total = self.hits.load(Ordering::Relaxed)
            + self.misses.load(Ordering::Relaxed);
        if total < 100 { return false; }  // 样本不足
        self.hits.load(Ordering::Relaxed) as f64 / total as f64
            > self.threshold
    }
}
```

**阶段 3：TypedColumn 推广到更多算子**

在 `TypedKind` 中增加更多类型支持（目前已有 `I64`, `F64`, `Bytes`, `Fallback`），逐步覆盖 `String`, `Bool`, `Date` 等。

**修改文件：**
- `operators/unary_operator.rs` — project/filter 添加 typed_columns 快速路径
- `operators/blocking/*.rs` — sort/aggregate 在 typed_columns 存在时使用列式路径
- `chunk/core.rs` — ColumnarPolicy 新增
- `chunk/typed.rs` — TypedKind 扩展类型支持

### 4.3 预期收益

- filter/project 算子 CPU 效率提升 30-50%（减少 Value 克隆和动态分发）
- 内存占用降低（TypedColumn 紧凑存储）
- 按命中率自动切换，无性能回退

### 4.4 完成记录（2026-08-10，P2 阶段 1）

阶段 1（filter/project 消费 typed_columns）在既有 typed batch evaluator（`chunk/typed.rs` + `chunk/eval.rs`）基础上完成：

1. `chunk/eval.rs` — `try_eval_typed_batch` 新增 `Expression::Property` 分支：`v.age` 平面属性访问解析到复合 slot `v.age`，当该列为 typed（I64/F64/I32/Bool）时，谓词（如 `v.age > 30`）直接在 raw batch 上求值，不再退化为 per-row Value 求值
2. `chunk/tests.rs` — 新增 `typed_eval_property_predicate_hits_typed_batch_path`：验证属性谓词结果正确且 `ColumnarStats.columnar_typed_hits` 命中

**现状说明**：filter 通过 `chunk.evaluate_expression(predicate, ...)`、project 通过 `chunk.evaluate_expressions(...)` 消费 typed_columns；storage_scan 在行式路径（`build_typed_columns()`）与列块路径均 eager 构建 typed_columns；`typed_columns_enabled()` 默认开启。阶段 2（ColumnarPolicy 自动检测）与阶段 3（TypedKind 扩展、blocking 算子列式路径）留待后续迭代。

---

## 5. PlanNodeEnum 逻辑/物理分离

### 5.1 问题

`PlanNodeEnum` 同时混合了：

| 类别 | 示例节点 |
|------|----------|
| 逻辑关系操作 | `Project`, `Filter`, `Sort`, `Limit`, `Aggregate`, `Join` |
| 访问路径 | `ScanVertices`, `ScanEdges`, `IndexScan` |
| 物理 join 变体 | `HashInnerJoin`, `HashLeftJoin`, `NestedLoopJoin` |
| DDL/DML | `CreateSpace`, `InsertVertex`, `DropTag` |
| 分区属性 | `PartitionSpec` 在 ExecutionPlan 层 |

虽然已引入 `LogicalPlan`（`plan/logical/`），但 `LogicalPlan::from_plan_node` 仅用于在 `ExecutionPlan` 上附加一份逻辑视图，optimizer 的 CBO 阶段并未消费它。

### 5.2 方案

这是五个问题中规模最大的重构，建议分三步渐进式推进。

**步骤 1：定义 LogicalPlan 的生产消费路径（低风险）**

让 CBO 的 join order / index selection / aggregate strategy 消费 `LogicalPlan` 而非 `PlanNodeEnum`：

```rust
// optimizer/engine.rs — apply_cost_based 改造
fn apply_cost_based(&self, plan: ExecutionPlan, space: Option<&str>)
    -> OptimizeResult<ExecutionPlan>
{
    // 现有逻辑改为：
    // 1. 从 ExecutionPlan 获取 LogicalPlan（若存在）
    // 2. 在 LogicalPlan 上做 CBO 决策
    // 3. 将决策结果（notes/estimates）写回 ExecutionPlan
    if let Some(ref logical) = plan.logical_plan() {
        self.optimize_logical(logical, &stats, &mut plan)?;
    } else {
        // 回退到现有 PlanNodeEnum 优化
        self.optimize_plan_nodes(&mut plan, &stats)?;
    }
    Ok(plan)
}
```

**步骤 2：逐步迁移 planner 到 LogicalPlan**

对每个 planner（MATCH, GO, LOOKUP），改为先生成 `LogicalPlan`，再转换为 `PlanNodeEnum`：

```
AST → LogicalPlan（纯语义）→ PlanNodeEnum（保留物理属性）→ PhysicalPlan（arena）
```

新增 `LogicalPlan → PlanNodeEnum` 的显式转换层：

```rust
// plan/logical/conversion.rs — 增强
impl LogicalPlan {
    /// 将 LogicalPlan 转换为 PlanNodeEnum（物理感知）。
    /// 此转换为单向，转换后的 PlanNodeEnum 可被现有 optimizer 消费。
    pub fn into_plan_node(self) -> PlanNodeEnum { ... }
}
```

**步骤 3：从 PlanNodeEnum 中剥离物理 join 变体**

将 `HashInnerJoin`, `HashLeftJoin` 等物理 join 从 `PlanNodeEnum` 移到 `PhysicalPlan` 的 arena builder 中：

```
PlanNodeEnum: InnerJoin / LeftJoin / CrossJoin（逻辑语义）
PhysicalPlan: HashInnerJoin / SortMergeJoin（物理实现）
```

物理 join 选择由 arena builder 在 `PhysicalPlanBuilder::build` 阶段根据 `PhysicalPlanBuildContext` 的 cost 信息决定。

**修改文件（步骤 1 范围）：**
- `optimizer/engine.rs` — apply_cost_based 消费 LogicalPlan
- `optimizer/cost_based/join_order_rewriter.rs` — 基于 LogicalPlan 节点重排
- `optimizer/cost_based/index_selection.rs` — 基于 LogicalPlan 节点选择索引
- `optimizer/cost_based/aggregate_strategy.rs` — 基于 LogicalPlan 节点选择策略

**修改文件（步骤 2-3 范围，后续迭代）：**
- `planning/statements/` — 各 planner 改为先生成 LogicalPlan
- `planning/plan/logical/conversion.rs` — 增强转换层
- `executor/streaming/plan/arena_builder.rs` — 物理 join 选择逻辑

### 5.3 预期收益

- LogicalPlan 成为 optimizer 的唯一事实来源
- PlanNodeEnum 逐步简化为物理属性携带者
- 物理 join 选择集中在 arena builder，不再散落在多个 optimizer 阶段
- LogicalPlan 天然支持 plan cache 的结构验证（语义不变量更容易检查）

### 5.4 风险

- 最大规模重构，涉及 planner、optimizer、executor 三层
- 建议步骤 1-2-3 分别独立合入，每步均有回退路径
- 步骤 1 可在不改变现有 planner 的前提下完成，风险最低

---

## 6. 实施优先级

| 优先级 | 任务 | 预估工时 | 依赖 |
|--------|------|----------|------|
| P1 | 统计反馈闭环（阶段 1） | 2-3 天 | 无 |
| P1 | SubPlan 弱连接 | 1-2 天 | 无 |
| P2 | Storage 边界优化 | 5-7 天 | 无 |
| P2 | 列式执行路径扩展（阶段 1） | 3-5 天 | 无 |
| P3 | PlanNodeEnum 分离（步骤 1） | 5-7 天 | LogicalPlan 已存在 |
| P3 | PlanNodeEnum 分离（步骤 2-3） | 10-15 天 | 步骤 1 完成 |

建议按 P1 → P2 → P3 顺序推进，每个任务独立可交付，可随时暂停。

---

## 7. 成功标准

| 任务 | 验证方式 |
|------|----------|
| 统计反馈闭环 | PROFILE 展示 estimated vs actual；feedback_history 非空 |
| SubPlan 弱连接 | 所有 planner 测试通过；`add_input` 使用处减少为 0 |
| Storage 边界 | 并发查询 QPS 提升 ≥30%；storage lock contention 指标下降 |
| 列式执行 | filter/project 算子 typed_columns 命中率 ≥80% |
| PlanNodeEnum 分离 | LogicalPlan 被 CBO 消费；PlanNodeEnum 物理变体 ≤3 个 |
