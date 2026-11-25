# 结果语义优先修复设计

**日期**：2026-11-24  
**范围**：只处理会改变查询结果、破坏已声明契约或让验证失去意义的问题。性能优化在这些问题关闭前不作为验收目标。

## 1. 审查结论

对查询执行、分区优化、packed 展开、身份直通、前端会话与流式通道做了一轮语义审计。结论分为三类：

1. **当前仍存在、会改变结果的问题**：两项，见第 2、3 节。
2. **历史分析记录过但当前代码已修复的问题**：JSON 客户端会话键、401 双触发、SSE 尾帧、DISTINCT 无 `ORDER BY` 的三方首值漂移、order-limit 逻辑与物理脱节、index cursor 跨 generation 不有序、事务内元数据计数计划缓存。逐项核对当前源码后确认修复存在，不重复改动。
3. **功能缺口而非语义错误**：跨源 ATTACH 查询、`LOAD FROM (query)`、`SET a = {map}`、`CALL option=value` 等属于未实现能力；不影响已支持语句的结果正确性，本次不改。

## 2. 分区局部 TopK 忽略上层算子

`local_distinct_topn` 和 `local_group_topn` 原来只检查 `Dedup/Aggregate` 后的第一个 `TopN`，没有检查 `TopN` 之上是否还有算子。若上层是 `Filter`、`Sort`、另一个 `Aggregate/Window/TopN` 等，分区内提前截断可能丢掉全局结果需要的行；只有不改变行集与顺序的投影可以安全地留在上方。

修复：

- 两个判定要求 `TopN` 之上的全局算子只能是 `Project`，或 `offset=0` 且 `count <= TopN.n` 的 `Limit`。前缀 `Limit` 只截取已排序结果的前缀，不会需要被局部 TopK 丢弃的行；带偏移、无上界或更大的 `Limit` 仍判为不安全。
- `local_distinct_topn` 对不安全尾部返回 `local_topk:unpushed,reason=unsafe-global-tail`，进入 EXPLAIN 诊断。
- `packed_expand_key` 复用该判定，因此不安全形状同时关闭身份直通，避免双重语义风险。

反例：`DISTINCT ... ORDER BY key LIMIT k` 之上再接 `Filter` 时，必须不生成 `LocalTopKDistinct`；同形状的 `LIMIT k` 前缀仍应保留局部 TopK。这两种分区物理计划单测都已补。

## 3. packed 键直通可能丢弃边属性

身份直通与 rowless packed payload 只输出分组键列。原证明只检查目的端需求为单个整数主键、目的端旁路列匹配，未检查边需求是否为“仅拓扑”。若计划仍需要边属性或整条边，这条路径会把下游需要的列丢掉。

修复：

- 运行时身份快路径要求所有 bypass 边列为空。`count(h)` 的整边变量需求只参与行数累加，不产生 bypass 列，因此仍可用；真正需要边属性的形状会产生 bypass 边列并回退。单列 `DISTINCT` / 单键 `count` 的证明形状不会同时要求整边值。
- rowless 非身份 payload 的 `eligible` 判定同时要求 bypass 边列为空；否则回退原列读取/逐边路径。
- 计划期证明不按 `edge_required_props` 的字符串编码猜测：整边变量在计数语义下是合法的，错误的猜测会关闭现有身份直通并让 `distinct_limit` 退回逐边路径。

该修复关闭了“键是主键 + 同时返回边属性”的形状，同时保留 `count(h)` 的已有直通.EXPLAIN 中 `expand_output_rows` 应保持每唯一键一行而不是回到每边一行。

## 4. 验证

修复后先跑结果语义门，再跑性能门：

- 分区物理计划：不安全全局尾部不得生成局部 TopK，诊断必须可见；
- `local_topk` / `typed_group_count` / partitioning 单测；
- DQL 全量、并行分区全量、storage 全量；
- 固定门两轮必须保持 `group_by_count [14|59]`、`distinct_limit [14]`、`result_stable=true`、`spill_rows=0`、`actual=10`；
- 生产默认单轮无结果漂移。

devs release 全门通过：构建、配置默认值、partitioning 49/49、typed group 9/9、local topk 11/11、parallel 27/27、DQL 全量、storage 全量、column batch 全绿，两轮固定门与生产默认均 `status ok`。固定门 `distinct_limit` 26.71 / 25.20 ms、`group_by_count` 29.15 / 27.98 ms，生产默认 32.30 / 35.41 ms；结果保持 `distinct_limit [14]`、`group_by_count [14|59]`，`result_stable=true`、`actual=10`、`spill_rows=0`。EXPLAIN 中 `distinct_limit` 的 `expand_output_rows=30473`，没有 `local_topk:unpushed`，说明前缀 `Limit` 没有误伤身份直通。语义门已全绿；这些耗时数字只用于确认没有退化，不是新的性能收益结论。

只有在这些门全绿时，后续 A/B 数字才有解释意义。
