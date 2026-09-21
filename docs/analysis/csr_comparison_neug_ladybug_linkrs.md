# CSR 实现对比分析：ref/neug、ref/ladybug 与当前项目

> 范围：三方各自“边邻接（CSR）”子系统的架构、功能、性能差异。
> 源码依据：`ref/neug/src+include/neug/storages/{csr,container,graph}`；
> `ref/ladybug/src/{include/storage/table/csr_*,storage/table/csr_*,storage/table/{node_group,chunked_node_group,column_chunk*,rel_table*}}`；
> `crates/graphdb-storage/src/edge/` 全目录（含 `mutable_csr/`、`edge_table/`、`node_group/`）。
> 本文档使用中文，代码标识保持英文原文。

## 1. 一览对比

| 维度 | ref/neug | ref/ladybug | 当前项目（linkrs） |
|---|---|---|---|
| 语言 | C++ | C++ | Rust |
| CSR 在系统中的位置 | 存储层独立子系统，被 `EdgeTable` 直接持有（`out_csr_ + in_csr_`） | 关系表内的一层：`RelTable → RelTableData(方向) → CSRNodeGroup(nodeGroup 粒度)`，CSR 只做“定位”，载荷仍是列存 | `EdgeStore` 门面下的分片拓扑：`EdgeStore → CsrShardSet(按 bound 端点区间分片) → CsrVariant`，时间戳权威与属性列存都在 CSR 之外 |
| 形态划分主轴 | 写模型 × 边数：`Immutable / SingleImmutable / Mutable / SingleMutable / Empty`（`CsrType` 五态）+ 属性内联/外置（bundled/unbundled） | 生命周期 × 文件位置：`persistentChunkGroup(已落盘) + transient chunkedGroups(内存) + csrIndex(内存倒排索引)`，同一 nodeGroup 内双态并存 | 记录形态 × 可变性：`Multiple / Single / Pure / Bundled / Frozen / Mapped / None`（`CsrVariant` 七态），由 `RecordForm(Pure/Bundled/Columnar)` 与 freeze 状态正交决定 |
| 单顶点多边表示 | 三数组：`deg[] + nbr[]（紧凑连续）+ adj[]（指针数组，纯内存重建）`；Mutable 另加 `cap[]` 每顶点预留空隙 | 双列 header：`offset[]（存 end-offset）+ length[]`，数据行是普通列存 `ColumnChunk` 行区间 | 主块 + 分级溢出：`adj_offsets/degrees/primary_capacities + hot_list(HotNbr 24B)/cold_list(ColdStamps 8B)` 锁步连续块 + `OverflowTable` 溢出链 + 宽行 live 索引 |
| 单边/单属性特化 | `Single*` 变体（无 degree 数组，按 vid 直接索引）；`EmptyType` 模板特化把条目压到 4B/8B；11 种 `EDATA_T` 显式实例化 | 无单边特化；`NBR_ID / REL_ID` 为保留列；header 两列固定 `UINT64` | `SingleMutableCsr`（每顶点固定 32B 槽）；`PureTopologyCsr` 12B/边；`BundledCsr` 20B/边（拓扑 12B + 内联 u64） |
| 并发写 | `MutableCsr::put_edge` 持每顶点 `SpinLock`；时间戳字段为 `atomic`；批量路径假设单写者 | 内存追加用行号原子预占 + `chunkedGroups` 短锁；COPY 按 `nodeGroupIdx` 分区免锁；checkpoint 持分片锁 | CSR 层零锁：`&mut self` 即单写者（借用检查器保证），顶点级锁是调用方决策；多核扩展靠分片（group）级并行 |
| 可见性/MVCC | 时间戳内联在邻居条目（`MutableNbr.timestamp`），读带 `ts`，迭代器过滤 `timestamp > read_ts`；Immutable 系无版本（恒可见） | 行级 `VersionInfo` + 列级 `UpdateInfo` 版本链，透传自 `ChunkedNodeGroup` 基类；persistent/in-mem 各有 handler | 时间戳权威外置于 `MVCCManager/EdgeTimestamps`，CSR 行 `delete_ts` 只是物理投影；查询可见性必须走版本权威做合并判定 |
| 持久化 | mmap 容器四形态（匿名/大页/私有映射/共享映射）+ `FileHeader(16B MD5)` + `.deg/.nbr/.cap/.meta` 多文件；checkpoint 即 `dump` 全量 | 影子分页（`ShadowFile`）+ WAL + 按 region 增量 checkpoint（Packed-CSR 合并，未变 segment 可跳写） | 分组基文件（列编码 + CRC32 校验）+ `edge_wal.bin`（commit 先 append+fsync）+ 脏组增量 checkpoint；frozen 组另有 mmap serving sidecar（派生缓存） |
| 读路径形态 | 零拷贝 `GenericView` 值对象 → `get_edges(v)` → `NbrList` range-for；`TypedView` 支持有序前缀二分 | 向量化 2048 行扫描状态机（`COMMITTED_PERSISTENT → COMMITTED_IN_MEMORY → NONE`），跨 bound-node 批量预取 | 三种行访问形状：`visit_physical`（借用回调，首选）/ `fill_physical_into`（调用方 buffer，批量首选）/ `physical_edges_of`（分配，仅测试离线）；生产遍历走 `visit_hot` 只碰拓扑行 |
| 有序性承诺 | `unsorted_since_` 水位线 + `batch_sort_by_edge_data` 全顶点 `std::sort`；`TypedView::foreach_nbr_gt/lt` 有序前缀二分、无序后缀线性 | CSR 行按 offset 追加序，无全局排序承诺 | `MutableCsr` 行内插入序（`is_row_sorted/sort_row` 供查询选路）；`ImmutableCsr/Frozen` 打包时按 `(endpoint, rank, edge_id)` 行内全排序，点查/阈值走二分 |

