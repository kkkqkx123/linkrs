# 边表与边属性表对比分析：ref/neug、ref/ladybug 与当前项目

> 范围：三方各自“边表（Edge Table）+ 边属性表（Edge Property Table）”子系统，不含 CSR 邻接拓扑本身的细粒度对比（见 `csr_comparison_neug_ladybug_linkrs.md`）。
> 源码依据：`ref/neug/include+src/neug/storages/graph/edge_table.*`、`storages/csr/generic_view.*`、`utils/property/table.*`；
> `ref/ladybug/src/include/storage/table/rel_table.*`、`rel_table_data.*`、`csr_node_group.*`、`column.*`、`local_storage/local_rel_table.*`；
> `crates/graphdb-storage/src/edge/` 全目录（含 `edge_table/`、`csr_with_properties/`、`node_group/`）。
> 本文档使用中文，代码标识保持英文原文。

## 1. 一览对比

| 维度 | ref/neug | ref/ladybug | 当前项目（linkrs） |
|---|---|---|---|
| 边表在系统中的位置 | 存储层独立构件，`EdgeTable` 直接持有 `out_csr_ + in_csr_ + table_` | 关系表层级，`RelTable → directedRelData[FWD/BWD]`，复用 `Table/NodeGroup/ColumnChunk` 基类 | `EdgeStore` 门面，聚合 `out/in CsrShardSet + MVCCManager + CsrWithProperties + WAL + 二级索引 + owner 映射` |
| 方向组织 | OE/IE 双 CSR 双写，unbundled 下共享同一 `row_id`，bundled 下数据写两份 | FWD/BWD 双 `RelTableData`，各有独立 CSR 头列与属性列，提交时双写 | out/in 双 `CsrShardSet`（按 bound 端点区间分片），`EdgeOwnerMap` 决定 ts/props 分片跟随哪条腿 |
| 属性模型主轴 | 按列数与类型二分：单定长标量 bundled 内联进 NBR，否则 unbundled 存 `row_id` 跳属性 `Table` | 全列存：`NBR_ID/REL_ID` 保留列 + 用户属性列，定位与载荷解耦 | 按记录形态三分：`Pure（无属性）/ Bundled（单标量内联）/ Columnar（列存全功能）`，建表时一次选定并持久化 |
| 变长/字符串 | `varchar` 永不 bundled，只能走 `Table` 的 `TypedColumn<string_view>`（`.data + .items` 双文件） | 字典压缩 + 溢出文件：内存 `StringChunkData{index + dictionary}`，落盘 `StringColumn{data/offset/index}` 三孩子列 + `OverflowFile` | `Column{Fixed/Variable}` + 超阈值 `OverflowStore`，`set_versioned/get_at_ts` 行级版本链，`zone_maps/HLL/stats` 辅助剪枝 |
| 边 ID 体系 | 无全局边 ID；unbundled 用 `table_idx_` 单调 `row_id`，拓扑定位靠 `(vid, offset)`，对侧靠 `fuzzy_search` | 全局 `nextRelOffset` 分配 `relID`，`REL_ID` 列常驻可扫，无全局 `relID → position` 索引 | 表内单调 `next_edge_id: EdgeId`，`INVALID_EDGE_ID` 哨兵 fail-closed；`LiveSetStorage` 做 `(endpoint, rank)` 去重，`AuthorityMap` 做可见性权威 |
| 写暂存 | 无暂存层，`AddEdge/BatchAddEdges` 直接写 CSR + Table | `LocalRelTable` 暂存（本地 `NodeGroup + DirectedCSRIndex`，临时 `relID = MAX_NUM_ROWS + localIdx`）+ `LocalWAL`，提交时 `pushInsertInfo` 落主存；COPY 走分区直写批量路径 | `EdgeStagingBatch{inserts/deletes/order}` + `CommitScratch` 复用缓冲，`commit_staging_batch` 固定顺序多段提交，失败逐段补偿；`PendingGate` 让同事务读到未提交写 |
| 删除/更新语义 | 删除写 `INVALID_TIMESTAMP` 墓碑，`RevertDeleteEdge` 回滚；更新经 `EdgeDataAccessor` 单列写（unbundled 写一次列，bundled 写双腿） | 删除/更新走版本链（`VersionInfo + UpdateInfo + UndoBuffer`），`detachDelete` 一腿主扫另一腿同步 | 删除全匹配语义 + 水位门控 tombstone 复用 + `rollback_insert` 物理擦除 + `revert` 系列；属性 `mark_deleted/revert_deletion_for_edge` 与权威 `record_deletion` 联动 |
| 事务/MVCC | 时间戳内联 `MutableNbr.timestamp`（atomic），读带 `ts` 过滤；`Immutable` 恒可见；`reset_timestamp` 做快照基线归零 | 行级 `VersionInfo` + 列级 `UpdateInfo`，`Transaction{startTS/commitTS}` 可见性过滤 | 时间戳权威外置 `MVCCManager/EdgeTimestamps`，CSR 行 `delete_ts` 只是投影，查询必须走权威判定 |
| 二级索引 | 无 | 无（只有点表主键哈希索引，边表无索引，`findMatchingRow` 线性比对） | 有：`EdgePropertyIndex`（`BestEffort/Strong` 契约）+ `LiveSetStorage` 宽行索引 + `GroupSegmentStats` 剪枝统计 |
| Schema 演进 | 加/删属性触发整表重建（`dropAndCreateNewBundled/UnbundledCSR`，经 `batch_export` 导出重灌，仅 Mutable 可走该路径） | 表层 `addColumn` 等操作 | 三段式列增删改名（`prepare → fill → durable → publish/abort`）+ 记录形态迁移（离线 `migrate_record_form` / 在表 `switch_record_form_online`，WAL fence） |
| 持久化 | mmap 主存：每表 `oe_/ie_` 前缀 `.deg/.nbr/.cap/.meta` + `e_..._data.col_i` + `statistics_` 文件 | 单分页数据文件 + 影子分页 + WAL，`DatabaseHeader` 占 page0，列按段存储 | 分组基文件（`out/in_g{gid}.bin`）+ `.append` 增量 sidecar + `ts_g/props_g` + `segment_stats` + `meta.bin（含 manifest tail）` + `groups_manifest.bin` + `edge_wal.bin`，全分组文件尾部 CRC32，frozen 另有 mmap serving sidecar |
| 校验与恢复 | `FileHeader` 16B MD5 但 open 时跳过校验；WAL 在 CSR 范围外 | 影子页原子切换 + `LocalWAL` + `WALReplayer`，失败可 `rollbackCheckpoint` | CRC32 先验后解析 + trailing bytes 拒绝 + WAL 幂等重放；torn tail 直接 fail load，需离线显式修复 |

