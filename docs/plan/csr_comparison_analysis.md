# CSR 实现对比分析：当前项目 vs ref/neug

本文对比当前项目 `crates/graphdb-storage/src/edge/` 的 CSR 实现与 `ref/neug`（`include/neug/storages/csr/`、`src/storages/csr/`）的实现，指出当前实现的功能设计问题与性能问题，并给出改进方向。

现状依据：`docs/storage/research/csr_comparison.md` 只对比了“指针数组 vs 偏移数组”这一个访问细节，结论是“当前实现合理”。该结论已过时：当前 `MutableCsr` 早已不是简单的 `adj_offsets + nbr_list`，而是 primary + overflow + live-set + group 分片的多层结构；neug 一侧也有 `GenericView`、`unsorted_since`、`batch_sort`、`EdgeDataAccessor` 等当时未被纳入对比的关键设计。本文重新做完整对比。

## 1. 双方实现概览

### 1.1 neug 的 CSR 全家桶

- 五种 CSR 共享 `CsrBase` / `TypedCsrBase<EDATA_T>` 接口（`ref/neug/include/neug/storages/csr/csr_base.h`）：`MutableCsr`、`SingleMutableCsr`、`ImmutableCsr`、`SingleImmutableCsr`、`EmptyCsr`，全部是 `EDATA_T` 模板，边属性内联存储（`MutableNbr<EDATA_T>` / `ImmutableNbr<EDATA_T>`，见 `nbr.h`）。
- 可变 CSR 布局（`mutable_csr.h`、`src/storages/csr/mutable_csr.cc`）：每个顶点独立分配邻接块，`adj_list_buffer_` 是 `nbr_t*` 指针数组（每顶点 8 字节），另有 `degree_list_` / `cap_list_`（每顶点各 4 字节 int），`nbr_list_` 只在 open / `batch_put_edges` 全表重建时作为连续后备。单次 `put_edge` 按 1.5 倍扩容单个顶点的块（`memcpy` 搬运该顶点旧块），持有该顶点的 `SpinLock`。
- 不可变 CSR（`immutable_csr.cc`）：`adj_list_buffer_` 指针数组 + `degree` 数组指向连续 `nbr_list_buffer_`，无 cap 数组；`batch_put_edges` 从后向前 `memmove` 整体扩容后追加。
- 边记录极小：`vid_t` / `timestamp_t` 均为 `u32`（`ref/neug/include/neug/utils/property/types.h`）。`MutableNbr<EmptyType>` 为 neighbor + atomic timestamp，共 8 字节；`ImmutableNbr<EmptyType>` 用 union 压缩到 4 字节。删除语义统一为墓碑：可变 CSR 置 timestamp 为 `INVALID_TIMESTAMP`（`0xFFFFFFFF`），不可变 CSR 置 neighbor 为 `vid_t::max`，`compact()` 统一回收。
- 零拷贝读视图 `GenericView`（`generic_view.h`）：只包装 `adjlists + degrees + NbrIterConfig{stride, ts_offset, data_offset} + timestamp + unsorted_since`，`get_edges(v)` 返回起止指针，`NbrIterator`（POD、`always_inline`）按行过滤 `timestamp <= read_ts`。另有 `TypedView::foreach_nbr_gt/lt`：已排序前缀二分 + 未排序后缀线性，利用 `unsorted_since` 跳过无用比较。
- 属性双形态 `EdgeDataAccessor`：bundled（数据内联在 `Nbr` 里）与 column（`Nbr` 里只存 `size_t` 列下标）共用同一读接口；`batch_sort_by_edge_data` 维护排序态并更新 `unsorted_since`；`batch_export` 可整体导出。
- 持久化：`.nbr / .deg / .cap / .meta` 四类文件经 `IDataContainer` 抽象（`kSyncToFile / kInMemory / kHugePagePreferred` 三种内存级别），dump 带 MD5 校验头。并发：顶点级 `SpinLock` 写互斥 + timestamp / edge_num 原子量，读视图无锁快照。

### 1.2 当前项目的 CSR 全家桶

