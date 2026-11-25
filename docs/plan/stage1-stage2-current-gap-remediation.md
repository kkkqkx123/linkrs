# 阶段一、二当前基线与差距收敛方案

**日期**：2026-11-25  
**定位**：本文以资源对齐后的三方测量为当前起点，重述阶段一、二差距和剩余主因；不重新定义原始问题，也不替代问题清单。历史实施回测见 `docs/issue/linkrs-remaining-parity-issues.md` 第 37–48 节及对应 `perf-data/parity/results/` 目录。后续分阶段改动见 `docs/plan/packed-topology-direct-write-and-key-passthrough.md` 与 `docs/plan/parity-remaining-gap-convergence-design.md`。

---

## 1. 当前基线

### 1.1 原始问题与已落地改动

原始阶段一问题是 `group_by_count` 1.31–1.50 秒、`distinct_limit` 约 1.11 秒；原始阶段二问题是 `join_two_hop` 35–49 ms、`variable_path_count` 1.10–1.35 秒。

已经落地并经正确性门验证的改动包括：扫描读取与展开读取子计时、packed 拓扑按父直写、rowless 单跳直连、身份主键直通、typed 聚合一次定位、`RETURN/YIELD DISTINCT` 计划期类型传播、深度一加二前沿度数合并、生产默认 `workers=8 / max_partitions=8 / allow_ordered_parallel_expand=true`、扫描枚举与可见性收敛、父端点身份批量解析、目的端 handle 复用、packed expand 至局部阻塞算子直连、`LocalTopKRows`，以及局部 TopK 全局尾部与边 bypass 语义守卫。

因此当前起点不再是秒级，也不是“生产默认未受益”；起点是资源对齐后生产默认 25.9–27.3 ms、固定门 30.0–32.1 ms 的两条阶段一用例，约 5.4 ms 的两条阶段二用例，以及仍未关闭的绝对耗时与倍率门槛。

### 1.2 资源对齐口径

本轮先统一三方资源，再重测；旧的三方 JSON 与基于它们的倍率不再作为当前结论。

- CPU/NUMA：`numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`。三方实际亲和性均为 `8-15,24-31`。
- 线程：三方参数均为 16。Ladybug 使用 16 connection threads；Neug `max_thread_num=16` 仅约束 TP 槽位，AP 查询仍是单线程 wall-clock。
- 次数：三方均 warmup 1 + run 5，去最大值平均；Linkrs 同时保留中位数。
- buffer：Linkrs 64 MB；Ladybug 通过 `buffer_pool_size=67108864` 显式对齐；Neug 为 `M_FULL`，无法等价为 64 MB，只作参照。
- 计时：Linkrs/Ladybug 为引擎内计时；Neug 为含完整结果物化的外部 wall-clock。
- 环境字段写入 JSON：`cpu_binding`、`numa_policy`、`cpu_affinity`、`threads`、`warmup`、`runs`、`buffer_pool_bytes` / `buffer_strategy`、`engine_thread_model`、`timing_basis`。

证据在 `perf-data/parity/results/2026-11-25/resource-aligned/`：Linkrs gate 日志与逐用例 JSON/EXPLAIN、Ladybug/Neug 原始 JSON 与 runner 日志均在该目录。实现落点为 `scripts/devs-stage1-stage2-gate.sh`、`scripts/devs-resource-aligned-parity.sh` 与 `graphdb-ben/neug_runner.py`。

### 1.3 Linkrs 对齐门

| 用例 | 固定门第一轮 | 固定门第二轮 | 生产默认 | 说明 |
|---|---:|---:|---:|---|
| `group_by_count` | 32.05 ms | 32.53 ms | 25.92 ms | 结果 `[14|59]`，固定门 `actual=10`、生产 `actual=8`，`typed_group_hits=110/112`，`spill_rows=0` |
| `distinct_limit` | 29.98 ms | 29.96 ms | 27.28 ms | 结果 `[14]`，固定门 `actual=10`、生产 `actual=8`，`spill_rows=0` |
| `join_two_hop` | 5.94 ms | 5.93 ms | 5.38 ms | 结果 `382018` 逐值一致 |
| `variable_path_count` | 5.60 ms | 5.90 ms | 5.38 ms | 结果 `400153` 逐值一致 |
| 九条纯计数 | 0.14–0.62 ms | 同量级 | 同量级 | 无实质回归 |

