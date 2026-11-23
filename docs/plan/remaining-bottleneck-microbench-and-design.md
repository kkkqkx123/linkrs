# 剩余性能瓶颈：微基准归因与改进设计

**日期**：2026-11-23  
**状态**：基于 `docs/issue/linkrs-remaining-parity-issues.md` 第 37 节和 `perf-data/parity/results/2026-11-23/stage1-stage2-final/` 的实测结果，定位阶段一、二未达标的剩余原因，并给出后续改进顺序。本文不重新定义原问题，也不扩大生产放行面。

## 1. 目标与口径

首轮收敛改动验证了三件事：生产默认配置收益真实；正确性门全绿；`scan_read_us` 与 `expand_read_us` 是端到端 Self-time 的主体。但没有回答“这两个字段内部究竟由什么构成”。本文补充两个不经过解析、计划、流水线的 Rust 微基准，把扫描分区和批量拓扑读取拆开，再结合 `ref/ladybug`、`ref/neug` 的实现确定改进方向。

微基准目标：

- `crates/linkrs-storage/benches/scan_division_bench.rs`：同一种拓扑扫描分别使用全表枚举、外部 ID 范围过滤、分片切片，比较枚举与裁剪成本。
- `crates/linkrs-storage/benches/topology_read_bench.rs`：比较当前 packed 拓扑读取、旧 per-parent 拓扑读取、lean 邻居 ID、度数读取，以及目的端整实体/投影读取，并记录分配量。

命令：

```bash
cargo bench -p linkrs-storage --bench scan_division_bench
cargo bench -p linkrs-storage --bench topology_read_bench
```

原始输出见 `perf-data/parity/results/2026-11-23/microbench/`。数据集为 2 万顶点、每点 16 条边、单批 2048 个父节点；它是用于形状归因的合成负载，不是 LSQB sf0.1 的绝对时延预测。

## 2. 微基准结果

### 2.1 扫描分区

| 方式 | 中位耗时 | 返回行 | 每行耗时 |
|---|---:|---:|---:|
| 全表枚举 | 3806 us | 20000 | 190 ns |
| 外部 ID 范围（1/10） | 2778 us | 2000 | 1389 ns |
| 分片切片（1/10） | 314 us | 2000 | 157 ns |

外部 ID 范围过滤只返回 1/10 行，却仍花掉全表枚举约 73% 的时间；分片切片耗时约为范围过滤的 1/9。当前 `group_by_count` 与 `distinct_limit` 的并行形状属于“按顶点 ID 值域共分区的 Join”，`crates/linkrs-query/src/executor/streaming/plan/arena_builder/partition/join.rs` 对两侧都使用 `ScanDivision::ByValueRange`，所以每个分区都重复一次全表可见性枚举与外部 ID 判断。这与固定门中 `scan_read_us` 在 10 个分区累计约 420–570 ms 的量级一致。

### 2.2 批量拓扑与目的端读取

| 路径 | 中位耗时 | 分配量 | 归一化 |
|---|---:|---:|---:|
| `get_packed_node_edge_headers_batch` | 4054 us | 15.5 MB/call | 1979 ns/parent，7574 B/parent |
| `get_node_edge_headers_batch` | 5155 us | 21.1 MB/call | 2517 ns/parent，10286 B/parent |
| `neighbor_dst_ids_batch` | 11144 us | 4.0 MB/call | 5441 ns/parent，1960 B/parent |
| `out_degree_batch` | 565 us | 42 KB/call | 275 ns/parent，20 B/parent |
| 目的端整实体读取 | 1544 us | 2.16 MB/call | 753 ns/id，1056 B/id |
| 目的端投影 `id` | 1431 us | 2.12 MB/call | 698 ns/id，1032 B/id |

packed 数组相比旧 per-parent 批次确实减少约 21% 时间和 26% 分配量，但没有改变数量级。当前实现仍在 `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs` 中为每个父节点建一个 `Vec`，为每条边克隆 `Arc<str>`，并用 `HashMap` 翻译外部端点。这些是 7.5 KB/parent 分配的主要来源。

