# 存储设计决策差异分析

## 概述

本文档对比分析 Ladybug（`ref/ladybug`）与当前项目（linkrs）在存储架构上的设计理念差异。这些差异反映了不同的设计目标和权衡，而非简单的实现缺陷。

文中所有事实均以代码为准，标注 `文件:行号`。本版对照 `docs/analysis/vertex_storage_comparison.md` 一并复核，修正了早期版本中若干与代码不符的描述（行组大小、ID 位移位数、向量容量、索引能力、`Nbr` 结构、磁盘格式版本策略等）。

---

## 1. 节点组织：行组 vs 分片

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **结构** | NodeGroup → ChunkedNodeGroup → ColumnChunk → ColumnChunkData 段 | ShardedVertexTable → VertexTable → ColumnStore → Column → ColumnChunk |
| **分组规模** | 行组 2^17 = **131072 行**；组内 chunk **2048 行**；段 ≤ 256KB | chunk **65536 行**；zone map 段 **1024 行** |
| **组织方式** | 固定大小行组（`LBUG_NODE_GROUP_SIZE_LOG2` 默认 17） | 动态分片（默认取 CPU 并行度，上限 256，取 2 的幂） |
| **并发模型** | 组级 mutex + 列 chunk 级 shared_mutex | 分片级 `RwLock<VertexTable>` |

**Ladybug 选择行组**：`offset >> 17` 得组号、`& 0x1FFFF` 得组内行号（`ref/ladybug/src/include/storage/storage_utils.h:58-69`），容量由 CMake 选项决定（`ref/ladybug/CMakeLists.txt:150`）。三级层次（组 131072 / chunk 2048 / 段 256KB）分别承担"并发与检查点单元""版本与向量单元""磁盘分配单元"三种职责。

**当前项目选择分片**：`fxhash(external_id)` 选分片，每分片一把 `RwLock`。分片是**唯一**并发单位，这是与 Ladybug 最实质的差别——Ladybug 的 latch 可以细到单个列 chunk，当前项目最细到整个分片。

**粒度解耦是当前的一个优势**：当前项目把"压缩单元"（65536 行 chunk）与"裁剪单元"（1024 行 zone map 段）拆成两个独立粒度（`vertex/column/chunk.rs:23`、`vertex/column/zone_map.rs:6`），而 Ladybug 复用同一个 chunk 概念兼作两者。

**设计理念差异**：Ladybug 面向读密集型 OLAP 优化批量扫描与细粒度并发；当前项目面向写密集型场景优化并发写入吞吐。

---

## 2. 内部 ID 编码

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **类型** | `internalID_t{offset, tableID}`，两个 uint64，**16 字节** | `u32`，编码为 `(segment << 14) \| slot` |
| **空间** | 16 字节（并非 8 字节） | 4 字节 |
| **是否落盘** | 顶点表**不存**该列，扫描时算出 | 不落盘，但外部 id → 内部 id 的映射表落盘 |
| **限制** | 无槽位概念 | 每段 16384 槽位；`num_shards` 被编进 ID 语义 |

**Ladybug 的 internalID_t**：`nodeID_t` 与 `relID_t` 是同一定义（`ref/ladybug/src/include/common/types/types.h:83-101`），顶点表内没有对应存储列，`_ID` 是合成列，扫描时按 `tableID` 与组起始偏移逐行算出（`src/storage/table/node_table.cpp:217-221`）。ID 稳定且**永不回收**。

**当前项目的编码**：分片 `s` 的第 `i` 段固定为 `s + i * num_shards`（段交错），因此 `shard = (id >> 14) % num_shards` 是纯位运算、无需映射表（`vertex/vertex_table/sharded/routing.rs:19-48`）。这样既保住了 4 字节紧凑性，又避免了"分片在低位"编码导致的稀疏放大。

**代价必须写明**：`num_shards` 参与 ID 语义，`table_manifest.json` 在分片数不匹配时**拒绝打开**（`vertex/vertex_table/sharded/persistence.rs:11,598-627`），改分片等于离线全量重建。