## 2. ref/neug CSR

### 2.1 架构

继承链为 `CsrBase → TypedCsrBase<EDATA_T> → {ImmutableCsr, SingleImmutableCsr, MutableCsr, SingleMutableCsr, EmptyCsr}`（`csr_base.h`）。
`CsrType` 五态与 `EdgeStrategy(kNone/kSingle/kMultiple)` 正交：边 schema 的 `oe/ie_strategy + oe/ie_mutable`
决定每个方向走哪一类（`edge_table.cc:create_csr` 按 `DataTypeId` switch 分发 11 种边数据类型）。

图层（`EdgeTable`）恒持 `out_csr_ + in_csr_` 双写：所有增删改（`BatchAddEdges/AddEdge/BatchDeleteEdges/DeleteEdge/BatchDeleteVertices/Compact/Open/Dump`）
均为 OE/IE 成对调用，`DeleteVertex` 用 `generic_view_utils` 在对侧 `fuzzy_search` 定位后双删。
顶点不走 CSR（`VertexTable` 用 `Indexer + Table + VertexTimestamp`），CSR 只存边。

底层是四种 mmap 容器（`i_container.h / mmap_container.h / file_mmap_container.h / anon_mmap_container.h`），
按 `MemoryLevel(kInMemory/kHugePagePreferred/kSyncToFile)` 选择：

- `kInMemory`：匿名映射，或 `MAP_PRIVATE` 读快照（写时 COW 不回文件）；
- `kHugePagePreferred`：大页匿名映射，降 TLB miss；
- `kSyncToFile`：snapshot 拷贝到 `runtime/tmp` 后 `MAP_SHARED` 直写，`Resize` 用 `ftruncate` + 重映射，`Sync` 用 `msync`。

`MMapContainer::Open` 一律映射整个文件并跳过 `FileHeader`（16B MD5），注释明示“假设文件正确，跳过校验”。

### 2.2 核心数据结构

多边 `ImmutableCsr` 为 `deg[]（.deg 文件） + nbr[]（ImmutableNbr 紧凑连续，.nbr 文件） + adj[]（nbr_t* 指针数组，纯内存重建、不持久）`，
另有 `unsorted_since_ + edge_num_` 持久于 `.meta`。
多边 `MutableCsr` 为 `sz[]（.deg） + cap[]（.cap，每顶点容量） + 带空隙的 nbr 后备存储（总长 Σcap，.nbr） + adj[]（各段段首指针，tmp .buf，不持久）`；
`open_internal` 按 `cap` 切分重建 `adj`，并校验 `Σdeg == meta.edge_num`，不等直接抛存储异常。
`Single*` 变体只有 `snbr[]`（`nbr[v]` 即顶点 `v` 的唯一边，空位哨兵 `neighbor=UINT32_MAX`），无 degree 数组，`GenericView.degrees_ == nullptr`。

邻居条目（`nbr.h`）：`ImmutableNbr{T} = {neighbor:u32, data:T}`（无时间戳）；
`MutableNbr{T} = {neighbor:u32, timestamp:atomic<u32>, data:T}`；
`EmptyType` 特化用 union 实现零开销（immutable 4B/条、mutable 8B/条，纯拓扑图最省）。
`get_generic_view(ts)` 的 Immutable/Mutable 差异仅在 `NbrIterConfig`：
Immutable 取 `ts=MAX-1` 恒可见；Mutable 把 `ts_offset` 指向条目内时间戳做 MVCC 过滤。

