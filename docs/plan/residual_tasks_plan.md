# 剩余任务修改方案

> 基于 `docs/plan/residual_issues.md`（2026-08-10）中标记为"后续迭代 / 未实施"的任务，
> 对照当前代码（2026-08-10）逐项分析的实施方案。
> 编写日期：2026-08-10
> 进度更新：2026-08-11（任务 1 阶段 2、任务 4 阶段 2、任务 5 步骤 3、任务 4 阶段 3 步骤 A 已完成）

---

## 0. 任务总览

| 来源 | 任务 | 规模 | 前置依赖 | 状态 |
|------|------|------|----------|------|
| §1.2 阶段 2 | 统计反馈闭环：选择率自动修正 | 小（2-3 天） | 无（阶段 1 已完成） | ✅ 已完成（§1.5） |
| §4.2 阶段 2 | ColumnarPolicy 自动检测 | 小（1-2 天） | 无 | ✅ 已完成（§2.5） |
| §4.2 阶段 3 | TypedKind 扩展 + blocking 算子列式 | 大（10-15 天） | 无（可最后做） | 🔄 步骤 A 完成；步骤 B/C 待做（§3.5） |
| §5.2 步骤 2 | planner 迁移到 LogicalPlan | 中（5-7 天） | 无 | 🔄 部分完成：GO/Lookup 已迁移；match/pattern 待做（§4.5） |
| §5.2 步骤 3 | 从 PlanNodeEnum 剥离物理 join 变体 | 大（7-10 天） | 步骤 2 | ✅ 已完成（§5.5） |

建议实施顺序：任务 1 阶段 2 → 任务 4 阶段 2 → 任务 5 步骤 2 → 任务 5 步骤 3 → 任务 4 阶段 3。

---

## 1. 统计反馈闭环：选择率自动修正（任务 1 阶段 2）

### 1.1 现状与缺口

阶段 1 已闭环：`QueryFeedbackHistory` 按 fingerprint 保存 estimated vs actual，
`AutoFeedbackTrigger`（`optimizer/stats/feedback/trigger.rs:140`，冷却期 + 阈值）与
`SelectivityFeedbackManager`（`optimizer/stats/feedback/selectivity.rs:229`，EWMA 修正）均已实现，
但均无生产调用点。

修正要落到"具体谓词"或"edge type"上，当前存在三个数据缺口：

1. **feedback 缺语义键**：`OperatorFeedback` 只有 operator_id/type/estimated/actual rows，
   无法知道偏差来自哪个谓词。
2. **缺 space 信息**：`QueryExecutionFeedback` 未携带 space 名，修正统计时无法定位到具体 space。
3. **消费端未接线**：`SelectivityEstimator::estimate_from_expression`
   （`optimizer/cost/selectivity.rs:237`）完全不查 feedback。

### 1.2 方案

**核心思路：** 执行反馈（阶段 1 数据）→ 提取"谓词级修正因子" → 写入
`SelectivityFeedbackManager` → `SelectivityEstimator` 在估计前优先查询修正值。

```
OperatorFeedback(estimated/actual) ─┐
                                    ├─→ 修正因子 = actual_rows/estimated_rows
FilterSpec.predicate 表达式 ────────┘      │
                                          ↓
                    SelectivityFeedbackManager (按规范化谓词 key)
                                          │
        SelectivityEstimator.estimate_from_expression ←─ 优先查 key
```

**需修改的文件与改动：**

1. **`optimizer/stats/feedback/query.rs`**：
   - `OperatorFeedback` 新增 `condition_key: Option<String>`（规范化谓词 key）
   - `QueryExecutionFeedback` 新增 `space: Option<String>`

2. **`executor/streaming/instance.rs`**：`collect_execution_feedback()` 扩展：
   - 遍历 plan 算子，遇到 `FilterSpec`（`operators/spec.rs:171` 的 `predicate: Expression`）
     时提取谓词，字符串化 + 规范化生成 `condition_key`
   - 从 bindings 取 space 名写入 feedback

