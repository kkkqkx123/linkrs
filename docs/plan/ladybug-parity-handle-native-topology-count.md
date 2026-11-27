# 面向 Ladybug 对齐的句柄直通拓扑计数改造

**日期**：2026-11-27  
**定位**：本文是 `group_by_count` 在达到当前扫描侧目标之后，继续向 Ladybug 8.16ms 收敛的下一阶段任务。问题定义沿用 `docs/issue/linkrs-remaining-parity-issues.md`；本文件只补充新的瓶颈证据、修改顺序与验收口径，不重新定义已完成事项。

## 1. 目标与差距

当前完成门是生产默认不超过 20ms 且对 Ladybug 倍率不超过 3×。这个门只说明不再显著落后，并不等于对齐。生产默认安静窗口最优约 22ms，高负载窗口约 32ms；Ladybug 同口径 8.16ms。即使按安静窗口计算，仍需约 3 倍工作量削减，不能靠分区微调、计数器优化或继续压缩扫描脚手架达成。

本阶段的目标是建立一条真正的对齐路径：把单跳 expand 从“为每条边构造完整外部身份拓扑”改为“按内部端点句柄读取并计数，外部身份只在唯一目的端上解析一次”。最终期望在无正确性退化、无采样近似的前提下，把 `group_by_count` 工作量压到约 60–80ms、安静窗口墙钟压到 8–12ms 区间，再评估是否可稳定进入 8ms。该数字是结构性估算，不是承诺。

## 2. 当前证据

devs 最后一轮生产默认 EXPLAIN 口径（node1 绑定，16 线程，8 workers / 8 partitions，warmup 1 + run 5 去最大值平均）：

| 指标 | 值 |
|---|---:|
| `group_by_count` | 32.221ms（负载窗口），安静窗口历史最优约 22ms |
| 父/边 | 215488 |
| 唯一目的端 | 30536 |
| expand 批数 | 112 |
| `scan_read_us` | 46300 |
| `scan_enumerate_us` | 26921（表加载 19064、归并 7857、窗口过滤 0） |
| `scan_visibility_us` | 15557 |
| `expand_topology_read_us` | 67297 |
| `expand_view_us` | 820 |
| `typed_group_hits` | 112/112，零回退 |
| `spill_rows` | 0 |
| 分区偏斜比 | 1.474（负载窗口），安静窗口通常约 1.1–1.3 |

工作量构成显示：扫描侧枚举与可见性合计约 42.6ms，拓扑读取约 67.3ms，二者占已计入存储工作量约 109.9ms。EXPLAIN 的 `parallel_work_us` 约 229ms，说明还有相当一部分时间不在存储自耗时计数器内，需在下述改造中补齐归因。

逐分区阶段计时显示：`ScanVertices` 的 `next` 累计约 9–19ms，`ExpandAll` 的 `next` 累计约 28–36ms；两者差值中存储拓扑读数每分区仅约 8ms，剩余约 9ms/分区未被现有计数器覆盖。该缺口位于 query 侧 packed 身份去重与结果组装，而非聚合或 Exchange。

## 3. 瓶颈判断

### 3.1 拓扑读取单价过高

devs `topology_read_bench` 同机数据（32768 边）：

| 形态 | 耗时 | 单价 |
|---|---:|---:|
| 连续父 packed topology | 3471us | 106ns/边 |
| 离散父 packed topology | 10211us | 311ns/边 |
| `out_degree (count only)` | 878us | 27ns/边 |

生产路径的 312ns/边与离散父 packed topology 完全一致，而只数边、不构造完整拓扑时仅需 27ns/边。两者约 11 倍差距来自：每条边的 `PackedEdgeTopology` 多数组写入、边类型与 rank 保留、逐边 `far_id` 哈希查表换取外部 `VertexId`，以及 query 侧再次按 32 字节 `VertexId` 做去重加权。

### 3.2 外部身份存在往返

