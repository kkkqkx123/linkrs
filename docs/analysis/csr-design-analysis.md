# CSR 设计分析：当前项目 vs Ladybug

> 路径：`crates/graphdb-storage/src/edge/` vs `ref/ladybug/src/storage/table/`
> 结论一句话：当前项目是“策略枚举 + 双写双 CSR + 属性旁路表”的可变 CSR 变体族，实现简单、单段内存友好；
> Ladybug 是“Packed CSR with gaps + 按 bound 切 NodeGroup + header 独立成列 + 三层可变性”的列存引擎。
> 当前设计在小规模单机场景可用，但存在变体冗余、拓扑与属性双重 CSR 索引、三副本可见性、 compaction 全量重建等结构性问题，
> 与 Ladybug 相比在可扩展性、写放大、检查点粒度上差距明显。

## 1. 当前项目 CSR 设计全貌

### 1.1 模块划分

| 文件 | 职责 |
|---|---|
| `src/edge.rs` | `Nbr{endpoint, rank, edge_id, create_ts, delete_ts}`（约 20B，`Copy`），`EdgeSchema`（oe/ie 策略），`EdgeRecord`。确立“拓扑与属性解耦”，CSR 只存拓扑四元组 |
| `edge/csr_trait.rs` | `CsrBase + MutableCsrTrait` 统一接口（insert/delete/delete_by_dst/delete_by_offset/remove/revert/compact 等），用 trait + 枚举分发代替 `dyn` |
| `edge/csr_variant.rs` | `enum CsrVariant{Multiple, Single, MultiSingle, Labeled, None}` + `dispatch!` 宏。持久化 `tag:u8 + inner.dump` |
| `edge/mutable_csr.rs (+overflow/serialization/iter)` | 主力多边 CSR（见 1.2） |
| `edge/single_mutable_csr.rs` | `Vec<Nbr>` 下标即 src，`edge_id==INVALID` 为空槽，O(1) 单边覆盖语义 |
| `edge/multi_single_mutable_csr.rs` | `Vec<Nbr>(vcap*K)` 定长 K 槽，`counts` 记录每行已用，满则报错 |
| `edge/labeled_mutable_csr.rs` | 全局 `nbr_list + nbr_sources + label_ranges: Vec<Vec<LabelRange{label,offset,count}>>`，按 label 分 run |
| `edge/csr_with_properties.rs` | Ladybug 风格列存：`offsets/lengths/heads + Column[] + visibility + edge_to_row + free_list`，按 CSR 行号对齐属性 |
| `edge/edge_table/{core,config,mvcc,persistence,compaction,remap,iterator}.rs` | `EdgeStore` 单段表：`out_csr + in_csr: CsrVariant` 双写 + `mvcc: MVCCManager` + `properties: CsrWithProperties` + `next_edge_id` |
| `edge/{bloom_filter,fragmentation_stats,property_schema}.rs` | 删除布隆（独立工具）、碎片统计（`wasted = cap-degree + Σ(chunk.cap-len)`）、属性 schema |

### 1.2 主力 `MutableCsr`：两级可变 CSR（非经典 CSR）

经典 CSR 是 `row_ptr/column/values` 三数组、不可变。`MutableCsr`（`mutable_csr.rs:42-58`）是：

- **主区**：`nbr_list: Vec<Nbr>` 扁平连续 + `adj_offsets/degrees/primary_capacities: Vec<u32>` 三数组 + `total_edge_capacity`。零度点懒分配（无边则 `cap==0` 不占 `nbr_list`，每行固定成本 12B）。
- **溢出区**：`OverflowStorage{entries: Vec<(vid, Vec<Vec<Nbr>>)>}` 有序 Vec 二分代替 HashMap；块大小固定 4096（`DEFAULT_OVERFLOW_CHUNK_EDGES`）；`overflow_live_sets: HashMap<vid, HashSet<(endpoint,rank)>>` 做 O(1) 查重；`OverflowIndex{sequential_runs}` 加速顺序 run。
- **增长策略**：顶点扩容 `×1.25`；主区满或已有溢出则固定块追加、永不拷贝旧块；单点超 32 chunk 且有死元则 `compact_overflow_for_vertex` 重打包。
- **删除/MVCC**：墓碑式，`delete` 置 `delete_ts`，`create<=ts<delete` 可见；同 ts 幂等、异 ts 报写写冲突；`remove_edge` 物理左移（仅回滚用）；读有 `iter(ts)`（过滤）/`iter_all`（含墓碑，供 remap）双模式。
- **持久化**（`mutable_csr/serialization.rs`，`FORMAT_VERSION=1`）：`[ver,vcap,edge_count,primary_len,chunk_size] + offsets + degrees + caps + nbr(36B/条) + 逐 vid[chunk_cnt + 每 chunk[len + nbr...]]`。load 重建 index + live_set。
- **Compaction**：`compact_with_ts_reporting(cutoff, reserve)` 全量重建扁平 CSR（`cap = valid/(1-reserve)`），按 `Visibility::is_gc_eligible` 丢弃并回调 `on_edge_removed` 上报墓碑。