- `MutableCsr`（`edge/mutable_csr.rs` + `mutable_csr/` 下 12 个子模块）：主块 `hot_list: Vec<HotNbr>` + `cold_list: Vec<ColdStamps>` 双向量 + 每顶点 `adj_offsets / degrees / primary_capacities`（各 `u32`，每顶点 12 字节），零度顶点延迟分配（首边才分配 `DEFAULT_VERTEX_DEGREE = 4` 槽）。溢出为 `OverflowStorage`：按顶点分段稀疏索引（`SegmentedTable`，1024 顶点一段）+ 每顶点 `Vec<OverflowChunk>` 链 + presence 位图；宽行（live > 8）另带 `LiveSetStorage`：`HashMap<(endpoint, rank), EdgePosition>`。
- 边记录大：`HotNbr{endpoint: u32, rank: i64, edge_id: u64}` 约 24 字节 + `ColdStamps{create_ts: u64, delete_ts: u64}` 16 字节 = **约 40 字节/边**（`edge.rs`）。属性不存于 CSR，经 `EdgeId` 存于外部列存；`PureTopologyCsr`（`pure_csr.rs`，约 1900 行）、`BundledCsr`（`bundled_csr.rs`，约 1200 行）、`SingleMutableCsr`（约 1100 行）、`ImmutableCsr`（`immutable_csr.rs`，约 1200 行，冻结只读快照）各自重复实现溢出与索引逻辑，上层再经 `CsrVariant` 枚举分发与 `CsrShardSet`（`node_group/`）按 `group_bits` 做 group 分片、BTreeMap + route cache 路由。
- MVCC 双权威：行内 `create_ts / delete_ts` 只是“物理副本”，注释明确要求查询经 `MVCCManager.edge_timestamps` 判定可见性（`edge.rs`），行内 `is_alive_at` 仅测试与离线使用。删除有三套模型并存：MVCC 墓碑戳（`delete_edge` 系列）、物理擦除（`remove_edge`，`memmove` 闭 gap，仅回滚用）、热路径墓碑复用（`tombstone_reuse_cutoff` + 64 槽有界扫描）。
- 读路径：`edges_of`（分配 Vec，仅测试/离线）、`fill_physical_into`（调用方复用 buffer）、`visit_physical / visit_hot`（`FnMut` 闭包访问器）；冻结组按 `(endpoint, rank, create_ts, edge_id)` 全排序并支持 key range 二分。持久化：各 variant 自研 `dump/load` 字节格式 + group/region 脏跟踪 + append log；冻结 serving 文件走 `memmap2`（`frozen_serving.rs`，明确无 checksum）。

## 2. 功能设计问题

### 2.1 缺少统一的零拷贝读视图，查询层被迫拷贝或走闭包

neug 的 `GenericView + NbrIterator` 是跨层契约：存储层只交出指针与布局描述，执行层（`edge_expand`、`EdgeDataAccessor`）直接遍历，无分配、无虚调用。当前项目没有对等物：生产遍历走 `visit_physical` 闭包或 `fill_physical_into` 中间 buffer。闭包访问器无法跨过存储/查询边界做内联与向量化，且每种 variant 都要重复实现一套 visitor（`CsrVariant::visit_physical_with_values` 等分发层见 `csr_variant.rs`）。`docs/storage/research/csr_comparison.md` 建议的 `get_neighbors() -> &[Nbr]` 从未落地，且即便落地也只覆盖单 variant。应补一个与 variant 无关的轻量行视图（起止 slice + stamp slice + 可见性谓词），把遍历契约固定下来，而不是继续在每个 variant 上加 visitor 方法。

### 2.2 时间戳双权威：行内副本与版本权威并存