3. **`optimizer/engine.rs`**：新增字段
   ```rust
   selectivity_feedback: Arc<SelectivityFeedbackManager>,
   feedback_trigger: AutoFeedbackTrigger,
   enable_feedback: bool,   // 配置开关
   ```
   并实现（文档 §1.2 伪码的落地版）：
   ```rust
   pub fn maybe_apply_feedback(&self) {
       let history = self.feedback_history();
       for fp in history.get_all_fingerprints() {
           let Some(avg_error) = history.get_avg_row_error(&fp) else { continue };
           if !self.feedback_trigger.should_trigger(avg_error) { continue; }
           let feedbacks = history.get_feedback_for_query(&fp);
           for f in &feedbacks {
               for op in &f.operator_feedbacks {
                   if let Some(key) = &op.condition_key {
                       // 修正因子 = actual/estimated，经 EWMA 平滑后写入
                       let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                       self.selectivity_feedback.update_feedback(key, ratio);
                   }
               }
           }
           self.feedback_trigger.mark_updated();
       }
   }
   ```
   - 在 `optimize()` 开头可选调用（受 `enable_feedback` 控制）

4. **`optimizer/cost/selectivity.rs`**：`SelectivityEstimator` 注入 `Arc<SelectivityFeedbackManager>`：
   - `estimate_from_expression` / `estimate_binary_expression` 入口先按表达式规范化 key
     查 `get_corrected_selectivity(key)`
   - 有命中则直接用修正值，未命中走现有直方图/默认估算
   - key 注册发生在估算侧（天然拿到 estimated 值），即 estimate 命中时回调
     `register_condition(key, estimated)`，之后用 `update_feedback` 持续修正

5. **失效策略**：`StatisticsManager::mark_space_dirty`（`stats/manager.rs:62`）时
   清空该 space 前缀的 feedback key，避免 schema 变更后误修正。

**实现注意：**
- 谓词规范化 key：复用表达式字符串化 + 排序参数（参考 `stats/feedback/fingerprint.rs`
  的 normalize 思路），如 `v.age > 30`
- 冷却期沿用 `AutoFeedbackTrigger`（trigger.rs 已有 `should_trigger`/`mark_updated`/冷却配置）
- `max_feedback_history`（trigger.rs:30，默认 100）保证内存上限

### 1.3 验证方式

- 单测：模拟 history 中 estimated=100/actual=10 的 filter 反馈 →
  `get_corrected_selectivity("v.age > 30")` 返回显著小于原估计值
- 集成：跑两遍相同查询（数据不变），第二遍 EXPLAIN 的 filter 估计行数向实际收敛
- 回归：`mark_space_dirty` 后修正值清空

### 1.4 风险

- feedback 与谓词一对多（同一谓词出现在不同查询）→ key 规范化后自然聚合
- 修正过度 → `FeedbackDrivenSelectivity` 已有 min/max correction 夹取（selectivity.rs:97-100）
- 热路径影响 → 仅在 `optimize()` 入口调用，且受开关控制

### 1.5 进度（2026-08-11）

**已完成：**

- `stats/feedback/query.rs`：`OperatorFeedback.condition_key`、
  `QueryExecutionFeedback.space` 扩展
- `stats/feedback/selectivity.rs`：`update_with_ratio`/`update_feedback_ratio`/
  `remove_feedback_by_space`；`register_condition` 改为 entry 插入（不覆盖已学修正）
- `cost/selectivity.rs`：`condition_key()`、`feedback` 字段 + `with_feedback()`
  构造器；`estimate_from_expression` 优先查修正、未命中时注册
- `optimizer/engine.rs`：`selectivity_feedback`/`feedback_trigger`/`enable_feedback`
  字段、`maybe_apply_feedback()`（optimize() 入口，开关受控）、
  `invalidate_space_feedback()`；执行反馈通过 `collect_execution_feedback`
  携带 space + Filter 谓词 condition_key
- `pipeline.rs` / `pipeline/prepared.rs`：force stats / DDL 后调用
  `invalidate_space_feedback` 清空修正
- 测试：`test_feedback_loop_corrects_selectivity`、
  `test_feedback_loop_respects_enable_switch`（engine.rs）

