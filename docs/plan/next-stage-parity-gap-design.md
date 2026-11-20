# 与 Ladybug / Neug 的剩余差距与下一阶段设计

**日期**：2026-11-20  
**依据**：`perf-data/parity/results/2026-11-18/` 的三方结果、第 22/23 节回填、本轮有序并行 A/B、`ref/nebula` 参考实现。本文只描述差距、成因判断与下一阶段任务，不重新定义已完成事项。

## 1. 当前指标对照

Linkrs 取 `filter-aggregate-final` 两轮中位数均值，口径为 warmup 1 + run 5、绑核 `16-31`、`threads=16`、`parallel-workers=16`。对手取 `lbug-ops.json` / `neug-ops.json` 的 `avg_ms`（warmup 1 + run 3 去最大值平均，`distinct_limit` 使用修正 `ORDER BY` 的单独结果）。口径不同，但只用于量级判断；20% 以内的差异不当作结论。

| 用例 | Linkrs ms | Ladybug ms | Neug ms | 对 Ladybug | 对 Neug | 判断 |
|---|---:|---:|---:|---:|---:|---|
| `distinct_limit` | 291.3 | 3.755 | 28.701 | 77.6× | 10.2× | 最大缺口，且当前测量未开并行 |
| `group_by_count` | 277.8 | 7.104 | 13.762 | 39.1× | 20.2× | 最大缺口，且当前测量未开并行 |
| `filter_id_range` | 1.63 | 0.974 | 0.159 | 1.7× | 10.3× | 小缺口；Neug 单线程 AP 极快 |
| `filter_or` | 1.49 | 1.517 | 0.317 | 0.99× | 4.7× | 与 Ladybug 对齐 |
| `aggregate_min_max` | 1.34 | 1.020 | 0.158 | 1.3× | 8.5× | 小缺口 |
| `order_limit` | 1.29 | 0.880 | 0.044 | 1.5× | 29.5× | 小缺口；Neug TopN 明显更强 |
| `undirected_count` | 4.57 | 1.926 | 1.450 | 2.4× | 3.2× | 慢路径预期内，可继续收敛 |
| `join_two_hop` | 6.10 | 2.422 | 24.088 | 2.5× | 0.25× | 已优于 Neug，距 Ladybug 约 2.5× |
| `variable_path_count` | 6.32 | 457.68 | 117.401 | 0.014× | 0.054× | 已大幅领先，不列为优化目标 |
| `aggregate_filtered_count` / `shortest_path` | 1.18 / skip | error | error | 不可比 | 不可比 | Linkrs 方言微基准 |

结论：真实差距集中在 `distinct_limit`、`group_by_count`、少量微扫描算子，以及库体积/RSS 口径。变长路径和两跳计数已经不再是主要矛盾。

## 2. 必须先修复的测量基线

`filter-aggregate-final` 的 EXPLAIN 显示 `requested=16, actual=1, fallback_reason="partitioning is disabled"`。这不是引擎退化，而是基准入口的行为变化：共发布并行前沿改动后，`linkrs-parity-bench` 把 `--ordered-parallel-expand` 从过去的隐式打开改成默认关闭，而当前固定门脚本没有显式打开它。历史阶段八/九使用同一查询时 `actual=12`，中位数约 100–120 ms。

本轮用同一二进制、同一 golden DB 做了四组 A/B（日志与 JSON 在 `perf-data/parity/results/2026-11-18/gap-ab/`）：

| 配置 | `group_by_count` 两轮中位数 | `distinct_limit` 两轮中位数 | 结果稳定性 |
|---|---:|---:|---|
| 当前默认（并行关闭） | 277.5 / 278.0 ms | 290.4 / 291.3 ms | 正确 |
| `--ordered-parallel-expand on` | 115.7 / 102.8 ms | 148.2 / 115.0 ms | 结果 14\|59 / 14，稳定 |
| 同时打开计数前沿 | 108.8 / 97.1 ms | 117.3 / 118.6 ms | 结果稳定，但收益不稳定 |

下一阶段第一项必须是恢复可比基线：

- 基准入口在 `--parallel-workers > 1` 时继续请求有序并行展开；生产配置默认值不动，仍由显式开关放行。
- 增加固定门回归断言：这两条用例的 EXPLAIN 必须出现 `actual > 1`，否则该轮结果标记为无效，不进入对比表。
- 重新采集三方或至少 Linkrs 基线后再谈后续优化比例。

## 3. 剩余差距的成因判断

### 3.1 分组计数与去重排序

有序并行打开后，`group_by_count` 约 100–116 ms、`distinct_limit` 约 115–148 ms，仍分别是 Ladybug 的约 14× 和约 31×。EXPLAIN 显示每个分区的执行形状是：