## 2. ref/neug 边表与边属性表

### 2.1 架构

`EdgeTable` 字段精简到三件套：`out_csr_ + in_csr_: unique_ptr<CsrBase>` 双腿拓扑，`table_: unique_ptr<Table>` 属性表，外加 `meta_: EdgeSchema`、`table_idx_/capacity_` 计数器与 `csr_alter_version_` DDL 版本。`work_dir_ + MemoryLevel` 决定 mmap 形态。上层 `PropertyGraph::AddEdge/DeleteEdge/UpdateEdgeProperty/BatchAddEdges/Compact` 均为透传。

创建分发 `create_csr` 按 `EdgeStrategy(kNone/kSingle/kMultiple)` 与 `oe/ie_mutable` 正交选择五种 CSR 变体，再按 `DataTypeId` switch 实例化 11 种 `EDATA_T`。边 schema 的 `oe/ie_strategy + oe/ie_mutable` 在建表时一次决定。

### 2.2 属性模型

`is_bundled` 规则只有三行语义：零属性为真，单非 string 属性为真，多属性或单 `varchar` 为假。bundled 时 NBR 条目为 `MutableNbr<T>{neighbor, timestamp, data:T}`，`data` 即属性值，`Table` 为空；unbundled 时 NBR 条目为 `CSR<uint64_t>`，`data` 是 `row_id`，属性存在 `Table{columns_: vector<shared_ptr<ColumnBase>>}` 中，`table_idx_` 单调分配，`capacity_` 按 `max(4096, size * 1.2)` 增长。

`varchar` 永不 bundled 的原因是定长 stride 约束：`MutableNbr` 要求 `sizeof(nbr_t)` 固定步长以支撑 `memcpy/batch_export` 定步迭代，变长无法满足。`EdgeDataAccessor` 的 bundled 路径同样只覆盖非 string 类型。

### 2.3 读写路径