`GenericView` 寻址：单边 `start = adjlists_ + v*stride`；多边 `start = adj[v], end = start + degrees[v]*stride`。
注意多边 `adjlists_` 存的是指针数组（8B/顶点），多一次间接跳转，但省去 offset 前缀和计算。
`EdgeDataAccessor` 区分 bundled（data 内联在 NBR）与 unbundled（NBR 存 `row_id`，跳属性表；`varchar` 永不 bundled）。

### 2.3 功能

`CsrBase` 虚接口覆盖生命周期（`open/open_in_memory/open_with_hugepages/dump/load_meta/dump_meta/close`）、
容量（`size/edge_num/capacity/resize`，多边 `capacity()` 恒为无限）、单条写（仅 Mutable 有意义：
`put_edge` 越界抛错、加顶点自旋锁、`cap += cap>>1` 最小 8 起扩、`ArenaAllocator` 16MB batch 搬迁）、
批量写（Immutable `batch_put_edges` 全量后移 `O(V+E)`；Mutable 按 `reserve_ratio=1.2` 全量重建；
Single 系逐槽覆盖）、删除（Immutable 写 `neighbor=MAX` 哨兵，Mutable 写 `timestamp=INVALID` 墓碑，均靠 `compact` 物理回收；
`revert_delete_edge` 供事务回滚）、扫描（`get_generic_view(ts) → get_edges(v) → NbrList` range-for）、
维护（`compact/reset_timestamp/batch_sort_by_edge_data/batch_export`）。
其中 `batch_export` 只有 Mutable 实现（Immutable 直接 `LOG(FATAL)`），
因此 unbundled→bundled 的属性 schema 变更重建（`dropAndCreateNewBundledCSR` 新建内存 CSR → `resize` 对齐 → 导出重灌 → 新旧替换）只适用于 Mutable 边类型。

### 2.4 性能特征

- 点查邻居 `O(1)`；单顶点扫描 `O(deg)` 顺序连续（Immutable 全连续；Mutable 段内连续、段间经 `adj` 跳转）。
- `MutableCsr::put_edge` 均摊 `O(1)`（1.5× 扩容 + arena 摊销）；`SingleMutable::put_edge` 严格 `O(1)`。
- 批量插入与 `compact` 均为 `O(V+E)` 全量搬迁，大批量场景昂贵；按值删边需建 `map<vid,set>` 索引（`O(k log k)` 节点式容器）+ 每顶点线性扫，对侧定位退化为 `fuzzy_search` 线性扫（仅当 `EdgeRecord.prop` 指针落在 `[start,end)` 内才有 `O(1)` 指针算术快路径）。
- 已实现的优化：`unsorted_since_` 水位线使有序前缀可用 `lower_bound` 二分（`TypedView::foreach_nbr_gt/lt`）；
  `reserve_ratio=1.2` + 每顶点 1.5× 扩容的双层预留；大页选项；`NbrIterator/NbrList` POD + 内联的零分配遍历。
- 明确没有的东西：CSR 层无 SIMD、无显式预取、无压缩编码（delta/varint）、无位图/跳表索引；
  `edge_num_` 是逻辑计数，删后未 compact 时与物理长度不一致。

## 3. ref/ladybug CSR

### 3.1 架构

层次为 `RelTable → directedRelData[FWD|BWD](RelTableData) → NodeGroupCollection → CSRNodeGroup(nodeGroup 粒度，131072 个 bound node) → ChunkedCSRNodeGroup`。
关键设计是 CSR 只解决“bound node → 行区间”定位，区间内各属性仍是普通列存 `ColumnChunk/ColumnChunkData` 向量，
谓词下推走 `ChunkState/columnPredicateSets`。`CSRNodeGroup` 继承 `NodeGroup`，
复用其 `append/merge/lookup` 与 MVCC 可见性语义，仅重写扫描与 checkpoint 相关虚函数。

同一 nodeGroup 内 persistent（`persistentChunkGroup: ChunkedCSRNodeGroup`，已落盘）与
transient（`NodeGroup::chunkedGroups` 内存追加 + `csrIndex` 内存倒排索引）双态并存，
`scan()` 按 `CSRNodeGroupScanSource{COMMITTED_PERSISTENT, COMMITTED_IN_MEMORY, UNCOMMITTED, NONE}` 在两者之间做状态机切换，
`update/delete_` 按 source 分发。`csrIndex` 是 `array<NodeCSRIndex, 131072>`，
顺序追加时存 `[startRow, length]` 二元组（`isSequential`），乱序后展开为逐行 `rowIdx` 并 `std::sort`，
头文件 TODO 承认其空间效率差（每节点一个 `vector`，稀疏关系浪费明显）。