**设计理念差异**：Ladybug 优先考虑寻址简单性；当前项目优先考虑内存紧凑性与分片可直接反解。

---

## 3. 版本管理粒度

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **位置** | 每列 chunk 的 `VectorUpdateInfo` 链 + 每 2048 行一对事务号数组 | 表级 `VertexTimestamp` + 列级 `RowVisibility` + 每列每行 before-image 链 |
| **粒度** | 向量 = **2048 行**（`LBUG_VECTOR_CAPACITY_LOG2` 默认 11） | Layer1 行级；Layer2 行×列级，惰性分配 |
| **插入/删除表示** | `insertedVersions[]`/`deletedVersions[]` 两个 `transaction_t` 数组 + 状态短路枚举 | `start_ts`/`end_ts` 两列 u64（**16 字节/行**） |
| **更新表示** | out-of-place：版本节点自带一份 `ColumnChunkData` | before-image 值链 `VersionEntry{start_ts,end_ts,value}` + 编码上的 `UpdateOverlay` |

**Ladybug**：更新只追加到 `VectorUpdateInfo{version, rowsInVector[2048], prev, next, ColumnChunkData}`（`ref/ladybug/src/include/storage/table/update_info.h:22-48`），只有 `DUMMY` 事务（检查点/恢复）才真正原地写。删除写 `deletedVersions[row]`，并用 `DeletionStatus::NO_DELETED` 短路避免无谓遍历（`src/storage/table/version_info.cpp:14-36`）。

**当前项目**：Layer1 `RowVisibility{create_ts}` 处理事务可见性（`vertex/column/mvcc.rs:31`），Layer2 版本链仅对实际被更新的列分配，历史值可二分查得（`get_at_ts`），由协调器水位驱动 GC（`vertex/gc_manager.rs:21`）。此外编码列上的点写先进 `UpdateOverlay`（`vertex/column/chunk_encoding.rs:23`），由 `UpdateDecision{InPlace,Overlay,OverlayAndRecode}` 三态决策——**这是 Ladybug 和 Neug 都没有的编码感知写入路径**。

> **更正**：早期版本称"磁盘格式从 V1 升级到 V2，V1 向后兼容"。实际 `COLUMNS_FORMAT_VERSION = 1`，且注释明确"与其他任何布局都不保持向后兼容"（`vertex/vertex_table/persistence/encoding_select.rs:1-3`）。同理，`fold_oldest_versions_filtered` 这一符号在代码中不存在。

**设计理念差异**：Ladybug 以吞吐优先，版本批量化；当前项目以时间点查询精度优先，历史值链按需分配。

---

## 4. CSR 实现策略

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **结构** | 持久 CSR（`ChunkedCSRNodeGroup` 的 offset+length 列）+ 内存 CSR 索引 | 主块 + 溢出块两级：`MutableCsr` / `PureCsr` / `ImmutableCsr` 等多变体 |
| **内存布局** | `offsets[v]` → `edges[start..start+len]`；稠密区用 `NodeCSRIndex(start,len)`，稀疏区退化为显式行号向量 | 主块扁平数组 + overflow 块动态扩展 |
| **重组策略** | 密度驱动的区段重打包（`PACKED_CSR_DENSITY=0.8`、`LEAF_HIGH_CSR_DENSITY=1.0`）+ 校准树 | 溢出块 + `OverflowIndex` + 顺序游程检测 + compaction |
| **复杂度** | 中（按密度切换两种表示） | 高 |

**关键点：Ladybug 的 `CSRNodeGroup` 是 `NodeGroup` 的子类**（`ref/ladybug/src/include/storage/table/csr_node_group.h:172`），目的是让边存储复用顶点的组/chunk/版本/检查点机器，而持久格式各不相同。这不是顶点存储的一部分，读码时容易误判。

**当前项目的两级设计**：主块预分配固定槽位、溢出块动态扩展，避免高频增量写入时的整体重分配。代价是实现复杂度与碎片化风险，也因此需要第 7 节的碎片率驱动压缩策略。

