# 父端点身份批量解析设计

**日期**：2026-11-24  
**依据**：`docs/plan/parent-endpoint-and-partition-jitter-design.md` 的复测结论、Ladybug / Neug 参考实现、devs 最近固定门与微基准。本文只描述本次实施范围、正确性边界与验证方法。

## 1. 实测结论

固定门 `group_by_count` 与 `distinct_limit` 的共享成本集中在扫描和展开：10 个分区累计 `scan_read_us` 约 90–257 ms、`expand_read_us` 约 159–192 ms。`scan_decode_us`、`scan_predicate_us` 为零，`multiplicity_expanded`、`selection_attached`、`spill_rows` 也为零，说明行式投影、谓词和行数膨胀都不是主要矛盾。

固定门工作量稳定为 110 个父批、215,488 个父端点、约 30,473 个唯一目的端。每个父端点恰好产生一条边，展开后无需 multiplicity 加权。Ladybug 通过 `PackedChildSlices` 让下游直接消费父区间与孩子偏移，Neug 通过 dense vid 与 POD 邻居迭代器避免逐行反查；Linkrs 当前仍在这两段路径上逐行解析身份。

## 2. 本次实施范围

### 2.1 批量反向 ID 解析

当前 `scan_topology()` 对枚举出的每个 internal id 调 `get_external_id_raw()`。该调用链会进入 `IdManager::get_key()`，每次持有一次 core 锁并 clone 一个 `IdKey`；215,488 个父端点因此产生同等次数的锁与 clone，且 `scan_topology()` 还会按 shard 重复访问并排序结果。

本次新增批量反查接口：在一次 core 锁内读取连续 internal id 区间，把外部 id 写入目标缓冲。拓扑-only 扫描在 shard 内一次完成 live id 枚举与外部 id 解析，避免二次加锁与逐行 clone。TTL、staged rows、allowlist 与 row 路径不进入该快路径，继续沿用现有语义。

### 2.2 批量目的端外部 ID 解析

`packed_node_edge_headers_batch()` 对每个唯一目的端调用 `endpoint_to_external()`，后者同样落入逐行 `get_external_id_raw()`。本次先把 `(label, internal endpoint)` 去重，再按 label 调用存储层已有的 `external_ids_by_internal_batch()`，最后按 child 顺序 scatter 回 `PackedEdgeTopology`。只替换身份解析方式，不改变边顺序、去重规则、悬空端点处理或 MVCC/pending gate 语义。

### 2.3 不在本次修改的内容

- 不新增“version 2”模块，不创建替代 reader。
- 不改变父批大小、分区数、worker 数、通道容量和并行放行条件。
- 不实现 packed expand 与聚合算子的直连契约；该项需要修改计划形状，单独验证。
- 不在 `group_by_count` 的 Exchange 前做局部 TopK；虽然按分组键裁剪在理论上成立，但本次先隔离身份批量化的收益。
- 不重引入 sorted destination dedup、身份绕行或完整目的端半连接物化。

## 3. 正确性边界

- 外部 id 的可见性仍由枚举阶段与读时间戳决定；批量反查只读取已确定 live 的 id。
- 文本 id 仍保留原 `IdKey::Text` 语义，批量接口只减少锁次数和临时对象，不改变编码。
- staged insert/delete、allowlist、TTL 表、row 路径、非 topology-only 扫描继续走原分支。
- 拓扑目的端解析失败时仍保留原始 internal 值作为 dangling endpoint，不使用近似值。
- `TopologySchema` 缓存契约、`VertexHandleColumn` 契约与边排序不变。

## 4. 验证方法

1. devs release 编译。
2. 正确性门：配置默认值、partitioning、typed group、local topk、并行分区执行、DQL、storage lib、column batch 集成测试。
3. 微基准：拓扑 packed 批量读取、visibility/decode、目的端身份读取；记录时间与分配。
4. 固定门：`group_by_count` 与 `distinct_limit` 两轮，校验 `actual=10`、`result_stable=true`、`spill_rows=0`。
5. 生产默认单轮九条纯计数与 q1–q9 回归检查。
6. 只有微基准与固定门同时显示收益的改动保留；任何正确性或稳定性回退直接撤回对应改动，不以配置掩盖。

结果目录按 `docs/dev/build/devs-sync.md` 的约定写入本地 `perf-data/parity/results/2026-11-24/parent-identity-batch/` 与 devs 对应目录。

## 5. 复测结论

devs release 编译通过，正确性门全绿：配置默认值、partitioning、typed group、local topk、并行分区执行、DQL、storage lib 与 column batch 全部通过。首轮编译暴露了两处作用域/类型错误，已直接修正，未改变设计。

固定门两轮 `group_by_count` 为 43.71 / 42.36 ms，`distinct_limit` 为 37.93 / 37.82 ms，结果逐值一致、`actual=10`、`spill_rows=0`。`scan_read_us` 由上一轮 90–257 ms 降到 40–45 ms，`scan_visibility_us` 由 67–234 ms 降到 15–19 ms，说明批量反向 ID 解析解决了扫描阶段的主要重复工作。分区偏斜为 1.18–1.33，较上一轮收敛。

`expand_read_us` 仍为 172–193 ms，成为当前唯一主导项；本轮的目的端批量解析未带来固定门级收益，微基准 F2 Out 从 7161 us 降到 5840 us、F3/F4 小幅改善，但 F1 从 2722 us 升到 3176 us，属于分配与噪声混合结果。`group_by_count ≤40 ms`、`distinct_limit ≤30 ms` 仍未同时达成。下一步应实施 packed expand 与局部聚合/TopK 的直连契约，不再在身份解析上叠加复杂度。

## 6. 展开阶段收敛复测

按上一轮分析依次实施并验证两项：

1. `out_topology_batch()` / `in_topology_batch()` 不再为每个父点建 `Vec`，改为 event 收集后按父点 prefix-sum scatter 到扁平 `entries + offsets`。目标形状每个父点仅一条边，因此消除了约 215,488 次小堆分配。另测试了 stable-sort 变体，虽然分配更低，但 F2 Out 从 6659 us 升到 11243 us、固定门也变差，已撤回。
2. `PackedEdgeTopology` 增加 child-aligned 目的端 internal handle；`read_destinations()` 把唯一目的端的 handle 传给新增的 `get_vertex_property_columns_with_handles_batch()`，跳过 external→internal 反查。目的端视图改为 `values + slots` 的索引结构，不再每条边 clone 一个 `Vertex`。

devs release 编译通过，正确性门全绿。固定门两轮 `group_by_count` 为 38.80 / 38.73 ms，`distinct_limit` 为 31.14 / 32.61 ms，结果逐值一致、`actual=10`、`spill_rows=0`。`expand_read_us` 从 172–193 ms 降到 124–161 ms，其中 topology 从 111–130 ms 降到 79–101 ms，destination 从 39–47 ms 降到 28–44 ms，view 从 18–20 ms 降到约 16 ms。

`group_by_count ≤40 ms` 已达成；`distinct_limit ≤30 ms` 在负载 5–12 的窗口内仍有 1–3 ms 差距，后续三次直接回测为 32.88 / 34.20 / 33.41 ms，未形成稳定反证。packed expand 与局部聚合/TopK 直连涉及计划形状和 Exchange 契约，本轮不混入身份收敛改动；在安静窗口确认仍不达标后再单独实施。结果与撤回记录见 `perf-data/parity/results/2026-11-24/expand-topology-flatten/`。