neug 每条边只有一个时间戳，读写都认它。当前行内 `create_ts / delete_ts` 与 `MVCCManager` 各存一份，注释用“调用方必须走权威”来约束，而不是用类型系统消除误用。后果有三：每边 16 字节副本开销；两处状态可能漂移（行内已删、权威未删或反之）；`get_edge(ts)`、`edges_of(ts)` 等行内时间戳过滤方法与生产规则互相矛盾，测试走一套语义、生产走另一套。应二选一：要么行内只存拓扑、可见性全部上交（则删除这些行内过滤 API，`ColdStamps` 缩为单个墓碑位或版本号）；要么行内时间戳即权威（则砍掉存储层之外的重复存根）。现状是两套都留着、用注释划界，这是典型的拿文档弥补设计问题。

### 2.3 三套删除/回收模型叠加，心智负担与误用风险高

neug 的删除就是“打墓碑 + `compact` 回收”，`delete_edge / revert_delete_edge` 语义对称。当前 `MutableCsr` 同时存在：墓碑戳（`delete_ts`，带跨时间戳写写冲突）、物理擦除（`remove_edge`，`copy_within` 闭 gap 并重建 live set）、水位驱动的热路径墓碑复用（`tombstone_reuse_cutoff`，哨兵关闭）。三者对 `edge_count`、`live_set`、capacity ledger 的维护各写一遍，且 `remove_edge` 会使已发放的 `EdgePosition` 失效（靠重建 live set 补救）。`delete_edge_by_offset` 对已墓碑槽直接返回 `Ok(false)` 而不走冲突状态机，与 `delete_edge` 的冲突语义不一致（`write.rs`）。应收敛为“墓碑 + 后台回收”单一模型，回滚也走 `revert` 而不是物理擦除；热路径复用若保留，必须与回收裁决共用同一谓词（目前 `is_reclaimable_cold` 已共享，这是对的）并补齐不变量测试。

### 2.4 Variant 爆炸：同一份溢出/索引逻辑复制三遍

neug 用 `EDATA_T` 模板参数表达“有无属性、属性类型”，溢出、删除、排序逻辑只写一次。当前 `PureTopologyCsr`、`BundledCsr`、`MutableCsr` 各自实现 `OverflowStorage`、`LiveKeySet`、`SegmentedTable` 路由（`pure_csr.rs` / `bundled_csr.rs` / `mutable_csr/`），`CsrVariant` 再包一层枚举分发（`csr_variant.rs` 约 1300 行）。任何溢出语义的修改要在三处同步，必然漂移。`RecordForm::Pure / Bundled / Columnar` 本是正交维度（属性存哪），却被做成了三个完整 CSR 实现。应在原模块内收敛：把溢出块与分段索引抽成 `mutable_csr/overflow.rs` 唯一的泛型实现，`Pure / Bundled` 只保留行编解码差异；或退一步让三者共用同一 chunk 与 table 类型，而不是各自定义一份。

### 2.5 冻结 CSR 只读且必须整体解冻，缺少在线压实形态

neug 的 `ImmutableCsr` 仍支持 `batch_put / batch_delete / compact / resize`（`immutable_csr.cc`），是可写的紧凑形态。当前的 `ImmutableCsr` 是冻结快照：任何写入直接报错，要求先整体 unfreeze 回可变 variant（`immutable_csr.rs`）。于是在服务层做一次小批量删除或回收，就要经历“解冻全组 → 修改 → 重冻全组”三次全量拷贝。`frozen_serving.rs` 的 mmap 服务文件与此正交。应给冻结形态补上 neug 式的原地 `compact`（已排序行的墓碑回收不需要解冻）与增量 `batch_put`（尾部追加段），或明确宣布冻结组永不接受写并把写路径在类型层面彻底封死（现在 trait 上仍挂着写方法，运行时才报错）。

### 2.6 缺少排序态跟踪，范围查询优化无从谈起

neug 用 `unsorted_since` 记录“自该时间戳后行内可能无序”，`TypedView::foreach_nbr_lt` 对有序前缀二分、仅对后缀线性扫描；`batch_sort_by_edge_data` 是显式维护手段。当前可变行是纯追加序（溢出链更无序），冻结行是全排序，两者之间没有任何中间态：可变行的范围/排序查询只能全行扫描。这不是小优化，而是 neug 查询执行器依赖的不变量。若查询层未来需要 `ORDER BY / range` 下推，当前存储层给不出任何有序性保证。应在可变行引入排序态标记（至少记录每行是否有序），批量导入路径提供排序写入选项；否则所有排序永远发生在查询层，数据量稍大即退化。

