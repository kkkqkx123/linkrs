# 边属性表实现对比分析：ref/neug、ref/ladybug 与当前项目

> 范围：三方各自"边属性存储"子系统的架构、功能、性能差异，不含 CSR 邻接结构本身（见 `docs/analysis/csr_comparison_neug_ladybug_linkrs.md`）。
> 源码依据：`ref/neug/include+src/neug/storages/graph/edge_table.*`、`ref/neug/include/neug/storages/csr/{nbr,generic_view}*`、`ref/neug/include+src/neug/utils/property/{table,column}*`；
> `ref/ladybug/src/include/storage/table/{rel_table,rel_table_data,csr_node_group,csr_chunked_node_group,column_chunk*,column,chunked_node_group}*`；
> `crates/graphdb-storage/src/edge/{csr_with_properties*,bundled_csr*,pure_csr*,property_schema,edge_table/*}`。
> 本文档使用中文，代码标识保持英文原文。只收录对比分析结论，不收录针对当前项目的局部修改建议。

## 1. 一览对比

| 维度 | ref/neug | ref/ladybug | 当前项目（linkrs） |
|---|---|---|---|
| 属性存放位置 | 二选一：bundled 内联于 CSR 邻居条目，或 unbundled 存独立 `Table`，`NBR.data` 存 `row_id` | 全列存：每个方向的 `RelTableData` 持有 `NBR_ID/REL_ID` 保留列 + 属性列，行区间即 CSR 定位区间 | 三形态：`Pure`（无属性）、`Bundled`（单标量内联于 CSR 值列）、`Columnar`（独立 `CsrWithProperties`，按 `edge_id` 经分段映射寻址） |
| 内联准入 | 恰好 1 列且非 varchar（`EdgeSchema::is_bundled`），varchar 永不内联 | 无内联概念 | 恰好 1 列、可编码标量（9 种）、双向皆非 Single（`is_bundled_eligible`）；Single 方向强制 Columnar |
| 外置表形态 | `Table`：每列独立 `TypedColumn/StringColumn` 向量，`table_idx` 递增分配 `row_id` | `ChunkedNodeGroup`：每列 `ColumnChunk` 分段（`ColumnChunkData`），persistent（落盘有序）与 transient（内存追加 + `CSRIndex` 倒排）双态并存 | `CsrWithProperties`：`Vec<Column>` 复用顶点列存，`edge_map_segments`（1024/段稀疏）+ `row_to_edge` 反向索引 + `free_list` 复用，无 per-vertex 偏移 |
| 可见性 | 时间戳内联于 CSR 条目（`MutableNbr.timestamp`，原子字段），读视图按 `read_ts` 过滤；`Table` 本身无版本 | 行级 `VersionInfo`（删）+ 列级 `UpdateInfo` 版本链（改），随行存储，读写时合并 | 版本权威外置于 `MVCCManager/EdgeTimestamps`；CSR 行戳与列存 `RowVisibility` 只是物理投影，仅供 GC/整理，不决定查询可见性 |
| 更新语义 | bundled 改双边条目，unbundled 只改共享 `Table` 行；原值丢失，回滚靠事务 `Undo` | `UpdateInfo` 挂 `VectorUpdateInfo` 链，读时合并，未提交读走同一链 | `Column::set_versioned` 写版本链（内存态），checkpoint 后坍缩；Bundled 直接覆写值字，无版本链 |
| 删除语义 | CSR 写墓碑时间戳，`Table` 行不回收（`PropTableSize` 自认删后不准） | 版本链删除，分 persistent/in-mem 双 handler，checkpoint 时物理过滤 | 双腿 CSR 墓碑 + 权威 `delete_ts` + 列存 `mark_deleted`；`rollback_insert` 物理擦除无墓碑；`revert` 系列可复活 |
| 谓词下推 | 无（`get_data` 逐条经 `EdgeDataAccessor` 跳表） | zone-map 整组跳过（`getZoneMapResult`）+ 2048 行向量化 + 仅物化请求列 | 列级 `prune_bounds` 短路 + 逐行 `pushdown_cell` 过滤（经 null 位图与版本链），不物化中间记录 |
| 字符串/变长 | `StringColumn`（items + data 双容器 + 原子 pos，截断至上限） | `StringChunkData`、字典列、变长段 | `Column` 的 Fixed/Variable 形态 + dictionary/FSST/ALP/RLE/bitpacking 编码族 |
| 双向存储 | `EdgeTable` 恒持 out/in 双 CSR 双写；bundled 两份数据，unbundled 共享同一 `row_id` | FWD/BWD 各为独立 `RelTableData`（可配 BOTH/FWD/BWD），属性各存一份 | Columnar 双腿共享单份属性（按 `edge_id` 一份行）；Bundled 双腿各存一份值（故意不跨腿跳读） |
| 持久化 | CSR `.nbr/.deg/.cap/.meta` 多文件 + `Table` 按列文件（varchar 再分 `.data/.items/.pos`）+ 统计文件；mmap 容器按内存级别选择 | 影子分页 + WAL + 按 region 增量 checkpoint（Packed-CSR 合并，`canSkipWrite` 跳过未变 segment） | 分组基文件（列编码 + CRC32）+ `edge_wal.bin` + 脏组增量 checkpoint；frozen 组另有 mmap 只读 sidecar；全部分组文件尾部 CRC 校验 |
| Schema 变更 | 加列：bundled 空表可迁，unbundled 直接 `add_columns`；删至 0/1 列时触发 bundled↔unbundled 全量重建（仅 Mutable 可走 `batch_export`） | 表层 `addColumn` 等列操作，存储按列 checkpoint | Columnar 支持列增删改名状态机（`prop_id` 稳定永不复用）；Pure/Bundled 拒绝在线改列，须经记录形态迁移（离线重建 / 在线原子切换 + WAL 围栏） |