在 `RelTable` 层，每个方向是独立 `RelTableData`（`columns[NBR_ID, REL_ID, props...]` + 独立 `csrHeaderColumns{offset, length}`），
存储方向（FWD/BWD/BOTH）由 catalog 配置。`update/delete_` 都先经 `RelTableData::findMatchingRow`
（按 `REL_ID` 列线性比对 `relOffset` 的 randomLookup）定位 `(source, rowIdx)` 再委托到 `CSRNodeGroup`，
因此点更新有二次定位开销，且 `REL_ID` 列必须常驻可扫。

### 3.2 核心数据结构

CSR header 是 offset/length 双列，`offset[i]` 存第 `i` 个 CSR list 的 **end offset**（start 由前一项推导），
`getStartCSROffset/getEndCSROffset/getCSRLength/getGapSize` 均据此算术；
`sanityCheck` 校验 `offset[i-1]+length[i] <= offset[i]`。
`CSRRegion` 是 Packed-CSR 校准树的基本单位：叶 region 覆盖 1024 个 bound node（`CSR_LEAF_REGION_SIZE=1024`），
`calibratorTreeHeight = 17-10 = 7`，每个 region 记录 `sizeChange/hasUpdates/hasInsertions/hasPersistentDeletions`。

### 3.3 功能

- 扫描：`initializeScanState` 把磁盘 header 全量读入内存（每 group 每方向约 2MB：131072 个 `uint64 offset + length`），
  数据按 2048 行向量化；单 bound node 走 `WithoutCache` 直扫，多 bound node 走 `WithCache`
 （一次预取一段连续 CSR 行入缓存，按 header 二分到各 bound node，减少随机 IO）；
  内存态顺序 list 直接区间扫，乱序 list 逐行 `lookup` 聚合。
- 插入：单条 `append + updateCSRIndex`；COPY 批量路径（`RelBatchInsert::appendNodeGroup`）按 nodeGroup 分区并行，
  `populateCSRLengths → populateStartCSROffsetsFromLength → finalizeStartCSROffsets → finalizeCSRRegionEndOffsets → resizeChunks/resetToAllNull → writeToTable`，
  新 group 直接落盘为 persistent，已存在 group 走内存追加；只有新 group 预留 gap（`leaveGaps = isEmpty()`）。
- checkpoint：首次落盘（`checkpointInMemOnly`）全量排序重排成 CSR；增量合并（`checkpointInMemAndOnDisk`，Packed-CSR）分七步：
  读旧 header → 128 个叶 region 收集变更并更新 length → 按密度界合并 region
 （叶密度上限 1.0，上层向 0.8 按步长插值收敛，超顶触发全量 `redistribute`）→ 左锚定右对齐重排 offset →
  统计全量元组数（为 0 则直接回收存储）→ 逐列逐 region 用 `LazySegmentScanner + CheckpointRead/WriteCursor` 合并
 （无持久删按 segment 批量搬运、可 `canSkipWrite` 跳过未变 segment；有删逐行过滤；内存插入合并；gap 复用或填 null）→
  落盘新 header 并原子替换 persistent 组、清空内存态。无 region 需写时仅重置版本信息。

### 3.4 性能特征

- COPY 构建按 nodeGroup 分区并行，`O(边数)` 计数 + `O(节点数+边数)` 排 offset，但 `resizeChunks + resetToAllNull` 预分配全 null，
  内存峰值 = 该 group CSR 总行数 × 列宽（含 gap）。
- 前向扩展友好（bound node 直接算 `(start, length)` 顺序区间扫 + 2048 行向量化 + 批量预取）；
  按 `relID` 的点查退化为该 node 全 list 线性比对；内存乱序 list 逐行跨 chunk `lookup` 开销大。
- 写放大：gap 预留 `len/0.8-len`；更新频繁时 `upgradeLevel` 合并更大 region 重写，写放大随 level 指数增长，顶层溢出触发全 group 重分布。
- 优势在定位与载荷解耦：扩展只读 `NBR_ID/REL_ID`，属性投影才扫对应列；checkpoint 按 `needCheckpointColumn` 跳过 region 内无变更列。

## 4. 当前项目 CSR

### 4.1 架构

两层分发：`CsrBase`（容量/计数/持久化）→ `MutableCsrTrait`（约 40 方法：插入/删除/读/维护/统计）→
六种具体实现（`MutableCsr, SingleMutableCsr, PureTopologyCsr, BundledCsr, ImmutableCsr, MappedFrozen`），
再由 `CsrVariant` 七态 enum（另加 `None` 占位）经 `dispatch!` 宏静态分发，避免 `dyn` 与泛型膨胀。
行视图有统一契约：每行物理条目恰好出现一次、组装为 `Nbr`、gap 哨兵排除、tombstone 包含、可见性由上层决定；
`EdgePosition{Primary{slot}, Overflow{chunk, slot}}` 是 variant-local，跨层必须先按 `edge_id` 重定位（fail-closed）。