`neighbor_dst_ids_batch` 虽然分配更少，但时间反而最高，因为它对每条 CSR 边单独做内部 ID 到外部 ID 的翻译并构造 V 形批次；不能把它当作 `packed` 的无条件替代品。`out_degree_batch` 快 7.2 倍，说明纯度数/计数需求必须留在度数路径，不能被通用拓扑路径吞掉。

目的端读取中，投影 `id` 与整实体读取差异很小：当前投影 API 仍返回 `Vertex` 及其属性映射。对于 `RETURN p.id` / `RETURN DISTINCT p.id` 这类只需要身份列的形状，这是可避免的分配。

## 3. 与参考实现的对照

### 3.1 Ladybug

- `ref/ladybug/src/processor/operator/scan/scan_node_table.cpp` 在节点组少于工作线程时把扫描切成子 morsel，目标为每线程约 8 个 morsel，让下游遍历获得并行度，而不是让每个 worker 重扫整表再过滤。
- `ref/ladybug/src/processor/operator/scan/scan_rel_table.cpp` 把多父批次的父子关系表达为 `PackedChildSlices`，由绑定侧 selection vector 与孩子前缀偏移共同描述，孩子缓冲本身仍是紧凑向量。
- `ref/ladybug/src/processor/operator/scan/count_extend_chain.cpp` 直接用 `packedChildOffsets` 做度数乘积累加，最后一跳完全不物化邻居实体。
- `ref/ladybug/src/processor/operator/aggregate/packed_filtered_count.cpp` 在 packed 批次内按父分片做局部计数，再把父键对应的计数写入本地聚合状态；全局聚合仍然最终归并。

结论：Ladybug 的收益来自“枚举一次、按 morsel 分发”和“父子偏移作为算子间契约”，不是来自更复杂的聚合。

### 3.2 Neug

- `ref/neug/include/neug/storages/csr/csr_view.h` 的 `NbrIterator` 是 POD，直接在连续 CSR 字节上按 stride 前进，并在迭代中跳过不可见 timestamp；`NbrList` 只持有指针区间。
- `ref/neug/include/neug/utils/top_n_generator.h` 与 `ref/neug/src/execution/execute/ops/retrieve/group_by_utils.cc` 使用紧凑键编码和 typed key 路径，避免每行构造通用对象。

结论：Neug 的邻接读取没有每父容器、没有每边 `Arc`/`HashMap`，可见性判断内联在迭代里。当前 Linkrs 的 packed DTO 只实现了“数组扁平化”，还没有达到这种布局。

## 4. 主要瓶颈判定与修正

按初始证据强弱排序：

1. **值域共分区重复全表枚举**：微基准显示范围过滤相对分片切片有约 8.8 倍退化；固定门的扫描自耗时与 10 分区重复枚举吻合。这是阶段一两条用例仍接近 100 ms 的首要原因。
2. **展开批次仍在高分配量物化拓扑**：4 ms/2048 父、7.5 KB/parent；固定门 `expand_read_us` 约 160–210 ms 与该量级一致。下一轮目标是消除 per-parent `Vec`、每边 `Arc` 与端点翻译 `HashMap`，而不是继续调整聚合/去重。
3. **目的端只读身份却构造完整实体**：约 1.4 ms/2048 且投影几乎不省分配，直接抬高 `group_by_count` 与 `distinct_limit` 的展开后处理成本。
4. **`join_two_hop` 仍约 6–10 ms**：度数读取本身很快，但两跳路径仍有批量 staging/外部 ID 往返。它先作为独立小切片处理，不阻塞前三项。
5. **编译中位数 2–3 ms**：尚未被微基准直接归因；在扫描与展开收敛前不作为主线。

修正：最终 EXPLAIN 显示 `group_by_count` 与 `distinct_limit` 由 anchored 规划器生成，物理扫描已经使用分片切片（每分区约 2.15 万行，总量约 21.5 万行），并没有重复全表值域枚举。因此第 1 条只适用于 multi-scan 值域共分区形状，不是这两条目标查询的当前瓶颈。补充微基准中带属性解码的分片扫描为 735 us/2000 行，接近纯拓扑分片扫描 342 us 的两倍，说明这里的扫描成本来自可见性枚举与属性解码本身，而不是分区方式。

