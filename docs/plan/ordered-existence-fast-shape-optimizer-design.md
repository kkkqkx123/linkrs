# 有序存在性形状的快速优化路径设计

## 1. 目标

`distinct_limit` 改写命中后，重复执行仍在优化阶段付约 0.84 ms。分项显示
`cost_based` 与 `physical_heuristic` 占主导，而统计采样与目录检查为零。
本方案只处理这一类已证明形状，降低重复查询的优化成本，不改变通用优化路径。

## 2. 适用形状

只允许严格的三层前缀加已有线性链：

```text
Limit(offset=0) -> TopN -> Dedup -> [Project/Filter]* -> 单跳 ExpandAll -> Scan
```

且 `Limit` 的 count 不超过 `TopN` 的 limit，`TopN` 与 `Dedup` 各只出现一次，
三者同时出现。其余形状继续走完整优化路径。

这个形状与 `OrderedExistenceScan` 的形状证明一致：单列 distinct、单列升序
主键、零偏移的小 limit、固定边类型、无 filter/flatten。计划缓存命中时重复跑
unnesting、join order、TopN wiring 与聚合策略属于无效工作。

## 3. 修改范围

- 扩展优化器的 `is_fast_shape` 门，识别上述严格前缀；现有 Project/Filter/单跳
  Expand/Scan 的快速路径保持不变。
- 命中快速路径后沿用已有的精简 cost pass 与 fast heuristic 规则集，不新增规则。
- 被精简掉的成本阶段必须在该形状上无作用：无子查询可 unnest、无 join 可重排、
  `Sort+Limit` 已在本层转换为 TopN、无 aggregate 可选策略。
- 不修改优化器结果缓存与执行引擎；保留了 `LINKRS_NO_FAST_SHAPE` 回退开关。

## 4. 正确性约束

- 新增单测直接比较同一形状在完整路径与快速路径下的计划树与诊断 notes，
  要求逐项一致。
- 既有的“快速路径必须保留全部 note”和“复杂形状仍被拒绝”测试继续成立。
- 结果必须逐值等于 `[14]`，`spill_rows=0`，A/B 关闭/开启改写一致。

## 5. 验证与判定

在 devs 上按资源对齐口径编译并采集：

- 优化阶段 `OPT_STAGE` 中 `cost_based` / `physical_heuristic` 应显著下降；
- bench JSON 的 `optimize_median_us` 应明显下降，`plan_cache_hits` 仍为 5/5；
- `exec` / `execute-stage` 不应因优化路径变化而退化；
- partitioning、local_topk、parallel、dql、storage 全量门保持通过。

若任一结果退化，则回退此门，不扩展到其他形状。

## 6. 实测结果

devs 实测中门确实命中，EXPLAIN 出现 `fast_shape:` 与
`redundant_global_ops=eliminated`。目标查询热态 `optimize_median_us` 从
约 870 us 降至约 554–593 us，`compile_median_ms` 从约 0.96 ms 降至
约 0.67–0.72 ms；`exec` 从约 1.76 ms 降至约 1.56 ms。A/B 两侧结果仍为
`[14]`，生产默认用例结果无退化。`OPT_STAGE` 中完整规则批次的
`physical_heuristic` 被单 batch pass 的 `fast_heuristic` 替代；
TopN wiring 在该形状上仍执行，保证计划与完整路径一致。

证据位于 `perf-data/parity/results/2026-11-25/ordered-existence-remediation/fast-shape/`。