---

## 2. ColumnarPolicy 自动检测（任务 4 阶段 2）

### 2.1 现状

- `ColumnarStats`（`runtime.rs:108`）已有 typed hits/misses 计数器；chunk 经
  `attach_columnar_stats`（`source_operator/util.rs:154`）携带 `Arc<ColumnarStats>`；
  `unary_operator.rs:209` 已记录命中
- 决策仍是全局开关 `typed_columns_enabled()`（`chunk.rs:62`），消费点在两处：
  `chunk/core.rs:343`（构建 typed_columns 门控）、`storage_scan.rs:359,506`
- **关键限制**：`ColumnarStats` 是 per-runtime（per 查询）创建，跨查询不累积，无法学习

### 2.2 方案

**核心思路：** 将门控从"全局开关"升级为"跨查询共享的命中率策略"，全局开关保留为强制覆盖。

```
ExecutionRuntime ── Arc<ColumnarPolicy>(hits/misses/threshold) ──┐
         │                                                       │
   per-query ColumnarStats（已有）──查询结束──→ 快照并入 policy（跨查询累积）
```

**需修改的文件与改动：**

1. **`chunk/core.rs`（或新增 `chunk/policy.rs`）**：
   ```rust
   /// 根据历史列式命中率决定使用行式还是列式路径（跨查询共享）。
   pub struct ColumnarPolicy {
       hits: AtomicU64,
       misses: AtomicU64,
       threshold: f64,   // 命中率阈值，如 0.8
       min_samples: u64, // 样本下限，如 100
   }

   impl ColumnarPolicy {
       pub fn should_use_columnar(&self) -> bool {
           let total = self.hits.load(Ordering::Relaxed)
               + self.misses.load(Ordering::Relaxed);
           if total < self.min_samples { return true; }  // 样本不足默认列式
           self.hits.load(Ordering::Relaxed) as f64 / total as f64 > self.threshold
       }
       pub fn record(&self, hit: bool) { /* Relaxed 原子累加 */ }
       pub fn snapshot(&self) -> (u64, u64);
   }
   ```
   - 样本不足时默认开启（与当前"默认开启"行为一致，无性能回退）

2. **`runtime.rs`**：`ExecutionRuntime` 持有 `Arc<ColumnarPolicy>`；`ColumnarStatsSnapshot`
   （runtime.rs:449）已可导出 per-query 统计，在 `flush_to_collector` 或查询收尾时并入 policy

3. **`pipeline/execution.rs` / `materializer.rs`**：创建 runtime 时注入 `Arc<ColumnarPolicy>`
   （由 OptimizerEngine 或 QueryEngine 持有，跨查询共享）

4. **替换两处门控**：
   ```rust
   // storage_scan.rs:359,506 与 chunk/core.rs:343
   if typed_columns_enabled()
       && policy.map_or(true, |p| p.should_use_columnar())
   ```
   - 全局开关保留为强制覆盖（`chunk/tests.rs:350` 的 `set_typed_columns_enabled(false)` 依赖它）

5. **避免抖动**：不要在逐 chunk 粒度决策。建议算子 `open` 时读取一次决策
   （或每 N 个 chunk 重评估），policy 内可用 CacheLine 对齐避免伪共享。

### 2.3 验证方式

- 单测：`ColumnarPolicy` 阈值/样本下限逻辑；模拟高回退率 → `should_use_columnar()` 翻转为 false
- 集成：强制制造 Fallback 列（混合类型），验证后续查询自动切行式；再恢复 typed 验证切回
- 回归：现有 `columnar_stats_record_hits_and_misses` 等测试（`chunk/tests.rs:116`）不受影响

### 2.4 风险

- 决策滞后：命中率反转需要 min_samples 个样本 → 阈值保守（0.8）即可
- 热路径原子开销：Relaxed 原子 + 非逐 chunk 决策，可忽略

### 2.5 进度（2026-08-11）

**已完成：**