扫描侧为每个父顶点构造外部 `VertexId`（约 72ns/父），expand 随后又把这些外部身份解析回内部 `(u32, LabelId)` handle 去读 CSR。当前已有 `TypedColumn::VertexHandle`，但 packed topology 仍依赖外部 `src/dst` 生成结果。对 `group_by_count` 这种只需要目的端主键与计数的查询，父端外部身份和每条边的外部端点都不是必需输出。

### 3.3 计数器盲区

`expand_view_us` 仅覆盖目的端存在性/权重视图组装，不覆盖 `expand_packed_identity` 中对全部边做 `HashMap<VertexId, usize>` 去重的前段，也不覆盖 seed 解析与 packed payload 组装。该盲区正是逐分区阶段计时里无法解释的约 9ms/分区。下一阶段必须先消除这段工作，而不是只增加计时。

### 3.4 不是主瓶颈的部分

- 聚合：typed 分组全命中、零回退、无 spill，全局 Exchange 与 TopN 仅约 0.1ms 量级。
- 编译：计划复用已命中，约 0.05ms。
- 分区偏斜：1.1–1.5，不是首要矛盾；在工作量未降之前增加分区不会改变数量级。
- 扫描脚手架：上一轮已把可复用中间向量收拢，继续压缩几纳秒无法支撑 3 倍目标。

## 4. 修改方案

### 阶段一：句柄级拓扑计数读取

新增一条面向分组计数的存储读取语义：给定一批父端点 handle、方向与边类型集合，遍历可见 CSR 行，按父端点聚合远端 `(label, internal_id)` 的计数，返回句柄级 packed payload。

契约要求：

- 读取仍使用现有读时间戳、pending gate 与边可见性判定，不改变快照语义。
- 保留边类型过滤、方向语义、源/目的标签校验和 staged write 的可见性处理。
- 输出不包含逐边 `src/dst` 外部身份、rank 与 edge type id；当一批内存在多类型、多标签或 `Both` 方向时，按类型分组或回退现有 packed topology 路径。
- 无悬空边证明关闭、TTL 激活、无法解析主键形状时回退现有路径。

预期收益：拓扑读取从 67ms 降到 15–25ms，单价从 312ns/边降到约 70–100ns/边。

### 阶段二：按内部句柄去重并批量解析唯一目的端

query 侧消费句柄级 payload，按 `(label, u32)` 整数句柄做去重与加权；随后对唯一句柄批量解析外部主键，构造现有 packed expand payload。

契约要求：

- 整数句柄去重必须与现有按 `VertexId` 去重逐值一致，边 multiplicity 不变。
- 主键解析失败、文本/整数键混用、TTL 与 staged write 条件不满足时回退现有路径。
- 复用已落地的无悬空证明与 TTL 守卫，不能因走快路径而跳过必要的存在性检查。
- 为原盲区补计数：句柄遍历、唯一键解析、权重组装分别计时，便于验证阶段一收益是否被 query 侧开销吃掉。

预期收益：消除约 9ms/分区的未归因开销，并让阶段一的存储收益完整反映到 EXPLAIN 与墙钟。

### 阶段三：扫描侧 identity-only handle 直通

当 planner 证明下游只消费 handle 时，存储扫描不再构造外部 `VertexId`，只输出 `(internal_id, label)` handle 列，并保持全局 id 有序与已有生命周期检查。expand 直接消费该列，不再做外部身份到内部 handle 的二次解析。

契约要求：

- 仅对身份直通形状启用；任何需要外部 id、属性行或语义排序的下游保持原路径。
- Int/Text 空格、删除、staged rows、TTL、limit/offset 与分区边界均需逐值一致。
- 扫描仍输出有序 handle；有序存在性、归并连接等消费者不受影响。

预期收益：`scan_visibility_us` 从 15.6ms 降到几毫秒，扫描总工作量从约 43ms 降到 10–15ms。

### 阶段四：并行参数复测

上述三项落地后重新测量分区偏斜与并行效率。不得在改造前通过增加分区或 worker 换收益；当前 8 个物理核下，更多分区只会放大尾部分区与调度开销。若工作量降到目标区间且偏斜仍高于 1.2，再评估按句柄分布而非行数/度数的切分策略。

## 5. 护栏与不做

