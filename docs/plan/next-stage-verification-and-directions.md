# 下一阶段验证结果与修改方向

**日期**：2026-11-21  
**依据**：`docs/plan/next-stage-module-design-review.md`、`docs/plan/next-stage-remaining-tasks-design.md` 与 devs 回测结果。本文给出剩余验证的收口记录和后续修改方向，不引入新的“版本 2”模块。

## 0. 本轮已落地修改

- 计划期类型守卫：`LocalTopKDistinct` 只在去重列类型未知或为 `BigInt` 时下发；其他已知类型不再进入局部路径。EXPLAIN 的 `choice_reason` 改为 `local_topk:pushed:<reason>`，区分 `typed` 与 `runtime`。nullable 仍由运行时 pass-through 保证结果正确，不伪造非空证明。
- 可观测性：ProfileBoard 记录每个分区的 work time，PlanDescription/EXPLAIN 输出 `partition_work_us=[id:us,...]`，可直接计算偏斜。
- typed 聚合：identity `COUNT` 参数走 AlwaysValid；`SUM(_expand_count)` 与 `COUNT` 共用 typed 定点状态，deopt 时按模式生成 `Sum` 或 `Count` accumulator。
- 分区成本：`desired_partition_count` 在设置固定成本/每分区成本时按 `saving - overhead` 选择分区数；exchange 成本继续由 cost gate 负责，零成本配置保持原确定性结果。基准侧 `--max-partitions 10` 仍是诊断覆盖。

未开放的项：存储 exactness token 的生产端、id-range/order-limit 下推、近似元数据计数、typed 命中与回收失败计数。这些路径没有消费者或缺少精确性证明，详细证明义务见 `docs/plan/remaining-paths-proof-design.md`。

## 1. 本轮补充验证

除固定门外，补做以下边界验证：

- `LocalTopKDistinct`
  - 重复键、并列、limit 大于局部行数；
  - rowless typed chunk 与 selection gather；
  - NULL、字符串等非定长键从首块进入 pass-through，所有可见行原样输出，由全局 Distinct/TopN 兜底；
  - 零 limit 不保留候选。
- `typed_group_count`
  - identity 列作为 `COUNT` 参数时直接得到 AlwaysValid，不再回退通用表。
- 固定门
  - `--require-parallel-explain` 校验两路均为 `actual=10`、`actual_partitions=10`；
  - 六轮结果逐值稳定，`spill_rows=0`；
  - checkpoint、快照、WAL 数量与库体积在每轮 close 后收敛。

六轮中位数（`distinct_limit` / `group_by_count`，单位 ms）：

| 轮次 | distinct_limit | group_by_count |
|---|---:|---:|
| fixed gate 1 | 98.35 | 110.06 |
| fixed gate 2 | 127.76 | 131.36 |
| repeat 1 | 122.25 | 103.37 |
| repeat 2 | 119.63 | 116.78 |
| repeat 3 | 119.33 | 126.15 |
| repeat 4 | 81.88 | 83.45 |

合并中位数约 119.48 / 113.42 ms，范围 81.88–127.76 / 83.45–131.36 ms。所有轮次 `result` 一致（`[14]`、`[14|59]`），EXPLAIN 的 `actual=10` 与 `actual_partitions=10` 均成立。波动不能由结果差异解释；后续必须先补分区偏斜与系统负载口径，再判断性能变化。

资源收敛：约 42.1 MB，2 个 checkpoint（`manifests` 不算 checkpoint）、0 个快照、2 个 WAL 数据文件加 `checkpoint.meta`。已修正 gate 脚本的 checkpoint 计数条件。

日志和原始数据：

- `perf-data/parity/results/2026-11-18/design-review-final/`
- `perf-data/parity/results/2026-11-18/design-review-repeat/`

实现后重新收集两轮固定门（`design-review-implemented`）：

| 用例 | 第一轮 | 第二轮 |
|---|---:|---:|
| `distinct_limit` | 105.05 ms | 108.39 ms |
| `group_by_count` | 95.71 ms | 103.66 ms |

结果逐值一致，`actual=10`、`actual_partitions=10`，`local_topk:pushed:runtime` 在 10 个分区计划中均可见。分区 work time 示例范围约 77.8–106.3 ms，偏斜比约 1.37，说明跨轮波动主要来自分区长尾与系统负载，而不是结果不一致。库体积约 38.4 MB，2 个 checkpoint、0 个快照、2 个 WAL 数据文件。

## 2. 明确不做的修改

以下路径在证明完成前保持关闭：

