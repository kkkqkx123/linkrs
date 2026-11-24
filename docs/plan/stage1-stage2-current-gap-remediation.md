# 阶段一、二当前基线与差距收敛方案

**日期**：2026-11-24
**定位**：本文以当前最新固定门为分析起点，重述阶段一、二的三方差距和剩余主因；不重新定义原始问题，也不替代问题清单。历史实施回测见 `docs/issue/linkrs-remaining-parity-issues.md` 第 37–44 节及对应 `perf-data/parity/results/` 目录。后续分阶段改动写入 `docs/plan/packed-topology-direct-write-and-key-passthrough.md`。

---

## 1. 当前基线

### 1.1 原始问题与已落地改动

原始阶段一问题是 `group_by_count` 1.31–1.50 秒、`distinct_limit` 约 1.11 秒；原始阶段二问题是 `join_two_hop` 35–49 ms、`variable_path_count` 1.10–1.35 秒。

以下改动已在当前代码中并经过正确性门验证：扫描读取与展开读取子计时、父批/孩子偏移的 packed 拓扑、rowless 单跳直连、typed 聚合一次定位、`RETURN/YIELD DISTINCT` 计划期类型传播、深度一加二前沿度数合并、生产默认 `workers=8 / max_partitions=8 / allow_ordered_parallel_expand=true`、扫描枚举来源标记与单锁区间枚举、展开读取拆分、父端点身份批量解析、拓扑扁平化与目的端 handle 复用、packed expand 至局部阻塞算子直连，以及 `LocalTopKRows`。

因此当前起点不再是秒级，也不是“生产默认未受益”；起点是固定门 30–40 ms 的两条阶段一用例、约 5–6 ms 的两条阶段二用例，以及仍未关闭的性能门槛。

### 1.2 最新固定门与生产默认

数据来自 `perf-data/parity/results/2026-11-24/group-by-local-topk/group-by-local-topk-r2/`。口径为 LSQB sf0.1、warmup 1 + run 5、绑核 16-31、固定门 `--parallel-workers 16 --max-partitions 10 --require-parallel-explain`，取两轮中位数；生产默认单轮不加 benchmark 并行参数。

| 用例 | 固定门第一轮 | 固定门第二轮 | 生产默认 | 说明 |
|---|---:|---:|---:|---|
| `group_by_count` | 39.59 ms | 33.89 ms | 31.86 ms | `actual=10`，`typed_group_hits=110`，每分区 `LocalTopKRows` 输出 10 行，Exchange 约 100 行 |
| `distinct_limit` | 37.95 ms | 32.80 ms | 29.51 ms | `actual=10`，每分区 `local_topk:pushed`，运行时判型仍在生效 |
| `join_two_hop` | 4.87 ms | 5.90 ms | 4.96 ms | 结果 `382018` 逐值一致 |
| `variable_path_count` | 4.86 ms | 5.90 ms | 4.81 ms | 结果 `400153` 逐值一致 |
| 九条纯计数 | 0.14–0.62 ms | 同量级 | 同量级 | 无实质回归 |
| 并行门编译中位数 | 3.59 ms | 3.60 ms | 1.4–1.6 ms | 纯计数仍为 0.01 ms 级 |