- 不做采样、近似计数、截断或放宽 buffer 策略。
- 不引入跨调用可见性缓存，不在无证明时跳过 MVCC 或边存在性检查。
- 不改变扫描输出顺序契约，不改变聚合语义，不改拓扑语义。
- 不新增替代模块或版本化模块；所有改动收敛到现有 reader、expand 与 scan 模块。
- 不继续在上一轮扫描脚手架上做单价优化；该方向收益上限已不足。
- 边级“全可见”证明只允许在本方案三项落地后、且仍有证据表明逐边可见性主导时才评估；其语义面大于当前改动，不先行实施。

## 6. 验证口径

正确性门沿用既有全量门：storage lib、column batch、query lib、parallel、DQL、typed group、local topk、配置默认；结果必须保持 `group_by_count [14|59]`、`distinct_limit [14]`、`join_two_hop [382018]`、`variable_path_count [400153]`，`result_stable=true`、`spill_rows=0`。

新增专项门：

- 句柄去重与现有 `VertexId` 去重逐值一致，覆盖重复边、多类型回退、空批、Int/Text 主键、删除与 staged write。
- 句柄直通扫描与现有身份扫描在有序性、limit/offset、TTL、分区边界上逐值一致。
- `topology_read_bench` 新增或复用句柄计数形态，证明离散父场景下单价显著低于 packed topology。
- EXPLAIN 必须给出句柄遍历、唯一键解析、权重组装三段计数，避免再次出现无法归因的 query 侧开销。

性能门分两步：第一步以 workload 结构为准，要求 `expand_topology_read_us` 不高于 25ms、`scan_visibility_us` 不高于 5ms、`parallel_work_us` 不高于 80ms；第二步在安静窗口按生产默认口径复核墙钟，先进入 8–12ms，再判断是否具备稳定达到 8ms 的条件。高负载窗口的墙钟只作参考，不作为结论。

## 7. 交付顺序

按阶段一、二、三顺序实施，每阶段独立 A/B 并保留回退路径。阶段一先补反例与空输入用例；阶段二锁定 multiplicity 与回退语义；阶段三锁定有序 handle 契约。任一阶段结果不逐值一致或 workload 未下降，立即停止后续阶段，先补反例再继续。

## 8. 落地结果与剩余差距

### 8.1 已落地范围

阶段一至阶段三均已落地：

- 存储侧新增句柄级批量计数读取，直接遍历可见 CSR 邻接并按内部远端 handle 聚合成排序后的计数数组，不再构造逐边外部身份、rank 与类型 id。
- query 侧新增句柄计数快路径，按排序 handle 线性合并 multiplicity，再对唯一句柄批量解析外部主键并复用既有 dangling-free 守卫；不满足单类型、单方向、单远端标签、整数主键或无 TTL 条件时回退原 packed topology 路径。
- 物理计划在证明 packed identity handoff 且扫描直接下游就是该 expand 时，启用扫描侧 handle 直通；整数主键、无 TTL、无外部 id range 的 shard-slice 分区跳过外部主键解码。
- EXPLAIN 新增 `expand_handle_traverse_us`、`expand_key_resolve_us`、`expand_weight_us`；parity bench 同步输出这三项。

### 8.2 devs 复测结论

服务器二进制编译成功；完整 gate 退出码为 0。正确性门结果：

| 门 | 结果 |
|---|---:|
| config default | 1 passed |
| partitioning | 50 passed |
| typed group | 9 passed |
| local topk | 13 passed |
| degree sketch | 4 passed |
| query stats | 91 passed |
| parallel | 27 passed |
| DQL | 324 passed |
| storage lib | 1423 passed |
| dangling token | 4 passed |
| column batch | 13 passed |
| api lib | 30 passed |

两轮固定门与生产默认门结果保持一致：`group_by_count [14|59]`、`distinct_limit [14]`、`join_two_hop [382018]`、`variable_path_count [400153]`，`result_stable=true`。`group_by_count` 的 `typed_group_hits=112/112`、`typed_group_fallbacks=0`、`spill_rows=0`。