- 不删除全局 Distinct/TopN；局部 TopK 只做裁剪，不负责 NULL 顺序和并列语义。
- 不恢复局部 TopK 的完整 `seen_values`；fallback 从首块 pass-through，不在中途退化。
- 不把 zone-map 的保守 min/max 当作精确统计返回给聚合。
- 不把 id 范围过滤或元数据近似基数当作精确查询结果。
- 不把生产默认分区数改成 10；该值只在基准入口冻结。
- 不做近似去重、采样截断或“局部计数超阈值即丢弃”的推断。

## 3. 修改方向

### 3.1 计划期证明与可执行标记（部分完成）

目标模块：`planner`、`streaming plan builder`、`EXPLAIN metadata`。

- 在分区链上给 `LocalTopKDistinct` 增加显式的 `pushed/unpushed` 原因，而不是只靠运行时首块探测。
- 计划节点需要携带 key 列的逻辑类型与 nullable 信息；只有单列非空定长数值键才下发 typed 模式。
- EXPLAIN 输出 `local_topk=pushed|unpushed,reason=...`，便于逐形状对拍。
- 未证明形状直接走原 Distinct + TopN，不让局部算子进入计划。

验收：NULL 键、Utf8 键、多列 DISTINCT、selection/opaque 边界均显示 `unpushed`，结果与串行一致。

### 3.2 可观测性补齐

目标模块：`runtime stats`、`parity bench JSON`。本轮已完成分区 work time 与 EXPLAIN 字段；typed 命中与回收失败计数仍未开放。

新增字段：

- 每分区 work_time / wall_time、偏斜比；
- typed 聚合命中次数与回退次数；
- WAL/checkpoint 回收失败次数；
- 局部 TopK 的 pushed/unpushed 计数。

用途：解释固定门的波动。当前同一构建中 `distinct_limit` 中位数跨轮约 94–128 ms，`group_by_count` 约 103–131 ms，单轮结果不足以判断收益。

### 3.3 分区成本模型（部分完成）

目标模块：`optimizer/partitioning/planner/core.rs`。

- 把每分区固定成本、per-partition rows、exchange row cost 与带宽争用系数纳入 `desired_partition_count`。
- `--max-partitions 10` 保留为基准诊断覆盖，不作为生产默认。
- 用偏斜指标验证 CBO 选择的分区数；若 16 分区仍因带宽争用退化，应在成本模型中体现，而不是在基准侧硬冻结。

验收：移除基准侧覆盖后，planner 在 SF0.1 上稳定选择经济分区数，EXPLAIN `actual > 1` 且耗时不退化。

### 3.4 存储统计与微扫描下推

目标模块：`storage stats_reader`、`storage predicate`、`query pushdown`。

- 将 `pruning_min/max` 与 `exact_min/max` 拆开，并引入 exactness token：无删除、无 TTL、无 pending write、单关系单列。
- 统一 storage typed comparator 与 expression evaluator 的 Int/BigInt、NULL、NaN、字符串顺序语义。
- `order_limit` 只有证明索引顺序与 TopN 的 NULL/并列顺序一致才开放。
- `undirected_count` 只在单类型、同标签、无 TTL、无采样的窄形状使用元数据计数；oracle 继续按种子侧公式生成。

验收：每种下推都有 pushed/unpushed 对拍，整数逐值相等、浮点相对误差不超过 1e-3。

### 3.5 typed 聚合扩展（已完成窄形状）

目标模块：`typed_group_count.rs`、`aggregate.rs`、`partial_aggregate.rs`。

- 把“参数非空”做成显式 validity proof，identity 与字面量 star 都归入 AlwaysValid。
- typed 状态支持 `SUM(_expand_count)` 定点累加，与 `COUNT` 共用同一分组状态。
- 保持 selection、multiplicity 与 opaque 边界的既有展开契约。

验收：无分组 count-only 与 `count(h)` 都能命中 typed 窄路径，且与通用表结果逐值一致。

### 3.6 资源回收

目标模块：`persistence_coordinator.rs`、`wal/writer/local/file_ops.rs`。

- checkpoint 与快照保留数已经拆分；下一步把回收失败次数暴露到诊断指标。
- WAL 回收继续使用 checkpoint seq 与 safe LSN 两条判据，并保留一个 retired 段。
- 回收前后查询结果逐值相等，库体积继续低于 50 MB。

## 4. 执行顺序

1. 计划期证明与 EXPLAIN 原因；
2. 可观测性字段；
3. typed 聚合 validity proof 与 `SUM(_expand_count)`；
4. CBO 分区成本；
5. 存储 exactness token 与微扫描下推；
6. 资源回收失败指标。

每一步都必须执行固定门、全量测试与 pushed/unpushed 对拍；未通过证明的下推保持关闭。