- 新增 `chunk/policy.rs`：`ColumnarPolicy`（hits/misses 原子计数，阈值 0.8、
  min_samples 100，`should_use_columnar()` 决策，`use_columnar_path()` 读取）
- `runtime.rs`：`ExecutionRuntime.columnar_policy` 字段 +
  `flush_columnar_stats_to_policy()`（查询结束将 per-query `ColumnarStats`
  快照并入跨查询共享 policy）
- `chunk/core.rs`：`build_typed_columns(use_columnar: bool)` 签名变化
- `storage_scan.rs`：行式/顶点列/边列三处扫描门控统一走 `use_columnar_path()`
- 注入链：engine（Arc<ColumnarPolicy>）→ `execution_context.rs`（5 处初始化）
  → `instance.rs` `QueryBindings` → `materializer.rs`；
  execute()/into_stream()（仅 on_drop 触发，避免持有指针竞态）/execute_discard()
  结束收尾合并统计
- 测试：`test_columnar_policy_flush_merges_into_shared_policy`、
  `test_columnar_policy_gates_typed_columns`（runtime.rs）

---

## 3. TypedKind 扩展 + blocking 算子列式（任务 4 阶段 3）

### 3.1 现状

- `TypedKind`：I64/F64/I32/Bool/Date/Utf8（`chunk/typed.rs`）；`TypedColumn`/`TypedBatch`
  与之对应（步骤 A 已完成，见 §3.5）
- sort/aggregate/window 全部行式缓冲：`sort.rs:21` 的 `all_rows: Vec<Vec<Value>>`；
  spill 也基于行（`spill_sorted_run`）

### 3.2 方案

按投入产出排序分三步，每步独立可交付：

**步骤 A：TypedKind 扩展（低成本）**

```rust
pub enum TypedKind {
    I64, F64, I32, Bool,
    Date,             // 内部存 i64 天数，复用现有数值求值路径
    Utf8,             // Vec<Arc<str>>，免 Value 装箱
}
```

- `TypedColumn::Date(Vec<i64>)`、`TypedColumn::Utf8(Vec<Arc<str>>)`；
  `value_at`/`to_values`/`estimated_size`/`typed_column_batch` 同步扩展
- 字符串批量比较（等值/字典序）在 raw 层做，减少 Value 构造
- 构建端（`build_typed_columns`，`storage_scan.rs`）按列类型映射到新变体
- 修改文件：`chunk/typed.rs`、`chunk/eval.rs`（求值器新增 Date/Utf8 分支）、
  `chunk/core.rs`（构建映射）、`chunk/tests.rs`

**步骤 B：sort/TopN 列式（收益最大）**

- 新增 `ColumnarBatch` 累积结构：`Vec<TypedColumn>` + 投影索引（投影在 typed 列上做）
- sort 算子：按 typed key 列求 permutation 排序，排序完再物化行输出
- spill 边界以下全列式、以上转行式走现有 `spill_sorted_run`
  （spill 序列化必须保留行式，不侵入现有 spill 机制）
- 修改文件：`operators/blocking/sort.rs`、新增 `chunk/columnar_batch.rs`

**步骤 C：aggregate/window 列式**

- aggregate：group-by key 用 typed 列哈希；sum/count/avg/min/max 对 I64/F64 直接批量聚合
- window：依赖排序键，在步骤 B 完成后做
- 修改文件：`operators/blocking/aggregate.rs`、`operators/blocking/window.rs`

**实现注意：** `evaluate_expression_per_row`（`chunk/eval.rs:275`）是回退路径，
新类型必须能正确 fallback（混合类型列保持 `TypedColumn::Fallback` 不变）。

### 3.3 验证方式

- 单测：Date/Utf8 列的构建、批量求值、与行式结果逐值对比
- 集成：sort/TopN/aggregate 列式与行式结果一致性测试（随机数据 + 全类型）
- 基准：`benches/` 下 sort 大表列式 vs 行式耗时对比

### 3.4 风险

- 字符串列式收益有限（无 SIMD 加速），优先保证 Date/数值类型
- ColumnarBatch 与现有 memory pool / spill 记账的整合需要小心（`estimated_size` 已提供口径）
- 规模最大，建议拆分多个独立 PR，每步有行式回退

