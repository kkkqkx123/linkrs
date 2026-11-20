# 下一阶段改进项模块设计审查

**日期**：2026-11-21  
**范围**：分区执行、局部去重 TopK、typed 聚合、存储统计与谓词下推、WAL/checkpoint 资源回收。本文只记录设计判断与调整顺序，不含完整代码。

## 1. 总体结论

当前实现在固定门形状上结果正确、收益明确，但部分模块仍带有基准适配色彩：

- 正确性依赖运行时退化，而不是计划期证明；
- 分区数上限由基准参数冻结，planner 的成本模型没有吸收这部分经验；
- 列统计没有区分“裁剪用保守边界”和“结果用精确边界”；
- 配置面没有暴露 checkpoint/快照保留策略。

因此继续优化前应先补齐契约与可观测性，再开放新的下推路径。不建议整体重写模块，建议按本文列出的点局部调整。

## 2. 局部 Distinct TopK

### 2.1 当前问题

`LocalTopKDistinct` 为了在运行时遇到 NULL 或非定长键时仍能精确退化，保存了完整 `seen_values`。这使内存复杂度从 O(k) 变成 O(n)，与设计目标不一致。

退化路径还用 `Debug` 字符串排序。最终顺序仍由全局 TopN 决定，所以结果不会错，但本地顺序没有语义依据，且把类型比较规则复制成了第二套实现。

### 2.2 调整方案

- 首块决定执行模式：
  - 若 key 列可由 typed I64 列提供，进入 typed 模式，只保留 k 个候选键；selection 通过可见索引 gather，multiplicity 不影响去重集合；
  - 否则进入 pass-through 模式，原样输出所有行，由 Exchange 之上的全局 Distinct/TopN 负责去重和排序。
- typed 模式后续块必须继续提供同一 typed key 列；否则返回明确的计划契约错误，不再静默退化。这样避免出现“已经丢弃早期行、事后又需要全量集合”的状态。
- 删除完整 `seen_values` 和基于 `Debug` 的排序。

### 2.3 计划期证明

当前仅通过列名字符串匹配判断排序键与去重键相同。建议后续给计划节点补充列的逻辑类型与 nullable 信息，只有非空定长数值键才下发该算子，并在 EXPLAIN 中记录 `pushed/unpushed` 及原因。当前先用首块模式选择保证正确性。

## 3. TopN limit

`build_topn_spec` 直接把 `i64` 转成 `u32`。`offset + count` 的正常路径安全，但异常大值或负值会回绕。应改为 checked conversion；转换失败时不进入分区下推路径，也不生成错误物理计划。

## 4. 分区数与枚举

`ShardSlice` 按 shard local-id 高水位切分，枚举边界本身没有问题：每个分区只扫描自己拥有的区间，staged 行也按同一规则归属。

问题在成本模型。`desired_partition_count` 只考虑行数、最小分区行数、worker 数和经济下限，没有计入：

- 每分区固定启动和交换成本；
- 多 worker 共享内存带宽后的边际收益递减；
- 分区偏斜导致的长尾。

`--max-partitions 10` 是基准侧的经验修正，不应长期掩盖 planner 问题。建议把每分区固定成本纳入 CBO，并将该参数保留为诊断覆盖入口。

## 5. typed 聚合

`typed_group_count` 对 `COUNT` 参数要求存在 typed column。边身份、顶点身份等非空列在通用路径里已有 `count_identity_arg` 证明，但 typed 状态没有复用，导致这类 `count` 仍回退通用表。

另外无分组 count-only 计划会把 `COUNT` 重写为 `SUM(_expand_count)`，typed 状态目前只接受 `Count`。建议：

- 把“参数非空”抽象为显式 validity proof；
- identity 列直接得到 `AlwaysValid`；
- typed 状态支持 `Count` 与 `Sum(_expand_count)` 的定点累加；
- 保留 selection 与 opaque 边界的既有展开契约。

## 6. 存储统计与微扫描下推

### 6.1 min/max

`ColumnStatsSnapshot::min_value/max_value` 来自 zone-map 聚合，注释明确说明边界只会 widening，不等于精确 min/max。直接返回给聚合会得到错误答案。

建议把统计拆成两类：

- `pruning_min/max`：仅供谓词裁剪，可以保守；
- `exact_min/max`：只有证明无删除、无 TTL、无 pending write、单关系单列时可用。

没有 exactness token 前，min/max 下推保持关闭。

### 6.2 filter_id_range / order_limit

存储谓词比较器与表达式 evaluator 必须逐类型对齐，包括 Int/BigInt、NULL、NaN 和字符串顺序。`order_limit` 下推到 id 索引还要求索引顺序与 TopN 的 NULL 和并列规则一致。当前 `n.id > 0` 覆盖全集，收益上限很小，不应为了这点收益先放宽证明。

### 6.3 undirected_count

单类型、同标签、无 TTL、无采样的窄形状可用出入度减自环计算，但 oracle 必须继续按种子侧公式生成。元数据计数进入执行路径前要有计划标记和 pushed/unpushed 对拍。

## 7. WAL 与 checkpoint

当前 WAL 回收判据是“header checkpoint seq 较旧，或者 start LSN 不超过安全 LSN”。审查后确认：`set_checkpoint_seq` 会在每次 checkpoint 刷新活动文件头，因此 header seq 严格较旧的段一定是该 checkpoint 之前关闭的；这条判据可以安全保留。实测只使用 LSN 判据时，部分 checkpoint 的 safe LSN 没有前移，WAL 从 2 个增至 6 个、库体积超过 50 MB，因此最终实现保留两条判据，并保留一个 retired 段作为回退。

另外 `checkpoint_retention_count` 目前是内部配置，而快照保留数被绑定到同一个值。建议拆成：

- `checkpoint_retention_count`：恢复点数量；
- `snapshot_retention_count`：手动/自动快照数量。

回收失败只记录日志是合理的，因为持久性优先于体积；但需要把失败次数纳入诊断指标。

## 8. 可观测性

EXPLAIN 已能看到 `LocalTopKDistinct`，但缺少 pushed/unpushed 原因；parity JSON 有 spill 和资源字段，但缺少：

- 每分区 work_time / wall_time 偏斜；
- typed 聚合是否命中；
- WAL/checkpoint 回收失败次数。

这些字段是后续判断优化是否有效的前提，应与下一批下推一起补齐。

## 9. 调整顺序

1. 先修正确性契约：LocalTopK O(k) 模式、TopN limit checked conversion、WAL LSN 主判据。
2. 再修配置边界：拆分 checkpoint/snapshot 保留数。
3. 补 typed identity validity、计划标记、分区 work time 与 typed `SUM(_expand_count)`。
4. CBO 已用固定成本/每分区成本选择分区数；微扫描下推仍需 exactness token 与 comparator 统一证明。
