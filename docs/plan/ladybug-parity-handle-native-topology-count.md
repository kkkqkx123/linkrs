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