生产默认安静窗口下 `group_by_count` 中位数由改造前约 32ms 降至 **14.78ms**，去最大值均值约 14.72ms；关闭 dangling 证明的 A/B 为约 16.46ms。

`topology_read_bench` 离散父、32768 边口径：

| 形态 | 中位耗时 | 单价 |
|---|---:|---:|
| packed topology | 7886us | 241ns/边 |
| 句柄计数 | 1846us | 56ns/边 |

单价下降约 4.3 倍，证明结构性改造方向有效。

### 8.3 尚未达到的性能门

生产默认带证明的 `group_by_count` 指标：

| 指标 | 当前值 | 门 | 差距 |
|---|---:|---:|---:|
| `scan_visibility_us` | 754us | ≤5000us | 已达标 |
| `expand_topology_read_us` | 31868us | ≤25000us | 高约 6.9ms |
| `parallel_work_us` | 145862us | ≤80000us | 高约 65.9ms |
| 安静窗口墙钟 | 14.78ms | 8–12ms | 高于上沿约 2.8ms |

其余关键指标：`scan_read_us=27830`、`expand_key_resolve_us=9145`、`expand_handle_traverse_us=458`、`expand_weight_us=607`、`partition_skew_ratio=1.236`。从已计入的扫描与 expand 自耗时看，约 72ms 仍落在并行工作量但未落入现有存储计数器；继续只看拓扑读取单价无法解释这部分缺口。

因此本轮结论是：**正确性与结构改造已完成，工作量显著下降，但第一步 workload 结构和第二步墙钟门尚未全部通过**。不应把 14.78ms 直接解释为已具备稳定 8ms 的条件。

### 8.4 后续修改方向

按以下顺序推进，不跳步：

1. **先补齐并行工作量归因。** 在现有执行器内为扫描 batch 组装、packed payload 消费、聚合累计与 exchange 发送补充分区级计时，定位约 72ms 未归因工作量。这里的目标不是继续加存储计数器，而是确认 `parallel_work_us` 的主要组成；只有证据明确后才进入下一步。分区级与算子级归因结果见第 8.7、8.8 节。

2. **拆分拓扑读取的 31.9ms。** bench 单线程单价为 56ns/边，生产并行口径约 148ns/边，差值更像分区并发下的边表访问竞争与排序开销，而不是单次 CSR 遍历本身。可先在现有 reader 内比较“排序后 run-length 聚合”和“按边表/父 handle 分桶后局部聚合”，以实测决定保留哪种形态；不得因此恢复逐边外部身份构造。

3. **优化唯一键解析。** 当前 `expand_key_resolve_us` 约 9.1ms，且原 packed 路径约 30.8ms，说明这部分已被显著压缩但仍是第二瓶颈。方向是让整数主键在不构造 `VertexId` hash key 的前提下完成去重与 key 生成，并继续复用已有 handle 解析与存在性守卫；文本主键保持回退。

4. **复核扫描侧剩余成本。** `scan_visibility_us` 已达标，`scan_read_us` 仍约 27.8ms，其中枚举、表加载和窗口过滤均有一定占比。只有在前两步归因确认扫描主导后才重新评估；不继续做上一轮已证明收益上限不足的扫描脚手架微优化。

5. **最后才做并行参数复测。** 当前偏斜比约 1.236，略高于 1.2，但在 workload 未进入目标区间前不得通过增加分区或 worker 换收益。待拓扑读取、键解析与未归因工作量下降后，再评估按句柄分布而非行数/度数切分，并用同源资源口径复核墙钟。

### 8.5 继续遵守的护栏

- 不采样、不近似、不截断，不放宽 buffer 或 multiplicity 语义。
- 不引入跨调用可见性缓存；任何快路径必须保留读时间戳、pending gate、staged write、TTL 与 dangling-free 守卫。
- 不恢复逐边外部身份构造；任何新形态都必须保持与 `group_by_count [14|59]` 等结果门逐值一致。
- 边级“全可见”证明仍只在上述方向落地且仍有逐边可见性主导证据时评估，不先行实施。

### 8.6 复测材料位置

