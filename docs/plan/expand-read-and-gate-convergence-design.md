# 展开读取与阶段一门收敛设计

**日期**：2026-11-23
**状态**：下一阶段设计文档，不含代码改动。
**证据来源**：`perf-data/parity/results/2026-11-23/scan-visibility-decode-remediation/`，以及 `docs/issue/linkrs-remaining-parity-issues.md` 第 38 节。

## 1. 剩余问题

扫描可见性与枚举收敛后，阶段一两条固定门用例的 `scan_read_us` 已从 492–700 ms 降到 87–120 ms，端到端中位数降到 47.67–58.04 ms，但 `group_by_count ≤40 ms`、`distinct_limit ≤30 ms` 尚未同时达成。最终回测中展开读取反而成为最大的未拆算子：

| 口径 | `expand_read_us` | 说明 |
|---|---:|---|
| 固定门第一轮 | 160514 / 170578 us | 两用例，10 分区累计 |
| 固定门第二轮 | 230090 / 207568 us | 同数据集，轮间波动明显 |
| 生产默认单轮 | 129762 / 131585 us | 8 分区累计 |

`expand_read_us` 目前只包住两次存储调用：packed 拓扑批量读取和目的端批量读取。它不包含标签解析、去重、端点数组组装、存在性过滤与 typed 列装配；因此既无法解释上述波动，也无法判断应改存储层还是查询层。最终回测的 packed 微基准为 3395 us / 2048 父、8.68 MB/call，目的端属性列读取为 1105 us / 2048 ID、1.41 MB/call，二者与累计值数量级一致，但仍缺少本轮同口径拆分。

本阶段目标是把展开读取拆开、用微基准定位主因，再只在有实测收益的环节收敛代码，最终把两条阶段一用例推过 40 ms / 30 ms 中间门。

## 2. 代码路径

- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs`：`expand_columnar` 在 packed 分支读取拓扑后构造 `parent_of_child` 与 `all_dst`，再调用 `read_destinations`；当前两个计时点分别覆盖拓扑读取与目的端读取。`read_destinations` 对每个批次的全部目的端建立 `HashMap<VertexId, slot>` 去重，再把唯一 ID 的属性值按边展开回输入顺序。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_dispatch.rs`：`closed_loop_dst_tag` 在每次展开中解析目的端标签；`hop_bypass` 决定目的端属性是否需要绕行。
- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs`：`packed_node_edge_headers_batch` 每次调用都解析 space、列全部边类型、解析端点标签；CSR 结果先写入 `Vec<Event>`，再计数散射到父有序数组，并维护跨批的 `(LabelId, u32) -> VertexId` 翻译表和 `Both` 方向去重集合。
- `crates/linkrs-storage/src/engine/graph_storage/context/edge_ops.rs`：`out_topology_batch` 与 `in_topology_batch` 已按父返回 `Vec<Vec<(LabelId, HotNbr)>>`，父顺序与 CSR 访问顺序稳定，具备直接写入 packed 输出的条件。
- `crates/linkrs-storage/src/engine/graph_storage/reader/vertex.rs`：`get_vertex_property_columns_batch` 每次调用重新解析 space/tag/TTL 与读时间戳；空投影已经可以作为存在性查询使用，不应把该路径改回完整实体构造。

参考实现中，Ladybug 的 `ref/ladybug/src/processor/operator/scan/scan_rel_table.cpp` 用父选择向量和偏移描述父子关系，Neug 的 `ref/neug/include/neug/storages/csr/csr_view.h` 在连续 CSR 上做 POD 迭代。当前 packed DTO 已具备数组形态，剩余成本在事件中转、重复解析与目的端展开，不应再更换 DTO。

## 3. 先拆分计时与微基准

### 3.1 查询层子计时

在 `ColumnarStats` 增加只描述代码意图的字段：`expand_topology_read_us`、`expand_destination_read_us`、`expand_tag_resolve_us`、`expand_view_us`、`expand_output_us`。分别覆盖 packed 拓扑读取、目的端批量读取、目的端标签解析、去重/存在性视图构造、typed 输出装配。`expand_read_us` 保留原边界，作为与历史结果可比的总量。

`expand_columnar.rs` 在现有两个计时点之外，把 `closed_loop_dst_tag`、`read_destinations` 中的去重与值展开、rowless 分支的 `kept` 收集与 typed 列构造分别计时。每个阶段的计数一并进入 EXPLAIN 与 parity JSON：父批数、事件数、唯一目的端数、缺失目的端数、输出行数。计数用于区分“读得慢”和“读得多”，不引入采样。

首轮只计时，不改算法。用同一固定门跑两轮，确认五个字段与事件计数可以解释 `expand_read_us`的量级和轮间波动；若总量仍无法解释，先停在归因，不继续改实现。

### 3.2 扩展微基准

扩展 `crates/linkrs-storage/benches/topology_read_bench.rs`，在同一 2 万顶点数据集上固定五组对照：

| 组 | 变量 | 目的 |
|---|---|---|
| F1 | packed 拓扑，父批 512 / 2048 / 4096 | 父批与事件基数 |
| F2 | Out / In / Both，带与不带边类型过滤 | 去重、方向、类型筛选增量 |
| F3 | 目的端唯一率 100% / 50% / 10% | 去重与展开放大系数 |
| F4 | 目的端属性列：存在性、单列、全实体 | flat 需求与实体构造差异 |
| F5 | packed + 目的端连续调用 | 与查询层 `expand_read_us` 对齐 |

分配计数沿用现有全局分配器方式，输出 us/call、B/call、B/edge、B/unique-dest。必要时扩展 `crates/linkrs-query/benches/query_stage_bench.rs` 覆盖查询层视图装配，但不新建替代基准套件。

### 3.3 归因门槛

- 五种子计时与 `expand_read_us` 的差额应稳定在 10% 以内；否则先补计时点。
- 若拓扑读取占 `expand_read_us` 主导，进入切片一、二；若目的端读取与视图展开主导，优先切片三。
- 若两者均不主导，停止本阶段存储改动，转去解释固定门线程间等待和分区调度，不叠加无效优化。

## 4. 修改方案

### 切片一：查询内解析复用与视图装配收敛

- `closed_loop_dst_tag` 的结果在 `ExpandCtx` 或等价的算子实例内按 `space + edge_types + direction` 解析一次，供同一查询的后续批复用。缓存生命周期限定为单次执行，不跨查询、不做全局可变状态；schema 变化不影响已固定读时间戳的查询快照。
- `read_destinations` 的去重改为对 `VertexId` 的排序聚合并保留输入槽位映射，替代逐边 `HashMap` 插入；输出仍逐边对齐，重复 ID 只触发一次批量读取。
- 对 `rowless || dst_empty` 的形状继续走空投影存在性读取；不要为了复用属性解码而把该路径改回构造 `Vertex`。
- 该切片只改变容器与解析次数，不改变边顺序、重复边语义、悬空端点过滤和缺失属性为 NULL 的语义。

### 切片二：packed 拓扑按父直接写入

- `packed_node_edge_headers_batch` 利用 `out_topology_batch` / `in_topology_batch` 已经按父分段的事实，对每个父直接把孩子写入紧凑数组并在父结束时追加 `child_offsets`，取消 `Event` 中转数组、`counts` 数组与 `cursors` 散射。
- 外部 ID 翻译仍按 `(LabelId, 局部 ID)` 去重，但作用域收敛到当前调用；`Both` 方向的去重键与顺序保持原规则。若同一批内相同端点跨父重复，允许复用翻译结果，但不能跨调用缓存。
- `PackedEdgeTopology` 的 `parents`、父子偏移、CSR 访问顺序与边类型 ID 契约不变；`get_node_edge_headers_batch` 继续由 packed 结果转换，外部语义不变。
- 先只对 Out / In 无去重分支落地；Both 分支在微基准证明需要且对拍一致后再采用同一写入方式。

### 切片三：目的端身份绕行（条件性）

- 仅当计划能证明目的端需求是标签声明的主键/外部 ID 列，且下游只把它当作 `VertexId` 身份使用时，允许用 packed 结果中的目的端 `VertexId` 直接生成该 bypass 列，目的端批量读取只负责存在性。
- 证明条件不成立时继续走现有属性列读取，不按列名猜测；字符串 ID、整数 ID、TTL、属性和 ID 不一致的 schema 都必须保持当前语义。
- 该切片的收益必须由 F4 微基准和固定门共同证明；只缩短代码路径而没有端到端收益则不保留。

### 切片四：父批大小验证

用 F1 比较 512 / 2048 / 4096 父批。只有当固定门也显示收益时才调整 `storage_scan.rs` 的批上限或 `ScanOptions::batch_size`；不新增生产开关，不把 bench 参数直接推广为默认值。

## 5. 正确性矩阵

每个候选改动必须在以下矩阵上与当前实现逐值一致：

- 空父批、单父、多父、重边、自环、平行边、悬空端点；
- Out / In / Both，多边类型与空边类型过滤；
- staged insert / update / delete 与已提交边混合；
- 快照早于/晚于提交边界，确认解析复用不引入跨快照读取；
- TTL 表与非 TTL 表、NULL 属性、多标签目的端；
- 目的端唯一率为 100% / 50% / 10% 时结果顺序与重复行一致；
- DQL 全量、`parallel_partition_execution`、列批相关 integration test、storage lib 全量；
- 分区并集与未切片全表结果逐值一致，行序不变。

## 6. 门槛

中间门（微基准）：

- packed 拓扑读取相对本阶段基线 3395 us / 8.68 MB 下降至少 25%，且分配不高于 6.5 MB/call；
- 目的端属性列读取在 F3=10% 唯一率的形状上，视图展开耗时下降至少 30%；
- 父批 512 与 4096 的拓扑-only 基线不得退化超过 5%。

最终门（固定门）：

- 两条阶段一用例的 `expand_read_us` 中位数相对本阶段基线下降至少 30%；
- `group_by_count ≤40 ms`、`distinct_limit ≤30 ms`，且 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`；
- 生产默认单轮无实质回归，九条纯计数与 q1–q9 中位数退化不超过 10%。