**设计理念差异**：Ladybug 追求读取路径的简洁和缓存友好，按密度自适应切换表示；当前项目追求写入路径的高效增量扩展。

---

## 5. 边属性与拓扑的耦合度

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **CSR 条目** | 邻接列 + `edge_id` 列（`NBR_ID_COLUMN_ID=0`、`REL_ID_COLUMN_ID=1`） | `Nbr{endpoint:u32, rank:i64, edge_id, delete_ts}` |
| **属性访问** | 按 CSR 偏移直接访问属性列 | 按 `EdgeId` 索引独立列式属性存储 |
| **拓扑条目是否含属性定位** | 否 | **否** |

> **更正**：早期版本称当前项目的 `Nbr` 携带 `prop_offset`、属"一体化设计"。实际代码相反：`Nbr` 只有 `endpoint / rank / edge_id / delete_ts` 四个字段，结构体文档注释明确写着"Edge properties are stored in a separate columnar store indexed by `EdgeId` — **no `prop_offset` indirection is stored per edge**"（`crates/graphdb-storage/src/edge.rs:610-621`）。

因此**这一维度上两方取向一致**：拓扑条目只承载定位信息，属性延迟到需要时按 id 取。真正的差异在寻址方式——Ladybug 用"CSR 偏移即属性行号"（零额外存储但要求拓扑与属性行一一对应、重排时联动），当前项目用显式 `EdgeId`（多存一个 id，换来边属性存储可独立组织与回收）。

---

## 6. 边属性空间管理

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **策略** | 标记删除 + checkpoint 时重写 | `free_list` 立即回收空闲槽位 |
| **复杂度** | 低 | 中 |
| **空间效率** | 低（删除后空间不立即释放） | 高（空闲槽位复用） |

**Ladybug 的简单策略**：删除仅标记，空间在检查点通过重写释放；`FreeSpaceManager` 回收的是**页**而非行槽。哈希索引里还留有一条未解决的 TODO："We should vacuum the index during checkpoint"（`ref/ladybug/src/storage/index/hash_index.cpp:84`）。

**当前项目**：`csr_with_properties` 维护 `free_list` 复用空闲属性槽位（`crates/graphdb-storage/src/edge/csr_with_properties/persistence.rs:29`，序列化时一并落盘）。

> **更正**：早期版本提到的 `TieredTombstoneManager` 分层墓碑管理器在代码中不存在。实际的墓碑由 CSR 侧的 `delete_ts` 与删除模型承担（`edge/mutable_csr/`），不存在热/冷分层结构。

**设计理念差异**：Ladybug 以实现简单性优先，空间回收延迟到批量操作；当前项目以空间效率优先，实时回收空闲资源。

---

## 7. 索引

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **主键索引** | 内置 **256 分片页式哈希索引** `PrimaryKeyIndex`（`SLOT_CAPACITY_BYTES=256`，自带 OverflowFile） | `IdManager`：`HashMap<IdKey,u32>` + `Vec<Option<IdKey>>` + `BTreeSet` 存活集 + `free_ids` 复用栈 |
| **是否落盘** | 是（可懒加载、可超内存） | 基线 + 增量（`PK_DELTA_ANCHOR_THRESHOLD=8192`），但**运行时结构在内存** |
| **属性二级索引** | **无**：`src/include/storage/index/` 下只有 hash 索引，未发现 ART/B-tree，也未发现 CREATE INDEX 通路 | **有**：`IndexDataManagerImpl`，分片内正/反向 BTreeMap |
| **范围扫描** | 依赖列扫描 | `OrderedCodec` 保序编码 → 直接算字节区间，**免回筛**；支持覆盖索引 |
| **Zone map** | 有，但**一旦该 chunk 存在更新即退化为 ALWAYS_SCAN**（`chunked_node_group.cpp:269-291`） | 有，加宽式 min/max + `ComplexZoneSummary`（长度区间、标量叶区间、64 位键 bloom），陈旧写超阈值后精确重建 |
| **删除时索引处理** | `PrimaryKeyIndex::delete_` 是 **no-op**，靠 `isVisible` 复查 | PK 槽 id 进 `free_ids`，墓碑在时间戳层 |

