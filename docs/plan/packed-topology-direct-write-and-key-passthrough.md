# packed 拓扑直写与身份键直通收敛方案

**日期**：2026-11-24
**状态**：设计文档，不含代码改动。
**基线**：`docs/plan/stage1-stage2-current-gap-remediation.md`，回测产物 `perf-data/parity/results/2026-11-24/group-by-local-topk/group-by-local-topk-r2/`。

---

## 1. 目标

当前固定门的两条阶段一用例为 `group_by_count` 39.59 / 33.89 ms、`distinct_limit` 37.95 / 32.80 ms；生产默认单轮为 31.86 / 29.51 ms。主要剩余成本不是局部聚合或 Exchange，而是每条拓扑事件都要经过父索引、目的端去重、属性列读取和行展开。固定门中 215488 个事件只对应 30473 个唯一目的端，放大约 7 倍。

本方案只在已证明身份的窄形状上把唯一目的端键直接送入局部聚合或局部 TopK，并在存储读取侧取消事件中转和散射。全局结果语义、并行放行面和精确 TopK 权威不变。

| 项目 | 本轮中间门 | 最终门 | 口径 |
|---|---:|---:|---|
| `group_by_count` | 两轮固定门均 ≤30 ms | ≤20 ms 且与 Ladybug 差距 ≤3 倍 | 固定门两轮 |
| `distinct_limit` | 两轮固定门均 ≤30 ms | ≤15 ms 且与 Ladybug 差距 ≤4 倍 | 固定门两轮 |
| `expand_read_us` | 两条阶段一用例两轮中位数较本切片前基线下降 ≥30% | 不再由拓扑事件展开主导 | 固定门 EXPLAIN |
| `join_two_hop` | ≤4 ms | ≤3 ms，结果逐值不变 | 固定门两轮 |
| `variable_path_count` | ≤6 ms | 不高于当前均值 | 固定门两轮 |
| 九条纯计数 | 无实质回归 | 无实质回归 | 固定门与生产默认 |
| 生产默认单轮 | 两条阶段一用例无退化，`actual=8` | 与固定门方向一致 | 不加 benchmark 并行参数 |

本方案不承诺一轮跨过最终门；最终门按中间门结果和三方对照逐步收敛。

---

## 2. 修改落点

### 2.1 拓扑扫描期直写

- `crates/linkrs-storage/src/engine/graph_storage/context/edge_ops.rs`：`out_topology_batch`、`in_topology_batch` 与 `flatten_topology_events`。把父编号事件的收集、计数、前缀和与散射合并为扫描期按父直写；`PackedTopologyBatch` 的 `entries` 和 `offsets` 契约不变。
- `crates/linkrs-storage/src/edge/edge_table/core/reads.rs`：`out_topology_with_gate`、`in_topology_with_gate` 及热邻居收集。提供调用方复用的父级缓冲，避免每个父点分配独立 `Vec<HotNbr>`，同时保持 CSR 访问顺序与可见性判定一致。
- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs`：`packed_node_edge_headers_batch`。消费直写后的父/孩子偏移，继续维护共享边类型名、rank、方向、外部 ID 与 `destination_handles`。

### 2.2 身份键直通

- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs`：在 packed 分支中，对已证明身份的键直接从未目的端属性列读取；不再构造逐边的 `parent_of_child`、`all_dst` 和逐边键数组。
- `crates/linkrs-query/src/executor/streaming/chunk/columnar_common.rs`：在现有 `PackedExpandPayload` 内携带每键计数或等价权重；不新增替代 payload 模块。非身份形状仍走现有 row/typed 路径。
- `crates/linkrs-query/src/executor/streaming/operators/blocking/aggregate_operator/typed_group_count.rs`：消费每键权重，把同一唯一键的边数一次累加到状态槽。
- `crates/linkrs-query/src/executor/streaming/operators/blocking/local_topk.rs`：既有 `LocalTopKRows` 与 `LocalTopKDistinct` 继续做精确局部截断；身份形状下输入行变为唯一键，NULL 键按分组语义处理。
- 计划期：在现有 Dedup/Aggregate 与投影类型传播位置增加身份证明。只有键是声明的主键或存储外部身份，且目的端属性需求只包含该身份列时才允许直通；不能按列名猜测，`local_topk:pushed:runtime` 不作为证明。

### 2.3 两跳度数直写

- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs` 的 `frontier_degree_batch`：在邻居访问回调中直接写父连续的 endpoint staging 并累计直接度数，去掉 `NeighborEvent` 与散射游标。
- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_frontier.rs`：保持 `1..2` 与固定深度二的度数汇总契约；TTL、开放 schema、`Trail`、多类型链、属性需求和空路径语义继续回退。
- 外部 ID 批解析、悬空邻居可见性和读集合记录保持原状；`allow_parallel_count_frontier` 继续关闭。

### 2.4 条件性 CSR 预取