## 7. 护栏与不做项

- 不改变 MVCC 可见性、TTL、staged write、边去重和 NULL 语义；不跨调用缓存可见性或端点存在性结果。
- 不引入近似、采样或结果截断；不跨查询缓存 schema，不扩大并行放行面，`parallel.enabled` 继续为 false。
- 不修改 CSR/属性页物理布局，不把属性内联进 CSR，不新增“version 2”模块；所有改动落在上述原模块。
- 不因父批微基准结果直接改变生产默认配置；配置变更需要固定门证明。
- 编译中位数仍约 2–3 ms，是独立剩余项。本阶段先完成展开收敛；若最终门达成后再复用 `query_stage_bench.rs` 做 parse/bind/分区决策/arena 构建归因，不在此阶段并行推进。

## 8. 执行顺序

1. 实现子计时与 F1–F5 微基准，跑两轮固定门，确认归因。
2. 实现切片一，跑正确性矩阵与微基准；视图装配无收益则回退。
3. 实现切片二，跑完整固定门；以 `expand_read_us`、B/edge 与端到端中位数为主门槛。
4. 仅在目的端成为主导时评估切片三；否则不追加身份绕行复杂度。
5. 全部通过后更新 `docs/issue/linkrs-remaining-parity-issues.md` 与 `docs/plan/stage1-stage2-current-gap-remediation.md`；未通过则保留收益最小的改动集合，并把未达标原因写回问题文档。