单条 `AddEdge` 直接双写两腿 CSR（`put_edge` 加顶点自旋锁，`cap += cap/2` 扩容，`timestamp = ts`），unbundled 另做 `table_->insert(row, props)`。批量 `BatchAddEdges` 先 `resize` 对齐顶点数、`filterInvalidEdges` 跳过非法边、`EnsureCapacity` 预留，再按 bundled/unbundled 分发到 typed 批量实现。删除 `DeleteEdge(src, dst, oe_offset, ie_offset, ts)` 双调 CSR 删除（`old_ts <= ts` 则写 `INVALID` 墓碑）。删点 `DeleteVertex` 遍历本侧可见边，由 `nbr_ptr` 算出本侧 offset，再经 `search_other_offset` 求对侧 offset 后双删。

对侧定位是该设计最脆弱的一环：`fuzzy_search` 先按 `neighbor` 收集候选，单候选直接返回，多候选再按 `data + timestamp` 精匹配。执行层 `EdgeRecord` 携带的 `prop` 指针若落在 `[start, end)` 范围内才有指针算术快路径，否则退化为线性扫描。

更新 `UpdateEdgeProperty` 经 `EdgeDataAccessor` 单列写：unbundled 下双 CSR 共享 `row_id` 只需一次列写，bundled 下需写双腿。`Compact` 做压紧加可选排序加时间戳归零。

### 2.4 评价

优点是简单直接：字段少、无暂存层、无外部索引、无形态迁移状态机，单条与批量路径泾渭分明，unbundled 共享 `row_id` 使更新写放大最小。缺点是能力天花板低：属性模型只能表达零或单定长标量，多属性即退到行式 `Table`；`varchar` 双文件但无字典去重；无谓词下推与列级剪枝；对侧定位在高度数顶点下是线性扫描；DDL 形态切换是全表停写重建且仅 Mutable 可走 `batch_export`；持久化 MD5 写而不验。

## 3. ref/ladybug 边表与边属性表

### 3.1 架构

`RelTable` 是 `Table` 子类，逻辑上一个边组（`RelGroupCatalogEntry`）按 `(from, to)` 对拆分为多个物理 `RelTable`。每个 `RelTable` 持有 `directedRelData[FWD/BWD]` 向量，方向由 catalog 配置（`BOTH/FWD/BWD`）决定，缺方向访问直接抛错。全局 `nextRelOffset` 是边 ID 分配器，`relOffsetMtx` 保护。

每个 `RelTableData` 独立持有 `nodeGroups: NodeGroupCollection`（按 bound node 分组，每组一个 `CSRNodeGroup`）、`csrHeaderColumns{offset, length}`（`UINT64` 双列）与 `columns[NBR_ID, REL_ID, props...]`。`NBR_ID` 只存对端内部 offset，`REL_ID` 存全局边 ID，用户属性列 ID 从 2 开始。CSR 头只解决定位，区间内各属性仍是普通列存行区间，谓词下推走 `ChunkState/columnPredicateSets`。

### 3.2 属性模型

全列存是其最大特征：拓扑列与属性列行对齐，按 CSR 顺序排列。内存态为 `ChunkedNodeGroup{chunks, VersionInfo}`，每列 `ColumnChunk{segments, UpdateInfo}`；落盘为 `Column` 分页经 `FileHandle + BufferManager` 读写。`STRING` 有完整字典路径（内存 `index + dictionary` 去重，落盘 `data/offset/index` 三孩子列加 `OverflowFile`），`List/Struct` 为多孩子列，`Bool` 有 bitpack，浮点有 ALP 例外块。压缩与变长处理是三方中最成熟的一个。

### 3.3 读写路径

单条写经 `LocalRelTable` 暂存（本地 `NodeGroup + DirectedCSRIndex` 双索引，临时 relID 偏置），提交时回填全局 relID 并按 `boundOffset` 路由到目标 `NodeGroup::append`。更新与删除先经 `findMatchingRow`（按 `relID` 列线性比对该 bound 节点全 list）定位 `(source, rowIdx)`，再按 persistent/in-memory 双 handler 分发，并写 `UndoBuffer`。扫描是 2048 行向量化状态机（`COMMITTED_PERSISTENT → COMMITTED_IN_MEMORY → UNCOMMITTED`），多 bound 节点走 `WithCache` 批量预取。COPY 批量路径按 bound 分区并行，直接构造 `InMemChunkedCSRHeader + ChunkedCSRNodeGroup`，空持久组可直接落盘为 persistent。