## 5. 改进设计

### 切片 A：ID 键共分区 Join 改为 vid 哈希交换 + 分片扫描（未采纳）

- 在 `crates/linkrs-query/src/optimizer/partitioning/planner/multi_scan.rs` 中，把“双方连接键都引用 vid”从 `Range` 改为 `PartitionStrategy::Hash { key: <vid 变量> }`；其余简单变量键策略保持不变。
- 在 `crates/linkrs-query/src/executor/streaming/plan/arena_builder/partition/join.rs` 的 `build_hash_exchange_join` 中移除 vid 键的拒绝条件，让两侧扫描改走 `ScanDivision::ByShardSlice`，再按同一 vid 哈希函数交换到桶内做局部 Join。
- 局部 Join、重复边语义、空值语义与 Concatenate 汇总保持不变；每个 vid 仍只映射到唯一桶，不引入去重、采样或近似。
- 文件落点限于上述两处及对应 partitioning/DQL 测试；不改 `chain.rs` 已使用的分片路径。

门槛：固定门 `group_by_count` ≤40 ms、`distinct_limit` ≤30 ms；每分区 `scan_read_us` 相对当前下降 ≥5 倍；结果逐值一致；原有 Range/Hash/RoundRobin 分区测试全绿。

实测中该策略切换没有改变目标查询的 EXPLAIN（仍由 anchored 规划器生成、仍走分片扫描），也没有改变 `scan_read_us`。按无实测收益不保留复杂度、不扩大未验证策略面的原则，代码已回退到原有 Range/Hash 选择；本切片不进入最终改动集合。

### 切片 B：packed 拓扑改为真正紧凑布局

- 在 `crates/linkrs-storage/src/client/traits_reader.rs` 的 `PackedEdgeTopology` 契约中保留父顺序、孩子前缀偏移和 CSR 访问顺序，不再要求每条边携带独立的 `Arc<str>` 类型名。
- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs` 改为两遍处理：先统计每个父的孩子数得到前缀偏移，再一次性写入扁平 `src`/`dst`/`rank` 与紧凑边类型 ID；只有边属性或下游确实需要类型名时才解析为 `Arc<str>`。
- 端点翻译按 (label, 内部 ID) 批量化到每个 shard 的连续数组，不按每条边进入 `HashMap`；`Both` 方向的去重继续保留，但用紧凑键集合替代当前三元组集合。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs` 继续消费同一父/偏移契约，不新增算子类型；`get_node_edge_headers_batch` 仍由 packed 结果转换而来，外部可见语义不变。

门槛：微基准 packed 读取 ≤1.5 ms/2048 父且分配 ≤4 MB/call；固定门 `expand_read_us` 至少下降 2 倍；`group_by_count` 与 `distinct_limit` 继续下降；空父、重边、悬空端点、TTL、staged write 对拍一致。

### 切片 C：目的端身份列免除实体构造