### 2.7 并发粒度倒退：顶点级锁丢失

neug `put_edge` 持该顶点 `SpinLock`，不同顶点可并发写。当前 `MutableCsr` 自身无任何锁（`grep` 可见 `edge/` 下无锁原语），并发控制上移到 table/事务层的大粒度锁。单线程下这减少了开销，但多写线程下吞吐上限就是 table 锁。这是从参考实现继承时丢失的能力。若实测多线程导入是瓶颈，应恢复顶点级锁（锁表与 CSR 同寿命、`resize` 时重建，与 neug 做法一致）；若坚持无锁，也应把“`MutableCsr` 非线程安全、调用方负责互斥”写进类型文档而不是默认 `Send + Sync` 透传。

### 2.8 持久化缺少完整性校验与大页支持

neug 的容器抽象统一处理 `SyncToFile / InMemory / HugePage`，dump 自带 MD5。当前各 variant 自研格式，`frozen_serving.rs` 明确声明无 checksum，mmap 载入只校验 magic 与结构尺寸。这意味着静默数据损坏可直接进入查询结果。至少应对持久化载荷加 CRC（`index` 模块已有 `compute_checksum / verify_checksum` 先例），mmap 场景评估 huge page；不要为每个 variant 各写一套校验，放在 `persistence` 编解码层统一做。

## 3. 性能问题

### 3.1 单边 40 字节 vs 4～8 字节：内存与缓存的根本差距

以无属性多边为例：neug 可变 8 字节/边、不可变 4 字节/边；当前 `HotNbr(24) + ColdStamps(16) = 40` 字节/边，且 reserved gap 也按此尺寸预留（`dead_gap` 填充）。同样 1 亿条边，neug 不可变约 0.4GB，当前约 4GB，还没算溢出链与 live set。遍历时 neug 一条 cache line（64B）装 8～16 条边，当前一条 line 装不满 2 条。`rank: i64` 与 `edge_id: u64` 是大头：`rank` 绝大多数场景恒为 0（`Pure` variant 已证明可省），`create_ts` 作为“调试副本”常驻行内（见 2.2）。改进按收益排序：`rank` 收窄或仅在多重边组存储；删除 `create_ts` 行内副本（若权威上移）或将其移出热行；`endpoint + edge_id` 紧凑排列保证顺序扫描的空间局部性。注意 `.cargo/config.toml` 已开 `x86-64-v3`，向量化友好布局（SoA 紧凑数组）才能吃到 AVX2 红利，现在 40 字节跨步的结构恰恰不利于自动向量化。

### 3.2 热/冷分离的双流扫描成本

热冷分离的初衷（拓扑扫描不污染 stamp 缓存行）是对的，但当前实现让每次“组装 `Nbr`”都要同时触达两个流（`slot_at`、`Nbr::from_parts` 处处调用），点查与全量遍历都付双倍缺失。neug 的 `Nbr` 是单结构体，一次缺失拿全字段；其 `EdgeDataAccessor` 的列存形态才把不常用数据分离。当前把“极少用的 `create_ts`”和“常用的 `delete_ts`”绑在同一个 cold 流里，导致任何存活判断都要加载 16 字节。建议：热行只留 `(endpoint, rank?, edge_id)` 紧凑数组；存活信息压缩为位图或单 `delete_ts` 稀疏表；`create_ts` 移入独立的版本侧表，按需查。

### 3.3 溢出链的指针跳跃：最坏 O(链长) 间接访问