MVCC 为行级 `VersionInfo` 加列级 `UpdateInfo`，`Transaction{startTS/commitTS}` 过滤。Checkpoint 是 Packed-CSR 增量合并：读旧 header，按 1024 bound node 的叶 region 收集变更，按密度界合并 region，左锚定右对齐重排 offset，逐列逐 region 经 `LazySegmentScanner + CheckpointRead/WriteCursor` 合并，无变更 segment 可 `canSkipWrite` 跳过。

### 3.4 评价

优点是分析型扫描能力强：定位与载荷解耦使前向扩展只读 `NBR_ID/REL_ID`，属性投影才扫对应列；向量化加批量预取对大扇出友好；增量 checkpoint 可跳过未变列与 segment；变长与嵌套类型支持完整。缺点是 OLTP 点写路径重：点更新有点查二次定位开销（`O(度)` 线性比对）；内存态 `CSRIndex` 为每节点一个 `vector`，稀疏关系浪费大（头文件自带 TODO）；`REL_ID` 列常驻可扫是固定成本；同一 nodeGroup 内 persistent/transient 双态并存使扫描与 checkpoint 状态机复杂。

## 4. 当前项目边表与边属性表

### 4.1 架构

`EdgeStore` 是三方中聚合度最高的门面：双腿分片拓扑（`out_csr/in_csr: CsrShardSet`，按 `group_bits` 区间路由的 `BTreeMap` 稀疏组）加 `MVCCManager`（`EdgeTimestamps` 权威）加 `CsrWithProperties` 属性库加 WAL 加 checkpoint 加 `EdgePropertyIndex` 加 `EdgeOwnerMap`。CSR 只存拓扑加行投影戳，时间戳权威与属性列存完全外置。`edge_table/` 下按职责拆出约二十个模块（`core/{store,owner,reads,writes,index,query,schema_ops,maintenance,recovery}` 加 `staging/iterator/freeze/wal/checkpoint/compaction/remap/mvcc/record_form/stats/config`），是三方中模块切分最细的一个。

### 4.2 属性模型

记录形态三选一，建表时 `Auto` 选择器按 schema 推断并持久化于 `meta.bin`，此后永不重推断：空属性走 `Pure`（12B/边纯拓扑），单可编码标量走 `Bundled`（20B/边拓扑加内联 `u64`），其余走 `Columnar`（拓扑 32B 加 `CsrWithProperties` 列存）。`Pure/Bundled` 表用 `inline_stub` 占位，使一切行读写 loud-fail，防止在 CSR 值列之外 fork 第二属性真相。

`CsrWithProperties` 以 `edge_id → row` 稀疏分段映射（1024/段，`UNMAPPPED_ROW` 哨兵）关联拓扑与属性行，`next_prop_id` 永不复用。`Column` 按类型选定宽或变宽实现，大 string 超阈值溢出到 `OverflowStore`，行级 `set_versioned/get_at_ts/gc_versions` 提供 checkpoint epoch 内的时间旅行，`zone_maps/HLL/stats/encoding` 支撑剪枝与自适应编码。读路径谓词下推只读谓词列加 null-bitmap，NULL 永不匹配。

### 4.3 读写路径

写为暂存加多段提交：`stage_insert/stage_delete` 先组装 `EdgeStagingBatch`，`commit_staging_batch` 按固定顺序（WAL 先行 append 加 fsync → 按 src/dst 计数预扩 → 按序应用插入与删除 → 二级索引 → owner 映射）提交，同 batch 内 `insert → delete` 同 key 直接擦除已应用插入而不留 tombstone，失败仅回滚本 batch 已应用前缀。`PendingGate` 让同事务读到外来未提交写。CSR 层插入经 live-set 去重、主块 gap 填充、水位门控 tombstone 复用（`TOMBSTONE_REUSE_SCAN_BOUND = 64` 有界回退）、溢出追加四级路径。

删除为全匹配语义：一次删同 key 所有 live 行，双腿任一失败按 position 或 id 回滚另一腿以保一致，成功后权威记 `record_deletion`（最早 `delete_ts` 获胜）并标属性删除。`rollback_insert` 做物理擦除无痕迹，`revert` 系列处理事务 undo。读有三件套行访问（`visit_physical` 借用回调首选、`fill_physical_into` 复用 buffer、`physical_edges_of` 仅测试离线）与生产遍历 `visit_hot`（只碰拓扑行减缓存污染），projected 查询按 `edge_id` 取列（columnar）或 `decode_scalar`（bundled），`merged_get_edge` 处理删后重建同 key 双 generation。扫描迭代器先整组 `segment_may_contain` 剪枝再逐行 `matches_pushdown` 过滤最后投影解码。