其余三个变体本质是特化：`Single`（一对一覆盖）、`MultiSingle`（定 K 稀疏一对多）、`Labeled`（按 label 分 run，`insert` 全局 `insert(end)` O(n) 右移后续 offset）。

### 1.3 `EdgeStore` 单段表

`edge_table/core.rs:23-32`：

```rust
pub struct EdgeStore {
    pub out_csr: CsrVariant,   // 前向拓扑
    pub in_csr: CsrVariant,    // 后向拓扑（双写）
    pub mvcc: MVCCManager,     // EdgeTimestamps 权威 + tombstones
    pub properties: CsrWithProperties,  // 自带 offsets/lengths/heads 的属性 CSR
    // + next_edge_id, config, property_index_cache, version_history
}
```

- **写**：`insert` 按 `has_edge → record_creation → insert_for_edge → out.insert → in.insert` 步步失败物理回滚；`delete` 按 `out.get → out.delete_by_id → in.delete_by_dst → record_deletion + mark_deleted`。
- **读**：`merged_get/edges_of = csr.* + mvcc.is_edge_visible(+PendingGate)`，属性经 `get_by_edge_id` 列快照读。
- **持久化**（`edge_table/persistence.rs`）：四文件 `meta.bin/out_csr.bin/in_csr.bin/properties.bin`，`PageWriter(Zstd) + ColumnFileHeader + shadow` 原子写；`meta` 含 schema JSON + `next_id` + `edge_timestamps` 全量。
- **Compaction**：双向 `compact_with_ts_reporting` + `HashSet` 去重后墓碑提升；`maybe_compact_for_flush(threshold, reserve=0.25)` 按碎片率触发；属性按 `is_edge_visible(bound)` 求活集后 `reclaim_slots`（活行原位不动，无需重映射）。
- **Remap**：顶点压实传播，`iter_all（含墓碑）→ 翻译 → 新建 Variant → 重插 create + 重删`，`max_row+1` 截断行空间。

### 1.4 可见性三副本

权威顺序（`csr_with_properties.rs:10-13` 注释明确）：`edge_timestamps`（`MVCCManager`）> `tombstones`（最早 delete）> `Nbr/RowVisibility` 物理投影。`Nbr.create/delete_ts` 是物理副本，读路径禁止直读，必须经 `mvcc.is_edge_visible`。

## 2. Ladybug（ref/ladybug）设计全貌

核心在 `ref/ladybug/src/storage/table/`，图接入在 `src/graph/`，事务在 `src/transaction/`：