记录形态由建表时一次决定并持久化于 `EdgeSchema.record_form`，load 永不重推断：

| 形态 | 每边 | rank/时间戳 | 属性 | 适用 |
|---|---|---|---|---|
| `PureTopologyCsr` | 12B（`u32 endpoint + u64 edge_id`） | rank 强制 0，非 0 拒绝；无时间戳 | 无（`CsrWithProperties::inline_stub` 占位防分叉） | 无属性纯拓扑、读多 |
| `BundledCsr` | 20B（拓扑 12B + `u64 value`） | rank 强制 0；无 MVCC | 恰好 1 个可编码标量，与拓扑槽位严格平行 | 单数值属性、schema 稳定 |
| Columnar（`MutableCsr/SingleMutableCsr + CsrWithProperties`，默认） | 拓扑 32B（`HotNbr` 24B + `ColdStamps` 8B）+ 列存 | 完整 `i64` rank；完整 MVCC（`delete_ts` 行投影 + `EdgeTimestamps` 权威） | 多属性/任意类型/schema 演进，`Column` + 版本链 + 谓词下推 | 通用 |

`SingleMutableCsr`（一对一语义）每顶点单槽、无 offset/degree/overflow/live-set，全 `O(1)`；
`ImmutableCsr` 是 packed 只读 CSR（`pack_from_mutable` 合并主块+溢出、行内排序、丢 gap 哨兵、保留 tombstone），写入口全拒绝；
`MappedFrozen` 是同一逻辑的 mmap serving 视图（`memmap2::Mmap`，clone 共享映射，组替换后仍可读旧快照）。
`dump` tag `0=None, 1=Multiple, 2=Single, 3=Frozen/Mapped（统一落盘为 Frozen）, 4=Pure, 5=Bundled`。

`MutableCsr` 内部按职责拆分为 12 个子模块（`core/write/read/row/live_set/overflow/persistence/compaction/stats/trait_impl/iter/serialization`），
是三方中模块切分最细的一个。门面 `EdgeStore` 做双写 `out_csr/in_csr: CsrShardSet`（按 `group_bits` 分片的 `BTreeMap<usize, Shard>`，
bound 端点按区间路由）+ `MVCCManager` + 属性库 + WAL + checkpoint。

### 4.2 核心数据结构

槽位热冷分离：`HotNbr{endpoint:u32, rank:i64, edge_id:EdgeId}`（24B）+ `ColdStamps{delete_ts}`（8B），
生产遍历走 `visit_hot` 只碰拓扑行以减少缓存污染；`create_ts` 不内联，存于版本权威。
`MutableCsr` 布局：主块 `hot_list/cold_list` 锁步连续 + `VertexBookkeeping{adj_offsets, degrees, primary_capacities}` +
`OverflowTable` 溢出链 + `LiveSetStorage` 宽行 `(endpoint, rank)→EdgePosition` 索引。
零度行零成本（首边前不分配主块，默认 4 槽）；溢出/live-set 按 1024 分段稀疏索引，未触碰段为 `None`，
另有每顶点 1bit `present` 位图前置过滤；`LiveKeySet::Hash/Sorted` 双形态以 `LIVE_SET_WIDTH_BOUND=8` 为界，
窄行（≤8）直接线扫，重建后晋升 `Sorted` 二分，写时回退 `Hash`。
`Pure` 删除写 `INVALID_EDGE_ID` 哨兵（endpoint 保留保位置有效）；`Bundled` 值列与拓扑等长严格跟随，删除清 `valid` 留 stale word。

序列化全部带尾部 CRC32（载入先验 CRC 再解析，`edge_count` 重算校验，尾随字节拒绝）；
`MutableCsr` 拓扑列按列选优编码（`Plain/BitPacked/Rle`）；`Mapped` serving 文件为定宽列可索引寻址
（`magic GCSR + rows/entries/live + 5×(offset,len) + degrees/endpoints/ranks/edge_ids/deletes + CRC`），
与基文件是兄弟 sidecar 关系，写后原子 rename。

### 4.3 功能

- 插入：live-set 去重（`EdgeAlreadyExists`）→ 主块尾 gap 填充 → 水位门控 tombstone 原位复用
 （hint `O(1)` + `TOMBSTONE_REUSE_SCAN_BOUND=64` 有界回退）→ 溢出追加；另有 `reserve_for_batch + batch_put_edges` 批量路径。
  `Single` 活槽二次插入一律 `Conflict`，tombstone 槽任意时间戳可重建。
