# Pipeline 模块重构方案

> 分析日期：2026-07-11
> 范围：`crates/graphdb-query/src/query/executor/pipeline/`
> 目标：明确 Pipeline 模块定位，要么实现真正的多阶段执行引擎，要么降级为 explain-only 辅助工具

## 一、现状分析

### 1.1 当前设计

Pipeline 模块分为四部分：

```
PipelineAnalyzer→ PipelineGraph → PipelineRunner
(classify_breaker)   (DAG)          .execute_flat()      ← 默认路径：忽略 pipeline，直接构建 StreamingExecutor
                                    .execute_pipelined()  ← experimental：分离执行，全量物化中间结果
```

`PipelineBreakerKind` 定义 10 种 breaker：
Sort, Aggregate, Distinct, HashJoinBuild, Window, Materialize, SetOps, VariableLengthTraversal, ShortestPath, DmlDdlSink

### 1.2 核心问题

| 问题 | 严重程度 | 说明 |
|------|---------|------|
| **Pipeline 模式从未被实际使用** | ★★★★★ | 默认路径 `execute_flat()` 完全忽略 pipeline 边界，直接构建单棵 StreamingExecutor 树 |
| **中间结果全量物化** | ★★★★ | `execute_pipelined()` 将中间数据序列化为 `Vec<Vec<Value>>` 再重扫，无共享内存或 channel |
| **双输入边界处理不正确** | ★★★★ | `replace_leaf_executor` 只处理单输入；Join 等双输入节点回退到仅扫描物化数据 |
| **wrap_node 是空操作** | ★★★ | `parent.clone()` 注释称 "it won't have correct inputs"，子计划树不完整 |
| **零并行能力** | ★★★ | 模块注释明确说 "Phase 6a focus ... no parallelism yet" |
| **breaker 分类冗余** | ★★ | 四分之一的逻辑只做算子类型清洗，本可从 PlanNodeEnum 类型系统派生 |
| **替换 leaf 的回退路径** | ★★ | `build_executor_with_materialized_inputs` 末尾直接 return 行扫描而不是报错，隐藏错误 |

### 1.3 根因分析

根本原因是 **Pipeline 模块缺乏明确的存在理由**：

1. 当前 streaming executor 不需要 pipeline 也能正确执行——单棵 Volcano 树足以处理复杂查询
2. 如果要实现 Pipeline 的核心价值（并行、内存流水线、spill 协调），需要大量非 trivial 的基础设施（exchange、线程池、morsel 调度），当前项目尚未抵达该阶段
3. `execute_pipelined()` 是全量物化的——它消除 pipeline 的全部优势（流式、少内存、低延迟）

结果：**Pipeline 模块目前是一个 elaborate 的 plan explain 工具，而非执行引擎**。它分析了 plan tree，标记了 breaker 边界，但从来不利用这些信息加速执行。

## 二、设计抉择

有两个可能的演进方向，选且只能选一个：

### 选项 A：降级为 Explain-only 元数据工具

保留 `PipelineAnalyzer` + `PipelineGraph`，移除 `PipelineRunner` 和所有执行相关代码。

**理由**：
- 当前 pipeline 信息对于 explain/profile 有潜在可视化价值（"查询在哪些节点被打断"）
- 投入成本低：删除 `runner.rs`，保留 analyzer 和 graph
- 不产生虚假期望（没人依赖 `execute_pipelined()`）

**放弃**：
- 永远无法用 pipeline 做并行执行、内存优化、spill

### 选项 B：建设真正的多阶段执行引擎

投入基础设施，让 pipeline 成为真实执行框架。

**必须完成的工作**：

1. **Exchange 机制**：pipeline 之间通过共享 channel 传递 DataChunk，而不是物化为 `Vec<Vec<Value>>`
2. **多阶段调度**：按拓扑顺序启动每个 pipeline，source pipeline 产出数据直接推入下游
3. **正确子计划构建**：`wrap_node` 必须真正构造正确的计划子树，而不是 `parent.clone()`
4. **双输入边界支持**：Join/BiExpand 等双输入算子的边界处理
5. **线程池集成**：按 morsel/partition 分发 source pipeline 任务

**当前不建议立即投入的原因**：
- 项目当前需要的是 `executor_cleanup_plan.md` 中的 Phase 2（补充 streaming 实现），而非并行调度
- 在没有稳定单线程执行内核前引入并行会引入不确定性问题
- 存储层尚未提供分区感知的游标接口（`open_vertex_scan(range)` 等）

## 三、推荐方案：选项 A（降级为 Explain-only 元数据工具）

### 3.1 分步实施

#### Step 1：清理 `PipelineRunner` — 删除 `execute_pipelined()`

删除 `PipelineRunner::execute_pipelined()`，保留 `execute_flat()` 作为 `PipelineRunner` 的唯一执行路径。

```rust
// Step 1 之后的 PipelineRunner
pub struct PipelineRunner {
    graph: PipelineGraph,
    context: ExecutionContext,
}

impl PipelineRunner {
    /// 执行查询（忽略 pipeline 边界，构建单棵 StreamingExecutor 树）
    pub fn execute(&self) -> Result<Vec<DataChunk>, QueryError> {
        // 等价于当前的 execute_flat()
    }

    /// Pipeline 分析结果（仅用于 explain/可视化）
    pub fn graph(&self) -> &PipelineGraph { &self.graph }

    /// 生成 pipeline explain 输出
    pub fn explain(&self) -> String { self.graph.explain() }
}
```