devs 上本轮最终材料：

- 服务器编译日志：`/data/kkkqkx/linkrs/build-handle-count-server4.log`
- 完整 gate 日志：`/data/kkkqkx/linkrs/gate-handle-count4.log`
- 诊断与 EXPLAIN：`/data/kkkqkx/linkrs/handle-count-gate4/`
- 最终 bench 日志：`/data/kkkqkx/linkrs/topology-read-handle-final.log`

### 8.7 并行工作量归因复测

按第 8.4 节第一步，先在 worker 分区执行入口补齐 `partition_open_us`、`partition_advance_us`、`partition_send_us`、`partition_close_us` 四项计时，并接入 EXPLAIN 与 parity JSON。随后在 devs 上重新编译、跑完整 gate，并单独执行三轮生产默认口径复测。完整 gate 全部通过，结果门未变化；计时字段本身没有引入可见语义变化。

完整 gate 运行期间 devs 上有其他编译任务，生产默认轮次出现明显外部噪声（`scan_read_us` 达 92818us，`group_by_count` 中位数 41.83ms），该轮只用于确认正确性与字段输出，不作为性能结论。随后在同一节点、同一 CPU 绑定下连跑三轮生产默认测量：

| 轮次 | `group_by_count` 中位数 | `parallel_work_us` | `partition_open_us` | `partition_advance_us` | `partition_send_us` | `partition_close_us` | 偏斜比 |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | 14.73ms | 148250 | 164 | 147810 | 17 | 225 | 1.497 |
| 2 | 14.68ms | 144187 | 177 | 143752 | 20 | 208 | 1.373 |
| 3 | 21.22ms | 175309 | 393 | 174623 | 32 | 221 | 1.737 |

第 1、2 轮与上一节稳定基线一致；第 3 轮同时出现 `scan_read_us` 从约 28ms 抬升到 33ms、偏斜比从约 1.4 抬升到 1.737，判定为外部负载噪声轮，不用于归因结论。

稳定轮次中 `partition_advance_us` 占 `parallel_work_us` 约 99.7%，`partition_send_us` 仅 17–20us，`partition_open_us` 与 `partition_close_us` 合计不足 0.4ms。由此得到两个明确结论：

- `parallel_work_us` 的缺口不在 exchange 发送阻塞，也不在分区打开/关闭；继续从 bounded channel 或 worker 调度方向找收益缺乏证据。
- 未归因部分位于分区 `advance()` 内部。已计入的存储自耗时约 69ms，而 `parallel_work_us` 约 144ms，剩余约 75ms 应来自 operator 内部的 batch 组装、packed payload 消费、聚合累计或逐 advance 的运行时检查，而不是存储层拓扑读取本身。

因此第 8.4 节第一步只完成了一半：分区级归因已排除 exchange，下一步应把 `advance()` 内部按 operator 责任拆开，再决定是否进入拓扑读取 A/B；不应直接按原先“72ms 主要来自存储未计数路径”的假设改存储实现。

本轮材料：

- 服务器编译日志：`/data/kkkqkx/linkrs/build-handle-count-server5.log`
- 完整 gate 日志：`/data/kkkqkx/linkrs/gate-handle-count5.log`
- 三轮生产复测日志：`/data/kkkqkx/linkrs/attrib-prod-{1,2,3}.log`
- 三轮诊断与 EXPLAIN：`/data/kkkqkx/linkrs/attrib-prod-{1,2,3}/`

### 8.8 advance 内部归因与瓶颈确认

按第 8.7 节结论继续拆分 `advance()` 内部责任。在现有统计模块中补入五个计时字段并接入 EXPLAIN 与 parity JSON：`scan_chunk_build_us`（存储列批到 DataChunk 的查询侧组装）、`expand_seed_us`（`parse_seeds` 与种子 handle 收集）、`expand_typed_clone_us`（expand 入口克隆输入 typed 列）、`expand_guard_us`（拓扑 schema、TTL 与 dangling 证明检查）、`expand_step_us`（一次 expand 分发的完整耗时）。计时只出现在既有查询侧代码路径上，不改变任何可见性、顺序或输出语义。