- 10 个分区，`actual=10`，并行 `wall_time≈125 ms`、`work_time≈1.16 s`；
- 每个分区扫描约 21.5k 行、自身约 86 ms，展开约 25 ms，局部聚合约 115 ms（累计计时）；
- 最终交换后只有约 11.5k 局部聚合行进入最终聚合/TopN；
- `column_block_hits=110`，但 `typed_hit_rate=0`，说明 TypedColumn 在这条链上没有覆盖。

据此判断：

1. 分区已经把输出切分，但扫描枚举、拓扑展开与局部聚合仍是三段独立流水线，行/列在段间重复组装；
2. 局部聚合虽已 typed 化，但输入仍走通用行/Project 链，没能让拓扑消费者直接累计分组状态；
3. `DISTINCT + ORDER BY + LIMIT` 仍在 Exchange 之后做全局 Distinct 与 TopN，未利用“每分区 TopK 再归并”的等价性；
4. 当前比较口径下对手还有库体积与 buffer manager 优势：Ladybug 使用约 90% 可用内存作为 buffer pool，Linkrs 的 197 MB 库体积与 948 MB RSS 口径尚未进入 parity JSON。

### 3.2 微扫描算子

`filter_id_range`、`aggregate_min_max`、`order_limit` 的绝对耗时都在 1.2–1.7 ms，主要落在顺序扫描与残差过滤/表达式求值，而不是聚合。`filter_or` 已经与 Ladybug 对齐，说明 OR 的 selection 路径可用；Neug 在这些单点算子上快一个数量级，更多反映其单线程内存列式 AP 执行形态，短期目标应是先对齐 Ladybug。

### 3.3 库体积与资源口径

远端 LSQB sf0.1 数据库目录为 197 MB：WAL 116 MB（157 个 4 MB 段）、checkpoint 81 MB（10 个保留点）、其余不足 1 MB；Ladybug 与 Neug 分别为 35 MB / 43 MB。`cleanup_old_checkpoints` 只提供手动 API，没有调度器自动调用；WAL  reclaim 依赖文件头里的 `checkpoint_seq`，需要核实旧段为何长期未回收。该差距不影响查询数值，但影响 parity 报告的可比性和冷启动成本。

## 4. ref 目录可参考的实现形态

`ref/nebula` 提供的是 Nebula Graph 的实现，不是 Ladybug/Neug 源码；只借鉴执行形态，不照搬布局或协议。

- `storage/query/GetNeighborsProcessor.cpp`：按 partId 构造独立 `RuntimeContext` 与存储表达式上下文，并发跑完再合并；与 Linkrs 的分区 fragments 思路一致。
- `storage/exec/AggregateNode.h`：在边迭代过程中直接累计 SUM/COUNT/MIN/MAX，不先物化边的属性列表。对应 Linkrs 应把局部聚合下沉到展开读取循环之后、行物化之前。
- `graph/executor/query/AggregateExecutor.cpp`：`COUNT(*)` 在无 GROUP BY 时直接返回输入行数；Linkrs 已有类似快路径，但分组路径仍需避免逐行 `Value` 构造。
- `graph/executor/query/TopNExecutor.cpp`：固定大小堆维护 TopK，先在局部数据流上收敛再输出；可用于分区局部 TopN 与最终归并。
- `storage/exec/ScanVertexPropNode.h`：按前缀游标顺序扫描并只解码投影列，强调“枚举、解码、输出”一次性完成；Linkrs 的分区扫描应朝同一方向收敛。

Ladybug 侧参考 `graphdb-ben/docs/ladybug-benchmarks.md`：其 runner 使用 90% 可用内存作为 buffer manager，并记录 profile 文件与库体积；下一阶段 parity JSON 应对齐这些字段。

## 5. 下一阶段任务

### 任务一：恢复并冻结可比基线

- 修改 `src/bin/linkrs_parity_bench.rs`，让 `--parallel-workers > 1` 的基准默认请求有序并行展开；保留生产配置默认关闭。
- 在固定门中校验 EXPLAIN 的 `actual_partitions` 与 `actual`，失败即判环境/配置无效。
- 重新采集两轮 Linkrs 结果并回填问题清单，再计算后续改进比例。

### 任务二：分区扫描、展开与局部聚合融合

- 以当前 `build_partition_local_fragments` 的 shard-slice 分区为基础，审计每个分区是否只枚举自己拥有的物理行；若仍有全表枚举或重复 topology 解析，先修枚举边界。
- 在 rowless typed chunk 上，让单跳展开直接把拓扑列交给局部聚合状态，跳过 Project 的行物化与重复 schema 解析；保持 TTL、过滤、多跳、属性需求走原路径。
- 为分组键为单列定长数值的 `count` 启用已有 typed 局部聚合状态，非定长或 nullable 键回退通用表。
- 护栏：pushed/unpushed 数值逐例相等；分区并集与串行结果一致；未放行形状计划不变。