## 2. ref/neug 边属性表

判定函数决定一切：边 schema 为空则无表；恰好一列非 varchar 则整表 bundled，邻居条目模板参数 `EDATA_T` 即该属性类型，`table_` 为空；其他情况 `EDATA_T=uint64_t`，条目内存放 `row_id`，另建 N 列 `Table`。`varchar` 被排除的原因是结构性的：变长需要 items/data 双容器加原子分配位，无法装入定长 `stride` 的邻居槽位，也不满足 `batch_sort_by_edge_data` 对定长可比的要求。

读路径为零拷贝值对象：`get_generic_view(ts)` 按条目内时间戳偏移做 MVCC 过滤得到邻居迭代器，再经 `EdgeDataAccessor` 分叉——bundled 直接解条目内 `data`，unbundled 用 `row_id` 调 `ColumnBase::get_prop` 跳表。`EdgeDataAccessor` 以 `data_column_ == nullptr` 区分两种形态，调用方无感。

写路径按批量/单条分流：bundled 批量走 `TypedCsr::batch_put_edges` 全量重建，unbundled 批量先 `table_idx_.fetch_add` 预占 `row_id` 再逐行 `table_->insert`；单条 `AddEdge` 同理。属性更新经迭代器 `+= offset` 定位后 `set_data`：bundled 写双边条目并刷新时间戳，unbundled 只写 `Table` 共享行。删除只改 CSR 时间戳为墓碑，`Table` 不动，`Compact` 只整理 CSR 并重置时间戳，不回收 `Table` 行。

落盘时 CSR 与 `Table` 各自 dump，统计文件记录容量与 `table_idx`，`Open` 时断言两者一致。DDL 集中在 `property_graph.cc` 与 `edge_table.cc` 的 `Add/DeleteProperties`：向 bundled 表加第二列、从 unbundled 表删到剩一列等跨形态转换，统一走"建新 CSR（`_v_{alter_version}` 临时目录）→ `batch_export` 导出三元组 → 按默认值或旧列值重填 → 删旧表换新表"流程。

## 3. ref/ladybug 边属性表

层次为 `RelTable → directedRelData[FWD|BWD] → NodeGroupCollection → CSRNodeGroup`。核心设计是定位与载荷统一：CSR 只解决"bound node → 行区间"，区间内的每一行就是完整的属性行，各属性是普通列存向量，谓词下推与投影天然走列路径。`NBR_ID` 列存邻表内部 id，`REL_ID` 列存全局边偏移，是点更新定位与双向删除枚举的唯一依据。

写入分三层缓冲：事务先写 `LocalRelTable`（单 nodeGroup + `DirectedCSRIndex`），提交时按 nodeGroup 搬运到已提交态；已提交态内新 group 直接落盘为 persistent，已存在 group 走内存追加并维护 `csrIndex`。`csrIndex` 按 bound node 存 `[startRow, length]`（顺序追加）或逐行 `rowIdx` 排序表（乱序后），头文件自认稀疏关系下空间效率差。