**当前项目**在顶点二级索引、范围扫描免回筛、覆盖索引、复杂类型 zone map 摘要四点上都强于 Ladybug；Ladybug 的优势是 PK 索引本身可落盘、可超内存。

**设计理念差异**：Ladybug 面向分析型查询优化批量扫描；当前项目兼顾点查询与分析查询。

---

## 8. 磁盘格式与恢复

| 方面 | Ladybug | 当前项目 |
|------|---------|----------|
| **文件组织** | **单一主数据文件** + `.wal` / `.shadow` / `.tmp` 同级文件；库内所有表共享页空间 | 每标签 → 每分片多文件（`meta.bin`/`columns.bin`/`timestamps.bin`/`id_indexer.bin`+`.delta`/`{col}.overflow`/`columns_pages/`） |
| **提交点** | 检查点 + `changeEpoch` 水位（可按表跳过检查点） | **`commit_manifest.json` 唯一提交点**（epoch、Full\|Incremental、文件清单、校验和） |
| **恢复** | 载入 + 重放 WAL；`recover` → `WALReplayer::replay` | 严格 `load` + `apply_delta_pages`，清单缺失文件或损坏页**拒绝打开**；另有离线 `inspect_commit_health` |
| **内存模型** | buffer manager：`MAP_ANONYMOUS` 匿名区、一页一帧、`MADV_DONTNEED` 驱逐 | 顶点主数据常驻堆；mmap 仅用于驱逐快照的只读恢复 |
| **页/压缩** | 4KB 页 + 段级编码 + 页级压缩标记 | zstd 页（`DEFAULT_PAGE_SIZE=64KB-1`，`PGZC` 魔数，逐页 raw/compressed + CRC32） + 列级编码 |

> **更正**：早期版本称 Ladybug"每个表一个文件"。实际其数据文件按数据库而非按表划分（`ref/ladybug/src/include/storage/storage_utils.h:71-82` 只给出 WAL/shadow/tmp 路径），表内容序列化进共享页空间。

> **补充**：Neug 的对照可一并参考——它是"全图 mmap 即主存储"，文件头仅带一个**打开时并不校验**的 MD5（`ref/neug/.../mmap_container.cc:68`），恢复后需强制压缩一次清墓碑。三方的持久化正确性工程以当前项目的提交清单最强。

**内存模型是三方分歧最大的一处**：Neug 可承载超内存数据集但受制于缺页；Ladybug 用 buffer manager 兼顾容量与可控性；当前项目**容量上限即 RAM**，这是"轻量单机部署"定位下最需要正视的约束。

**设计理念差异**：Ladybug 追求文件管理简单与容量弹性；当前项目追求运维粒度、提交原子性与可校验性。

---

## 9. 总结

| 设计维度 | Ladybug 偏向 | 当前项目偏向 |
|----------|-------------|-------------|
| **读写模型** | 读密集（OLAP） | 写密集 + 点查询混合 |
| **并发策略** | 组级/列 chunk 级细粒度 latch | 分片级 latch（唯一并发单位） |
| **ID 空间** | 宽松（16 字节，不落盘） | 紧凑（4 字节，但 `num_shards` 被固化） |
| **版本管理** | 批量粗粒度（2048 行向量） | 分层 + 编码感知的三态写入决策 |
| **CSR 设计** | 按密度切换两种表示 | 主块 + 溢出块动态扩展 |
| **拓扑-属性关系** | 分离（按 CSR 偏移） | 分离（按 `EdgeId`）——**两者一致** |
| **空间回收** | 延迟批量、行号不回收 | 实时回收 + 稳定行号优先、重映射压缩为离线例外 |
| **索引** | 仅落盘哈希（PK） | 内存 PK 哈希 + 二级 BTreeMap + 复杂 zone map |
| **文件粒度** | 单文件 + 页管理 | 多文件 + 提交清单 |
| **容量** | 超内存（buffer manager） | **受限于 RAM** |