### 3.5 进度（2026-08-11）

**步骤 A 已完成**（TypedKind 扩展）：

- `chunk/typed.rs`：`TypedKind::Date/Utf8`、`TypedColumn::Date(Vec<i64>)/Utf8(Vec<Arc<str>>)`、
  `TypedBatch::Date/Utf8`；`len`/`value_at`/`to_values`/`estimated_size`/
  `typed_column_batch`/`typed_literal_batch`/`gather_typed_column` 同步扩展；
  批量比较新增 Date（i64 天数序，等价于 `cmp_date` 的年月日序）与
  Utf8（`Arc<str>` 字典序，与 `Value::String` 序一致）；`typed_cast_batch`
  的 Bool 分支补 `_ => None` 兜底
- `chunk/core.rs`：`build_typed_columns` 新增 `Value::Date → Date`、
  `Value::String → Utf8` 映射（混合/含 null 列仍 Fallback）
- `chunk/pool.rs`：`acquire_typed`/`release_typed` 新增 Date/Utf8 缓冲池
- `storage_scan.rs`：`typed_from_column` 对全 Date/全 String 的
  `ColumnValues::General` 提升为 typed 列
- `chunk/tests.rs`：新增 Date/Utf8 构建与批量求值测试；更新原
  `typed_columns_fallback_on_null_and_mixed_and_string`（字符串列现为 Utf8 不再回退）
- 验证：`cargo test -p graphdb-query --lib` 1325 项全过（含 chunk 23 项）

**剩余（后续阶段完成）：**

- 步骤 B：sort/TopN 列式 —— 新增 `chunk/columnar_batch.rs`，sort 按 typed key 列求
  permutation，spill 边界以上仍转行式走 `spill_sorted_run`
- 步骤 C：aggregate/window 列式 —— group-by key 用 typed 列哈希；
  sum/count/avg/min/max 对 I64/F64 批量聚合；window 依赖步骤 B

---

## 4. planner 迁移到 LogicalPlan（任务 5 步骤 2）

### 4.1 现状（比文档预期更有利）

- `convert_logical_to_physical`（`planning/physical_planner.rs`，1015 行）已覆盖几乎
  所有 LogicalNodeEnum 变体 —— **文档要求的"显式转换层"实际已存在**，
  目前仅 `unwind_planner.rs:124` 在用
- 现状路径是反的：planner 产出 PlanNodeEnum → `pipeline/compiler.rs:119` 用
  `LogicalPlan::from_plan_node` 反向剥离出 LogicalPlan 供 CBO 消费

### 4.2 方案

**核心思路：** 逐 planner 迁移为"先生成 LogicalNodeEnum，出口处一次转换"，
SubPlan 组合机制（`connect_upstream` 等）保持物理不动。

```
AST → LogicalPlan（纯语义）→ convert_logical_to_physical → PlanNodeEnum（保留物理属性）
        ↑ 新增：planner 原生产出
```

**需修改的文件与改动：**

1. **`planning/statements/dql/go_planner.rs`**（首个迁移对象）：
   - 语句内部 SubPlan 组合保持物理（connect_upstream 不动）
   - **仅在 planner 出口处**把根逻辑树 `convert_logical_to_physical` 一次转换
   - 这样 SubPlan/plan_combiner 机制完全不受影响

2. **`pipeline/compiler.rs`**：`from_plan_node` 改为直接构造 LogicalPlan（原生逻辑树），
   省去反向剥离，消除 `conversion.rs` 的 `NotYetImplemented` 回退

3. **`planning/statements/dql/lookup_planner.rs`、`match_statement_planner.rs`、
   `paths/pattern_planner.rs`**：按同样模式依次迁移（match_statement 规模最大放最后）

4. **DDL/DML planner 不动**：LogicalNodeEnum 无 DDL/DML 节点，保持物理直出

**实现注意：**
- 启发式规则作用于物理根，转换输出的物理形状必须与现状逐节点一致
  （尤其 Expand/Apply/Join 的 input 接线、PartitionSpec 在 ExecutionPlan 层）