两轮固定门与生产默认均 `result_stable=true`、`spill_rows=0`。EXPLAIN 中两条阶段一用例 `expand_output_rows` 为 30473（固定门）与 30536（生产默认），即每唯一目的端一行；身份直通没有被语义守卫误关。正确性门全绿：配置默认值、partitioning 49/49、typed group 9/9、local topk 13/13、parallel 27/27、DQL、storage、column batch。

### 1.4 三方差距

主对比使用 Linkrs 生产默认；固定门仅作为 16 workers / 10 partitions 的上限口径单独列出，不混入三方默认对比。

| 用例 | Linkrs 生产默认 | Ladybug | Neug | 对 Ladybug | 对 Neug |
|---|---:|---:|---:|---:|---:|
| `group_by_count` | 25.92 ms | 8.16 ms | 12.28 ms | 3.18× | 2.11× |
| `distinct_limit` | 27.28 ms | 3.20 ms | 28.33 ms | 8.52× | 0.96× |
| `join_two_hop` | 5.38 ms | 1.54 ms | 20.14 ms | 3.50× | 0.27× |
| `variable_path_count` | 5.38 ms | 441.42 ms | 102.90 ms | 0.01× | 0.05× |
| `aggregate_min_max` | 1.22 ms | 0.95 ms | 0.09 ms | 1.29× | 14.36× |
| `filter_id_range` | 1.69 ms | 1.24 ms | 0.15 ms | 1.37× | 11.33× |
| `filter_or` | 1.53 ms | 1.21 ms | 0.32 ms | 1.27× | 4.81× |
| `order_limit` | 1.21 ms | 0.46 ms | 0.04 ms | 2.64× | 30.23× |
| `undirected_count` | 4.53 ms | 1.59 ms | 1.51 ms | 2.85× | 3.01× |

固定门下 `group_by_count` 32.05 ms、`distinct_limit` 29.98 ms；与生产默认相比没有稳定优势，说明在 node1 的 8 个物理核上，16 workers / 10 partitions 不再是免费收益。配置默认值保持不变。

### 1.5 门槛状态

| 项目 | 最终门 | 资源对齐后判定 |
|---|---:|---|
| `group_by_count` | ≤20 ms 且与 Ladybug ≤3× | 未达成：生产默认 25.92 ms、3.18× |
| `distinct_limit` | ≤15 ms 且与 Ladybug ≤4× | 未达成：生产默认 27.28 ms、8.52× |
| `join_two_hop` | ≤3 ms 且结果逐值不变 | 未达成：5.38 ms、3.50× Ladybug，但已快于 Neug |
| `variable_path_count` | 不高于原当前均值 | 达成：5.38 ms，显著快于三方 |
| 简单查询 | 不明显退化 | 达成；对 Ladybug 1.27–2.85×，Neug 差距主要是单线程执行形态 |

门槛未达成不通过放宽并行、近似、截断或放宽 buffer 策略换取。

---

## 2. 差距归因

### 2.1 事件到键的放大已经消除

固定门 EXPLAIN 仍显示 110 个父批、215488 个拓扑事件与 30473 个唯一目的端，但 `expand_output_rows` 已从 215488 降到 30473/30536。父批拓扑直写与身份主键直通取消了事件中转和逐边属性展开；剩余展开读取由拓扑区间读取和唯一目的端存在性读取构成。

