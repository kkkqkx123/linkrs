# Phase 3 实施计划：并行执行完整落地 + 存储列块改造

- 状态：实施中（验证已执行，结论已定）
- 实测数据归档：`docs/archive/benches/phase3-parallel-storage-validation.md`
- 问题跟踪：`docs/issue/parallel-execution-dead-chain.md`（并行死链路）、`docs/issue/vertex-batch-insert-quadratic.md`、`docs/issue/traversal-query-pathology.md`
- 上级文档：`docs/plan/columnar-optimization-phases-design.md` Phase 3

## 1. 结论摘要（实测）

| 项 | 结论 | 数据 |
|----|------|------|
| P3.1 存储层列式属性块 | **立项** | 投影/全行 3.26~5.04x ≥1.5x；R：Q1 33% / Q2 50% / Q3 92%（>30%） |
| P3.2 并行扩展 | **未达标** | `actual_workers` 恒 1，E(4) ∈ [0.96,1.00]；根因=并行链路未接通（死链路），**功能需先完整实现** |

## 2. 并行执行完整实现方案（先接线，再扩展）

### 2.1 现状（已代码级确认，见 issue 文档）

- 执行侧基础设施**已存在且经引擎级单测验证**：`MorselWorkerPool`（query 级）、`SharedScheduler`（引擎级 M6）、Gather/Exchange 算子并行分支、`StreamingExecutionEngine::build_partitioned_executor`/`register_partitioned_root`
- 物理计划模型已具备分区表示：`PartitionInput`/`InputContract::PartitionedInputs`（`streaming/plan/types.rs:321/346`）、materializer 已处理 Exchange 与 PartitionedInputs（`materializer.rs:212/447`）、`PartitionView::from(&PartitionSpec)`（`streaming/partition.rs:28`）、`PhysicalPlanBuildContext.partition_spec` 字段（`context.rs:78`，恒 None）
- **缺口（两层死链路）**：① 分区决策拿不到 tag 统计；② 决策产出的 `partition_spec` 从未进入物理计划

### 2.2 接线步骤（本计划主体）

**B1.1 修复 tag 统计键**（1 个改动点组）

- DQL 扫描计划器建 `ScanVerticesNode` 时调用 `set_tag`：`planning/statements/dql/group_by_planner.rs:355`、`lookup_planner.rs:251`，并排查全部 `ScanVerticesNode::new` 调用点（`rg "ScanVerticesNode::new" planning/`），对齐 `update_planner.rs:138` 的做法
- 验收：真实查询下 `PartitioningPlanner::decide` 返回 spec（不再 fallback "vertex scan has no tag statistics key"）

**B1.2 接通 partition_spec → 物理分区片段**（核心改动）

1. `pipeline/compiler.rs:315` `build_physical_plan`：`build_ctx.partition_spec = plan.partition_spec().cloned()`
2. arena_builder：当 `build_ctx.partition_spec` 为 Some 且计划形态符合（单个 tagged vertex scan + 上拉算子）时，生成分区片段图：
   - N 个本地片段：扫描子树副本，`StorageScanVertices` 的 `ScanOptions` 绑定 `with_vertex_id_range`（`storage_scan.rs:39/82` 构造点 + `cursor.rs:149` 已支持）；副本间仅 range 不同
   - 根片段：`InputContract::PartitionedInputs`（成员 = N 个本地片段），根算子按 `PartitionedPhysicalNode::from_logical`（`execution_plan.rs:205`）的划分规则选择：`AggregateSplit`（局部聚合 + 全局聚合，`execution_plan.rs:240`）/ `DistinctSplit` / `TopNSplit` / `GlobalUnary`（sort/limit 上拉）/ `Local`
   - 片段间衔接复用既有 Exchange/Gather 物化路径（materializer 已实现）
3. 验证器与 EXPLAIN：`PhysicalPlanValidator` 增加分区片段形态校验；`EXPLAIN ANALYZE` 的 `partition_spec_description`/`actual_workers` 由既有 overlay 逻辑输出

**B1.3 决策可观测**

- `optimizer/engine.rs:374`：`decide()` 返回 None 时把 `decision.reason` 写入 `PlanDescription.parallel_fallback_reason`（当前丢弃），串联 `pipeline/diagnostics.rs` 的 overlay

**B1.4 执行期绑定**

- `QueryExecutionInstance` 按 fragment 图 + `bindings.max_workers` 走既有 `MorselWorkerPool`（`materializer.rs:416` 已按 `max_workers>1` 建池）；确认分区根走 `StreamingExecutionEngine` 的 partition 路径（`register_partitioned_root` 或 fragment 物化内建 Exchange 的等价路径）

### 2.3 扩展步骤（接线达标后，逐个增量）

