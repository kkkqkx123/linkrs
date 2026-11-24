# group_by_count 的 Exchange 前局部 TopK 设计

**日期**：2026-11-24  
**依据**：`docs/plan/packed-expand-local-sink-design.md` 的验证结论与 `docs/issue/linkrs-remaining-parity-issues.md` 第 43 节。本文只描述本次窄形状优化的范围、正确性边界与验证方法。

## 1. 问题

`group_by_count` 在分区内完成 `PartialAggregate` 后，每个分区输出约 1000–1260 行局部组结果，Exchange 累计约 11,554 行。查询是：

```cypher
MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(p:Person)
RETURN p.id, count(h) ORDER BY p.id LIMIT 10
```

排序键就是分组键 `p.id`。某个分区内排不进前 10 的组键，前面至少有 10 个更小的组键；这些键都会进入最终聚合，因此该键不可能进入全局前 10。可以在 Exchange 前按分组键精确裁剪到 k 行。

## 2. 放行条件

`build_partition_local_fragments()` 只在以下条件全部成立时插入新算子：

- 存在 split aggregate；
- `chain.global[1]` 是紧跟聚合的 `TopN`；
- 聚合为单分组键、单个 `COUNT`、无聚合过滤；
- `TopN` 的 limit 大于 0；
- `TopN` 为单列排序，排序列与分组键文本一致；
- 排序方向为升序或降序。

不满足任一条件时不插入算子，计划保持原状。

## 3. 执行契约

新增 `BlockingSpec::LocalTopKRows` 与对应 `BlockingOperatorKind`，位于每个分区 fragment 的 `PartialAggregate` 之后、Exchange 之前。

状态维护：

- 按 key 去重的至多 k 行；
- key 从布局中的键槽位读取；
- 比较使用与全局 TopN 相同的 `compare_values` 语义：NULL 在升序中最后、降序中最前；
- 新行优于当前最差行时替换，重复 key 不占用额外名额；
- 输出前按键排序，通过既有 `emit_batch_slice()` 发出 row batch。

最终 `FinalAggregate`、`TopN`、`Limit` 全部保留，Exchange 仍接收 row batch。该优化只减少 Exchange 载荷，不改变最终结果。

## 4. 不做项

- 不改 Exchange 的 Concatenate / row 契约；
- 不合并分区哈希表；
- 不放宽多列分组、聚合过滤、非 `COUNT` 聚合或排序键不一致的形状；
- 不做截断采样或近似去重；
- 不改变 NULL、并列值、空分区与 k 大于局部行数的语义。

## 5. 验证

1. devs release 编译；
2. 正确性门全绿，包括 typed group、local topk、parallel、DQL、storage lib、column batch；
3. 固定门两轮与上一轮 `40.17 / 40.58 ms`、`34.84 / 35.30 ms` 对比；
4. EXPLAIN 校验 `actual=10`、`result_stable=true`、`spill_rows=0`，观察 Exchange rows 从约 11,554 降到不超过 100；
5. 生产默认单轮九条纯计数与 q1–q9 无回归。

结果按 `docs/dev/build/devs-sync.md` 写入本地 `perf-data/parity/results/2026-11-24/group-by-local-topk/` 与 devs 对应目录。

## 6. 验证结论

首轮编译暴露了一个返回引用生命期缺失和一处 `next_rows` 调用参数顺序错误，均已直接修正。devs release 编译通过，正确性门全绿：配置默认值、partitioning、typed group、local topk、parallel、DQL、storage lib、column batch 全部退出码为 0。

最终轮 EXPLAIN 显示每个分区在 `PartialAggregate` 后新增 `LocalTopKRows`，每分区输出 10 行；Exchange 从约 11,554 行降到 100 行，`actual=10`、`result_stable=true`、`spill_rows=0`。固定门两轮 `group_by_count` 为 39.59 / 33.89 ms，`distinct_limit` 为 37.95 / 32.80 ms；生产默认单轮为 31.86 / 29.51 ms。相比上一轮 `40.17 / 40.58 ms` 与 `34.84 / 35.30 ms`，固定门有改善，但轮间波动仍存在。

判定：保留 `LocalTopKRows`。它只裁剪 Exchange 前的局部组结果，全局 FinalAggregate、TopN、Limit 仍在，结果逐值一致；不需要改 Exchange 契约或做近似。
