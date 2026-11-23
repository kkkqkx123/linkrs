# 阶段一、二当前差距收敛方案

**日期**：2026-11-23
**问题来源**：`docs/issue/linkrs-remaining-parity-issues.md`，当前状态以第 35 节为准。
**方案定位**：本文只描述阶段一、二的实施顺序、代码落点、生产配置更新、护栏与验收，不重新定义问题，不替代原分阶段方案，也不改变阶段三至七的任务边界。

---

## 1. 当前状态与目标

### 1.1 当前状态

- 阶段一：`group_by_count` 约 103–119 ms，`distinct_limit` 约 89–137 ms。秒级问题已消除，但相对 Ladybug 仍约 15–30 倍。PROFILE 中 `Project` 已为 `rows: 0`，`typed_group_hits=110`、10 个分区均下发 `LocalTopKDistinct`；主要成本是每分区约 50–74 ms 的扫描和约 25 ms 的单跳展开。
- 阶段二：`join_two_hop` 约 5.9–6.2 ms，`variable_path_count` 约 6.3–6.5 ms，结果与通用路径逐值一致。两跳相对 Ladybug 2.42 ms 仍约 2.5 倍，变长计数已不再列为优化目标。
- 生产配置未享受上述收益：`config.toml`、`crates/linkrs-config/src/parallel.rs` 与 `crates/linkrs-query/src/optimizer/partitioning/config.rs` 中并行默认关闭，`workers=1`、`max_partitions=1`、`allow_ordered_parallel_expand=false`。benchmark 通过命令行显式请求并行。
- 最新四轮 A/B 显示 16 分区不如 10 分区稳定，比较器/hash/顺序变体没有稳定收益；不应继续在这些维度上叠加复杂度。

### 1.2 目标

| 项目 | 中间门 | 最终门 | 口径 |
|---|---:|---:|---|
| `group_by_count` | ≤ 40 ms 且相对当前均值至少提升 2 倍 | ≤ 20 ms 且与 Ladybug 差距 ≤ 3 倍 | LSQB sf0.1，固定门，两轮 |
| `distinct_limit` | ≤ 30 ms 且相对当前均值至少提升 2 倍 | ≤ 15 ms 且与 Ladybug 差距 ≤ 4 倍 | 同上 |
| `join_two_hop` | ≤ 4 ms | ≤ 3 ms，结果逐值不变 | 同上 |
| `variable_path_count` | ≤ 6 ms | 不高于当前均值，结果逐值不变 | 同上 |
| 九条纯计数 | 0.14–0.61 ms | 无实质回归 | 同上 |
| 并行门编译中位数 | ≤ 1 ms | 回到 0.1 ms 级或给出可复现的成本归因 | 同上 |

若最终门受内存带宽或固定经济分区数限制无法达成，必须先给出扫描/展开自耗时证据与三方对照，不得用近似、采样或截断采样替代精确结果。

---

## 2. 实施顺序

### 切片一：先把自耗时拆开

**目标**：在改执行代码前确定每分区 50–74 ms 扫描与约 25 ms 展开的内部构成，避免继续误调聚合或去重。

**修改位置**：

- `src/bin/linkrs_parity_bench.rs`：固定门 JSON 增加扫描子阶段与展开子阶段字段；保留 `typed_group_hits`、`local_topk_pushed`、`partition_skew_ratio`、`result_stable`。
- `crates/linkrs-query/src/executor/streaming/plan/arena_builder/metadata.rs`、`crates/linkrs-query/src/planning/plan/explain/description.rs`：把新字段接入 EXPLAIN，不改变计划哈希与查询数值。
- `crates/linkrs-query/src/executor/streaming/operators/source_operator/storage_scan.rs`、`crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs`：只在 Profile 侧记录“枚举、可见性、范围过滤、属性解码、邻接读取、端点翻译、目的端读取、typed 组装”的自耗时，不改变读取路径。

**护栏**：本切片不改执行计划和行数；同一二进制跑两轮，字段稳定后才进入下一片。

### 切片二：阶段一的父批/孩子偏移与局部状态融合

**目标**：参考 Ladybug 的 packed extend 和 multiplicity 加权聚合，让单跳拓扑结果直接进入局部聚合或 LocalTopK 状态，取消逐边中间对象。

**修改位置**：