neug 读是 `adj[src][i]` 两次解引用（且 block 内连续）。当前溢出行的遍历是：presence 位图 → `SegmentedTable`（段指针 + 槽指针）→ `Vec<OverflowChunk>` → 逐 chunk `hot_slice / cold_slice`。`single_chunk` 快径只覆盖单 chunk 行；一旦超过 `OVERFLOW_REPACK_CHUNKS_PER_VERTEX = 8` 触发 repack，rep ack 本身是全行拷贝（`compact_overflow_for_vertex` 经 `remove + insert` 重建）。偏斜顶点（真实图谱的幂律头）恰恰长期处于多 chunk 态，每次遍历付全链跳转。应保证溢出段在合并后长期保持单块（合并阈值与写入路径联动，而不是写后 repack、下次追加又分裂），并用 chunk 内 unr oll 友好的平坦数组替代 `Vec<OverflowChunk>` 的嵌套 Vec（每个 chunk 各自堆分配，分配器压力与局部性都差）。

### 3.4 LiveSet HashMap：点查加速的代价是写放大与内存

宽行点查从 O(degree) 降到 O(1) 是好事，但代价是：每宽行一个 `HashMap<(u32,i64), EdgePosition>`（每条目数十字节，8 度以下行豁免）；任何结构性操作（`remove_edge` 的 gap-close `memmove`、rebalance、repack、`reserve_for_batch` 全表重建）都使已存 `EdgePosition` 失效，必须整行重建 set（`rebuild_live_set_for_vertex` 全行扫描）；`delete_edge` 先扫 primary 再扫 overflow，最坏仍是 O(degree)。而 neug 根本没有这个索引：点查就是行内线性扫描，对平均度数小的图谱反而更快。当前设计对“宽行高频点查 + 低频结构变更”最优，对“持续写入的偏斜行”每次写都可能触发重建。建议：冻结/只读行用排序 + 二分替代 HashMap（零额外内存，`ImmutableCsr` 已有 `partition_point` 范例）；可变宽行保留 HashMap 但只在行稳定后构建，结构性操作批量合并而不是每次重建。

### 3.5 `reserve_for_batch` 与 `batch_put_edges` 的全表重建

`reserve_for_batch` 为少量 touched rows 重建整个 `hot_list / cold_list`（O(total)，`write.rs`），neug 的 `batch_put_edges` 同样全表重建（`mutable_csr.cc`），但 neug 只在 bulk 路径用、单边 `put_edge` 是 O(1) 顶点级扩容。当前单边 `insert_edge` 虽也是 O(1) gap 填充，可一旦主块满就溢出到 chunk 链而不是顶点级扩容，长期导致行 pop 分散；批量路径又走向另一个极端的全表拷贝。两者之间缺少 neug 式的“顶点级 1.5 倍扩容”中间档。建议补顶点级扩容（只搬运该行，需要行锁配合，见 2.7），把 `reserve_for_batch` 限定为真正的 bulk load 场景，并复用同一次重建完成排序（与 2.6 联动）。

### 3.6 窄行无索引时的每写全行扫描

窄行（≤8）每次 `insert_edge` 经 `row_live_scan` 做“判重 + 计数”整行扫描：O(degree) 两次语义合一遍，尚可接受。但 `delete_edge(edge_id)` 在 primary 未命中时必扫全 overflow 链；`get_edge` 在 `ts != MAX` 且 live set 缺失对应 key 时回退全行扫描。也就是说读放大的最坏仍是 O(degree)，live set 只加速了 `ts == MAX` 的缺失 key 短路。偏斜顶点的窄行阶段（从 0 涨到 8 的过程）每次插入都扫描渐长的前缀，总计 O(d²) 构建成本。建议插入路径维护行 live 计数（增量 `u32`，而非每次扫描），判重在宽行走索引、窄行走短扫描但跳过墓碑段（墓碑计数同样增量维护）。

### 3.7 Group 分片与路由的每访问税

neug 的顶点寻址是两次数组下标。当前每次跨 group 访问经 `CsrShardSet::route`：route cache 命中则返回三元组，未命中则 `group_id_for` 位运算 + `BTreeMap::contains_key`（O(log G)）。`group_size` 典型 2^10～2^16 时，BTreeMap 本身很小，但 cache 是按 `vid` 的哈希表，每次 `visit_physical_with_values` 都要过它。更贵的是 group 内的二次寻址：全局 vid → local vid → variant 内下标。对遍历密集型负载，这层分发是纯开销。建议：读热点旁路 route cache，直接按 `gid = vid >> bits` 下标 shard 数组（group id 稠密时用 Vec 而非 BTreeMap；稀疏时保留 BTreeMap 但读路径用预计算的稠密路由表），把“分片”从每次访问税降为构造期决策。