| 增量 | 内容 | 前置 |
|------|------|------|
| E1 | 多 tagged vertex scan 分区（星型两侧），放宽"恰好一个 scan"约束（`partitioning.rs:96-100`） | 接线稳定 |
| E2 | `ScanEdges` 分区 + 与 Phase 2 P2.4 scan 谓词下沉联动 | E1 |
| E3 | 配置化暴露：`graphdb-config` 增 `parallel.workers`/`partitioning.*`，`graph_service.rs:192` 已从 `partitioning_config().max_workers` 建 `SharedScheduler`，数值接入用户配置，默认关闭 | E1 |
| E4 | 图遍历型渐进分区（保持白名单拒绝，记录 fallback reason） | 待 Q3 类查询性能改善（issue: traversal-query-pathology） |

### 2.4 非目标

- 并行写路径（规划器已拒绝写/事务边界）；spill/排序阻塞算子不分区；不对齐旧版 `coordinator.rs`（已废弃）

## 3. 验证方案（如何验证并行）

### 3.1 正确性验证（接线完成的准入）

- 分区结果 = 串行结果（同一查询、同一数据、逐值断言）：
  - Q1 `count`/`sum`（过滤 + 聚合）
  - Q2 `group_id, count(*)`（分组聚合，局部+全局两阶段语义）
  - `LIMIT`/`ORDER BY` 上拉形态（`GlobalUnary`）结果与顺序
  - 空表/单分区边界、分区数 > 实际行数、range 覆盖稀疏 ID
- 用例落点：`crates/graphdb-query` 集成测试（经 `QueryPipelineManager` + `PartitioningConfig` 全链路），与既有 `engine.rs` 引擎级并行测试互补
- 行为断言：`EXPLAIN ANALYZE` 输出 `requested=N`、`actual_workers = min(partitions, workers)`、`partition_spec_description` 非空、`parallel_wall_time_us > 0`、`parallel_work_time_us ≥ parallel_wall_time_us`

### 3.2 性能验证（重跑 `benches/parallel_scale_bench.rs`）

| 维度 | 设定 |
|------|------|
| 数据 | 100k / 400k 顶点宽表 + 3 边/顶点；`collect_statistics(force=true)` |
| 配置 | workers {1,2,4,8}；`min_rows_per_partition = rows/workers`；`max_partitions = workers` |
| 查询 | Q1 scan+filter+agg、Q2 scan+groupby（表扫描型）；Q3 遍历型仅记录不参与判定 |
| 测量 | 11 次中位 T(n)；E(n)=T(1)/T(n)；η(n)=work/wall；actual_workers；fallback_reason |
| 环境 | release（含 Phase 0 `x86-64-v3`）；**固定 CPU：`taskset -c 0-7` 独占 8 核**；关闭干扰进程；记录 CPU 型号与负载 |
| 门槛 | 接线后 Q1/Q2：E(4) ≥ 2x 且 E(8) > E(4) 且 η(4) ≥ 0.5 → 进入 §2.3 扩展；不达标 → 用 fallback_reason/work-wal 数据定位（Gather 串行化、分区粒度过细、扫描路径锁竞争），调整后重测 |

### 3.3 回归

- 每步：`cargo test --test '*'` + clippy 全绿；`operator_bench`/`query_bench`/`end_to_end_bench` 无回归（串行路径为默认，分区配置关闭时行为不变）
- 基准设施：`cargo bench --bench storage_read_baseline`、`cargo bench --bench parallel_scale_bench`（结果归档按 §3.2 环境说明标注）

## 4. 存储列块改造（P3.1 立项项，与并行独立）

- A1.1 `graphdb-storage`：`ScanOptions` 列块模式；`GraphVertexCursor::next_column_batch(prop, batch_size)`（复用 `ColumnStore`/`encoding`，对齐冷快照列式读取）
- A1.2 `graphdb-query` `source_operator/storage_scan.rs`：列块产出 → Phase 1 `TypedColumn`，过滤/投影在 typed 列求值；随机取属性（GetVertices）保持行式（实测投影无收益）
- 验收：Q1/Q2 端到端 ≥1.5x 且 R ≤ 20%；开关控制，关闭回行式
- C1（独立小项）：批量顶点插入 O(n²) 修复，见 issue 文档

## 5. 里程碑

| 里程碑 | 内容 | 产出 |
|--------|------|------|
| M1 | B1.1 + B1.3（tag、决策可观测） | `decide` 返回 spec；EXPLAIN 显示 fallback reason |
| M2 | B1.2 + B1.4（物理分区片段接线） | Q1 真实并行，`actual_workers ≥ 2` |
| M3 | §3.1 正确性用例全绿 | 分区 = 串行语义 |
| M4 | §3.2 性能验证 + 归档 | E(4)/η(4) 数据，决策进入扩展或调优 |
| M5 | 扩展增量 E1~E3（条件性） | 见 §2.3 |
