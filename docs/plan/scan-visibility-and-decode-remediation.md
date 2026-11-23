# 扫描可见性枚举与属性解码：验证与修改方案

**日期**：2026-11-23  
**状态**：设计文档，不含代码改动。前提与归因直接引述 `docs/plan/remaining-bottleneck-microbench-and-design.md` 第 2、4、8 节、`docs/issue/linkrs-remaining-parity-issues.md` 第 37 节，以及 `perf-data/parity/results/2026-11-23/stage1-stage2-remediation-final/`。

## 1. 前提与当前结论

上一轮已确认拓扑与目的端的物化成本下降：`packed` 读取由 3985 us / 15.5 MB 降到 3280 us / 8.68 MB，目的端身份列读取由 1361 us 降到 1090 us，固定门 `expand_read_us` 由约 160–210 ms 降到 110–135 ms。但 `group_by_count` 与 `distinct_limit` 的端到端中位数仍为 88.25 / 98.20 ms 与 84.29 / 135.52 ms，未跨越 40 ms / 30 ms 中间门。

最终 EXPLAIN 显示两条查询由 anchored 规划器生成，物理扫描已经按分片切片枚举（每分区约 2.15 万行、总量约 21.5 万行），不存在重复值域全表扫描。补充微基准中，分片纯拓扑扫描为 303–342 us / 2000 行，加属性投影后为 666–735 us / 2000 行，接近两倍。由此本轮把问题限定为存储扫描内部的可见性枚举、重复可见性复核、列解码锁开销与批次装配，不再回到分区策略。

## 2. 代码路径定位

- `crates/linkrs-query/src/executor/streaming/operators/source_operator/storage_scan.rs` 的 `scan_read_us` 只包住 `next_column_batch`，因此它衡量的是存储游标内部成本，不包含查询层把列批转成 `DataChunk` 的部分。
- `crates/linkrs-storage/src/engine/graph_storage/cursor_impl/vertex.rs` 的 `load_next_table` 调用 `live_ids_in_slice`，`collect_column_batch` 再做外部 ID 范围裁剪、zone-map 裁剪、属性解码、选择与列合并。
- `crates/linkrs-storage/src/vertex/vertex_table/sharded/read.rs` 的 `live_ids_in_slice` 已按 `VisibilityGuard` 过滤一次；`scan_columns` 与 `scan_topology` 又通过 `shard_row_visible` 对同一批 ID 过滤一次。两次都基于同一个游标快照，这是首要验证点。
- `scan_columns` 每次调用都会 `group_by_shard` 重建按 shard 分组的向量，构造与候选数等长的 `vids: Vec<Option<VertexId>>` 和预分配列，再在末尾 `select` 压缩。候选已经可见时，这段预分配/压缩是可疑成本。
- `crates/linkrs-storage/src/vertex/column/column_store.rs` 的 `decode_column_values_at_ts` 对每一行调用 `Column::get_at_ts`；`crates/linkrs-storage/src/vertex/column/mvcc.rs` 的 `get_at_ts` 每次都会 `self.chunks.read()`。一个 2048 行、两列的属性批因此产生数千次 RwLock 获取，这是第二个首要验证点。
- `crates/linkrs-storage/src/vertex/vertex_table/sharded/read.rs` 的 `zone_prune_mask` 每行再查 zone-map；是否有谓词决定这条路径是否存在。目标两条查询没有下推谓词，因此它不是本轮主因，但需要在验证矩阵中保留。

参考实现中，Ladybug 的 `ref/ladybug/src/processor/operator/scan/scan_node_table.cpp` 把 morsel 作为并行单位，`ref/ladybug/src/storage/table/column_chunk.cpp` 以 chunk 为单位把连续行区间扫进 `ValueVector`；Neug 的 `ref/neug/include/neug/storages/csr/csr_view.h` 在连续 CSR 上用 POD 迭代器完成可见性过滤。两者的共同点是：可见性判断与属性解码都按连续块进行，不按单行重复获取锁或重建容器。

## 3. 验证方案

### 3.1 先拆分 `scan_read_us`，再改代码

在 `ColumnarStats` 增加只描述代码意图的子计时字段：`scan_enumerate_us`、`scan_visibility_us`、`scan_decode_us`、`scan_assemble_us`、`scan_predicate_us`。它们分别覆盖游标枚举、可见性复核、列解码、批内合并、谓词过滤。经 `columnar_summary` 进入 EXPLAIN，并由 `src/bin/linkrs_parity_bench.rs` 写入固定门 JSON。

首轮只做计时，不改算法；用同一固定门跑两轮，确认五个字段之和可以解释 `scan_read_us` 的主体。若 `scan_visibility_us` 或 `scan_decode_us` 不能稳定占比，则先停在归因，不继续改实现。

### 3.2 扩展非 e2e 微基准

扩展 `crates/linkrs-storage/benches/scan_division_bench.rs`，在同一 2 万顶点数据集上固定五组对照：

| 组 | 变量 | 目的 |
|---|---|---|
| A | 分片拓扑-only，批大小 512 / 2048 / 4096 | 枚举与批大小基数 |
| B | 分片 + 一列投影，同样批大小 | 列解码增量 |
| C | 分片 + 全属性解码 | 与 B 对比是否空投影也有解码 |
| D | 分片 + 可下推谓词 | zone-map 与谓词过滤增量 |
| E | 全表枚举 + 外部 ID 范围 | 保留为 multi-scan 形状对照，不用于本轮门槛 |