| 文件 | 要点 |
|---|---|
| `table/csr_node_group.h/.cpp` | `CSRNodeGroup{persistentChunkGroup, chunkedGroups(已提交未 checkpoint), csrIndex}`；`NodeCSRIndex{isSequential, rowIndices}`（顺序存 `[start,length]`，否则逐行 `rowIdx`，支持删除打孔）；`CSRNodeGroupScanState` 统一 persistent/committed-inmem/local 三路扫描 |
| `table/csr_chunked_node_group.h/.cpp` | **Packed CSR with gaps + 分层 region**：Header 本质两列 `offset/length(UINT64)`；`CSRRegion{level,left/right,hasUpdates/Insertions/Deletions}`；gap 公式 `ceil(len/0.8)-len`（`PACKED_CSR_DENSITY=0.8`），checkpoint 按 region 密度（0.8→1.0 渐变）重分布，只有 `needCheckpoint()` 的 region 才重写 |
| `table/chunked_node_group.h` | `InMemChunkedNodeGroup{chunks: ColumnChunkData[], capacity}` 内存行式追加 → `ChunkedNodeGroup{chunks: ColumnChunk[]}` 落盘；`NodeGroupDataFormat{REGULAR, CSR}` |
| `table/node_group*.h`, `storage_utils.h` | **按 bound 切分**：`nodeGroupIdx = offset >> LOG2`；Node 表 `REGULAR`，Rel 表强制 `CSR`；`NodeGroupCollection` 分片 |
| `table/rel_table_data.h/.cpp` | **方向化双份**：`RelTable{directedRelData[FWD,BWD]}`，每份 `RelTableData{csrHeaderColumns{offset,length}, columns[]}`；`initCSRHeaderColumns` 建独立 `CSR_OFFSET/CSR_LENGTH` 列；`initPropertyColumns`：`col0=NBR_ID, col1=REL_ID, col2+=properties`，**同 CSR 行号对齐** |
| `table/rel_table.h/.cpp` | `RelTableScanState` 按 bound→`[csrOffset,csrOffset+len)` 寻址；`scanNext` 先 committed 再 uncommitted；`reserveRelOffsets` 全局分配 relOffset；`checkRelMultiplicityConstraint(ONE/MANY)` |
| `local_storage/local_rel_table.h` | **可变性第一层**：未提交写进 `LocalRelTable{localNodeGroup, directedIndices: map<boundOffset,rowIdxVec>}`，commit 时批量搬入 CSR |
| `table/column*.h`, `column_chunk*.h` | 持久 `Column`（分页 `FileHandle+ShadowFile`）/ 内存 `ColumnChunkData`（连续 buffer + nullMask + stats）/ 落盘 `ColumnChunk`（多 segment）；CSR 的 offset/length 也是普通 Column，复用页/压缩/统计路径 |
| `table/version_info.h`, `transaction.h`, `undo_buffer`, `wal`, `shadow_file` | **MVCC**：每 2048 行一 `VectorVersionInfo`，读按 `startTS` 过滤、写按 `txnID` 标记；单写者 MVCC + UndoBuffer 回滚 + WAL + ShadowFile 原子 checkpoint；`checkpointSegment(inPlace/outOfPlace)` |
| `graph/on_disk_graph.h` | 纯扫描适配，无独立邻接结构，拓扑即 `RelTableData` 的 CSR |

一句话：Ladybug 的 CSR 是**带 gap 的 Packed CSR**，可变性分三层（local 未提交 → csrIndex 已提交未 checkpoint → persistent），持久化按 region 增量 checkpoint，属性与拓扑同行号不同列、扫描可裁剪。

## 3. 对比