- 验证手段：现有 planner 测试 + EXPLAIN 输出对比（golden 测试）

### 4.3 验证方式

- 现有 planning/optimizer 全量测试通过
- EXPLAIN 输出与迁移前逐节点一致
- 移除 compiler.rs 的回退分支后，DQL 语句的 `logical_plan()` 恒为 Some

### 4.4 风险

- 转换器个别节点行为差异 → 以 golden EXPLAIN 对比兜底
- 顺序迁移，每合入一个 planner 独立验证

### 4.5 进度（2026-08-11）

**已完成：**

- `plan/execution_plan.rs`：`SubPlan.logical_root: Option<LogicalNodeEnum>` +
  `from_logical_root()` / `logical_root()`；全部其余构造器置 `logical_root: None`
  （约 25 处 SubPlan 字面量，涉及 11 个 planning 文件）
- `compiler.rs`：`LogicalPlan::new` 优先消费 `sub_plan.logical_root()`，
  无逻辑树时才回退 `from_plan_node`
- `go_planner.rs`：重构为原生逻辑树构建
  （LogicalStart/LogicalArgument/LogicalExpandAll/LogicalFilter/LogicalProject/
  LogicalDedup），出口处经 `from_logical_root` 一次性转物理；
  逐节点比对验证物理输出与迁移前一致
- `lookup_planner.rs`：并行构建逻辑树（ScanVertices/ScanEdges/Filter/Project），
  物理计划（含 IndexScan）保持不变，仅在 SubPlan 挂逻辑根
- `physical_planner.rs`：converter 补充 `step_limit` 映射

**剩余（后续阶段完成）：**

- `match_statement_planner.rs` / `pattern_planner.rs` / `plan_combiner.rs`
  迁移（规模最大）：需要逻辑版连表机制；文档建议单独一个阶段专门验证，
  与物理输出逐节点比对

---

## 5. 从 PlanNodeEnum 剥离物理 join 变体（任务 5 步骤 3）

### 5.1 现状

- `PlanNodeEnum` 仍有 `HashInnerJoin/HashLeftJoin`（`plan_node_enum.rs:158-159`），
  由 `physical_planner.rs:309,339` 按 **hash_keys 是否为空**这一启发式产生（非 cost）；
  go_planner 等也直接造 `HashInnerJoinNode`
- executor 侧 `arena_builder/assembler/conversion.rs:607` 已能消费；
  `JoinSpec` 本身已有 HashJoin/NestedLoopJoin 变体
- `cost_based/join_order.rs:503` 已有 `JoinAlgorithm::NestedLoopJoin` 的 cost 决策，
  但该决策目前只写 notes，**未被 arena builder 消费**

### 5.2 方案

**核心思路：** planner 只产出逻辑 join 语义（InnerJoin/LeftJoin），
物理算法选择上移到 arena builder，由 cost 决策驱动。

```
planner: InnerJoin/LeftJoin（逻辑语义）
                  ↓
optimizer: join_order → JoinAlgorithm 决策（结构化，非 notes）
                  ↓
arena builder: 按 op_id 查决策 → HashJoin / NestedLoop / join_condition
```

**需修改的文件与改动：**

1. **planning 侧**：go_planner/lookup_planner 不再直接造 `HashInnerJoinNode`，
   统一产出 `InnerJoinNode/LeftJoinNode`（步骤 2 完成后经 logical 转换天然达成）

2. **`optimizer/cost_based/join_order.rs` + `decision/types.rs`**：
   把 `JoinAlgorithm` 决策从 notes 升级为结构化决策（`HashMap<op_id, JoinAlgorithm>`），
   写入 ExecutionPlan 供 arena builder 消费
   —— 这是该决策首次真正生效，行为变化需用 join 测试兜底

3. **`executor/streaming/plan/arena_builder/assembler/conversion.rs`**：
   InnerJoin/LeftJoin 分支根据 `exec_ctx.join_algorithm`：
   - `Hash` → `build_hash_inner_join_spec`（有 key）
   - `NestedLoopJoin` → 走 join_condition 路径
   - 缺省 → 现有启发式（key 非空则 hash）