验证采用两层对账：EXPLAIN 的逐算子 `next_time_us` 沿依赖链取差得到算子独占时间；新字段用于解释 scan 与 expand 独占时间的内部构成。devs 生产默认、node1 绑定、warmup 1 + run 5 的稳定轮次（中位数 14.77ms，`work_time=140347us`，偏斜比 1.190）：

| 层次 | 累计耗时 | 占 `parallel_work_us` |
|---|---:|---:|
| 分区 advance | 139911us | 99.7% |
| 算子独占时间之和 | 140592us | 100.2%（含舍入） |
| scan 独占 | 50390us | 35.9% |
| expand 独占 | 76966us | 54.8% |
| PartialAggregate 独占 | 11217us | 8.0% |
| Project / LocalTopK / Exchange / 全局尾部 | 2019us | 1.4% |

expand 独占时间的内部构成：

| 阶段 | 耗时 | 说明 |
|---|---:|---|
| `expand_step_us` | 69928us | expand 分发内部 |
| `expand_topology_read_us` | 30810us | 句柄级 CSR 计数读取 |
| `expand_seed_us` | 18216us | 种子解析与逐行 row 克隆 |
| `expand_key_resolve_us` | 8666us | 唯一句柄解析与主键生成 |
| `expand_typed_clone_us` | 2150us | 入口 typed 列克隆 |
| `expand_guard_us` | 1001us | schema / TTL / dangling 证明 |
| `expand_handle_traverse_us` + `expand_weight_us` | 998us | 句柄合并与权重组装 |
| 分发内残余 | 8087us | step 内未单独计时的输出构造与循环开销 |
| 分发外残余 | 7038us | expand operator 的逐批次调度与字段准备 |

scan 侧对账更直接：`scan_read_us=27446us` 与 `scan_chunk_build_us=22688us` 合计 50134us，已解释其 50390us 独占时间的 99.5%。因此第 8.4 节遗留的约 72ms 未归因工作量不再是未知项：它主要由查询侧 chunk 组装、种子抽取、PartialAggregate 和 expand step 残余构成，而不是存储层隐藏路径。

由此确认的瓶颈优先级：

- 拓扑读取仍是最大单项（30810us，215488 条边约 143ns/边；单线程 bench 为 56ns/边）。并发访问与排序开销仍是主要怀疑点，但仍需先在现有 reader 内做排序聚合与分桶聚合的 A/B，不直接改语义面更大的边级可见性证明。
- 新确认的两处查询侧主要浪费：scan 的列批到 chunk 组装 22688us，expand 的种子抽取 18216us。两者都是为当前句柄直通形状构造无人消费的 row view / seed row，合计约占 29% 并行工作量，且不需要触碰拓扑或可见性语义。
- PartialAggregate 独占 11217us，是第一轮改造后新浮现的第三梯队成本；unique key 解析 8666us 紧随其后。两者只有在前两项下降后才值得继续压缩。
- typed 列克隆 2150us 与各类 guard 合计 1001us 均不足工作量的 2%，不应作为下一步优化目标。

正确性复核：查询 lib 2364 项、parallel 27 项、DQL 324 项全部通过；三轮生产默认结果门稳定为 `group_by_count [14|59]`、`distinct_limit [14]`、`join_two_hop [382018]`、`variable_path_count [400153]`，`result_stable=true`、`spill_rows=0`，`typed_group_hits=112/112`、零回退。性能门状态未变：`expand_topology_read_us` 30810us 仍高于 25000us，`parallel_work_us` 140347us 仍高于 80000us，安静窗口墙钟 14.77ms 仍高于 12ms。

后续修改顺序相应调整为：先消除句柄直通形状中无人消费的 row view 与 seed row 克隆并对逐值结果门复测；再在 reader 内做拓扑读取 A/B；然后评估唯一键解析与 PartialAggregate；最后才做并行参数复测。新增计时字段继续保留，用于确认每一步是否把对应工作量真正移出关键路径。

本轮材料：