- `crates/linkrs-storage/src/engine/graph_storage/context/edge_ops.rs` 与 `crates/linkrs-storage/src/edge/csr_variant/read.rs`：仅在拓扑读取仍是主导项时评估邻接 head 的预取和微批排序。
- 若排序父端点，必须用原槽位映射恢复父顺序与行顺序；不因微批改变 CSR 顺序、边身份或重复边语义。
- 不因父批微基准直接修改生产批大小；固定门证明前不新增配置开关。

---

## 3. 实施顺序

### 切片一：先把 packed 读取改成扫描期直写

先在同二进制上记录本切片前的 F1/F2 微基准与两轮固定门基线。随后按父直写 `entries` 和 `offsets`，去掉 event 中转与散射。只接受可测收益：拓扑读取时间相对基线下降至少 25%、分配不增加、结果逐值一致。若收益不足，保留原读取路径，不继续叠加身份直通。

### 切片二：在身份证明下直通唯一目的端键

补计划期身份证明，然后让 packed 展开在窄形状上输出唯一键与每键计数。先对 `group_by_count` 与 `distinct_limit` 逐值对拍，再跑固定门和生产默认。该切片通过后，`expand_output_rows` 在目标窄形状上应从每边一行变为每唯一键一行；`expand_unique_destinations` 与最终查询结果保持权威一致。若身份证明无法落地或结果漂移，停止该切片，不按列名回退到猜测式直通。

### 切片三：两跳度数读取去掉事件 staged

在阶段一切片稳定后再改 `frontier_degree_batch`。目标是保留度数快路径的同时减少一次事件收集与散射。以 `join_two_hop ≤4 ms`、`variable_path_count ≤6 ms` 和结果逐值一致为门槛；若只有微基准收益而固定门无收益，不保留该改动。

### 切片四：条件性预取与局部性验证

只有前三个切片后 `expand_topology_read_us` 仍是主导项才实施。用同一 2 万顶点数据集的 F1/F5 微基准确认预取收益至少 15%，固定门端到端收益至少 10%；否则回退，不保留只缩短代码路径的复杂度。

### 切片五：门槛复核与文档回写

全部采纳改动合并后跑完整固定门两轮、生产默认单轮、三方对照和正确性门。若最终门达成，更新配置参考中受影响说明；若未达成，写回问题文档的未达标原因和自耗时证据。本切片不改 `parallel.enabled` 与既有灰度开关。

---

## 4. 正确性矩阵

每个候选改动必须先在以下形状上与当前实现逐值对拍：

- 空分区、单父、多父、空孩子区间、高基数键、全同键、重复键、并列值；
- NULL 分组键、nullable 列、字符串键与整数键；
- 自环、平行边、悬空端点、缺失目的端属性；
- Out、In、Both 与多边类型；
- TTL 过期边、staged insert/update/delete 与已提交边混合；
- 快照早于或晚于提交边界；
- selection 与 multiplicity 合计大于一的输入；
- 分区并集与未切片全表结果逐值一致，行序和最终 TopN 顺序不变；
- DQL 全量、streaming lib、`parallel_partition_execution`、列批相关集成测试、storage lib 全量。

---

## 5. 验证口径

1. 固定门沿用当前基线口径：LSQB sf0.1、warmup 1 + run 5、绑核 16-31、`--parallel-workers 16 --max-partitions 10 --require-parallel-explain`。每轮必须满足 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`。
2. 生产默认单独跑一轮，不加 benchmark 并行参数，确认两条阶段一用例 `actual=8` 且九条纯计数无回归。
3. 三方对照沿用基准 JSON：整数逐值相等，浮点相对误差 1e-3；`distinct_limit` 使用修正排序后的 Ladybug 数值。
4. 微基准沿用现有拓扑读取套件，记录 F1（父批基数）、F2（方向/类型）、F3（唯一率）、F4（属性读取形状）和 F5（连续调用），同时记录时间与分配。
5. EXPLAIN / parity JSON 继续输出扫描、展开子计时与事件计数；新增每键计数后，输出行数与唯一键数的变化必须可解释。
6. 原始日志、逐用例 JSON、EXPLAIN、微基准与验证日志写入新的 `perf-data/parity/results/2026-11-24/` 子目录，不覆盖旧产物。

---

## 6. 护栏与不做项

- 不改变 MVCC 可见性、TTL、staged write、边去重、空路径和 NULL 语义；不跨调用缓存可见性或端点存在性。
- 不引入近似、采样、截断采样或不可证明的早停；全局 `Distinct` / `TopN` 保持最终权威。
- 不在无法证明身份时按列名推断主键或外部 ID；身份证明不成立则保持现有属性列读取。
- 不改变 CSR/属性页物理布局，不把属性内联进 CSR，不新增“version 2”模块。
- 不扩大并行放行面，不把 `allow_global_aggregate`、`allow_pure_count_chain`、`allow_parallel_count_frontier` 或总开关打开。
- 编译中位数 3.1–3.6 ms 是独立未归因项，不在本方案内并行推进。
- 每步只保留有固定门或微基准收益的改动；无收益即回退，并把原因写回问题文档。