- 删除：`delete_edge`（`Ok(false)`=不存在/同 ts 幂等，`Err(write_write_conflict)`=异 ts 重复删）、
  `delete_edge_by_dst[_reporting]`、`delete/revert_at_position/by_offset`（期望 id 重验，stale 拒绝）、
  `rollback_insert`（物理擦除无 tombstone）、`revert_delete_by_edge_id`。
- 读：行读三件套 + `visit_threshold`（sorted 主块前缀二分 + 溢出恒线扫）+ `is_row_sorted/sort_row` 供查询选路；
  `EdgeStore` 层以前向/后向组织（`out/in_edges[_with_gate/_projected/_limit]`、`visit_out/visit_in_with_gate` 零分配扇出、
  `merged_get_edge` 处理删后重建同 key），属性投影 Columnar 走按 `edge_id` 取列，Bundled 走 `decode_scalar`。
- 持久化：各 CSR `dump/dump_into/load`；`EdgeStore::flush/load`（组分片先落，`meta.bin` + manifest 最后发布，
  `load_incremental` + WAL 重放，重放幂等，torn tail 直接 fail load，需离线显式修复）；
  freeze（`freeze_group/unfreeze_group`，`Frozen` 行内全排序承诺 `(endpoint, rank, edge_id)`）；
  记录形态迁移（`migrate_record_form` 离线 / `switch_record_form_online` 在表，纯重建 + 边 id/双向/索引键保留，
  切换前 WAL redo 被 fence 永不重放）。
- 已知限制（源码自述）：`Frozen::compact_row` 重建全表 offsets（单行 GC 也是 `O(table)`）；
  `Bundled` 不能 freeze（含有效值时拒绝）、无 rank、无 MVCC、无 id-keyed revert，超一列/不可编码类型/在线改 schema 必须回 Columnar；
  `Pure` 无 rank/时间戳，删除留哨兵需紧缩，阈值忽略 rank；跨变体位置永不可复用。

### 4.4 性能特征

- 内存：`Pure 12B/边`，`Bundled 20B/边`，Columnar 拓扑 `32B/边` + 属性列；`Immutable` 无 capacity/overflow/index 开销；
  `Single` 每顶点固定 32B（含空槽哨兵，空顶点也占槽）。
- 扫描：`Mutable/Pure/Bundled` 行内插入序，宽行 live 索引 `O(1)`，窄行线扫；`Frozen/Mapped` 行内全排序，
  点查/阈值 `O(log d + hits)`；`Single` 全 `O(1)`；批量用 `fill_physical_into` 复用 buffer 避免每顶点分配。
- 写放大控制：主块 gap 优先填充（稳态写不碰溢出）；水位门控 tombstone 复用；溢出分级块（`graded = next_pow2(live).clamp(8,4096)`）
  + 单行超 8 链合并；`compact` 两遍重建（计数定容后直拷，峰值为一个新 list）；`rebalance_row` 只动单行不增长容量。
- 短板：高度数行仍有主块 gap + 溢出链指针追逐；`vertex_census/row_gap` 等维护探针为行扫。

## 5. 架构差异

1. **分层位置不同**。neug 的 CSR 是存储层独立构件（`EdgeTable` 直接拥有双份 CSR，外加可选属性 `Table`）；
   ladybug 的 CSR 是关系表内部的一层索引结构（定位与载荷分离，复用 `NodeGroup/ColumnChunk` 基类与 MVCC）；
   当前项目是分片拓扑构件（`CsrShardSet` 按 bound 端点区间分片，时间戳权威与属性列存完全外置，CSR 只存拓扑 + 行投影戳）。
2. **形态划分主轴不同**。neug 按“写模型 × 边数”（Immutable/Mutable × 多边/单边）；
   ladybug 按“生命周期”（persistent/transient 双态 + 内存索引）；
   当前项目按“记录形态 × 可变性”（Pure/Bundled/Columnar × Mutable/Frozen/Mapped），把“有无属性、几个属性”也编码进变体。
3. **扩展性设计不同**。neug 用 C++ 模板（`TypedCsrBase<EDATA_T>`，11 种边数据类型显式实例化）；
   当前项目用 Rust enum + `dispatch!` 宏静态分发（无 `dyn`、无泛型膨胀）；
   ladybug 用继承（`CSRNodeGroup: NodeGroup`、`ChunkedCSRNodeGroup: ChunkedNodeGroup`）复用列存与版本链。
4. **分片粒度不同**。neug 以整表为单位（单个 CSR 覆盖全顶点，`resize` 即全表扩容）；
   ladybug 以 nodeGroup（131072 bound node）为单位（COPY/checkpoint/扫描都按 group 分区）；
   当前项目以可配 group（`group_bits` 区间分片 + `BTreeMap` 稀疏组）为单位，组内再行级管理。