生产默认自耗时：`scan_read_us` 约 41–43 ms，`expand_read_us` 约 77–81 ms，其中拓扑读取约 65–69 ms、目的端读取约 12 ms。阶段一剩余瓶颈是分区扫描 + 单跳拓扑读取，而不是聚合、TopK、Exchange 或生产配置。

### 2.2 机制状态

| 机制 | Ladybug / Neug 形态 | 当前项目状态 | 剩余差距 |
|---|---|---|---|
| packed 多父扫描 | Ladybug `PackedChildSlices` 与父/孩子偏移直接作为算子契约 | `PackedTopologyBatch` 已按父直写，不再构造 event/scatter | 拓扑读取仍按区间逐个 CSR 头部访问，条件预取未证明收益 |
| 展开即计数 | Ladybug `CountExtendChain` 按 offset 累计，不物化邻居 | `frontier_degree_batch` 合并深度一加二；两跳仍保留必要存在性判断 | 缺少无悬空边证明，不折叠存在性读取 |
| 聚合与 multiplicity | Ladybug typed `append` 按 multiplicity 累加；Neug typed flat map | typed 聚合一次定位，packed 键按权重累加，局部 TopK 精确截断 | 输入已是唯一键，聚合不是主因 |
| 目的端属性 | Ladybug 直接从 packed 批取键；Neug POD CSR 内联可见性 | 身份主键直通已按 schema 证明并带运行时标签守卫 | 非主键整数属性在当前 LSQB schema 中不存在，无法激活更宽证明 |
| CSR 局部性 | Neug 按度数距离预取 head 与 metadata | 黑盒读与非阻塞候选均已验证但被拒绝，A/B 钩子已从生产路径移除（2026-11-25） | 无 ≥15% 微基准收益，不进入生产；重测需从历史恢复实验代码 |

### 2.3 判定

- 阶段一：秒级问题已消除，事件放大已切断；剩余差距是单次拓扑区间读取和分区扫描的常数成本。
- 阶段二：变长计数已大幅领先三方；两跳仍慢于 Ladybug 但快于 Neug，只在可证明形状下继续收敛。
- 简单查询：对 Ladybug 的差距为 1.27–2.85×，对 Neug 的差距主要来自执行口径与固定管线开销，不新增专用快路径。
- 不再以旧 `16-31` 跨节点绑定或未对齐 buffer 的数字判断收益。

---

## 3. 范围与护栏

- 只修改已列出的原模块，不新增替代版算子或“version 2”模块，不改变阶段三至七的放行范围。
- 保持全局 `Distinct` / `TopN` 最终权威；局部 TopK 只做精确截断，不引入采样、近似或不可证明的早停；`TopN` 之上的前缀 `Limit` 必须为零偏移且不超过局部 n。
- 不改变 `EdgeTopology` 外部 ID、边身份、rank、共享边类型名、读集合记录与 CSR 顺序；不跨调用缓存可见性或端点存在性。
- 身份直通必须由 schema 证明，运行时目的端标签必须与计划一致；需要边 bypass 列的形状回退。
- `parallel.enabled` 继续关闭；不打开 `allow_global_aggregate`、`allow_pure_count_chain`、`allow_parallel_count_frontier`。
- 生产配置保持 `workers=8 / max_partitions=8 / allow_ordered_parallel_expand=true`；任何默认值变更都需要相同资源口径的固定门与容量证据。
- 任一快路径必须先在空输入、NULL、重复键、并列值、自环、平行边、悬空端点、TTL 过期边、staged write 与 multiplicity 大于一的输入上与慢路径逐值对拍。

---

## 4. 后续方案入口

分阶段实施顺序、代码落点、门槛、验证口径与失败处理见 `docs/plan/packed-topology-direct-write-and-key-passthrough.md`；剩余差距的三方收敛设计见 `docs/plan/parity-remaining-gap-convergence-design.md`；结果语义修复见 `docs/plan/semantic-correctness-remediation.md`。未通过门槛时保留可验证收益最小的改动集合，并把未达标原因写回 `docs/issue/linkrs-remaining-parity-issues.md`。