删除文件：
- `runner.rs` 中的 `execute_pipelined()`
- `build_executor_with_materialized_inputs()`
- `replace_leaf_executor()`
- `try_get_single_input()`

#### Step 2：保留但精简 PipelineAnalyzer

当前 `PipelineAnalyzer` 的 `analyze_node_with_map` + 大量 helper 函数（`try_get_single_input`、`has_binary_input`、`has_multiple_input`）本质上是重复声明 PlanNodeEnum 的类型信息。

精简方向：
- 移除 `try_get_single_input()`（与 `breaker.rs::is_source` 等功能重叠）
- 合并 `has_binary_input`/`has_multiple_input` 为更通用的 getter
- 删除 `#[allow(dead_code)]` 的 `analyze_node()` 旧方法

#### Step 3：移除 breaker 中的冗余分类

`breaker.rs` 中的 `classify_breaker` 和 `is_source` 函数本质上是从 PlanNodeEnum 推导类别。如果 PlanNodeEnum 自身带一个 `node_category()` 方法，则可以消除：

```rust
// 理想形态：从 PlanNodeEnum 类型系统派生
impl PlanNodeEnum {
    pub fn node_category(&self) -> NodeCategory {
        match self {
            Self::Sort(_) | Self::TopN(_) => NodeCategory::Breaker(PipelineBreakerKind::Sort),
            Self::Aggregate(_) => NodeCategory::Breaker(PipelineBreakerKind::Aggregate),
            Self::Start(_) | Self::ScanVertices(_) => NodeCategory::Source,
            Self::Filter(_) | Self::Project(_) => NodeCategory::Transform,
            // ...
        }
    }
}
```

这一步骤是可选优化，不属于必做。

#### Step 4：为 explain 增强 PipelineGraph 输出

当前 `PipelineGraph::explain()` 只输出一行摘要。可以增强为：

```
PipelineGraph (4 pipelines, root=3)
  Pipeline 0 [ScanVertices]: source → breaker(HashJoinBuild)
  Pipeline 1 [ScanEdges]: source → breaker(HashJoinBuild)
  Pipeline 2 [HashJoin]: inputs=[0,1] → breaker(Sort)
  Pipeline 3 [Sort]: inputs=[2] → root
```

将 explain 输出接入 `EXPLAIN PLAN` 命令，供用户查看计划分段。

### 3.2 影响范围

| 文件 | 变更 |
|------|------|
| `pipeline/runner.rs` | 删除 ~100 行（execute_pipelined 相关），保留 ~50 行 |
| `pipeline/analyzer.rs` | 保持核心逻辑，删除冗余 helper |
| `pipeline/breaker.rs` | 保持 `classify_breaker` 和 `is_source`，移除建议整理为 TODO |
| `pipeline/graph.rs` | 增强 `explain()` 输出 |
| `pipeline/mod.rs` | 更新文档注释，移除引用 |
| 外部调用方 | 搜索 `execute_flat()`/`execute_pipelined()` 调用，更新为 `execute()` |

### 3.3 验证标准

- [ ] `cargo test` 全部通过（当前无测试依赖 `execute_pipelined`）
- [ ] `EXPLAIN PLAN` 输出包含 pipeline 信息
- [ ] breaker 分类与 streaming operator 实现状态一致（如果某算子标记为 breaker 但 streaming 尚为 stub，需注释说明）

## 四、长远展望：如果未来需要真正的 Pipeline 执行

如果项目未来（存储层支持分区游标后）需要并行执行，建议参考以下架构重建 pipeline 模块：

```
PipelineScheduler
  ├── PipelineDAG (当前 PipelineGraph 的进化版)
  ├── Exchange (channel-based, 非物化)
  ├── Morsel (分区任务单元)
  ├── WorkerPool (线程池)
  └── Global/Local OperatorState 区分
```

但在此之前，以下条件不满足时不应引入：
1. 所有 streaming operator 具备真实实现（不是 pass-through stub）
2. 存储层提供 `open_vertex_scan(range)` 分区感知接口
3. `OperatorBase` 已建立且包含 memory tracker、cancel token、resource owner
4. 连接类型（inner/left/right/full/semi/anti）已经收敛到统一 HashJoin
5. 已有针对 pipeline 状态的 profiler/instrumentation 支持

## 五、总结

| 步骤 | 内容 | 工作量 | 优先级 |
|------|------|--------|--------|
| Step 1 | 清理 runner.rs，删除 `execute_pipelined()` | ~0.5 天 | 高 |
| Step 2 | 精简 analyzer.rs 冗余 helper | ~0.5 天 | 中 |
| Step 3 | 可选：breaker 分类与 PlanNodeEnum 合并 | ~1 天 | 低 |
| Step 4 | 增强 PipelineGraph::explain() 输出 | ~1 天 | 中 |
| 最终产出 | Pipeline 模块降级为 explain-only 元数据工具，移除未使用的执行代码 | 总计 ~2-3 天 | — |

核心原则：**不做伪 pipeline，直到真正需要并行执行**。当前 pipeline 模块的价值在于 explain 时告诉用户"这些是 pipeline breaker"，不在执行路径上。