5. **mmap 角色不同**。neug 的 mmap 是主存储（三种内存级别，`MAP_SHARED` 直写即持久化）；
   ladybug 无 mmap（页式存储 + 影子分页 + WAL）；
   当前项目堆内 CSR 非 mmap，只有 frozen serving sidecar 用 mmap 做只读加速（派生缓存，坏缓存可丢弃重建）。

## 6. 功能差异

| 功能 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 前向/后向 | `EdgeTable` 双 CSR 双写（`oe_/ie_` 文件前缀） | 双 `RelTableData`（FWD/BWD，可配 BOTH/FWD/BWD） | 双 `CsrShardSet`（`out_csr/in_csr`）+ 边 owner 映射 |
| 单边优化 | `Single*`（无 degree，直接索引） | 无 | `SingleMutableCsr`（固定槽，全 `O(1)`） |
| 属性模型 | bundled（data 内联 NBR）/ unbundled（NBR 存 row_id 跳表），`varchar` 永不 bundled，单非 string 属性才可 bundled | 全列存（`NBR_ID/REL_ID` 保留列 + 属性列），定位与载荷解耦，谓词下推 | Pure（无属性）/ Bundled（单标量内联）/ Columnar（`CsrWithProperties` 列存 + 版本链），建表时 `Auto` 选择器按 schema 推断 |
| 排序 | `batch_sort_by_edge_data` + `unsorted_since_` 水位线，有序前缀二分 | 无排序承诺（offset 追加序） | Mutable 插入序（`sort_row` 维护路径，位置失效）；Frozen 打包全排序 |
| 删除语义 | Immutable 哨兵 / Mutable 墓碑时间戳；`revert_delete_edge` 回滚 | 版本链删除（persistent/in-mem 双 handler）+ undo buffer | tombstone + 水位门控复用 + `rollback_insert` 物理擦除 + `revert` 系列；异 ts 重复删报写写冲突 |
| 事务回滚 | `revert_delete_edge` | `commitInsert/commitDelete/rollbackInsert/rollbackDelete` 经 handler | `commit_staging_batch` 固定顺序（权威 → 属性 → out 拓扑 → in 拓扑 → 二级索引 → owner），失败逐段补偿；`PendingGate` 让未提交写对同一事务可见 |
| WAL/恢复 | WAL 在 CSR 范围外（事务层关联 `revert_delete_edge`） | `LocalWAL` + undo buffer + 影子分页原子切换 | `edge_wal.bin`（commit 先 append+fsync，checkpoint 后截断，重放幂等，torn tail 显式修复） |
| Schema 变更 | bundled↔unbundled 重建（仅 Mutable 可走 `batch_export`） | `addColumn` 等表层操作 | 记录形态迁移（离线/在线双路径，WAL fence）+ 列增删改名状态机 |
| 压缩 | 无（CSR 层） | `BoolChunk` bitpack、字符串字典压缩、浮点 ALP 例外块；header 双列固定 `UINT64` 不压缩 | 拓扑列编码（`Plain/BitPacked/Rle` 按列选优）+ CRC32；`postcard` 用于属性值 |
| Freeze/只读 serving | 无（Immutable 即只读态，无打包转换） | 无（persistent 即服务态） | 有：`pack_from_mutable` 打包 + mmap serving sidecar + `open_or_rebuild` |
| 完整性校验 | `FileHeader` 16B MD5（open 时跳过校验，dump 时回填） | 页式存储自带校验（影子文件） | 全部分组文件尾部 CRC32（载入先验后解析，失败拒绝） |

## 7. 性能差异

