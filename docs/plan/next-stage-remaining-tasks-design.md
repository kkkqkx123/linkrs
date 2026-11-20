# 下一阶段剩余差距任务设计

**日期**：2026-11-21  
**依据**：`docs/plan/next-stage-parity-gap-design.md` 的第 2–5 节与本轮回测结果。本文只描述剩余任务的设计边界、执行顺序和验收方式。

## 1. 已完成部分与剩余边界

首轮已恢复可比基线：基准入口在请求 worker 并行时默认打开有序并行展开，生产默认值不变；固定门用 EXPLAIN 校验 `actual` 与 `actual_partitions`，并回写资源字段。基准侧另加 `--max-partitions` 把经济分区数冻结为 10：本环境实测 16 个分区受内存带宽和交换开销影响反而更慢，该上限只作用于基准入口。typed 分组计数已支持 multiplicity 加权，分区内单列 DISTINCT + 同键 TopN 已在 Exchange 前收敛。

剩余工作不再重复上述改动，集中在三类：

1. 分区扫描、展开与局部聚合的进一步融合；
2. 局部 TopK 与精确去重的算子级合并；
3. 微扫描算子和库体积收尾。

## 2. 分区融合

分区枚举审计结论：`ShardSlice` 按每个 shard 的 local-id 高水位切成连续区间，`live_ids_in_slice` 只扫描本分区拥有的区间，staged 行也按同一归属规则过滤。因此当前分区没有全表枚举或重复 topology 解析，不需要再改枚举边界。

下一步只在窄形状上融合单跳展开和局部聚合：

- 输入是 rowless typed chunk，跳数为 1，无 hop filter、TTL、采样、属性需求和多标签歧义；
- 拓扑读取返回父批选择向量和孩子前缀和，展开结果直接以 typed 列呈现，不构造 `Vec<Value>` 行，也不经 Project 中转；
- 局部聚合状态直接消费 typed 分组键和父批 multiplicity；`count` 与 `sum(_expand_count)` 共用同一个定点累加状态；
- 聚合输出仍按现有 PartialAggregate/FinalAggregate 协议交给全局阶段，未放行形状保持原计划。

关键不变量：分区并集与串行结果逐值相等；selection 或 opaque 边界仍负责展开 multiplicity；属性需求、TTL 和多跳继续走原路径。

## 3. 精确局部 TopK

当前“分区 Distinct + 分区 TopN + 全局 Distinct + 全局 TopN”已经正确，但每个分区会物化约千级去重键。下一步改为单个 typed 局部 TopK 状态：

- 对单列定长键 `DISTINCT ... ORDER BY key LIMIT k`，分区内用有序集合保留最小或最大 k 个键，同时保留完整去重集合以便遇到 NULL/非定长键时精确退化；Exchange 后仍执行一次全局 Distinct/TopN，保证边界与并列处理一致；
- 对 `GROUP BY ... ORDER BY count LIMIT k`，仅当分组键由 Exchange 按 key 分区、每个 key 的完整聚合在分区内完成后，才允许局部 TopK；当前按行分区的 PartialAggregate 形状不放行；
- 排序方向、NULL 顺序和并列规则复用全局 TopN 的比较器，不引入第二套排序定义；
- 反例必须先于实现：重复键、并列值、NULL 键、空分区、k 大于局部行数。

不做近似去重、截断采样或“局部计数超过阈值即丢弃”的推断。

## 4. 微扫描算子

微扫描优化必须带计划标记，并要求 pushed/unpushed 数值一致：

- `min/max`：只有列统计携带“边界精确”标记、表无 TTL、无 pending write、无过滤且标签/边类型单一时，才允许从统计快照返回边界；zone-map 保守外延统计不得直接用于结果。任一前提失败回退完整扫描。
- `filter_id_range`：先把比较语义规范化到存储层的 typed comparator，再允许扫描直接输出满足范围的有序小批；当前 `n.id > 0` 覆盖全集，收益上限有限，优先级低于正确性证明。
- `order_limit`：TopN 下推到有序 id 索引前，必须证明存储索引顺序与 evaluator 的 NULL、数值类型和并列顺序完全一致。
- `undirected_count`：仅对单类型、同标签、无 TTL、无采样的窄形状考虑元数据计数；oracle 继续按种子侧出入腿减自环计算，不能把近似基数当作答案。

自适应 morsel 数和按度数预取属于 CPU 侧微优化，只有在上述窄形状的数值稳定后才进入实现。

## 5. 库体积与资源口径

首轮回测发现根因不是 WAL 回收条件，而是基准进程退出时没有显式关闭数据库：`checkpoint_on_close` 只在显式 close 时触发，反而每个基准进程多留一个新 WAL 段。修正方式：

- 基准完成所有计时与 EXPLAIN 后释放 session 并显式 `database.close()`；
- 每次成功 checkpoint 后保留最新点和一前一个回退点；WAL 只保留活动段加一个 retired 段；
- 旧 checkpoint 目录回收不得删除快照仍引用的序列；
- parity JSON 继续记录库体积、RSS 峰值、buffer pool 上限和 spill 计数。

验证门槛：恢复用例、容量超限用例和并发写入用例全绿；回收前后查询结果逐值相等；库体积降到 50 MB 以内。

## 6. 执行顺序与验收

建议顺序：先完成库体积根因修复并回测资源口径，再做局部 TopK 合并，最后按收益决定是否开放微扫描下推。每步都要求：

- 两轮固定门结果稳定，EXPLAIN `actual > 1`；
- 整数逐值相等，浮点相对误差不超过 1e-3；
- streaming、partitioning、storage 与 DQL 全量通过；
- 计划标记可读，pushed/unpushed 路径可分别测量。

## 7. 首轮回测结论

库体积根因已修复：显式 close 触发 checkpoint，快照保留与 checkpoint 保留对齐，WAL 收敛到活动段加一个 retired 段；固定门关闭自动快照后库体积约 33.5 MB。经济分区数冻结为 10，EXPLAIN 校验仍为 `actual > 1`。`group_by_count` / `distinct_limit` 尚未达到耗时门槛，剩余工作按第 2–4 节继续执行。

## 8. 精确局部 TopK 完成情况

`LocalTopKDistinct` 已替换分区 Distinct + TopN 两段流水线，定长键路径只保留 k 个候选，退化路径保留完整去重集合以保证 NULL 和非定长键的精确性。两轮固定门 `distinct_limit` 中位数降到约 84–88 ms，`group_by_count` 约 92–95 ms，库体积约 33.7 MB。第 2、4 项仍需精确性令牌和下推一致性验证后再开放。
