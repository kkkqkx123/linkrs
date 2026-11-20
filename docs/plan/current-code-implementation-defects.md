# 当前代码实现缺陷分析

**日期**：2026-11-21  
**范围**：只讨论当前代码已经声明或半实现的功能是否存在实现缺陷。本文不评价改进项是否值得立项，也不把“收益尚不明确”写进缺陷结论；只有会造成错误结果、契约不一致或不可验证行为的点才列为缺陷。

## 1. order-limit 逻辑改写与物理执行脱节

`PushLimitDownIndexScanRule` 与 `PushTopNDownIndexScanRule` 会在逻辑 `IndexScanNode` 上设置 `limit/order_by`，但物理转换 `plan/arena_builder/specs/source.rs` 没有把这两个字段传入 `SourceSpec::IndexScan`，`IndexScanPlan` 构建处始终写 `limit: None, offset: 0`。

这是实现缺陷：优化器声称完成了 limit 下推，执行层完全没有消费。它影响可观测性和后续推导，必须修复，与是否继续投入 order-limit 优化无关。

修复内容：

- `IndexScanNode` 增加 `offset`；
- `SourceSpec::IndexScan`、`SourceOperatorKind::IndexScan` 携带 `limit/offset`；
- `build_index_scan_plan` 使用传入的 `limit/offset`；
- `PushLimitDownIndexScanRule` 设置 `offset` 与 `count`，而不是把 `offset + count` 错当成 emit 数量。

## 2. index cursor 不保证全局有序

`ChainForwardIterator` 按 generation 顺序依次读取，每个 generation 内部按键有序，但 newest generation 的 key 10 可能先于 older generation 的 key 5 被返回。虽然 `seen/tombstoned` 解决了 MVCC 可见性，却没有解决跨 generation 的全局排序。

`VertexIndexCursor` / `EdgeIndexCursor` 又按 shard 顺序串联多个 iterator。manifest 已证明 shard 范围有序且连续，所以 shard 串联本身可以保持全局有序；问题只在 generation 串联。

这是实现缺陷：任何依赖 index 顺序的语义都不能建立在这种游标上。修复方式是把 generation 合并改成按 key 的 k 路归并，并保持 newest generation 对同 key 的优先语义。

## 3. limit 规则缺少 offset 语义

`PushLimitDownIndexScanRule` 原先把 `offset + count` 写入 `IndexScanNode.limit`，但 `IndexScanPlan` 的 `limit` 表示“跳过后发出的行数”，`offset` 是独立字段。当前实现既没有传 offset，也把 limit 写成总数，语义不一致。

这是实现缺陷，修复见第 1 节。

## 4. 异常数值未防御

`PushLimitDownIndexScanRule` 原先直接计算 `offset + count`。`LimitNode::UNBOUNDED` 为 `i64::MAX`，存在溢出；负数也没有拒绝。这属于实现健壮性缺陷，已在本轮加入 checked conversion 并拒绝负值。

`PushTopNDownIndexScanRule` 原先允许非正 limit 进入改写，也已拒绝。

## 5. 证明 token 未进入物理可观测面

`IndexOrderProof` 已在逻辑节点生成，但如果物理层只显示 `IndexScan`，EXPLAIN 无法区分有证明和无证明的计划。这不是立项判断问题，而是可观测性缺陷。已把 proof reason 写入 `SourceSpec` 和 EXPLAIN。

## 6. 修复状态

上述缺陷已在本轮全部修复：

- `IndexScanNode` 增加 `offset`；limit/offset 贯穿 SourceSpec、SourceOperatorKind 与 `IndexScanPlan`；
- `PushLimitDownIndexScanRule` 分别设置 offset 与 emit count，并拒绝 UNBOUNDED、负数、零 count；
- `PushTopNDownIndexScanRule` 清零 offset，并要求 `IndexOrderProof`；
- `ChainForwardIterator` 改为跨 generation 的 k 路归并，保持全局 key 顺序与 newest 优先的 MVCC 语义；
- EXPLAIN 显示 proof reason 与 pushed limit/offset；
- 新增跨 generation 有序、newest 优先、cursor offset/limit 与规则守卫测试。

这些修复不改变任何改进项的立项判断；它们只是让已声明的功能完整且正确。

修复后重新执行完整验证：query lib 2313 passed、storage lib 1412 passed、gate 全绿。固定门 `distinct_limit` 93.22 / 93.56 ms，`group_by_count` 99.41 / 98.79 ms，`actual=10`、结果逐值一致、`spill_rows=0`。指标目录：`perf-data/parity/results/2026-11-18/implementation-defects/`。

## 7. 与改进项立项无关的结论

以下不属于实现缺陷，不写入本文的修复任务：

- min/max 结果下推是否值得做；
- id-range 下推收益是否足够；
- 元数据计数是否应该作为强一致结果；
- order-limit 作为优化项的优先级。

这些由设计文档讨论；当前代码是否完整、是否会产生错误结果，不应影响它们的立项判断。除非正确性构成硬约束（如 index cursor 全局有序），否则实现缺口应作为独立任务修复。
