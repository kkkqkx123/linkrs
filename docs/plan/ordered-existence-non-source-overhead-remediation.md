# 有序存在性扫描：源算子外剩余开销治理计划

**日期**：2026-11-25  
**范围**：只处理 `distinct_limit` 命中改写后仍存在、但不在
`OrderedExistenceScan` 源算子内部的开销。源算子内的候选批次与批量 external id
解析已经优化完成，本计划不重复调整。

## 1. 背景

资源对齐后的最终数据（`perf-data/parity/results/2026-11-25/ordered-existence/`）：

| 项 | 耗时 |
|---|---:|
| Ladybug 内部执行 | 3.20 ms |
| Linkrs `exec`（含编译） | 1.80 ms |
| Linkrs `execute-stage` | 1.70 ms |
| 编译 | 0.96 ms |
| 源算子 `next`（游标 + 存在性门） | 0.57 ms |
| 源算子之外（推算） | 约 1.13 ms |

源算子内部已经从最初的约 7.3 ms 存在性门降到约 0.52 ms。当前若继续压缩
端到端，重点应转向执行实例装配、冗余全局算子、结果物化以及编译阶段。

本计划不通过放宽并行、近似、截断、缓存过期目录或改小数据集换取收益。
任何改动都必须保持结果 `[14]`、严格形状证明与 A/B 回退能力。

## 2. 问题清单

| 编号 | 问题 | 证据 | 预期方向 |
|---|---|---|---|
| 全局算子 | 源已保证有序、去重、最多 k 行，但计划仍保留 `Distinct` / `TopN` / `Limit` 三个全局算子 | EXPLAIN 中三个算子 `rows:10`，自耗时被合并计入源之后 | 减少两层缓冲与逐批检查 |
| 执行装配 | `QueryExecutionInstance::instantiate_plan` 每次做结构校验、arena 物化、runtime/engine/sink 构建 | `execute-stage` 减源算子后仍有约 1.13 ms | 拆分计时，再决定是否复用不可变骨架 |
| 结果物化 | `ResultSink::Materialize` 把 10 行整数键装入通用 `DataSet` | 单行小结果仍走通用分配路径 | 仅对已证明形状做窄路径 |
| 编译 | 同一查询每次执行仍付约 0.96 ms 解析/计划成本 | bench 单列 compile 中位数 | 区分冷编译与缓存命中，优先复用已编译计划 |
| 计时异常 | EXPLAIN 的 `ordered_existence_open_us` 显示秒级，直测仅约 0.11 ms | `final-detail.log` 与 `explain-prod3/` | 先修观测，避免误判瓶颈 |
| 批次通用性 | 当前批次为 `limit.clamp(1, 16)`，仅对 limit=10 做了密集验证 | 现有差分只覆盖目标查询 | 补稀疏命中与大 limit 的形状测试 |

## 3. 修改方案

### 3.1 移除已证明冗余的全局算子

`build_ordered_existence_group` 目前把 `chain.global` 全部压回全局算子。对
命中有序存在证明的形状，源算子已经保证单列唯一、升序、最多 k 行；
`safe_global_tail_after_topn` 也已证明上层只允许顺序无损的投影或前缀 Limit。

修改方向：

- 对证明命中的链，只保留源算子与必要的输出投影；不再生成
  `Dedup`、`TopN`、`Limit`。
- 若物理输出契约要求保留根算子 id，用一个零缓冲的契约算子代替三者：
  只断言输入行数不超过 k、键严格递增且无重复，并透传 chunk；
  断言失败时记录诊断并回退语义，不能静默丢行。
- EXPLAIN 继续显示 `ordered_existence:proven` 与冗余算子已消除的说明；
  普通路径的全局算子逻辑不变。

验证：分区计划单测确认命中形状不再生成三者；反例形状仍按原计划执行；
`Distinct/TopN` 的删除不得改变 output layout、列名或结果顺序。

### 3.2 拆分执行实例装配与结果物化计时

当前 `execute-stage` 是粗粒度值，无法区分装配与执行。先在诊断路径增加
可选的低开销阶段计时（用环境变量开启，默认关闭），落点包括：

- `PhysicalPlanValidator::validate` 与 `PhysicalPlanMaterializer::materialize`；
- `QueryExecutionInstance` 创建 runtime、engine、sink 与注册；
- 算子 `open` / `next` / `close` 的聚合边界；
- `ResultSink::Materialize` 的结果集构造。

计时只在有诊断消费者时启用，不进入热路径默认分支。拿到分项后，若装配占
主导，再考虑缓存“不可变执行骨架”；若物化占主导，再考虑窄形状的结果构造。
不以猜测直接改热路径。

### 3.3 编译与计划缓存口径

编译约 0.96 ms，已经接近执行阶段本身。先确认这 0.96 ms 中真正的新编译
比例，而不是把缓存查询、统计采样与目录检查混在一起。

修改方向：

- 在 bench JSON 中分开记录冷编译、缓存命中与统计/目录检查耗时；
- 用同一 session 重复执行同一查询文本做 A/B，验证 `plan_cache_context`
  的命中路径是否确实生效；
- 若命中仍有固定成本，再评估按“查询文本 + 空间 + 目录版本 + 读上下文”
  复用已编译物理计划，读上下文中存在 staged write 时必须失效。

本项不改变 SQL 语义，也不缓存结果数据。

### 3.4 修复 EXPLAIN 的 open 计时异常

`ordered_existence_open_us` 在 EXPLAIN 中多轮显示 7.5–13.1 秒，但同点直测
约 0.11 ms，查询总耗时仅 1.8 ms。先定位是统计快照读错实例、跨查询累计、
还是 profiling 路径下真的发生锁等待。

修改方向：

- 增加单测：EXPLAIN 后读回的 `open_us` 必须与该次执行的总耗时同量级；
- 对比 `overlay_execution_profile` 使用的 runtime 与源算子更新 stats 的
  runtime 是否为同一实例；
- 在确认根因前，EXPLAIN 的该字段只作诊断，不参与性能验收判断。

### 3.5 批次通用性与安全边界

当前 `batch_size = limit.clamp(1, 16)` 是为了避免一次给存在性门过多候选。
需要证明它对不同 limit 与命中密度都安全：

- 增加 limit 为 1、10、17、100 的源算子单测，覆盖前 k 个连续命中、
  稀疏命中、全部不命中三类分布；
- 断言输出始终是按主键升序的前 k 个存在结果，并且 `candidates` 不会把
  不存在的候选误判为命中；
- 对 limit > 16 的查询，验证分页续读后 generation 与快照漂移仍会关闭早停。

若某类分布出现回退或结果漂移，优先缩小批次上限，而不是放宽证明。

## 4. 验证门槛

每项改动单独 A/B，复用资源对齐口径：

- 绑定：`numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`；
- warmup 1 + run 5，去最大值均值；同时记录中位数；
- 结果必须逐值等于 `[14]`，`result_stable=true`，`spill_rows=0`；
- EXPLAIN 断言改写已启用：`ordered_existence:proven` 或 `redundant_global_ops=eliminated` 出现，`ordered_existence_open_us` 合理；差分测试由单测覆盖，不再保留运行时 A/B 环境变量；
- 现有 partitioning、local_topk、parallel、DQL、storage 全量门保持通过；
- 性能回复只接受可复现的下降；单次波动不作为结论。

优先级顺序：先修计时异常与批次通用性以保证判断可靠，再做全局算子消除，
最后根据分项计时决定是否动装配/缓存/物化路径。