| 方面 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 点查邻居 | `O(1)`（指针解引用 + degree） | `(start, length)` 算术 + 行区间扫 | 宽行 live 索引 `O(1)`，窄行（≤8）线扫；Frozen/Mapped 二分 `O(log d + hits)`；Single `O(1)` |
| 单顶点扫描 | `O(deg)` 连续（Immutable 全连续；Mutable 段内连续） | 2048 行向量化，跨 bound 预取；内存乱序 list 逐行 `lookup` | `visit_hot` 只扫拓扑行；批量 `fill_physical_into` 复用 buffer；溢出链有指针追逐 |
| 单条插入 | Mutable 均摊 `O(1)`（1.5× 扩容 + arena） | `O(1)` append + 索引更新（转非 sequential 时 `O(k log k)`） | 主块 gap 填充 `O(1)` → tombstone 复用（有界 64 槽回退）→ 溢出追加；去重经 live-set |
| 批量插入 | `O(V+E)` 全量搬迁（Immutable `memmove`，Mutable `memcpy×2` + `Resize`） | 按 group 分区并行，`O(边数)` 计数 + `O(节点+边)` 排 offset，但预分配全 null 峰值高 | `reserve_for_batch + batch_put_edges` 预留批量路径；`CsrShardSet` 按组并行 |
| 删除 | 按值删 `O(k log k)` 建索引 + 每顶点线性扫；按 offset 删 `O(k)`；对侧定位线性 `fuzzy_search` | 先 `findMatchingRow` 二次定位（全 list 比对），再版本链删除 | 按 id 经 live-set 定位；`delete_by_dst` 全匹配；位置型删除期望 id 重验防 stale |
| 排序/整理 | `batch_sort_by_edge_data` 为 `Σ O(deg log deg)`；`compact` 为 `O(E)` + 全 `adj` 重建 `O(V)` | 全量 `redistribute` 为全 group 重排；增量合并写放大随 region level 指数增长 | `compact` 两遍重建（峰值一个新 list）；`rebalance_row` 单行；Frozen 单行 GC 仍 `O(table)`（已知短板） |
| 空间 | `EmptyType` 特化 4B/8B/条最省；Mutable `.nbr` 落盘含 `cap` 空隙（文件大于 `edge_num×sizeof(nbr)`）；`adj` 指针数组 8B/顶点常驻内存 | 磁盘态每 group 每方向常驻 header 约 2MB + 按需数据列；内存态 `CSRIndex` 每节点一个 `vector`（稀疏浪费大）；gap 预留 `len/0.8-len` | Pure 12B / Bundled 20B / Columnar 32B+列存；零度行零成本 + 1024 分段稀疏索引 + 1bit `present` 位图；Single 空顶点也占 32B |
| 读放大 | `adj` 一次间接跳转；Mutable 段间跳转 | header 全量常驻 + 数据列按需扫；`REL_ID` 列常驻可扫是固定成本 | 热冷分离减缓存污染；`edge_id → 权威/属性` 二次查是固定成本 |
| 并发扩展 | 顶点级自旋锁（扩容期不安全，需调用方串行扩大）；读无锁 | 分区/分片并行 + 短锁；checkpoint 持分片锁 | CSR 零锁（编译期单写者），并行靠分片；点写并行留给上层决策（文档要求先跑 contention benchmark 才允许加锁） |
| 已知未做 | SIMD、预取、压缩编码、位图/跳表索引 | `CSRIndex` 结构待优化（两级索引、落盘）；`checkpointInMemOnly` 待分段 | 高度数行溢出链追逐；维护探针行扫；`Bundled` 不可 freeze、无 MVCC；跨变体位置不可复用 |

## 8. 结论

1. **三者解决的是同一个核心问题（稀疏邻接的 O(V+E) 存储与 O(deg) 扫描），但优化目标不同**：
   neug 优化单机事务 + 分析混合负载（Mutable/Mutable 细粒度时间戳、mmap 主存）；
   ladybug 优化列存分析型扫描（定位载荷分离、向量化、Packed-CSR 增量合并）；
   当前项目优化形态适配与运维确定性（Pure/Bundled/Columnar 按 schema 选形、CRC 全覆盖、WAL fence、fail-closed 变体隔离）。
2. **可见性设计是最大分歧点**：neug 时间戳内联条目、ladybug 版本链随行、当前项目权威外置。
   外置权威使 CSR 保持纯拓扑（Pure 12B/边成为可能），代价是每次可见性判定与属性投影都要二次查表。
3. **当前项目相对两 refs 的增量**：七态变体（含 Pure/Bundled/Frozen/Mapped 三类 refs 没有的形态）、
   分组分片 + 脏区增量 checkpoint、列级编码选优 + CRC32、记录形态在线迁移、mmap 只读 serving sidecar。
4. **相对短板与可借鉴方向**：
   - 向 neug 借鉴：有序前缀二分（`unsorted_since_` 水位线）可用于加速当前项目 Mutable 行的阈值查询（目前仅 sorted 主块前缀可二分）；
     每顶点 1.5× 扩容 + 1.2 全量预留的双层预留策略可与当前 `graded` 分级块对照调参。
   - 向 ladybug 借鉴：跨 bound-node 批量预取（`WithCache`）可用于优化高度数顶点的扇出扫描；
     `canSkipWrite` 跳过未变 segment 的增量 checkpoint 思想已在脏组协议中部分体现，可继续下沉到列级。
   - 自身待补：Frozen 单行 GC 的 `O(table)` 重建、高度数行溢出链追逐、`CSRIndex` 式内存倒排（当前用 live-set，已优于 ladybug 的每节点 `vector`，但宽行内存仍需关注）。