持久化以组为单位：组基文件加 `.append` 增量 sidecar 加 `ts_g/props_g` 加 `segment_stats` 先写 shadow，原子发布 `meta.bin（含 manifest tail）` 再发布 `groups_manifest.bin` 为提交点，仅脏组重写。`edge_wal.bin` 为 length-prefixed `postcard`，重放幂等，torn tail 直接 fail load。freeze 将组打包为行内全排序只读 CSR 并附 mmap serving sidecar。Schema 演进有列增删改名三段式状态机与记录形态离线/在线双路径迁移（切换前 WAL redo 被 fence）。

### 4.4 评价

优点是形态适配与运维确定性：`Pure 12B / Bundled 20B / Columnar 32B 加列存` 按 schema 选形，小边表不为大而全的通用路径买单；CRC32 全覆盖加 trailing bytes 拒绝加 fail-closed 变体隔离；WAL fence 与 manifest tail 使提交点清晰；mmap 只做只读 serving sidecar 而非主存，坏缓存可丢弃重建；维护探针（碎片率、删除统计、剪枝报告）与背压/自动维护是两 refs 都没有的运维面。

缺点是复杂性代价：门面字段与模块数是三方之最，`edge_id → 权威/属性` 二次查是每次可见性判定与投影的固定成本；`AuthorityMap/EdgeOwnerMap` 稠密 `Vec` 下标在大稀疏 vid 空间下有内存压力；`Bundled` 不可 freeze、无 rank、无 MVCC、单列限制使其更像特化快路径而非一等形态；frozen 单行 GC 仍是全表重建；高度数行仍有溢出链追逐。

## 5. 架构差异

1. **分层位置不同**。neug 的边表是存储层独立构件（CSR 加可选属性 `Table` 即全部）；ladybug 的边表是关系表内部的一层（定位与载荷分离，复用列存与版本链基类）；当前项目是分片拓扑加外部权威加属性库的聚合门面，CSR 只存拓扑。
2. **方向组织不同**。三者都是双写，但共享程度递减：neug unbundled 双腿共享同一 `row_id`（更新只写一次列）；ladybug 双 `RelTableData` 完全独立（各有 CSR 头与属性列）；当前项目双 `CsrShardSet` 独立分片，靠 `EdgeOwnerMap` 让 ts 与 props 跟随 owner 腿，bundled 值随 `edge_id` 携带。
3. **暂存层不同**。neug 无暂存，直接写 CSR；ladybug 有 `LocalRelTable + LocalWAL` 暂存，提交时回填全局 relID；当前项目有 `EdgeStagingBatch + CommitScratch` 暂存，提交时做 net-effect 合并（同 key 插入后删除直接擦除）。
4. **分片粒度不同**。neug 整表为单位；ladybug 按 131072 bound node 的 nodeGroup 分区；当前项目按可配 `group_bits` 区间分片加组内行级管理，稀疏组用 `BTreeMap` 跳过。
5. **mmap 角色不同**。neug 的 mmap 是主存储；ladybug 无 mmap（页式加影子分页）；当前项目堆内 CSR 非 mmap，只有 frozen serving sidecar 用 mmap 做只读加速。

## 6. 功能差异