更新与删除按扫描源分发：persistent 行走 `persistentChunkGroup` 的版本接口，内存行按行号商余定位到 `chunkedGroups`。删除是懒标记（`VersionInfo::delete_`），修改是向列段挂 `VectorUpdateInfo` 节点链并入事务 undo；读（`scan/lookup/scanCommitted`）一律是"基值 + 版本链合并"。zone-map 在 `ChunkedNodeGroup::scan` 入口先行：任一谓词列判定 `SKIP_SCAN` 则整组直接置空，有更新的列则保守全扫。

COPY 批量构建按 nodeGroup 分区并行：计数各 bound node 边数 → 由 length 推 offset（新 group 预留 `len/0.8-len` 空隙）→ 向量化写入 → 尾部 gap 填 null。checkpoint 分两条路：无持久态全量排序重排成 CSR；有持久态走 Packed-CSR 增量合并，按叶 region（1024 bound node）收集变更并按密度界（叶 1.0、向上层向 0.8 收敛）决定合并粒度，逐列逐 region 用游标合并，可整 segment 跳写。

## 4. 当前项目边属性表

记录形态在建表时一次锁定并持久化，load 永不重推断。`PureTopologyCsr` 每边 12B（endpoint + edge_id），rank 恒 0，无时间戳，删除写 `INVALID_EDGE_ID` 哨兵保留 endpoint；`BundledCsr` 在纯拓扑上加严格平行的 `u64` 值列与有效位图（20B/边 + 1bit），删除清有效位留陈旧字，位置型 revert 可复活保留字；`Columnar` 为默认通用形态，拓扑（`MutableCsr/SingleMutableCsr` 32B/边，热冷分离）加 `CsrWithProperties` 列存。`Auto` 选择器只推导安全默认（空属性 Pure，其余 Columnar），`Bundled` 须显式指定；`Single` 任一方向必 Columnar，多属性/不可编码类型必 Columnar。

`CsrWithProperties` 的拓扑索引只有 CSR 一份，自身无 per-vertex 偏移：`edge_id>>10` 分段映射到行号，空段为 `None`，`free_list` 复用释放行，`row_to_edge` 提供 O(1) 反查。列存复用顶点 `Column`（含版本链、zone-map、编码），谓词过滤只解谓词列、经 null 位图判定、空值永不命中；批量投影一次解析列号，多边复用。属性更新走 `set_versioned` 入版本链并标记脏列，checkpoint 时按脏列刷新统计与自适应编码，版本历史在落盘时坍缩（跨 checkpoint 无时间旅行）。

事务提交顺序固定为权威 → 属性行 → out 拓扑 → in 拓扑 → 二级索引 → owner，失败逐级补偿；`PendingGate` 让同一事务读到自己的未提交写。`Bundled` 双腿各存一份值是故意为之（读路径永不跨腿）；`Columnar` 双腿共享同一 `edge_id` 行，内存为 ladybug 双份模型的一半。`Pure/Bundled` 配 schema-only 占位列存，任何行操作直接拒绝以防双真理。freeze 打包行内全排序 `(endpoint, rank, edge_id)` 并产出 mmap 只读 sidecar；Bundled 打包顺带携带值列与有效位，含有效值的分组可直接冻结，解冻时经值写入口回放。

## 5. 架构差异

1. **形态划分主轴不同**。neug 按"列数 × 类型"（0 列/单列非 string/其他）；ladybug 不分形态，全列存；当前项目按"记录形态 × 可变性"（Pure/Bundled/Columnar × Mutable/Frozen/Mapped），把有无属性、几个属性编码进变体。
2. **行身份不同**。neug unbundled 以 `row_id`（CSR 条目内）为行身份；ladybug 以 CSR 行区间位置为行身份（位置即数据）；当前项目以 `edge_id → row` 映射为行身份，与拓扑位置解耦。
3. **双向共享策略不同**。neug unbundled 与当前 Columnar 双腿共享一份属性；neug bundled、ladybug、当前 Bundled 双腿各存一份。这是内存减半与扫描局部性的直接交换。
4. **可见性位置不同**。neug 时间戳内联条目，ladybug 版本链随行，当前项目权威外置。外置使 CSR 保持纯拓扑（Pure 12B/边成为可能），代价是可见性判定与属性投影各需一次权威/映射查询。
5. **演进机制不同**。neug 靠模板实例化 + 全量重建切换形态；ladybug 靠继承复用列存与版本链；当前项目靠 enum + 静态分发 + 记录形态迁移（离线/在线双路径，发布前 WAL 截断围栏旧 redo）。