- 服务器编译日志：`/data/kkkqkx/linkrs/build-attrib-deep4.log`
- 测试日志：`/data/kkkqkx/linkrs/attrib-deep4/gate-tests.log`
- 三轮生产复测日志与 JSON：`/data/kkkqkx/linkrs/attrib-deep4/r{1,2,3}.log`、`/data/kkkqkx/linkrs/attrib-deep4/r{1,2,3}/`
- 三轮诊断与 EXPLAIN：`/data/kkkqkx/linkrs/attrib-deep4/explain{1,2,3}/`

### 8.9 消除无效 row 构造与拓扑 A/B 结果

按第 8.8 节确认的优先级，先实施了不触碰可见性与拓扑语义的两项改造：

- 物理计划构建器在 packed identity handoff 证明成立且扫描直接下游就是该 expand 时，将扫描的 `skip_rows` 一并开启。句柄直通扫描从此不再构造任何 row view，只输出 typed handle 列。
- `parse_seeds` 增加是否需要物化 seed row 的契约；列式 expand 在 typed 列完整覆盖输入布局时跳过逐行 row 克隆，只保留 seed id 与绝对位置。frontier 路径仍要求完整 seed row，保持原语义。新增专项单测确认两种形态的 id 与位置逐值一致。

devs 生产默认稳定轮次对比（同为 node1 绑定、warmup 1 + run 5）：

| 指标 | 改造前（8.8 节） | 改造后 | 变化 |
|---|---:|---:|---:|
| `group_by_count` 中位数 | 14.77ms | 9.94ms（另轮 9.70ms） | 约 1.5 倍 |
| `parallel_work_us` | 140347 | 103965 | -25.9% |
| `scan_chunk_build_us` | 22688 | 5230 | -77% |
| `expand_seed_us` | 18216 | 3354 | -82% |
| `expand_topology_read_us` | 30810 | 34098 | 未改善（噪声区间内） |
| `expand_key_resolve_us` | 8666 | 10230 | 未改善 |

同一形状下 `distinct_limit` 中位数也由约 20ms 降到约 12ms，结果仍为 `[14]`。三轮结果门稳定为 `group_by_count [14|59]`、`distinct_limit [14]`、`join_two_hop [382018]`、`variable_path_count [400153]`，`result_stable=true`、`spill_rows=0`、`typed_group_hits=112/112`、零回退。正确性复核：storage lib 1423 项、query lib 2365 项、parallel 27 项、DQL 324 项全部通过。

针对剩余最大的拓扑读取项，按第 8.4 节第二步做了两项 A/B，均未达标并已回退：

- 把句柄直通扫描的 batch 放大 4 倍（2048 → 8192）：批数从 112 降到 32，但 `expand_topology_read_us` 从约 34ms 升到约 42–47ms，`parallel_work_us` 升到 108–113ms；墙钟没有稳定收益。说明拓扑成本不是每批固定开销主导，而是随批量排序/聚合规模增长。
- 把 reader 内的“排序 + run-length 聚合”改为“逐边哈希分桶后只排序唯一端点”：`expand_topology_read_us` 升至 45–47ms，`parallel_work_us` 升至 122–128ms。排序当前形态优于哈希分桶；已恢复排序实现。

两项 A/B 都保持结果门不变，仅因工作量上升而回退，没有引入任何新模块或替代路径。

当前性能门状态：安静窗口墙钟已进入 8–12ms（9.70–9.94ms），但外部负载波动下仍会观察到约 14.9ms；`expand_topology_read_us` 约 34–38ms 仍高于 25ms，`parallel_work_us` 约 104–115ms 仍高于 80ms。因此第一步 workload 结构门仍未通过，不把偶发的 9.7ms 解释为已稳定达到 8ms。

下一步应以“逐边可见性在 8 路并发下主导”为工作假设补证据，只有在证据明确后才评估边级全可见证明；在此之前不恢复已回退的批量放大与哈希分桶，不增加分区或 worker。唯一键解析与 PartialAggregate 各约 10ms，属于第二梯队，也不应先于拓扑归因改动。

本轮材料：