| 维度 | 当前项目 | Ladybug | 评价 |
|---|---|---|---|
| CSR 形式 | 两级变体：主区三数组 + 固定 4096 溢出块；compact 后瞬时扁平 | Packed CSR with gaps（density 0.8）+ leaf-region 分层 | Ladybug 更优：gap 吸收插入，steady-state 写放大低；当前项目主区满即溢出，高度数点退化为 chunk 链表扫描 |
| 变体数量 | 5 种（Multiple/Single/MultiSingle/Labeled + CsrWithProperties 自带 CSR） | 1 种 CSR + multiplicity 约束（ONE/MANY） | 当前冗余：`Labeled` O(n) 插入移位、`MultiSingle` 定 K 不灵活、`Single` 持久化丢 `create_ts`；Ladybug 用统一 CSR + 约束覆盖 |
| 拓扑/属性关系 | **彻底分离且双重索引**：`Nbr` 无属性指针，`CsrWithProperties` 自带 `offsets/lengths/heads`，与 `out/in_csr` 行空间各自维护 | 分离但**同行号对齐**：`NBR_ID/REL_ID/props` 同 CSR 行，header 双列独立 | 当前是最大结构问题：同一条边的邻接位置被索引两次（topo CSR + prop CSR），双写一致性、remap、compaction 都要双份处理；Ladybug 一份行号多列投影 |
| 切分 | 单段 `EdgeStore`（全图一个 out + 一个 in） | 按 `boundOffset>>LOG2` 切 NodeGroup + Collection | 当前简单但不可扩展：大图单 `Vec<Nbr>` + 全量 compact；Ladybug region 级增量 checkpoint |
| 可变性 | 主区追加 + 溢出块 + 全量 compact（reserve 0.25） | 三层：Local（未提交）→ csrIndex（已提交未 checkpoint）→ Packed（checkpoint 重平衡） | Ladybug 写不碰 persistent CSR，checkpoint 按 region 增量；当前 compact 全量重建，大表停顿 |
| MVCC | 三副本：`edge_timestamps` 权威 + `tombstones` + `Nbr/RowVisibility` 投影；`Single` 覆盖语义依赖上层 ts 单调 | `VersionInfo`（每 vector）+ `UpdateInfo` + UndoBuffer + startTS/commitTS | 当前三副本同步是正确性风险点（回滚路径 `erase/remove/revert` 多分支）；Ladybug 版本信息与数据同 chunk，语义集中 |
| 双向存储 | `out_csr + in_csr` 双写双 compact | `directedRelData[FWD,BWD]` 双份 + `LocalRelTable` 方向投影 | 一致（图数据库常规做法），但当前双写失败回滚是手写多步补偿，Ladybug 走事务 UndoBuffer |
| 持久化 | 四文件自定义 dump（`tag + ver + 全量数组`）+ Zstd PageWriter + shadow | `FileHandle(PageManager) + BufferManager + ColumnReadWriter + 压缩（ALP/bitpack/dict）`，segment 级 `inPlace/outOfPlace` checkpoint | Ladybug 复用列存页路径、可压缩、可增量；当前全量 dump/load，大表启动与 checkpoint 成本高；另有 `total_rows` 校验不一致（CSR 报错 vs 属性仅 warn） |
| 读路径 | 物化 `Vec<EdgeRecord>`（`EdgeTableScanIterator` 构造时全物化，`with_limit` 提前断） | 按 `DEFAULT_VECTOR_CAPACITY` 向量扫描 + 列裁剪（只读 `NBR_ID` 做遍历） | 当前无向量化、无列裁剪，遍历带属性查询会放大 IO；`iter_all` 物化 + `HashSet` 去重亦是内存风险 |
| 辅助结构 | `EdgeDeletionBloomFilter` 独立未接入、`FragmentationStats(zombie 恒 0)` 遗留字段 | calibrator 树高度、密度检查、`CSRIndex` transient 内存索引（TODO 两级压缩） | 双方都有 TODO，但当前的 zombie/bytes_per_edge fallback 表明统计层尚未收敛 |

## 4. 合理性分析

### 4.1 合理的部分（保留）

1. **策略枚举选型**：`Single` O(1) 一对一、`Multiple` 通用多边，方向可独立开关（`validate` 要求至少一方向），符合属性图常见约束，比“全用一种 CSR”省内存。
2. **零度懒分配 + 1.25 顶点扩容**：每行 12B 固定成本、无边不占 `nbr_list`，对稀疏图友好。
3. **溢出块永不拷贝**：避免旧 doubling-copy 产生不可达块，高度数点追加线性，方向正确。
4. **墓碑 + `cutoff==MAX` 不删 + 独占水位 GC**：语义谨慎，`compact` 回调上报墓碑提升，`revert` 限定 `delete<=ts`，时序正确。
5. **四文件 + shadow 原子写 + 版本校验 + 尾随拒绝**：持久化底线意识好，拒绝老多段格式符合“不做向后兼容”的项目约定。

### 4.2 不合理的部分（按严重度排序）

