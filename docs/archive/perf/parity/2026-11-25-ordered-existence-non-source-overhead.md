# 有序存在性扫描：源算子外开销治理结果（2026-11-25，devs）

## 1. 范围与口径

本文对应
`docs/plan/ordered-existence-non-source-overhead-remediation.md` 的落地与复测。
目标查询、绑定方式、warmup 1 + run 5 去最大值均值、16 线程与 64 MB buffer
均沿用 `docs/archive/perf/parity/2026-11-25-distinct-limit-ordered-existence.md`。
devs 代码按 `docs/dev/build/devs-sync.md` 全量同步，HEAD 仍为 `fe92c527`，
所有改动以工作树差异随包传输；构建与采集脚本为
`scripts/devs-ordered-existence-measure.sh`。

## 2. 代码修改

| 项 | 修改 |
|---|---|
| 冗余全局算子 | `build_ordered_existence_group` 不再生成 `Distinct` / `TopN`，改为一个零缓冲的 `OrderedExistenceContract` 边界：断言键严格递增且唯一，超额行按语义截断并记录诊断；投影与末尾 `Limit` 保持原布局与算子链路 |
| 契约退化语义 | 快照/generation 漂移导致源算子关闭早停时，契约边界仍按有序主键语义保留前 k 行；重复键记录诊断并去重，逆序键直接报错，不静默丢行 |
| 诊断计时 | `LINKRS_STAGE_TIMING=1` 开启实例装配、引擎执行、反馈、结果物化、算子 open/next/close 与索引游标的分段计时；默认关闭，不进入热路径 |
| 编译口径 | `CompileTimes` / `StageMetrics` / 结果元数据新增 cache probe、plan cache hit 与 parse/plan/optimize/statistics 分项；bench JSON 逐次记录并给出中位数与命中次数 |
| open 计时异常 | 游标打开拆分为 build / lock / cursor 三段；EXPLAIN 后的诊断脚本校验 `ordered_existence_open_us` 与执行同量级，该字段继续只作诊断 |
| 批次通用性 | 抽出纯函数 `select_emitted`，新增 limit 为 1、10、17、100 的连续命中、稀疏命中、全不命中与关闭早停测试；新增命中形状不再生成 `Distinct` / `TopN` 的计划单测 |

## 3. 最终指标

devs 最终复测（`perf-data/parity/results/2026-11-25/ordered-existence-remediation/`）：

| 指标 | 值 |
|---|---:|
| `exec`（含编译） | 1.76 ms |
| `execute-stage` | 1.66 ms |
| `compile` | 0.96 ms |
| `ordered_existence_candidates` | 10 |
| `ordered_existence_matches` | 10 |
| `ordered_existence_output_rows` | 10 |
| `ordered_existence_drift` | 0 |
| `spill_rows` | 0 |
| 结果 | `[14]` |

同库 A/B：`off` 的 `exec` 30.98 ms（`execute-stage` 30.76 ms），`on` 的
`exec` 1.82 ms（`execute-stage` 1.71 ms），两侧结果逐值一致。与原最终数据
（`exec` 1.80–1.95 ms）相比，绝对收益在单次波动范围内；主要收益仍是改写本身，
本次改动的价值在于去掉不必要的全局算子并量化剩余开销位置。

计划形状从四层缩减为三层：

```text
OrderedExistenceScan -> OrderedExistenceContract -> Limit
```

EXPLAIN 显示 `ordered_existence:proven` 与
`redundant_global_ops=eliminated`，表中已无 `Distinct` / `TopN` 行。
`group_by_count [14|59]`、`join_two_hop [382018]`、
`variable_path_count [400153]` 与其余生产用例结果均无退化。

## 4. 分项计时与判断

`LINKRS_STAGE_TIMING=1` 下目标查询热态分段：

| 阶段 | 耗时 |
|---|---:|
| `instantiate.validate` | 4 us |
| `instantiate.materialize` | 11 us |
| `instantiate.engine` | 29 us |
| 结果物化 | 2–3 us |
| 引擎执行（含源算子与契约） | 约 0.70 ms |
| 源算子 `next` | 约 0.53 ms |

装配与结果物化均不是主导项，因此未按计划中的备选方向引入“不可变执行骨架”
或窄形状结果构造。

编译分项（5 次计时运行均为 `plan_cache_hit=true`）：

| 阶段 | 中位数 |
|---|---:|
| parse | 30 us |
| plan | 86 us |
| optimize | 842 us |
| statistics/catalog 检查 | 0 us |
| cache probe | 933 us |

结论：缓存确实命中，但分区计划缓存的命中发生在逻辑优化之后，命中路径仍付
parse/plan/optimize 的固定成本。若要继续压缩编译，需要单独评估
“查询文本 + 空间 + 目录版本 + 读上下文”的已编译物理计划复用，并在 staged
write 读上下文下失效；本次不直接改热路径缓存结构。

`ordered_existence_open_us` 最终 EXPLAIN 读回为 124 us；分段日志显示锁等待为
0 us，耗时集中在游标调用。此前秒级读数未在新一轮复现，字段继续只作诊断，
不参与性能验收。

## 5. 验证

| 门 | 结果 |
|---|---|
| devs 双二进制 release 构建 | 成功 |
| `linkrs-storage --lib ordered_existence` | 1 passed |
| `linkrs-query --lib ordered_existence` | 8 passed |
| partitioning | 49 passed |
| local_topk | 13 passed |
| `parallel_partition_execution` | 27 passed |
| dql | 324 passed |
| `linkrs-storage --lib` | 1417 passed |
| A/B 差分 | 均为 `[14]`，`result_stable=true`，`spill_rows=0` |

## 6. 证据

本地证据目录：`perf-data/parity/results/2026-11-25/ordered-existence-remediation/`。
devs 原始目录：`/data/kkkqkx/linkrs/ordered-existence-remediation/`。
关键文件：`final-rerun.log`（构建与最终四轮指标）、
`measure-remediation-driver.log`（全量门与第一轮采集）、
`explain-prod-final/lsqb_operators_distinct_limit_r1.txt`（计划形状与编译分项）、
`stage-explain-final/lsqb_operators_distinct_limit_r1.txt`（open 诊断）、
`diff-off-final/` 与 `diff-on-final/`（逐次 JSON）、`prod-final/`（生产默认逐用例 JSON）。