- `crates/linkrs-storage/src/client/traits_reader.rs`、`crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs`：在现有拓扑批量读取原语上扩展“父选择向量 + 孩子前缀偏移”的返回形态；保持 CSR 顺序、边身份、rank、共享边类型名与外部 ID 权威不变。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs`：rowless 单跳消费父批/孩子偏移，直接在批内产出分组键或去重键；不再为每条边构造 `PendingEdge`、`all_dst` 与逐边 typed 向量。
- `crates/linkrs-query/src/executor/streaming/operators/blocking/aggregate_operator/typed_group_count.rs`、`partial_aggregate.rs`、`aggregate.rs`：把“预扫描新键 + 逐行二次哈希”收敛为一批一次定位状态槽并累加；继续消费 `chunk.multiplicity()`，不在入口展开重复行。
- `crates/linkrs-query/src/executor/streaming/operators/blocking/local_topk.rs`、`crates/linkrs-query/src/executor/streaming/plan/arena_builder/partition/chain.rs`：把 typed 键选择前移到计划期；非定长或不可证明类型整批 pass-through，由全局 `Distinct`/`TopN` 继续负责最终权威。

**前置收窄**：单跳、rowless、无过滤、无 TTL、单一边类型、单一方向、单列定长分组键或单列定长去重键；其它形状保持原路径。

**反例先于实现**：空分区、全同键、高基数键、nullable 键、重复键、并列值、自环、平行边、悬空端点、TTL 过期边、staged write、selection 与 multiplicity 合计大于一的输入。

### 切片三：阶段二的度读取与前沿收敛

**目标**：在阶段一融合稳定后，再压两跳计数的剩余约 2.5 倍差距；不重做已闭环的变长路径语义快路径。

**修改位置**：

- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs`：扩展固定深度二的“父批度数”原语，按内部端点批量读尾边度数，外部 ID 只用于保持悬空邻居可见性与读集合记录。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_frontier.rs`、`expand.rs`：固定深度 `2` 与范围 `1..2` 直接消费度数批，不构造邻居 ID 向量；属性需求、`Trail`、多类型链、TTL、开放 schema、空路径语义继续回退物化路径。
- `crates/linkrs-query/src/optimizer/partitioning/planner/anchored.rs`、`core.rs`：只在两跳形状上评估分区前沿；`allow_parallel_count_frontier` 继续默认关闭，直到同二进制 A/B 证明稳定收益且结果不漂移。

**护栏**：`382018` 与 `400153` 在跨轮、跨 worker 数下逐值稳定；`partition_count_check` 继续保留；自环、平行边、悬空端点与方向变体沿用现有 DQL 固件。

### 切片四：生产配置更新与文档同步

**目标**：在切片二、三的最终门通过后，才把经过验证的窄形状灰度配置写入生产默认；实现阶段不提前改默认。

**修改位置**：

- `config.toml` 的 `[parallel]` 段。
- `crates/linkrs-config/src/parallel.rs` 的 `ParallelConfig::default()` 与默认值测试。
- `crates/linkrs-query/src/optimizer/partitioning/config.rs` 的 `PartitioningConfig::default()` 及相关单测。
- `docs/release/statements/07_configuration_reference.md` 的并行配置表与配置示例。
- `crates/linkrs-query/tests/parallel_partition_execution.rs` 与 `crates/linkrs-config/src/lib.rs`：增加“默认配置只放行有序 rowless 展开形状，纯计数和通用聚合仍串行”的断言。

**拟定生产默认值**：

```toml
[parallel]
enabled = false
workers = 8
max_partitions = 8
min_rows_per_partition = 100000
max_buffered_chunks = 10
allow_pure_count_chain = false
allow_global_aggregate = false
allow_ordered_parallel_expand = true
allow_parallel_count_frontier = false
allow_partition_metadata_count = false
```

含义与理由：

- `enabled=false` 保持总开关关闭；`allow_ordered_parallel_expand=true` 只放行阶段一已证明的有序 rowless 单跳形状。
- `allow_pure_count_chain=false`：九条纯计数已在 0.14–0.61 ms，不应为它们引入分区开销。
- `allow_global_aggregate=false`：避免把放行面扩大到所有聚合根形状。
- `allow_parallel_count_frontier=false`：当前不同 worker 数收益不稳定，继续关闭。
- `workers=8`、`max_partitions=8` 是保守生产初值；bench 固定门的 `16 / 10` 只用于本机经济点测量，不直接作为通用默认。上线前必须跑 `workers × max_partitions` 的矩阵，若 8 分区相对 10 分区超过 20% 退化，则按实测值调整并在配置文档记录原因。

**回滚**：把 `allow_ordered_parallel_expand` 改回 false、`workers=1`、`max_partitions=1` 即可；无数据迁移，无 WAL 格式变更。

---

## 3. 验证口径

1. 固定门沿用 `perf-data/parity/results/2026-11-23/observability-plan-proof` 的 golden DB 与脚本口径：warmup 1 + run 5、绑核 16-31、`--parallel-workers 16 --max-partitions 10 --require-parallel-explain`。
2. 每轮必须满足 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`；任一不满足即判环境或配置无效。
3. 生产配置单独跑一轮：不加 benchmark 并行参数，确认两条阶段一固定门仍出现 `actual>1`，九条纯计数仍为串行且无回归。
4. 正确性门：`crates/linkrs-query/tests/dql` 全量、streaming lib 全量、`parallel_partition_execution.rs`、predicate/count-frontier/order 相关单测、storage lib 全量。
5. 三方对照沿用 `perf-data/parity/results/2026-11-18/lbug-ops.json`、`neug-ops.json` 与修正后的 `ladybug-distinct-fixed.json`；整数逐值相等，浮点相对误差 1e-3。