新增 `crates/linkrs-storage/benches/visibility_decode_bench.rs`，只驱动存储层公开入口，不经过 parser/planner/pipeline：分别测量可枚举 ID 的可见性复核、按列批解码、整批装配，并记录分配字节/行。分配计数沿用现有 `topology_read_bench.rs` 的方式。

### 3.3 正确性对拍矩阵

每个候选改动必须在以下矩阵上与当前实现逐值一致：

- 空表、空分片、单分片、默认多分片；
- staged insert、staged update、staged delete 与已提交行混合；
- 快照早于/晚于提交边界，确认重复可见性检查删除后边界仍正确；
- TTL 表与非 TTL 表；
- 外部 ID 范围、`AND/OR` 谓词、NULL 属性、多标签表；
- 分区切片并集必须等于未切片全表扫描，行顺序一致；
- `linkrs-query` DQL 全量、`parallel_partition_execution`、storage lib 全量。

### 3.4 门槛

中间门（微基准）：

- 一列投影分片扫描相对当前 666–735 us / 2000 行下降至少 30%；
- 可见性复核与列解码合计下降至少 40%，或分配字节/行下降至少 30%；
- 拓扑-only 基线不得退化超过 5%。

最终门（固定门）：

- 两条阶段一用例的 `scan_read_us` 中位数相对当前下降至少 30%，且 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`；
- `group_by_count` ≤40 ms、`distinct_limit` ≤30 ms；生产默认单轮无实质回归；
- 九条纯计数与 q1–q9 中位数退化不超过 10%。

## 4. 修改方案

### 切片一：删除游标内部的重复可见性复核

- `live_ids_in_slice`、`live_ids` 产出的 ID 已在同一个 `VisibilityGuard` 下完成行级可见性判断；`scan_columns` 与 `scan_topology` 对该批 ID 的二次 `shard_row_visible` 不增加语义，只增加成本。
- 在 sharded vertex table 契约中增加显式来源标记，区分“游标已验证的可见 ID”和“点读/外部调用传入的未验证 ID”。只有前者跳过复核；`resolve_projected`、`resolve_projected_batch` 等点读路径继续保留原检查。
- staged merge 产生的 `dropped_ids` 与 `staged_rows` 仍在 `load_next_table` 里按原规则处理；TTL 仍由游标和投影列按原路径判断，不能借来源标记跳过。
- 正确性论据：游标快照在打开时绑定，晚于快照的写入在 `PendingGate` 下不可见；同一 guard 对同一 stamp 的重复判断恒等。需要新增一个针对“枚举后、解码前提交”的回归测试。

### 切片二：按 chunk 批量解码列

- 在 `Column` 上增加批解码入口：锁定一次 `chunks`，按 chunk 分组行号，再逐行复用现有 `get_in` 语义。保持当前类型化 `ColumnValues` 输出、NULL 语义和类型不符时回退 `General` 的行为。
- `decode_column_values_at_ts` 改调用该入口，消除每行 `chunks.read()`；不修改列页布局、版本链格式或写出路径。
- `scan_columns` 在拿到可见行后再预分配列，避免按候选数预分配再 `select` 压缩；`vids` 直接收集可见行的外部 ID，去掉 `Option<VertexId>` 中间数组。
- 若单行版本链查找仍是热点，再在同一批内把同一 chunk 的 state guard 提升到组级；该优化必须在切片二的微基准证明有收益后才做。

### 切片三：分片内枚举与解码融合

- 当前 `load_next_table` 生成全局 ID，`collect_column_batch` 再 `group_by_shard` 解码，相当于每个 shard 锁两次、ID 编解码两次。
- 在 sharded vertex table 增加“按分片可见切片直接解码”的批量入口：在一次 shard 读锁内完成局部 ID 枚举、可见性判断、外部 ID 读取与列解码，输出已经按全局 ID 排序的列批。
- 游标只消费该列批；`zone_prune_mask` 与 staged merge 的过滤顺序保持不变，避免改变谓词与 staged write 的交互。
- 该切片依赖切片一、二的计时结果；若两者已把 `scan_decode_us` 降到非主导，则不追加融合复杂度。

### 切片四：批大小对齐（仅验证，不直接改默认）

- 用微基准 A/B/C 比较 512 / 2048 / 4096 的行批；只有当固定门也显示收益时才调整 `storage_scan.rs` 的 `fixed_batch` 上限或 `ScanOptions::batch_size`。
- 不在生产配置中新增开关，也不把 bench 参数直接推广为默认值。

## 5. 护栏与不做项

- 不改变可见性语义、TTL 语义、staged write 语义或版本链格式；来源标记只用于跳过同 guard 下的重复检查。
- 不引入近似、采样、结果截断；不跨调用缓存可见性结果。
- 不修改 CSR/属性页物理布局，不把属性内联进 CSR。
- 不恢复或新增“重复全表值域扫描”路径；分片切片仍是并行扫描的枚举方式。
- 不扩大生产默认灰度：`parallel.enabled` 继续为 false，仅保留已验证的 `allow_ordered_parallel_expand` 窄形状。
- 不新增“version 2”模块；所有改动落在上述原模块。

## 6. 执行顺序

1. 先实现子计时与微基准矩阵，跑一轮固定门，确认归因。
2. 实现切片一，跑正确性矩阵与微基准；若 `scan_visibility_us` 未下降，停止并复查。
3. 实现切片二，跑完整固定门；以 `scan_decode_us` 与分配字节/行为主门槛。
4. 仅在前两步收益成立时评估切片三。
5. 全部通过后更新 `docs/issue/linkrs-remaining-parity-issues.md` 与 `docs/plan/stage1-stage2-current-gap-remediation.md` 的状态；未通过则保留已验证收益最小的改动集合，不叠加无收益复杂度。