这些差异源于不同的应用目标：Ladybug 面向分析型图查询（大表扫描、批量处理），当前项目面向交互式图查询（高并发写入、点查询、时间旅行）。没有绝对优劣，需根据实际工作负载选择。

按"正确性、可靠性、防误用优先"的项目准则，当前真正需要优先偿还的是两项：**容量（顶点主数据与 PK 索引必须装进 RAM）** 与 **并发粒度（分片是唯一 latch 单位，且脏读防护摊派到每条读路径）**。具体改进方向见 `docs/analysis/vertex_storage_comparison.md`。

---

## 10. 复核中发现的一处风险：已定案并修复

### 现象与触发条件

`ColumnEncoding::None` 的序列化只写一个 tag 字节、不写任何数据，而 `append_column_payload` 的"已编码"分支只逐 chunk 写 `encoding` 与 overlay、不写原始基线缓冲，加载端完全按 chunk 记录重建。因此只要出现"列级镜像已编码、但某些 chunk 为 None"的混合布局，这些 chunk 的行就会在重载后消失。

混合布局是可达的，已用受控用例复现：`Double` 列按 chunk 独立选择编码时，`select_for_chunk_profile`（列级投票用）对 Float/Double 无条件给出 `Alp`，而 `apply_encoding_to_chunks` 里的 `select_for_chunk` 会按该片实际值判定——首值为 null、或 ALP 例外率超过 `alp_exception_threshold`（0.25）时返回 `None`。两个选择器语义不同，于是首片 `Alp`、后片 `None` 的布局在正常 flush 路径上就能产生，而列级镜像取首片的编码，整列被判定为"已编码"。

复核过程中排除了一条最初假设：顶点表 `insert` 省略某个属性并不会把该行留成 null，未写入的行槽位读到的是该类型的零值（`Double(0.0)`），所以"NULL 开片"在表层不成立；真正的触发源是 ALP 例外率阈值。

### 三类危害

- 读路径：`Column::get` 在 chunk 层未命中后会落到列级镜像解码，而混合布局下镜像只覆盖首片的值域，落在 raw 片里的行会被按错误方案解码成 `None`。这个问题在内存中即可观察，比持久化更早暴露。
- 持久化：如上所述，raw chunk 的行在重载后不可达。
- `materialize_chunks` 在"列级镜像已编码、该片编码失败"时不回填原始缓冲，而列级编码之后 `inner` 已不再权威（点写只落到镜像编码），于是该片读到的是编码前的陈旧值。

### 修复（不改磁盘格式、无兼容层）

- `Column::get` 的镜像回退改为仅在 `chunks.is_empty()` 时生效：分段存在时未命中行一律读原始缓冲，列级镜像只描述未分段布局。
- `append_column_payload` 的记录形态改为按实际布局判定：先补建分段，再要求"所有 chunk 都已编码"（新增 `Column::all_chunks_encoded()`）才使用"已编码"记录，否则整列走原始 dump。`get_flush_data` 已具备逐行解码混合布局的能力，因此 `COLUMNS_FORMAT_VERSION` 保持 1，无需扩展记录。这同时也让"驱逐提升失败"的列自动退回到保真的原始 dump 路径。
- `materialize_chunks` 在"镜像已编码但该片回落 raw"时，把该片基线值写回原始缓冲，使 raw 片自洽。

### 验证

新增列级用例（混合布局的产生与可读性、镜像下 raw 片的值保真）与一个 flush/reload 端到端用例（`chunk_capacity` 8、前片 ALP 可编码、后片例外率超阈值）。`graphdb-storage` lib 1233 项全通过，`graphdb-query`/`graphdb-transaction`/`graphdb-api` 无回归。

### 遗留观察

列级镜像是以"克隆首片编码"的方式维护的（`apply_encoding_to_chunks` 末尾），对已分段列而言这份克隆是首片编码缓冲的完整副本，属于后续可清理的内存冗余；本次未触碰，因为多个 `encoding_type()` 消费方仍把它当作列级方案读取。