| 功能 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 属性类型覆盖 | 11 种定长 `EDATA_T`，`varchar` 只能走 `Table`，无嵌套类型 | 全类型列存，string 字典加溢出，`List/Struct` 多孩子列，`Bool` bitpack，浮点 ALP | Columnar 全类型，`Fixed/Variable` 分派，大 string 溢出，`postcard` 编解码属性值；Pure/Bundled 只覆盖无属性与单标量子集 |
| 谓词下推 | 无 | `ChunkState/columnPredicateSets` 列级下推 | `matches_pushdown/filter_edge_ids` 只读谓词列加 null-bitmap，不物化中间记录；另有整组 `segment_may_contain` 剪枝 |
| 点查/点写定位 | `(vid, offset)` 直写，对侧 `fuzzy_search` 线性 | `findMatchingRow` 按 `REL_ID` 线扫该 bound 节点全 list | 按 id 经 live-set 定位，位置型删除期望 id 重验防 stale；`delete_by_dst` 全匹配 |
| 删点级联 | `DeleteVertex/BatchDeleteVertices` 遍历本侧逐条对侧定位双删 | `detachDelete` 一腿主扫另一腿同步删 | `remap/reclaim/freeze` 路径覆盖，批次删点需走多次单边删除，无 `BatchDeleteVertices` 式专用批量级联 |
| 事务回滚 | `undo_log{InsertEdge→DeleteEdge, RemoveEdge→RevertDeleteEdge, UpdateEdgeProp→旧值}` | `commitInsert/commitDelete/rollbackInsert/rollbackDelete` 经 handler 加 `UndoBuffer` | batch 内前缀回滚（先 revert deletes 后 erase inserts）+ `rollback_insert` 物理擦除 + `revert_delete_by_edge_id` |
| 二级索引 | 无 | 无 | `EdgePropertyIndex`（`BestEffort` 计数失败，`Strong` 失败回滚写） |
| Schema 变更 | 整表停写重建（仅 Mutable 可走 `batch_export`） | 表层列操作 | 列增删改名三段式状态机 + 记录形态离线/在线迁移 |
| 压缩 | 无（`EmptyType` union 除外） | string 字典、`Bool` bitpack、ALP 浮点；CSR 头双列固定 `UINT64` 不压缩 | 拓扑列 `Plain/BitPacked/Rle` 按列选优 + 属性自适应编码 + 全文件 CRC32 |
| 只读 serving | 无 | persistent 即服务态 | `pack_from_mutable` 打包 + mmap sidecar + `open_or_rebuild` |
| 完整性 | MD5 写而不验 | 影子页原子切换 | CRC32 先验后解析 + torn tail fail-closed（需离线显式修复） |

## 7. 性能差异

| 方面 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 稳态单条插入 | 直接双写 CSR，`put_edge` 均摊 `O(1)` | `O(1)` 本地暂存加索引更新，提交时批量落主存 | 暂存组装 + WAL fsync + 预扩 + 双腿写 + 索引 + owner，单条延迟高于前两者，批量可摊销 |
| 批量插入 | `O(V+E)` 全量搬迁，大批量昂贵 | 按 group 分区并行，`O(边数)` 计数加 `O(节点加边)` 排 offset，但预分配全 null 峰值高 | `reserve_for_batch + batch_put_edges` 预留路径 + 按组并行，但无 COPY 式分区直写 persistent 的专用快路径 |
| 点更新 | 单列写，unbundled 一次列写最优 | 版本链写加 undo，二次定位开销 | `Column::set_versioned`（before-image 保留）加权威时间戳，写放大高于 neug，epoch 内时间旅行是补偿收益 |
| 按值删除 | 建 `map<vid,set>` 索引加每顶点线性扫，对侧线性 | 先二次定位再版本链删除 | live-set 定位加双腿一致性回滚，单次删除做多次映射与权威查表 |
| 前向扫描 | `O(deg)` 连续，零分配 `NbrList` range-for | 2048 行向量化加跨 bound 批量预取，大扇出最优 | `visit_hot` 只扫拓扑行，批量 `fill_physical_into` 复用 buffer；溢出链有指针追逐 |
| 属性投影 | bundled 零跳，unbundled 一次 `row_id` 跳表 | 定位后按需扫对应列，扩展只读保留列 | 每次投影一次 `edge_id → row → Column` 跳表加一次权威可见性判定 |
| 空间 | `EmptyType` 4B/8B 每条最省；Mutable 落盘含 `cap` 空隙；`adj` 指针 8B/顶点常驻 | 磁盘每组每方向 header 约 2MB 常驻；内存 `CSRIndex` 稀疏浪费大；gap 预留 `len/0.8 - len` | Pure 12B / Bundled 20B / Columnar 32B 加列存；零度行零成本加 1024 分段稀疏索引；但 `AuthorityMap/EdgeOwnerMap` 稠密 `Vec` 在稀疏大 vid 下仍有压力，`Single` 空顶点固定 32B |

## 8. 优缺点小结