1. **拓扑与属性各持一份 CSR 行索引（最严重）**。`EdgeStore{out_csr, in_csr, properties: CsrWithProperties{offsets,lengths,heads}}` 意味着邻接寻址信息存两遍，insert/delete/remap/compact/GC 全部双份逻辑。Ladybug 同行号多列天然一致。建议收敛为**一份行空间 + 多列投影**（即以 `CsrWithProperties` 的行号为唯一真相，或反向让属性表按 `EdgeId → topo 行` 单映射，去掉第二套 `offsets/lengths/heads`）。
2. **变体族冗余**。`Labeled`（O(n) 全局移位）、`MultiSingle`（建时固定 K）、`Single`（持久化丢 `create_ts`、`load` 置 0，破坏时间旅行）三者都可用“通用 CSR + 约束/索引”替代。`CsrVariant` 分发 + 各变体独立 `dump/load/compact/remap` 是四倍维护成本。建议只保留 `Multiple + Single`，删除或冻结其余两种（项目约定允许不做兼容）。
3. **三副本可见性**。`edge_timestamps` + `tombstones` + `Nbr.delete_ts/RowVisibility` 三处存“同一条边是否活着”，`single_mutable_csr` 覆盖语义还依赖“上层保证 ts 单调”。Ladybug 的 `VersionInfo` 随 chunk 走，单点真相。建议以 `MVCCManager` 为唯一真相，CSR 内只保留 GC 需要的最小投影，并用断言/测试锁住三者一致性；`Single` 的静默 `Conflict` 应显式化。
4. **全量 compact**。`compact_with_ts_reporting` 重建整个方向 CSR，大边表 checkpoint 停顿；平时无 gap（reserve 只在 compact 时预留），写入稍满即溢出，高度数点读退化为多 chunk 跳跃。Ladybug 的 gap（0.8 density）+ region 级增量 checkpoint 是正确方向。建议引入**行级 gap 或 region 级增量回收**，至少先做到“按顶点/按 chunk 增量 compact”而非全表重建。
5. **单段扩展性**。无 NodeGroup 切分，`remap`/`iterator` 全表 `iter_all` 物化 + `HashSet` 去重，顶点压实是 O(E) 重建。数据量稍大即内存/停顿双爆。建议按 bound 区间分片（哪怕先分 2^16 固定片），为增量 checkpoint 铺路。
6. **读路径无向量化/列裁剪**。`EdgeTableScanIterator` 构造时物化 `Vec<EdgeRecord>`（含属性 `Vec<(String, Value)>`），遍历查询也被迫物化属性。Ladybug 按 vector 扫描 + 按 `columnIDs` 裁剪。建议扫描先只读拓扑，属性按需延迟物化。
7. **持久化全量 dump**。`properties.clone().dump()`（`persistence.rs:99` 全克隆后序列化）、`total_rows` 校验双标（CSR 错即失败、属性仅 warn）、`Single` 丢 `create_ts`，三者都是隐患。建议统一校验语义、避免 clone-dump、大表走增量/分页。
8. **统计与辅助结构半成品**。`zombie` 恒 0、`bytes_per_edge` 零时 fallback `sizeof(Nbr)`、`EdgeDeletionBloomFilter` 未接入主路径。建议要么接入（bloom 跳过墓碑查）要么删除，避免“看起来有优化实际无效果”的误导。

### 4.3 Ladybug 可直接借鉴的三件事（不照搬全部）

1. **Header 独立成列 + 同行号多列**：解决双重索引问题，属性扫描可裁剪。
2. **Gap + region 增量 checkpoint**：解决全量 compact 停顿，`needCheckpoint()` 粒度先行。
3. **Local 未提交层 + commit 批量搬入**：替代手写多步物理回滚，把原子性还给事务层（与本项目 `graphdb-transaction` 的对接点）。

## 5. 建议路线（按序）

1. **统一行空间**：去掉 `CsrWithProperties` 的第二套 `offsets/lengths/heads` 或反向合并，`EdgeStore` 只保留一份邻接索引；补一致性测试（topo 行数 == 属性行数，remap/compact/GC 后仍成立）。
2. **收敛变体**：保留 `Multiple/Single`，删除 `Labeled/MultiSingle`（或明确标记实验态、移出主路径）；修复 `Single` 持久化丢失 `create_ts`。
3. **读写分离**：扫描默认只读拓扑，属性延迟物化；`EdgeTableScanIterator` 改流式/向量批量，消除构造时全物化。
4. **增量回收**：先做按顶点 chunk 级 compact + 行级 gap，再做分片（NodeGroup）+ region 级 checkpoint；每步用 `FragmentationStats`（先修掉 zombie 恒 0）量化验证。
5. **持久化收敛**：统一 `total_rows` 校验、消除 `clone().dump()`、大表分页；校验失败一律显式错误而非 warn（fail-fast 符合项目“不用 fallback 掩盖问题”的约定）。
