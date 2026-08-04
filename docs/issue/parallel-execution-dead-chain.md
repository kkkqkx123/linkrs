# 问题：端到端并行执行链路未接通（死链路）

- 状态：已修复（commit `464f411`，2026-08-04 实测达标）
- 类型：功能缺陷（既有 P8 并行设计的接线缺失，非新扩展）
- 关联：`docs/archive/benches/phase3-parallel-storage-validation.md` §3（实测 `actual_workers` 恒为 1）
- 关联代码：分区规划器、优化器、物理计划构建、执行器

## 问题描述

配置 `max_workers > 1` 且分区规划器配置完整时，端到端查询仍以串行执行：`EXPLAIN ANALYZE` 显示 `requested=N, actual=1`，`parallel_wall_time_us`/`parallel_work_time_us` 恒为 0。1/2/4/8 workers 实测 E(n) ∈ [0.89, 1.03]，**并行加速比从未得到验证**。

## 证据（实测 + 代码级确认）

1. 插桩实测（`OptimizerEngine::apply_partitioning_selection` 加日志后对真实查询执行）：分区决策恒返回 None，reason = `"vertex scan has no tag statistics key"`
2. 分区决策链路：
   - `optimizer/engine.rs:374` `apply_partitioning_selection` → `PartitioningPlanner::decide(root, &stats)`（`optimizer/partitioning.rs:65`），其中 `collect_vertex_scans` 取 `scans[0].tag()`（`partitioning.rs:142`）
   - `ScanVerticesNode.tag` 默认 `None`（`planning/plan/core/nodes/access/graph_scan_node.rs:379`），`set_tag` 只在 `planning/physical_planner.rs:119`（逻辑→物理转换）与 `statements/dml/update_planner.rs:138` 被调用
   - DQL 扫描计划器建点时不设 tag：`statements/dql/group_by_planner.rs:355`、`lookup_planner.rs:251` 等 → **优化阶段（分区决策发生处）tag 恒为 None**
3. 即使 `decide()` 返回 spec，物理计划也不消费：
   - `apply_partitioning_selection` 设 `plan.set_partition_spec(spec)`（`optimizer/engine.rs:377`）
   - `pipeline/compiler.rs:315` `build_physical_plan` 从不读取 `plan.partition_spec()`；`PhysicalPlanBuildContext.partition_spec` 字段存在（`streaming/plan/context.rs:78`）但两个构造点恒为 None（`:106/:120`）
   - `PartitionedPhysicalNode::from_logical`（`planning/plan/execution_plan.rs:205`）仅在单元测试中使用；物理计划构建（arena_builder）不生成任何 Exchange/分区片段
   - 执行侧基础设施齐全但收不到输入：`MorselWorkerPool` 按 `max_workers>1` 创建（`streaming/plan/materializer.rs:416`），Gather/Exchange 并行分支要求 `children.len() > 1`（`gather_operator.rs:110`、`exchange_operator.rs:138`）——真实查询永远没有多子节点

## 影响

- 所有端到端查询恒串行，`max_workers`/分区配置形同虚设；P8 Morsel 执行器（引擎级单测通过）在生产路径不可达
- 并行扩展评估（Phase 3 P3.2）无法进行：没有基线加速比可测

## 修复方向（接线，非扩展）

| 步骤 | 内容 | 位置 |
|------|------|------|
| B1.1 | DQL 扫描计划器建 `ScanVerticesNode` 时调用 `set_tag`（对齐 update_planner 的做法） | `planning/statements/dql/group_by_planner.rs`、`lookup_planner.rs` 及全部扫描建点路径 |
| B1.2 | `build_physical_plan` 将 `plan.partition_spec()` 注入 `PhysicalPlanBuildContext.partition_spec`；arena_builder 在 spec 存在且根为可分区源时生成：N 份本地片段（`StorageScanVertices` 绑定 `ScanOptions.with_vertex_id_range`，`storage_scan.rs` + `cursor.rs:149` 已支持）+ 根 Gather/Exchange 片段（`InputContract::PartitionedInputs`、`PartitionInput` 类型已存在，`streaming/plan/types.rs:321/346`；materializer 已处理 Exchange 与 PartitionedInputs，`materializer.rs:212/447`） | `pipeline/compiler.rs:315`、`streaming/plan/arena_builder/`、`streaming/plan/context.rs:78` |
| B1.3 | `apply_partitioning_selection` 的 None 分支将 `decision.reason` 写入 `PlanDescription.parallel_fallback_reason`（当前丢弃），决策可观测 | `optimizer/engine.rs:374`、`pipeline/diagnostics.rs` |

聚合算子分区需两阶段语义（局部聚合 + 全局聚合），复用 `PartitionedPhysicalNode::from_logical` 的 `AggregateSplit` 结构（`execution_plan.rs:240`）作为片段划分依据。

## 验证

- 正确性：分区执行结果与串行逐值相等（count/sum/groupby/filter+agg），见 `docs/plan/columnar-phase3-parallel-storage-verification-design.md` §验证方案
- 行为：`EXPLAIN ANALYZE` 显示 `actual_workers = min(partitions, workers)` 且 fallback_reason 为空
- 性能：修复后重跑 `benches/parallel_scale_bench.rs`，Q1/Q2 E(4) ≥ 2x 且 η(4) ≥ 0.5 才继续并行扩展（多 scan / edge scan 分区）

## 修复验证结果（2026-08-04，`taskset -c 0-7`）

- **B1.1/B1.2/B1.3 全部落地**，端到端链路接通：Q1/Q2 实测 `actual_workers = requested_workers`（2/4/8），`EXPLAIN ANALYZE` 输出 `PartialAggregate ×N + Exchange + FinalAggregate`，`parallel_work_us > 0`
- **性能达标**：Q1 E(4)=7.06、Q2 E(4)=5.83（≥ 2.0），η(4)=3.18/3.72（≥ 0.5）；E(8) 回落至 6.60/4.58（Exchange 汇聚 + 内存带宽约束）
- Q3 图遍历按白名单回退串行，`fallback_reason="plan contains recursive graph traversal; partitioning not supported"` 可观测；workers=1 显示 `"partitioning is disabled"`
- 完整数据与修复前对照见 `docs/archive/benches/phase3-parallel-storage-validation.md` §3