- **neug**：优点是简单、写路径短、unbundled 更新写放大最小；缺点是属性模型表达力弱、无谓词下推与剪枝、无边索引、对侧定位与批量搬迁在大图下退化、DDL 靠停写重建、校验写而不验。
- **ladybug**：优点是列存分析扫描成熟（向量化、批量预取、按列跳过、字典与嵌套类型、增量 checkpoint 跳过未变 segment）；缺点是 OLTP 点写重（暂存加提交加二次定位加版本链）、内存倒排索引稀疏浪费、无边二级索引、双态并存状态机复杂。
- **当前项目**：优点是形态适配（小边表不为通用路径买单）、运维确定性（CRC 全覆盖、WAL fence、提交点清晰、维护探针与背压）、读路径形状多样（借用回调、复用 buffer、热遍历）与在线形态迁移；缺点是门面复杂、每次读写的二次查表固定成本、稠密权威与 owner 映射在大稀疏空间下的内存压力、Bundled 形态能力受限、torn tail 需离线修复的操作负担。

## 9. 当前项目需要的改进

以下按优先级排序，每条只给方向，不规定具体实现。

1. **补齐列级脏跟踪的增量 checkpoint**。当前仅到组级脏跳过，组内任一列变更即重写整组基文件。可借鉴 ladybug 按 segment 跳过未变列的思想，把脏标记下沉到列级，未变列复用旧基文件段。
2. **收敛 `Bundled` 与 `Columnar` 的能力断层**。当前 `Bundled` 不可 freeze、无 rank、无 MVCC、仅单列，超限即要求回迁 `Columnar`。短期应明确该形态仅为稳定单数值属性的快路径并在建表选择器中收紧准入；长期应评估冻结态 bundled 或单列 columnar 是否可替代该变体，减少形态分叉。
3. **降低权威与属性二次查表的读放大**。当前每次可见性判定与投影各至少一次 `edge_id` 跳表。可对只读扫描引入行级可见性投影缓存（一次判定多次复用），对纯拓扑扫描保持只碰热行的现状，对带谓词投影优先复用已判定的可见行集。
4. **稀疏化稠密 owner 与权威映射**。`EdgeOwnerMap` 与 `AuthorityMap` 为稠密下标，在大稀疏 vid 或边 ID 空洞下内存与重建成本高。可将其改为与属性映射同构的 1024 分段稀疏结构，未触碰段为 `None`，并复用同一截尾逻辑。
5. **给 `Single` 空槽瘦身**。当前每顶点固定 32B 槽，空顶点也占槽，稀疏一对一关系浪费明显。可引入位图或稀疏段表示存在性，热路径仍保持 `O(1)` 槽寻址。
6. **补批量删点级联专用路径**。当前无 `BatchDeleteVertices` 式接口，级联需多次单边删除并重复做对侧定位。可增加按组批量收集边 ID 后双腿批量删除的路径，复用已有批量预留与前缀回滚逻辑。
7. **补 COPY 式分区直写快路径**。当前批量经暂存加 WAL 加逐条应用，超大初始导入的峰值与放大高于 ladybug 按 group 分区直写 persistent 的路径。可为空表或新组增加跳过暂存的直接打包路径，提交点复用现有 manifest tail 协议。
8. **优化高度数行溢出链**。主块 gap 加分级溢出在稳态下有效，但高度数行仍有指针追逐。可评估高度数行独占连续段或打包后只读化的阈值策略，与 frozen 打包形成互补。
9. **frozen 单行 GC 去 `O(table)` 重建**。当前 `compact_row` 重建全表 offsets，单行回收即全表代价。可改为标记加延迟合并，或限制 frozen 组只做整组解冻后回收。
10. **torn tail 从 fail-closed 转向可操作恢复**。当前 torn tail 直接拒绝加载，需离线显式修复，对运维不友好。可在保持默认拒绝的前提下，提供只读诊断模式输出末有效 entry 位置与可抢救条数，修复动作仍由显式命令触发。
11. **二级索引一致性可观测性**。`BestEffort` 下写失败仅计数，若长期滞后不易察觉。可为索引增加滞后水位指标与后台重建任务，`Strong` 模式保留给必须同步的场景。
12. **点写延迟的基准对照**。当前单条提交含 WAL fsync、多段应用、索引与 owner 维护，延迟天然高于 neug 直接双写与 ladybug 本地暂存。在加顶点级锁或其它并发优化前，应先跑同等硬件下的单条插入与批量导入对照基准，再决定是否引入调用方并行或分片级并行写。