两轮均满足 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`；生产默认 `actual=8`。正确性门全绿：配置默认值 1/1、partitioning 49/49、typed group 8/8、local topk 10/10、parallel 27/27、DQL 324/324、storage 1415/1415、column batch 13/13。`linkrs-config` lib 单测仍因离线依赖未启动，配置默认值由 partitioning 断言覆盖。

### 1.3 三方差距

对手数据来自 `perf-data/parity/results/2026-11-18/lbug-ops.json`、`neug-ops.json` 及修正排序后的 `ladybug-distinct-fixed.json`、`neug-distinct-fixed.json`。Ladybug/Neug 为各引擎内部计时，Neug 为 AP 单线程 wall-clock，只作参照；倍率用于判断量级结构，不作严格同口径跑分。

| 用例 | Ladybug | Neug | 当前 Linkrs 两轮中位数 | 对 Ladybug | 对 Neug |
|---|---:|---:|---:|---:|---:|
| `group_by_count` | 7.104 ms | 13.762 ms | 33.89 / 39.59 ms | 约 5.2× | 约 2.7× |
| `distinct_limit` | 3.755 ms | 28.701 ms | 32.80 / 37.95 ms | 约 9.4× | 约 1.2× |
| `join_two_hop` | 2.422 ms | 24.088 ms | 4.87 / 5.90 ms | 约 2.2× | 0.22× |
| `variable_path_count` | 457.680 ms | 117.401 ms | 4.86 / 5.90 ms | 0.01× | 0.05× |
| `filter_or` | 1.517 ms | 0.317 ms | 1.49 / 1.63 ms | 约 1.0× | 约 4.9× |
| `order_limit` | 0.880 ms | 0.044 ms | 1.22 / 1.34 ms | 约 1.5× | 约 29× |
| `undirected_count` | 1.926 ms | 1.450 ms | 4.60 / 5.02 ms | 约 2.5× | 约 3.3× |

结论：阶段二已基本闭环，变长计数大幅领先三方；阶段一仍是对 Ladybug 的主要差距，简单查询对 Ladybug 基本在一个量级内，对 Neug 的差距主要是不同执行口径和固定管线开销。

### 1.4 门槛状态

| 项目 | 中间门 | 最终门 | 当前判定 |
|---|---:|---:|---|
| `group_by_count` | ≤40 ms 且较原均值至少 2 倍 | ≤20 ms 且与 Ladybug 差距 ≤3 倍 | 中间门达成；最终门未达成 |
| `distinct_limit` | ≤30 ms 且较原均值至少 2 倍 | ≤15 ms 且与 Ladybug 差距 ≤4 倍 | 两轮固定门未达成；生产默认单轮 29.51 ms |
| `join_two_hop` | ≤4 ms | ≤3 ms 且结果逐值不变 | 未达成，但已快于 Neug |
| `variable_path_count` | ≤6 ms | 不高于原当前均值 | 达成 |
| 九条纯计数 | 无实质回归 | 无实质回归 | 达成 |
| 并行门编译中位数 | ≤1 ms | 回到 0.1 ms 级或给出可复现归因 | 未达成 |

最终门的绝对值和与 Ladybug 的倍率门均未同时满足；本轮不通过放宽配置、近似或截断来换取门槛通过。

---

## 2. 差距归因

### 2.1 当前主因是事件到键的放大，不是聚合或交换

固定门 EXPLAIN 中，两条阶段一用例均为 110 个父批、215488 个拓扑事件、30473 个唯一目的端、缺失目的端为 0。事件到唯一键的放大约 7 倍。

`expand_read_us` 两轮中位数：

| 用例 | 第一轮 | 第二轮 | 生产默认 |
|---|---:|---:|---:|
| `group_by_count` | 196758 us | 131147 us | 110547 us |
| `distinct_limit` | 199390 us | 145529 us | 110242 us |

当前代码已经具备 packed 拓扑和 `PackedExpandPayload`，但查询层仍构造 `parent_of_child`、`all_dst`，对 215488 个事件做 HashMap 去重，读取 30473 个唯一目的端属性，再展开回 215488 行；直连只消除了最终 typed 列装配。`LocalTopKRows` 已把 Exchange 从约 11554 行降到 100 行，继续改 Exchange 或局部聚合不会产生与事件放大同量级的收益。

### 2.2 与参考实现的机制差异

| 机制 | Ladybug / Neug 实现 | 当前项目状态 | 差距 |
|---|---|---|---|
| packed 多父扫描 | Ladybug `CSRNodeGroupScanState::tryScanCachedTuplesPacked` 在 CSR 缓存窗口内服务多个父点，直接写父选择向量与 `packedChildOffsets`，输出 selection 只含可见行 | `packed_node_edge_headers_batch` 已有父/偏移 DTO；但 `out_topology_batch` 仍收集 event、`flatten_topology_events` 再做前缀和与散射，`out_topology_with_gate` 仍按父分配 Vec | DTO 已具备，读取路径仍有事件中转与多次分配 |
| 展开即计数 | Ladybug `CountExtendChain` 用 dense 按 offset 计数，最后一跳用子区间长度直接乘路径数，中间跳按邻居散射，不物化邻居 ID 向量 | `frontier_degree_batch` 已合并深度一加二，但仍收集 `NeighborEvent`、散射、外部 ID 解析后再读度数 | 两跳已接近，但仍有可去掉的事件 staged 成本 |
| 聚合与 multiplicity | Ladybug `AggregateHashTable::append` / `PartitioningAggregateHashTable::append` 一批键一次算哈希、一次定位槽位，按 `multiplicity` 更新状态；Neug `Key<EXPR>::group` 用 typed `flat_hash_map` | `typed_group_count` 已一次 `entry` 累加，LocalTopK 精确截断；但输入仍是 215488 个逐边键 | 聚合实现已不是主因，输入规模才是 |
| 目的端属性 | Ladybug 在 packed 批内从父亲选择与孩子偏移直接得到分组键；Neug 用 POD CSR 迭代与内联时间戳可见性 | `read_destinations` 仍对逐边 ID 去重后读属性列，再按边展开 | 若键是身份列，可绕开属性列读取和逐边展开 |
| 身份证明 | Ladybug `RemoveUnnecessaryDistinctOptimizer` 只认主键/内部 ID 且输入 multiplicity 可证明为 1 的链 | 计划期已传播投影列类型，但 LSQB 形状仍显示 `local_topk:pushed:runtime`，没有主键/外部身份证明 | 不能按列名推断身份，需补可证明条件 |
| CSR 局部性 | Neug `CsrView::prefetch_metadata/head` 与 `prefetch_next_vertex` 按距离预取，`edge_expand.cc` 用 256 顶点微批与内部 ID 排序提升局部性 | 当前端点解析已批量化，但未做邻接 head 预取或微批排序 | 条件性后续项，需微基准证明 |

### 2.3 判定

- 阶段一：秒级问题已消除；剩余差距集中在“拓扑事件 → 唯一键/计数”的展开链，而不是聚合、去重最终态、Exchange 或生产配置。
- 阶段二：两跳约 2.2 倍 Ladybug、已快于 Neug；变长计数已显著领先三方。两跳只保留低风险的事件直写收敛，不以追平对手为名扩大并行放行面。
- 简单查询：对 Ladybug 的 1.0–2.5 倍差距可接受；对 Neug 的差距主要是执行口径与固定开销，不新增专用快路径。
- 编译中位数：3.1–3.6 ms 是独立未归因项，不在本轮阶段一、二主线中并行推进。

---

## 3. 范围与护栏

- 只修改已列出的原模块，不新增替代版算子或“version 2”模块，不改变阶段三至七的放行范围。
- 保持全局 `Distinct` / `TopN` 的最终权威；局部 TopK 只做精确截断，不引入采样、近似或不可证明的早停。
- 不改变 `EdgeTopology` 的外部 ID、边身份、rank、共享边类型名、读集合记录和 CSR 顺序；不把属性内联进 CSR，不跨调用缓存可见性或端点存在性。
- `parallel.enabled` 继续关闭；不打开 `allow_global_aggregate`、`allow_pure_count_chain`、`allow_parallel_count_frontier`。
- 生产配置已验证有效，本轮不因未达标改动默认值；配置变更仍需固定门证明。
- 任一快路径必须先在空输入、NULL、重复键、并列值、自环、平行边、悬空端点、TTL 过期边、staged write 与 multiplicity 大于一的输入上与慢路径逐值对拍。

---

## 4. 后续方案入口

分阶段实施顺序、代码落点、门槛、验证口径与失败处理见 `docs/plan/packed-topology-direct-write-and-key-passthrough.md`。该文件以本文基线为输入；未通过门槛时保留可验证收益最小的改动集合，并把未达标原因写回 `docs/issue/linkrs-remaining-parity-issues.md`。