4. **删除 `HashInnerJoinNode/HashLeftJoinNode` 变体** + 全部 match 点
   （`plan_node_enum.rs`、visitor、children、operations、traits_impl、
   `describe_visitor.rs`、`schema_validation.rs`、`conversion.rs:392,410` 等约 15 处）

5. **依赖步骤 2**：先让 planner 经 logical 转换产出（转换器已不产生 hash 变体），
   再删变体，改动面最小

### 5.3 验证方式

- join 全量测试（inner/left/right/full/semi/cross、hash key 与非 key 场景）通过
- EXPLAIN 输出中 join 节点类型与决策 notes 一致
- 成功标准：`PlanNodeEnum` 物理 join 变体归零

### 5.4 风险

- 最大规模删除，涉及 planner、optimizer、executor 三层
- JoinAlgorithm 决策首次生效可能改变执行计划 → 建议先只加"决策通道 + 回退启发式"，
  行为无变化时再删变体
- 每个删除步骤独立合入，均有回退路径

### 5.5 进度（2026-08-11）

**已完成（整体）**：`PlanNodeEnum` 物理 join 变体归零。

- 删除 `HashInnerJoin/HashLeftJoin` 枚举变体及 `HashInnerJoinNode/HashLeftJoinNode`
  结构体（含 impl 与测试，约 400 行）；`core.rs`/`nodes.rs`/`join.rs` 重导出同步清理
- converter 统一产出 `InnerJoinNode/LeftJoinNode`（hash_keys/probe_keys 附于节点）；
  `join_order_rewriter.rs` 的 `build_hash_inner_join` → `build_inner_join`，
  classify 只处理 InnerJoin
- 清理重复分支：`conversion.rs` 合并重复 InnerJoin/LeftJoin 组装分支、
  `partition.rs` 删除重复 inner join 分支、`specs.rs` 删除重复
  `build_hash_*_join_spec`（与普通版完全一致）
- visitor 清理：`describe_visitor.rs`、`plan_node_visitor.rs`、
  `heuristic/visitor.rs` 删除 hash 访问方法与分支
- 谓词下推：删除 `push_filter_down_hash_inner_join` 模块；
  `push_filter_down_hash_left_join.rs` 改名 `push_filter_down_left_join.rs`；
  `rule_enum.rs` 变体替换（`PushFilterDownLeftJoin`），registry 计数 49→48
- 全库批量替换 Hash* → 普通变体（约 10 个文件）
- 保留项：executor 层 `JoinSpec`/`JoinOperator`/`JoinState` 的 `HashLeftJoin`
  变体（属合法 executor 语义，已回退该层改动）
- 验证：`cargo test -p graphdb-query --lib` 1325 项、dql 179 项、optimizer 122 项全过；
  集成测试因磁盘空间未跑（环境问题，非代码问题）

---

## 6. 成功标准汇总

| 任务 | 验证方式 | 状态（2026-08-11） |
|------|----------|--------------------|
| 统计反馈闭环（阶段 2） | 相同查询两遍执行，第二遍 filter 估计向实际收敛；`mark_space_dirty` 清空修正 | ✅ 已实现，单测覆盖 |
| ColumnarPolicy | 高回退率场景自动切行式、恢复后切回；现有 typed 测试不回归 | ✅ 已实现，单测覆盖 |
| 列式执行（阶段 3） | sort/aggregate 列式与行式结果一致；bench 有可测量提升 | 🔄 步骤 A 完成；步骤 B/C 待做 |
| LogicalPlan 迁移 | 所有 DQL planner 出口经逻辑转换；EXPLAIN golden 一致；`logical_plan()` 恒有值 | 🔄 GO/Lookup 完成；match/pattern 待做 |
| PlanNodeEnum 分离 | 物理 join 变体归零；JoinAlgorithm 决策被 arena builder 消费 | ✅ 变体已归零；JoinAlgorithm 决策通道为后续任务 |