- 在存储 reader 契约中增加“只读身份列”的批量接口，返回与输入 ID 对齐的 `VertexId` 列和存在性位，而不是 `Vec<Option<Vertex>>`。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs` 的 `read_destinations` 在需求仅为 `id`/身份时使用该接口；整实体或多数属性需求继续走现有投影接口。
- 局部 TopK、全局 Distinct、ORDER BY 仍使用最终身份值；悬空端点仍按缺失处理，不因接口变化改变行数。

门槛：微基准目的端身份读取 ≤0.5 ms/2048；固定门 `group_by_count`/`distinct_limit` 有可测下降；DQL 中 NULL、重复 ID、并列值用例逐值一致。

### 切片 D：两跳计数度数路径

- 以 `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_frontier.rs` 与 `frontier_degree_batch` 为唯一改动面，确认两跳计数只使用度数、不构造邻居实体或外部 ID 列表。
- 若仍有外部 ID 往返，在存储层把第一跳端点保留为内部 ID，直接对第二跳度数求和；不复用切片 B 的通用拓扑 DTO。
- 门槛：`join_two_hop` ≤4 ms，`variable_path_count` 不退化；`count(k2)` 的 walk 语义（重边、自环、并行边）逐值一致。

### 切片 E：编译路径归因

只有 A–C 完成后才做。复用 `crates/linkrs-query/benches/query_stage_bench.rs` 的窗口，把 parse、bind、分区决策、arena 构建拆成独立计时点，不新建“替代版”基准套件。门槛是解释 2–3 ms 的来源；若无法归到单一热点，则不为此改生产路径。

## 6. 护栏与验证

- 本轮不新增生产开关，也不打开 `allow_pure_count_chain`、`allow_global_aggregate`、`allow_parallel_count_frontier`、`allow_partition_metadata_count`；`parallel.enabled` 继续为 `false`，只保留已放行的 `allow_ordered_parallel_expand`。
- 每个切片先在固定门数据集上通过 `result_stable=true`、`spill_rows=0`、`actual=10`、`actual_partitions=10`，再谈合并；生产默认单轮必须继续出现 `actual>1` 且九条纯计数无实质回归。
- 正确性门沿用上一轮：`linkrs-query` DQL 全量、`parallel_partition_execution`、partitioning/`typed_group_count`/`local_topk` lib 测试、`linkrs-storage` lib 全量。
- 微基准与固定门都在 devs 上执行并记录到 `perf-data/parity/results/2026-11-23/`；横向比较必须说明数据集和时间戳口径，不把合成微基准的绝对值当作 LSQB 结论。
- 不新增“version 2”模块，不保留旧路径作为长期兼容分支；策略切换失败时回退到当前已验证的规划选择，而不是保留双实现。

## 7. 非目标

- 不改近似语义、不采样、不截断结果。
- 不改 CSR/属性页的物理布局，不把属性内联进 CSR。
- 不在本轮处理阶段三微扫描下推、阶段四至七资源/事务/图算法深水项。
- 不因 `neighbor_dst_ids_batch` 在微基准中较慢而删除它：它服务于不同调用契约，删除需另立语义审计。

## 8. 落地结果与最终判定（2026-11-23）

最终采纳的改动只有切片 B、C、D：packed 拓扑改为事件流 + 计数散射 + 紧凑边类型 ID；目的端 flat 需求使用属性列/存在性批量读取；两跳度数读取去掉每父容器。切片 A 如上回退。

微基准（同一 devs 机器，绑核 16）：

| 路径 | 改前 | 改后 |
|---|---:|---:|
| packed 拓扑读取 | 3985 us / 15.5 MB | 3280 us / 8.68 MB |
| 目的端投影 `id` | 1361 us / 2.12 MB | 属性列 1090 us / 1.41 MB |
| 分片扫描 + 属性解码 | — | 735 us / 2000 行 |

固定门两轮中，`expand_read_us` 从约 160–210 ms 降到 110–135 ms，说明 B/C 的收益在端到端 Self-time 中可验证；`join_two_hop` 为 5.78 / 5.94 ms，`variable_path_count` 为 5.77 / 5.45 ms，均较上轮稳定改善。但 `group_by_count` 为 88.25 / 98.20 ms，`distinct_limit` 为 84.29 / 135.52 ms，生产默认单轮为 118.40 / 111.98 ms；受 `scan_read_us` 波动影响，端到端中位数没有稳定跨越 40 ms / 30 ms 中间门。

结论：拓扑与目的端的分配/物化问题已收敛，阶段二的计数前沿有可测收益；剩余主因回到扫描自身的可见性枚举与属性解码。按护栏不扩大放行面、不引入近似，下一轮应从扫描游标的条件枚举和列解码合同入手，并把 `scan_read_us` 的稳定性作为首要门槛；具体验证与修改方案见 `docs/plan/scan-visibility-and-decode-remediation.md`。完整日志与逐用例 JSON 见 `perf-data/parity/results/2026-11-23/stage1-stage2-remediation-final/`。