## 6. 功能差异

| 功能 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 点属性读 | 条目直解（bundled）或一次跳表（unbundled） | 行区间 `lookup` + 版本链合并 | `mapped_row` → 列 `get_at_ts`，批量一次解析列号 |
| 点属性写 | 迭代器定位后原地覆写 + undo 记原值 | 版本链挂节点 + 入事务 undo | `set_versioned` 入链 + 脏列标记 |
| 范围/谓词过滤 | 无下推 | zone-map 整组跳过 + 向量化 | 列级 bounds 短路 + 逐行单元过滤 |
| 字符串 | 变长双容器列，上限截断 | 变长段/字典列 | 变长列 + 字典/FSST 编码 |
| rank | 无（边数据即模板类型） | N/A（关系模型无 rank） | Columnar 完整 `i64` rank；Pure/Bundled 非 0 拒绝 |
| 多属性/schema 演进 | unbundled 直接加列；跨形态全量重建 | 列操作 + 按列 checkpoint | Columnar 列状态机（稳定 `prop_id` 永不复用）；内联形态须先迁移 |
| 只读 serving | 无（Immutable 即只读态） | 无（persistent 即服务态） | `pack_from_mutable` 打包 + mmap sidecar + `open_or_rebuild`；Bundled 打包顺带携带值列 |
| 完整性 | 16B MD5 文件头（open 时跳过校验） | 页式存储自带校验 | 全部分组文件尾部 CRC32，载入先验后解析 |

## 7. 性能差异

| 方面 | neug | ladybug | 当前项目 |
|---|---|---|---|
| 点查属性 | O(1)（条目直解或单跳） | O(1)定位 + 跨段 `lookup` | 宽行 O(1)映射 + 列读；窄行同量级 |
| 邻域投影扫描 | bundled 段内连续；unbundled 逐边跳表 | 行区间顺序段扫 + 向量化，最优 | 拓扑行顺序 + 属性行随机（插入序），批量复用 buffer 缓解 |
| 单条插入 | 均摊 O(1)（1.5× 扩容） | O(1)追加 + 索引维护 | 主块 gap 填充 O(1) → 墓碑复用（有界回退）→ 溢出追加 |
| 删除 | CSR 墓碑 O(1)，`Table` 永不回收 | 版本标记 O(1)，checkpoint 过滤回收 | 双腿墓碑 + 权威 + 列存三处 O(1)，回收靠整理/迁移 |
| 空间 | `EmptyType` 4B/条最省；Mutable 落盘含容量空隙 | header 常驻 + gap 预留 `len/0.8-len` | Pure 12B / Bundled 20B+1bit / Columnar 32B+列存；零度行零成本 + 稀疏段索引 |
| 读放大 | unbundled 一次跳表 | header 常驻 + `REL_ID` 列常驻可扫 | 热冷分离减缓存污染；`edge_id → 权威/属性` 二次查为固定成本 |
| 写放大 | 批量全量搬迁 O(V+E) | gap 预留 + region 合并重写（随 level 指数增长） | 主块 gap 优先 + 墓碑复用 + 溢出分级块；整理两遍重建 |

## 8. 结论

1. 三者解决的是同一核心问题（边属性的"定位 → 可见性 → 值"三段解析），但优化目标不同：neug 优化单机事务与分析混合负载下的零拷贝直达；ladybug 优化列存分析型扫描的顺序性与向量化；当前项目优化形态适配与运维确定性（按 schema 选形、CRC 全覆盖、WAL 围栏、变体隔离）。
2. 最大分歧点是行身份与可见性位置：neug 行身份在条目内（`row_id`/时间戳内联），ladybug 行身份即位置（区间即数据、版本随行），当前项目行身份在映射中（`edge_id → row` 解耦、权威外置）。解耦带来单份共享与 O(1) 点操作，代价是邻域投影的随机访问；同序带来最优扫描，代价是双份存储与二次定位。
3. 当前项目相对两 refs 的增量：Pure/Bundled/Frozen/Mapped 四类 refs 没有的形态、双腿共享单份列存、稳定列标识与在线形态迁移、mmap 只读 sidecar、CRC 全覆盖。
4. 两 refs 相对当前项目的保留优势：neug 单属性内联仍带完整条目时间戳（事务性单属性场景零额外列存）；ladybug 属性行与 CSR 同序的顺序扫描与整组 zone 跳过。两者都是"用一份冗余换一路性能"的实例，与当前项目的取舍方向相反。