### 任务三：精确局部 TopN 与 Distinct 归并

- 对 `DISTINCT ... ORDER BY key LIMIT k` 增加分区局部 TopK：每个分区在 typed 键集合上保留最小/最大 k 个键，Exchange 后再做一次全局 TopK。
- 对 `GROUP BY ... ORDER BY count LIMIT k` 复用同一局部 TopK 接口，先砍掉不可能进入全局前 k 的分组。
- 必须先补反例：重复键、并列值、NULL 键、空分区、k 大于局部行数；稳定排序规则与最终 TopN 完全一致。
- 不做近似去重、不做截断采样。

### 任务四：微扫描算子的存储侧下推

- `min/max`：在 schema 与 MVCC 可证明的窄前置下，从 zone-map/列统计返回边界值，失败回退完整扫描；TTL 与 pending write 不参与。
- `filter_id_range` / `order_limit`：评估让扫描直接输出满足 id 范围的有序小批，或把 TopN 下推到有序索引；必须先证明存储侧比较与 evaluator 一致。
- `undirected_count`：在单类型、同标签、无 TTL、无采样的窄形状上评估元数据计数放行，oracle 继续使用种子侧出入腿减自环公式。
- 每个下推都要求计划标记可读，且 pushed/unpushed 数值一致。

### 任务五：库体积与资源报告

- 在 parity JSON 增加 `db_size_bytes`、RSS 峰值、buffer pool 上限、spill 计数；compare 脚本单列资源表。
- 审计 WAL 段的 `checkpoint_seq` 写入与 reclaim 条件，让已落盘且被最老保留 checkpoint 覆盖的段可被回收。
- 给 checkpoint 调度器增加保留上限（建议保留最新一个加一个回退点），回收旧目录前确认恢复不依赖它们。
- 崩溃恢复、容量超限与并发写入用例必须全绿；不通过牺牲持久性换体积。

## 6. 验收门槛与不做项

| 项目 | 目标 | 验证 |
|---|---|---|
| 可比基线 | 固定门 EXPLAIN `actual>1`，两轮结果稳定 | 两轮 Linkrs JSON + EXPLAIN |
| `group_by_count` | 从约 100 ms 降到 20 ms 量级，且与 Ladybug 差距 ≤3× | 同数据、同绑核、两轮 |
| `distinct_limit` | 从约 115 ms 降到 15 ms 量级，且与 Ladybug 差距 ≤4× | 同上 |
| 微扫描算子 | 与 Ladybug 差距 ≤1.5×，或给出可复现的存储层原因 | 计划标记 + 两轮数值 |
| 库体积 | 从 197 MB 降到 50 MB 以内，回收前后查询结果一致 | WAL/checkpoint 目录体积 + 恢复用例 |
| 正确性 | 整数逐值相等，浮点相对误差 1e-3 | DQL、streaming、partitioning、storage 全量 |

不做：近似算法、截断采样、跨分片事务、动态重分区、以牺牲持久性换体积，以及在生产配置中静默打开并行。

## 7. 基于 Ladybug / Neug 源码的可借鉴设计

本地已补齐 `ref/ladybug`、`ref/neug`，以下按文件给出可借鉴的具体形态。它们只作为设计参照；Linkrs 的 MVCC、边身份、布局版本与持久化权威不变。

### 7.1 Ladybug