### 3.8 墓碑复用有界扫描与 `tombstone_reuse_cutoff` 的维护耦合

热路径复用只扫前 64 槽（`TOMBSTONE_REUSE_SCAN_BOUND`），超宽行的尾部墓碑永远等后台回收；cutoff 由 table 维护线程刷新，刷新不及时则退化为只追加（注释自认“degrades to pre-reuse behavior”）。这意味着写放大在水位推进延迟时无界增长，而 64 的界是拍脑袋常数（无 benchmark 引用）。neug 没有热路径复用：墓碑只由 `compact` 批量回收，行为可预测。建议：要么以后台回收为主、删除热路径复用（简化）；要么把复用扫描改为“每行记录首个可复用槽 hint”，做到 O(1) 复用而不是每插扫 64。无论哪种，64 这类魔数应有 bench 支撑（`benches/csr_perf_bench.rs` 可扩展覆盖）。

## 4. 改进路线（按优先级）

1. **收敛删除模型**（改 `mutable_csr/write.rs`、`csr_shared.rs`）：统一墓碑 + 后台回收；`remove_edge` 物理擦除仅保留为明确的回滚原语并隔离其调用点；对齐 `delete_edge_by_offset` 与 `delete_edge` 的冲突语义；补不变量测试（`edge_count`、`live_set`、ledger 三方一致）。
2. **收敛时间戳权威**（改 `edge.rs`、`mutable_csr/` 读写路径）：二选一后删除另一套 API；若权威上移，行内 `ColdStamps` 缩为墓碑位/版本号，`edges_of(ts)`、`get_edge(ts)` 等行内过滤方法全部删除，测试改走权威。
3. **行记录瘦身**（改 `edge.rs` 及各 variant 编解码）：`rank` 按需存储、`create_ts` 移出热行；目标是可变行热部 ≤16 字节/边。瘦身后再评估热冷分离是否保留。
4. **统一溢出实现**（改 `pure_csr.rs`、`bundled_csr.rs`，只用 `mutable_csr/overflow.rs`）：三者共用 chunk 与 table 类型；`Pure / Bundled` 仅保留编解码差异。禁止新增第四个 CSR 形态（架构规则：优化只改原模块，不建 v2 模块）。
5. **引入轻量行视图**（改 `mutable_csr/iter.rs`、`csr_trait.rs`）：与 variant 无关的 `(hot_slice, cold_slice, offset)` 行视图 + 内联可见性谓词，作为跨层遍历契约，逐步替代闭包 visitor 与分配式 `edges_of`。
6. **排序态与批量有序写入**（改 `mutable_csr/row.rs`、`write.rs`）：每行有序标记；bulk 路径一次重建完成“扩容 + 排序”，冻结行的 `(endpoint, rank)` 二分查找推广到稳定宽行，替代其 HashMap。
7. **持久化校验**（改各 `persistence.rs` / `serialization.rs`）：复用 `index` 模块的 CRC 模式统一加校验；评估 serving mmap 的 huge page。
8. **顶点级并发与路由减税**（按需，实测驱动）：恢复顶点级锁表；读热点用稠密路由表替代 BTreeMap + cache。两项都先用 `benches/csr_perf_bench.rs` 量化后再动。

## 5. 一句话总结

neug 的 CSR 是“小记录 + 指针数组 + 单一墓碑 + 零拷贝视图”的精益设计；当前实现用“大记录 + 主块/溢出/索引/分片四层结构 + 三套删除模型”换来了功能丰富性，但每条边 5～10 倍的内存开销、重复三遍的溢出逻辑、双权威时间戳是必须偿还的设计债。改进应先做减法（收敛删除模型、时间戳权威、记录瘦身、统一溢出），再做加法（行视图、排序态、校验），且严格在原模块内修改，不新增替代性模块。
