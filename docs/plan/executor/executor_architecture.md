# Executor 架构参考

> 日期：2026-07-11
> 范围：`crates/graphdb-query/src/query/executor/`
> 说明：当前架构描述、已清理内容、演进方向

## 一、当前架构

### 执行模型

标准的 Volcano pull 模型——单线程，每个算子通过 `Box<StreamingExecutor>` 链接为树，父算子拉取子算子数据。

```
StreamingExecutionEngine::execute()
  └── root.open() → loop root.next() → root.close()
```

### 算子枚举

`StreamingExecutor` 是 12-variant 的薄 dispatch 层，每个 variant 持有 `OperatorBase` + 子算子 + 领域算子枚举：

| Variant | 子算子数 | 领域算子 |
|---------|---------|---------|
| Source | 0 | `SourceOperator` |
| Unary | 1 | `UnaryOperator` |
| Join | 2 | `JoinOperator` |
| Set | 2 | `SetOperator` |
| Apply | 2 | `ApplyOperator` |
| Blocking | 1 | `BlockingOperator` |
| Graph | 1 | `GraphOperator` |
| Sink | 1 | `SinkOperator` |
| Ddl | 1 | `DdlOperator` |
| Fulltext | 1 | `FulltextOperator` |
| Vector | 1 | `VectorOperator` |
| Txn | 1 | `TxnOperator` |

生命周期 dispatch 仅 12 arm，每个 arm 委托给领域算子的对应方法。

### 公共基础设施

- **OperatorBase**：`plan_node_id`、`runtime`、`opened`，提供 `ensure_not_cancelled()`、`record_profile_timing()` 等共享能力
- **ExecutionRuntime**：`QueryIdentity` + cancel token + `MemoryTracker` + `ProfileCollector` + resource cleanup
- **DataChunk**：`Vec<Vec<Value>>` + `col_names: Vec<String>`（行存，运行时推断 schema）

### 执行入口

`query_pipeline_manager.rs` 中所有查询执行路径统一通过 `StreamingQueryExecutor`：

- `execute_plan()` → `StreamingQueryExecutor::from_plan_node()` → `.execute()` → `ExecutionResult`
- `execute_plan_to_stream()` → `StreamingQueryExecutor::from_plan_node()` → `.into_stream()` → `ResultStream`
- `execute_explain()` / `execute_profile()` → `ExplainExecutor` / `ProfileExecutor`（旧路径，独立于 streaming）

## 二、已删除内容

### 2.1 Pipeline 模块

`pipeline/` 目录（runner.rs、analyzer.rs、graph.rs、breaker.rs）已完全删除。

**删除理由**：

| 文件 | 死代码表现 |
|------|-----------|
| `runner.rs` | `execute_flat()` 是 `StreamingQueryExecutor` 的重复实现；`execute_pipelined()` 是损坏的实验代码；`PipelineRunner` 从未被构造 |
| `analyzer.rs` | `analyze()` 仅在单元测试中调用，无外部消费者 |
| `graph.rs` | `PipelineGraph` 唯一构造路径是分析器，从未用于执行或 explain |
| `breaker.rs` | `classify_breaker` 和 `is_source` 无调用方 |

**根本原因**：Pipeline DAG 在单线程 Volcano 执行中没有价值。breaker 语义已内化在每个算子的实现中（Sort 在 `next()` 中缓冲全量输入，HashJoin 在 `open()` 中 build hash table）。不需要外层 DAG 分析层来"告诉系统该在哪里打断"。

### 2.2 旧分析文档

`docs/plan/executor/` 中的 `pipeline_refactor.md` 和 `streaming_refactor.md` 已删除。`streaming_refactor.md` 描述的 79-variant 枚举膨胀问题已在 streaming_refactor 阶段解决（当前是 12-variant dispatch）。

## 三、关键判断

### 3.1 DAG 对执行是否有作用？

| 场景 | 判断 |
|------|------|
| 单线程 Volcano pull | **无作用**。树结构已满足全部需求 |
| 未来并行执行 | **必要**。DAG 定义 partition 边界、exchange 拓扑、morsel 调度顺序 |

### 3.2 现有 Streaming 是否符合最佳实践？

**是。** 它遵循教科书 Volcano 模型：

- 清晰的算子生命周期（open → next → close）
- 领域枚举拆分（12 variant，不是 79）
- OperatorBase 统一管理公共字段
- ExecutionRuntime 提供 cancel/memory/profile
- Partition 支持已预留（单线程串行执行）

存在差距但不影响架构判断的方面：
- `DataChunk` 使用 `Vec<Vec<Value>>` 行存，无 slot layout
- HashJoin key 使用 `Debug format`，应改为 slot-based key
- Scan 预读（`StorageScanVertices` 已定义但 builder 仍用 buffer）
- Spill、memory budget 强制、profile 算子覆盖待完善

### 3.3 Pipeline 应执行还是仅分析？

**都不应该。**

- **不执行**：Volcano 树不需要 pipeline 阶段划分
- **不分析**：breaker 分类是算子固有属性，"Sort 是阻塞算子"不需要分析层告知
- **不为未来保留**：未来的并行 pipeline 分析基于全新原语（partition-aware scan、exchange、morsel、local/global state），当前 `classify_breaker` 的 145 行代码对那个阶段无贡献

## 四、演进方向

### 短期（当前清理完成）

- [x] 删除 pipeline 模块
- [x] 删除陈旧分析文档
- [ ] 按 `streaming_reliability_plan.md` 修复 source 正确性、生命周期治理、内存预算

### 中期（Phase 2 of executor_cleanup_plan）

补充 streaming operator 真实实现（graph traversal、DDL、data modification、data access）。

### 长期（并行执行的前提条件）

满足以下条件后方可重新设计 pipeline DAG：

1. 存储层提供 `open_vertex_scan(range)` 分区游标接口
2. 所有 streaming operator 已实现（无 pass-through stub）
3. OperatorBase 已包含 memory budget、cancel token、resource owner
4. 连接类型已收敛到统一 HashJoin
5. 已建立针对 pipeline 状态的 profiler/instrumentation 支持

届时 pipeline 模块需要基于 `StreamingExecutor` 树生成并行 DAG，并引入：
- `PipelineScheduler` + `PipelineDAG`
- `Exchange`（channel-based，非物化）
- `Morsel`（分区任务单元）
- `WorkerPool`（线程池）
- `Local/Global OperatorState` 区分