- `src/processor/operator/scan/scan_node_table.cpp`：当 node group 数少于线程数时，按 `MORSELS_PER_THREAD_TARGET = 8` 把 group 切成子区间 morsel。Linkrs 当前的 `min_rows_per_partition` 是固定行数，可以在不改变计划形状的前提下引入“按 worker 数生成足够 morsel”的自适应上限，避免 10 个分区各自做粗粒度扫描。
- `src/processor/operator/scan/scan_rel_table.cpp` 与 `docs/multi_parent_lifetime.md`：`PACKED_EXTEND` 一次扫描多个父点的孩子，用 `PackedChildSlices` 保存父选择向量和孩子前缀偏移。下游 `PackedFilteredCount` 直接按父区间累计，不构造路径行。Linkrs 的 rowless typed chunk 可以扩展为“父批 + 孩子偏移”契约，让单跳展开和局部聚合在同一批内对接，消除 Project 与行物化。
- `src/processor/operator/aggregate/hash_aggregate.cpp`、`aggregate_hash_table.cpp`：每个 worker 先写本地哈希表，再按哈希分区合并；`append()` 接收向量化 keys、payloads 与 `resultSetMultiplicity`，直接更新 factorized table 中的状态偏移，`limitNum` 达到后提前终止。Linkrs 应在 typed 聚合中消费 `chunk.multiplicity()` 做加权累加，而不是入口处 `expand_multiplicity_in_place()`；DISTINCT 也可用局部表上限提前收敛。
- `src/optimizer/factorization_rewriter.cpp` 与 `src/processor/operator/multiplicity_reducer.cpp`：保持 unflat 分组与 multiplicity，只在必要处 flatten。对应 Linkrs 应把 multiplicity 作为聚合输入属性，而不是行复制指令。
- `src/optimizer/limit_push_down_optimizer.cpp`：LIMIT 通过 row-count-preserving 算子下推，并把 `skip + limit` 作为 DISTINCT 的上限；FILTER/JOIN/AGGREGATE 是硬屏障。可直接用于任务三的精确局部 TopK 边界。
- `src/processor/operator/order_by/top_k.cpp`：用边界值先过滤整批，sort state 到达阈值后 reduce 到 `skip + limit`，最后 merge 排序块。Linkrs 的 TypedColumn TopN 可借用“边界剪枝 + 周期收缩”，避免全量排序。
- `src/storage/wal/wal.cpp`：`rotateForCheckpoint()` 把活动 WAL 改名为单个 frozen WAL，checkpoint 后 `retireFrozenWAL()` 做 truncate + sync + remove；任何时刻至多一个活动段加一个 frozen 段。`client_context.cpp` 对 COPY 强制 checkpoint。Linkrs 的 157 个 WAL 段应参照该模型收敛，并保留“损坏即 panic、重启恢复”的持久性语义。
- `src/storage/stats/column_stats.cpp` 与 `src/storage/predicate/constant_predicate.cpp`：列块维护 min/max 与 HLL，常量谓词先查 zonemap，再对 typed vector 用 selection vector 过滤，且 `hasNoNullsGuarantee()` 有专门快速路径。可用于 min/max 下推与 `filter_id_range` 微扫描。

### 7.2 Neug

- `include/neug/storages/csr/csr_view.h`、`src/storages/csr/prefetch_utils.cc`、`include/neug/execution/common/operators/retrieve/edge_expand_impl.h`：邻居迭代器是 POD，仅保存指针、结束位置、stride、timestamp offset 和 data offset；按度数分布（max、nonzero avg、p99）选择 metadata/head 预取距离与 locality，批量顶点循环里显式 `prefetch_next_vertex`。Linkrs 已有的拓扑批量读取可继续朝“零对象、按度数预取”收敛，尤其针对分区内每点的 CSR 区间读取。
- `include/neug/execution/common/operators/retrieve/scan.h`：顶点扫描是 dense vid 遍历 + 谓词闭包 + typed builder，不在扫描阶段构造 `Value` 行；`index predicate` 走 primary key 定位。可借鉴其“id-only 扫描 + 特化谓词”形态处理 `filter_id_range`。
- `src/execution/common/operators/retrieve/group_by_utils.cc`：非空定长列用 typed `Key` 特化，nullable 或多列回退 encoded generic key。与 Linkrs typed group count 的方向一致，但提示下一阶段应把 typed key 选择前移到计划期并写入计划标记。
- `include/neug/utils/top_n_generator.h` 与 `src/execution/execute/ops/retrieve/order_by.cc`：typed 优先队列在 `ValueColumn<T>` 上直接取 TopN，并列值通过 `replicated_indices_` 精确处理；`order_by_limit()` 对非空 typed 列直接成功。可作为 Linkrs `order_limit` 与局部 TopK 的实现模板。
- `src/execution/execute/pipeline.cc` 与 `src/compiler/common/task_system/task_scheduler.cpp`：AP 路径是整块算子的顺序链，配合 `__SINGLE_THREADED__` 取消调度开销。它解释了 Neug 在微查询上的数量级优势；Linkrs 不改成单线程，但应减少小查询上的算子边界、row 复制和 multiplicity 展开。

### 7.3 对任务的修订

- 任务二增加“multiplicity 加权聚合”：窄形状下不在 Aggregate 入口展开 multiplicity，把 `chunk.multiplicity()` 传入 typed 局部聚合状态；保留 selection 与 opaque 边界的展开契约。
- 任务二增加“父批/孩子偏移契约”：单跳拓扑读取返回父选择向量和孩子前缀和，聚合直接消费，替代逐父批构造与 Project 中转。
- 任务三增加“DISTINCT limit 上限”和“typed TopN 并列精确处理”，两者都先补反例再改执行代码。
- 任务四增加自适应 morsel 数与列统计 min/max 下推；CPU 侧参考 Neug 的零对象迭代与显式预取，按度数分布选择策略。
- 任务五把 WAL 收敛目标改为“活动段 + 单个 frozen 段”，并把 COPY/批量导入后的强制 checkpoint 纳入验证。