- 服务器编译日志：`/data/kkkqkx/linkrs/build-final-no-row.log`
- 测试日志：`/data/kkkqkx/linkrs/bucket-handoff/gate-tests.log`
- 最终三轮生产复测：`/data/kkkqkx/linkrs/final-no-row/r{1,2,3}.log`、`/data/kkkqkx/linkrs/final-no-row/r{1,2,3}/`
- 回退 A/B 数据：`/data/kkkqkx/linkrs/wide-handoff/`、`/data/kkkqkx/linkrs/bucket-handoff/`

### 8.10 拓扑读取的三段归因与最终结论

为判定第 8.9 节留下的工作假设，在 `PackedHandleCounts` 中补入三段只读计时：`setup_us`（快照、schema、读时间戳、handle 解析与边表/gate 准备）、`traverse_us`（可见 CSR 逐边遍历）与 `aggregate_us`（排序与 run-length 聚合），并接入 EXPLAIN、parity JSON 与 `topology_read_bench` 的单线程拆分输出。这些字段不改变任何语义。

单线程 `topology_read_bench`（32768 边、batch=2048）：总耗时 1004us，`setup < 1us`、`visit 535us`、`aggregate 468us`；同机 `out_degree (count only)` 为 491us。可见即使包含逐边 MVCC gate，单次 CSR walk 也没有数量级异常。

生产默认稳定轮次（中位数 10.70ms，`work_time=96165us`，偏斜比 1.438）：

| 阶段 | 耗时 | 占拓扑读取 |
|---|---:|---:|
| `expand_topology_read_us` | 31955us | 100% |
| `expand_handle_setup_us` | 11929us | 37.3% |
| `expand_handle_visit_us` | 14634us | 45.8% |
| `expand_handle_aggregate_us` | 5247us | 16.4% |

三段之和 31810us，解释拓扑读取的 99.5%。另外两轮（`work_time` 110679us 与 128469us）保持相同比例。

由此得到明确结论：

- “逐边可见性主导剩余拓扑读取”的假设不成立。逐边 CSR 遍历只占约 46%，且单线程口径与裸 degree 读取相差不大；因此没有依据实施边级“全可见”跳过证明，继续沿用现有 MVCC 与 pending gate。
- 真正的并发新增开销在 `setup`：单线程不足 1us，生产 8 分区下累计到约 12ms，来自每批次重复的 schema、读时间戳、边表与 gate 准备在共享结构上的竞争。它解释了为何放大 batch 会让聚合变大却没有收益，也解释了哈希分桶为何更慢。
- 排序聚合本身约 5ms，已优于哈希分桶；不要恢复已回退的实现。
- 结构改造至此已完成：句柄直通、无 row view、无 seed row 克隆、三段归因全部落地；正确性与结果门全部通过。剩余性能门不是由采样、近似或可见性保守性造成，而是由并发下的每批 setup 竞争和仍需保留的逐边遍历造成。

最终口径：安静窗口墙钟 9.79–10.70ms，已进入 8–12ms 区间；但 `expand_topology_read_us` 约 32ms、`parallel_work_us` 约 96–128ms 均未达到 25ms / 80ms 的工作门，且墙钟随负载偏斜波动到约 14.6ms。因此**不能宣称达到稳定 8ms，也不能宣称工作量门通过**。在不引入跨调用可见性缓存、不跳过 MVCC、不增加分区/worker 的护栏下，没有低风险改动可以继续压缩这两项；下一步若要推进，应先做“每个分区复用固定快照下的 schema/边表准备”的独立设计验证，而不是继续做可见性证明或聚合形态微调。

最终材料：

- 编译日志：`/data/kkkqkx/linkrs/build-setup-proof.log`
- 单线程拆分 bench：`/data/kkkqkx/linkrs/setup-proof/topology-split.log`
- 测试日志：`/data/kkkqkx/linkrs/setup-proof/gate-tests.log`
- 三轮生产复测：`/data/kkkqkx/linkrs/setup-proof/r{1,2,3}.log`、`/data/kkkqkx/linkrs/setup-proof/r{1,2,3}/`
