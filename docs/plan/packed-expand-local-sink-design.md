# Packed Expand 与局部阻塞算子直连设计

**日期**：2026-11-24  
**依据**：`docs/plan/parent-identity-batch-resolution-design.md` 第 6 节、`docs/issue/linkrs-remaining-parity-issues.md` 第 42 节。本文描述已实施的直连契约、边界与验证结论。

## 1. 当前计划边界

分区计划为：

```text
StorageScan → Filter/Project/ExpandAll → LocalTopKDistinct / PartialAggregate
            → Concatenate Exchange → 全局 Distinct/TopN 或 FinalAggregate/TopN
```

Exchange 保持行式 `DataChunk` 契约：Concatenate 校验 schema，必要时 materialize typed rows；局部阻塞算子输出 row batch。该契约未改变。

## 2. 已实施直连

在 `DataChunk` 上增加可选 `PackedExpandPayload`，只在计划确认下游是窄形状局部阻塞算子时填充：

- `group_by_count`：单分组键、单个 `COUNT`、无聚合过滤；
- `distinct_limit`：单列 `Dedup` 紧跟同键 `TopN`，沿用既有 `LocalTopKDistinct` 放行逻辑。

负载携带投影后的目的端属性值（下游 key 表达式期望的值）和属性名，不携带拓扑数组。展开在过滤悬空目的端后直接生成 payload；`Project` 识别 payload 后仅替换 layout 并透传；`PartialAggregate` / `LocalTopKDistinct` 消费 payload 后继续输出原有 row batch。非定长或不支持的键类型回退为普通 row 路径。

计划层只选择语义已经证明安全的形状；不去掉全局 `FinalAggregate`、`Distinct`、`TopN`、`Limit`，不改 Exchange 契约，不合并分区哈希表或 TopK 状态。

## 3. 验证结论

首轮编译缺少 `PlanNode` trait import，第二轮 payload 未匹配 `p.id` 与 `id` / identity-only 两种来源，均已修正。最终轮 EXPLAIN 显示 `expand_output_us=0`、`typed_group_hits=110`、`local_topk:pushed`，证明 payload 已进入局部阻塞算子；`actual=10`、`result_stable=true`、`spill_rows=0`，DQL 324/324、storage 1415/1415、column batch 13/13 全绿。

固定门两轮为 `group_by_count` 40.17 / 40.58 ms、`distinct_limit` 34.84 / 35.30 ms；生产默认单轮为 41.06 / 30.73 ms。devs 负载在复测期间达到 9–10，visibility 微基准从约 61 us 升到 74–81 us，不能据此判定直连本身回退。判定为保留直连：它消除了可测的 `expand_output_us` 与 Project typed 重建，且没有正确性退化。

## 4. 后续

`group_by_count` 的 Exchange 前局部 TopK 需要新增保留 count payload 的局部算子，单独设计与验证，见 `docs/plan/group-by-count-local-topk-design.md`。