---

## 4. 护栏与不做项

- 任一快路径必须先在空输入、NULL、重复键、并列值、TTL、悬空端点和 staged write 上与慢路径逐值对拍；出现差异立即停止该切片。
- 全局 `Distinct`/`TopN` 保持最终权威；局部 TopK 只做精确截断，不做近似去重或截断采样。
- `EdgeTopology` 的外部 ID、边身份和读集合记录不变；不改变拓扑/属性布局，不把属性内联进 CSR，不建立跨调用可见性缓存。
- 不在生产配置中静默打开总开关，不把 `allow_global_aggregate`、`allow_pure_count_chain`、`allow_parallel_count_frontier` 一并打开。
- 不因本方案扩大阶段三至七的放行范围；min/max、id-range、order-limit 下推、元数据计数和跨分片事务仍按各自证明条件执行。
- 不新增“替代版”算子模块；所有改动收敛到本文列出的原模块。

---

## 5. 首轮落地与判定（2026-11-23）

四个切片均已在 devs 实施并完成 release 构建：

- 自耗时：`ColumnarStats` 新增 `scan_read_us` / `expand_read_us`，经 `columnar_summary` 进入 EXPLAIN，parity JSON 新增同名字段；未改计划形状与查询数值。
- 阶段一：拓扑批量读取新增父批 + 孩子偏移的 packed 形态；rowless 单跳展开改为直接消费 packed 数组；`typed_group_count` 用一次 `entry` 累加；`RETURN/YIELD DISTINCT` 在计划期传播投影列类型。
- 阶段二：存储新增 `frontier_degree_batch`，一次邻接 staging 同时产出深度一和深度二度数；范围 `1..2` 的计数前沿改为一次调用。
- 生产配置：`config.toml`、`ParallelConfig::default()` 与 `PartitioningConfig::default()` 已切到 `workers=8`、`max_partitions=8`、`allow_ordered_parallel_expand=true`，总开关和其余灰度开关保持关闭；配置参考文档同步更新。

验证：配置默认值 1/1、partitioning 49/49、typed group 8/8、local topk 10/10、parallel 27/27、DQL 324/324、storage 1415/1415。`linkrs-config` lib 单测因 devs 离线缺少 `serial_test v3.5.0` 未能启动，配置默认值由 partitioning 新增断言覆盖。固定门两轮中 `group_by_count` 为 96.01 / 94.66 ms、`distinct_limit` 为 91.90 / 81.91 ms、`join_two_hop` 为 6.24 / 10.31 ms、`variable_path_count` 为 6.09 / 6.36 ms；生产默认单轮中前两项为 99.64 / 85.41 ms 且 `actual=8`。计划期类型传播已接入逻辑与物理 Dedup，但当前 LSQB 形状仍显示 `local_topk:pushed:runtime`，运行时判型继续生效。原始产物见 `perf-data/parity/results/2026-11-23/stage1-stage2-final/`。

判定：生产配置切换带来实际收益，正确性门全绿；但本方案的性能中间门未达成，packed 拓扑与 typed 状态调整没有可测端到端收益。按第 4 节护栏不扩大放行面、不引入近似；后续应以新增自耗时为输入，直接处理扫描枚举/可见性与目的端属性读取，并在有实测收益前不再叠加 DTO 复杂度。微基准证据与后续改进设计见 `docs/plan/remaining-bottleneck-microbench-and-design.md`，第二轮落地与最终回测见同文档第 8 节及 `perf-data/parity/results/2026-11-23/stage1-stage2-remediation-final/`。

---

## 6. 扫描归因后的最终回测（2026-11-23）

按 `docs/plan/scan-visibility-and-decode-remediation.md` 继续收敛扫描：新增五个扫描子计时并接入 EXPLAIN / parity JSON；游标为已枚举 ID 增加来源标记，跳过同一 guard 下的二次行可见性复核；分片切片改为单锁枚举连续 ID 区间。按 chunk 批量解码列、跳过候选预分配后 select 的切片二在中间版实测中未达微基准门槛且增加分配，已按无实测收益原则回退。

基线子计时之和占 `scan_read_us` 的 96%–98%；固定门基线枚举为 262–463 ms、重复可见性复核为 102–262 ms，目标用例 `scan_decode_us=0`，确认阶段一走拓扑-only 扫描。最终回测中两轮 `scan_read_us` 降为 87–120 ms（基线 492–700 ms，降约 82%）；`group_by_count` 为 53.34 / 58.04 ms、`distinct_limit` 为 47.67 / 39.84 ms；生产默认单轮为 55.53 / 38.38 ms，九条纯计数无实质回归。正确性门全绿，并新增枚举后提交快照回归用例。

最终门中的 `group_by_count` ≤40 ms 与 `distinct_limit` ≤30 ms 尚未同时达成；剩余自耗时转向展开读取。按第 4 节护栏不扩大并行放行面、不引入近似，保留已验证的扫描改动集合。完整数据见 `perf-data/parity/results/2026-11-23/scan-visibility-decode-remediation/`。
