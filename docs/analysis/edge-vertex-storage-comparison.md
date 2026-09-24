# 边属性表、顶点与顶点属性存储设计对比分析

对比对象：

- **Ladybug**（`ref/ladybug`）：LBug 系列风格的单机嵌入式图数据库，磁盘为主（page + buffer pool + WAL + checkpoint）
- **Neug**（`ref/neug`）：GraphScope 风格的内存为主图存储，mmap container + Arrow 批量加载
- **本项目 linkrs**（`crates/graphdb-storage`）：Rust 实现的轻量单节点图数据库，列存 + 行级 MVCC

分析范围：边属性表（Rel/Edge Table）、顶点表（Node/Vertex Table）与顶点属性列存的设计。

---

## 1. Ladybug 的设计

### 1.1 顶点表（NodeTable）

`src/include/storage/table/node_table.h`：

- 顶层结构：`NodeTable = columns (vector<unique_ptr<Column>>) + NodeGroupCollection + pkColumnID + indexes`。
- **每列一个独立 `Column` 对象**，nullable 通过独立的 `NullColumn` 实现（`Column::nullColumn`）。
- 属性按 **NodeGroup（默认 64K 行）→ Segment → Page** 三级组织，扫描时按 NodeGroup 粒度批量向量化读取（`ValueVector` + SelectionVector）。
- 主键走独立的 `PrimaryKeyIndex`（hash index），lookup 走 `lookupPK`， uniqueness 约束在插入时校验。
- MVCC：`VersionRecordHandler` + `update_info.h`/`version_info.h`，行级 version chain 挂在 NodeGroup 上；未提交数据在 `LocalStorage::LocalNodeTable` 中缓冲，commit 时 merge 进持久层。
- checkpoint：`canCheckpointInPlace` / `checkpointColumnChunkOutOfPlace`，支持原地或异地（split segment）两种刷盘策略。

### 1.2 顶点属性列（Column 族）

- 抽象基类 `Column`，子类按物理类型特化：`StringColumn`（字典编码：主列存 dictionary 索引 + `DictionaryColumn` + `indexColumn` 三段）、`ListColumn`、`StructColumn`、`InternalIDColumn`。
- 列内用函数指针（`readToVectorFunc` / `writeFunc` / `readToPageFunc`）按物理类型分发，避免热路径虚调用。
- 每列带 `ColumnChunkMetadata`（min/max 统计），供扫描时的 zone-map 式谓词下推（`ColumnPredicateSet`）。
- 压缩按列启用（`enableCompression`），有专门的 compression 子系统。

### 1.3 边属性表（RelTable）

- `RelTable = directedRelData[2]`（FWD/BWD 两个方向的 `RelTableData`），每个方向一套独立的列。
- 邻接结构是 **CSR 化的列存**：CSR offset/length 本身也是两个 `Column`（`getCSROffsetColumn`/`getCSRLengthColumn`），与属性列统一走同样的 NodeGroup/Page 管线和 buffer pool。
- 边属性按 bound node 分组存储（`CSRNodeGroup`），同一条边的属性紧邻其拓扑位置，局部性好。
- 支持双向、detachDelete、multiplicity 约束；`nextRelOffset` 全局分配边 ID。
- 扫描状态机 `RelTableScanState` 显式区分 committed / uncommitted（LocalRelTable）两个数据源。

**优点**：磁盘友好、批量向量化扫描极快、列裁剪 + 压缩 + 统计完备、事务/恢复体系完整（WAL + ShadowFile + checkpoint）。
**缺点**：实现复杂度极高（30+ 个存储层类型）；单点 lookup 需要经过 buffer manager，延迟高于纯内存方案；写路径依赖 LocalStorage 缓冲 + checkpoint 合并，小事务代价大。

---

## 2. Neug 的设计

### 2.1 顶点表（VertexTable）

`include/neug/storages/graph/vertex_table.h`：

- 顶层结构：`VertexTable = IndexerType（pk→vid 索引） + Table（属性列集合） + VertexTimestamp`。
- 属性存放在通用的 `Table`（`utils/property/table.h`）里，`Table = vector<shared_ptr<ColumnBase>>`，**每列一个 typed column**。
- 列的底层是 `Container`（`storages/container/`）：可选 **匿名 mmap / 文件 mmap / 纯内存 / HugePage** 四种内存级别（`MemoryLevel`），即"把 mmap 当内存池用"。
- 单主键假设（`assert(primary_keys.size() == 1)`），主键索引 `IndexerType` 支持 O(1) 的 oid→lid。
- 删除/事务靠 **timestamp 机制**（`vertex_timestamp.h`）：每个 vid 记录插入/删除时间戳，`GetVertexSet(ts)` 返回某时刻可见顶点的惰性迭代器，无完整 MVCC 版本链（旧值不可查，只有存活性）。
- Schema 变更（AddProperties/DeleteProperties/RenameProperties）直接操作列集合。

### 2.2 边属性表（EdgeTable）

`include/neug/storages/graph/edge_table.h`：

- 顶层结构：`EdgeTable = out_csr_ + in_csr_（双向 CsrBase） + Table（边属性列）`。
- **拓扑与属性分离**：CSR（`mutable_csr.h`，`MutableNbr` 内嵌 timestamp + 边偏移）只存邻接；边属性放在独立的行式 `Table` 中，按 `table_idx_` 对应。
- CSR 分 **bundled / unbundled** 两种形态：bundled 把小体量边数据直接绑进 Nbr 槽位（`dropAndCreateNewBundledCSR`），unbundled 则分离到属性表；可在 Compact 时互相转换。
- 属性列同样是 mmap container 上的 typed column（`TypedColumn<T>` 直接 `reinterpret_cast<T*>(buffer_->GetData())`）。
- 更新/删除通过 `UpdateEdgeProperty`/`DeleteEdge` 带 timestamp 原子改写；批量加载走 Arrow RecordBatch（`BatchAddEdges`），面向 OLTP-lite + 大规模导入场景。

**优点**：实现极简、内存效率高（零拷贝 mmap、HugePage）、批量导入快、列类型特化无虚函数开销、可按内存级别灵活部署。
**缺点**：没有真正的 MVCC（只有 timestamp 存活判定，无法做快照一致的多版本读）；无谓词下推/统计信息；typed column 直接裸指针访问，边界依赖检查少；schema 变更非事务化；无 WAL/checkpoint 增量恢复（只有 Dump 快照）。

---

## 3. 本项目 linkrs 的设计

### 3.1 顶点表（ShardedVertexTable）

`crates/graphdb-storage/src/vertex/`：

- 顶层：`ShardedVertexTable`（分片路由）→ `VertexTable`（core/persistence/compaction/schema 分模块）+ `IdIndexer`（外部 ID→internal u32）+ `VertexTimestamp`。
- 属性列存 `ColumnStore` → 每列 `Column`，分 **`FixedWidthColumn`（10 种定长标量）** 与 **`VariableWidthColumn`（其余全部类型，长度前缀 + postcard 编码）** 两个变体；`FixedString`、`Decimal` 等也归入变长，无字典编码。
- MVCC 是本项目的核心特色：每列带 **`version_chains`（逐行 before-image 链，懒分配）+ `RowVisibility`（Layer 1 快速可见性）+ `zone_maps`（per-chunk min/max）+ `ColumnStats`**，`get_at_ts` 可查任意快照。
- 主键是**镜像列**（`primary_key_mirror_value`）：主键属性列物化外部 ID，类型在 schema 创建时校验（仅整数族 + 字符串）。
- 有 GC 管理（`gc_manager.rs`）、脏页跟踪（`dirty_page`）、分片与压缩协调。

### 3.2 边属性表（EdgeStore）

`crates/graphdb-storage/src/edge/`：

- 顶层：`EdgeStore`（core 分为 store/owner/reads/writes/index/query/schema_ops/maintenance/recovery 子模块）+ 每方向 node-group 分片 CSR。
- CSR 变体丰富：`BundledCsr`（拓扑 `PureTopologyCsr` + per-slot `primary_values: Vec<u64>` + valid 位图 + `overflow_values`）、`CsrVariant`、`MutableCsr`、`SingleMutableCsr`、`ImmutableCsr`、`PureCsr`，可在形态间迁移（`record_form::MigrateStats`）。
- 事务：**staging batch**（`EdgeStagingBatch`，批量原子提交 + 前缀回滚）+ 集中式 MVCC（`edge/mvcc`：tombstone、GC watermark）+ WAL（`edge/wal`）+ 增量 checkpoint（manifest + group 文件）。
- Schema 变更是**分阶段状态机**（prepare/fill/publish/abort，add/drop/rename 列各有独立模块）。
- 辅助设施：fragmentation stats、tombstone stats、freeze/unfreeze、顶点 ID remap + 离线 reshard。

**优点**：MVCC 快照读完备；边表工程化程度高（staging、WAL、增量 checkpoint、状态机式 DDL）；CSR 形态可按负载演进。
**缺点**：见第 5 节改进点。

---

## 4. 三者横向对比

| 维度 | Ladybug | Neug | linkrs |
|---|---|---|---|
| 定位 | 磁盘为主，嵌入式分析型 | 内存为主，mmap，分析+轻事务 | 单节点轻量，内存列存 + MVCC |
| 顶点属性组织 | NodeGroup→Segment→Page，列存+压缩 | 每列一个 mmap container 的 typed column | 定长/变长两类列变体 + chunk |
| 字符串存储 | 字典编码（dictionary + index + data 三列） | 直接存 utf8 / string_view | 长度前缀 + postcard，无字典编码 |
| 主键索引 | 独立 hash index（PrimaryKeyIndex） | IndexerType（单主键） | IdIndexer + 主键镜像列 |
| 边拓扑 | CSR offset/length 本身是列，走 buffer pool | MutableCsr/ImmutableCsr（Nbr 内嵌 ts） | PureTopologyCsr + 多种 CSR 变体 |
| 边属性位置 | 属性列按 bound node 分组（CSRNodeGroup） | 与拓扑分离的独立 Table | bundled（槽内值+溢出 chunk）/分离两态可迁移 |
| MVCC | 完整（version_info + update_info + undo buffer + LocalStorage） | 仅 timestamp 存活判定，无多版本 | 行级 version chain + visibility + tombstone（完整快照读） |
| 谓词下推/统计 | ColumnPredicateSet + per-chunk min/max | 无 | zone map + ColumnStats |
| 向量化 | ValueVector + SelectionVector（深度向量化） | Arrow RecordBatch（加载/导出） | 逐行 Value 解码 + ColumnValues 批量（部分） |
| DDL（加删列） | addColumn 事务化 + checkpoint | 直接改列集合 | 分阶段状态机（prepare/fill/publish/abort） |
| 持久化 | WAL + ShadowFile + checkpoint + 压缩 | Dump 快照 | WAL + 增量 checkpoint + 脏页跟踪 |
| 压缩 | 按列启用，专门子系统 | 无 | chunk_encoding（有限） |
| 复杂度 | 最高 | 最低 | 中高（边表偏复杂） |

---

## 5. 本项目可改进的方向

结合两者之长，按优先级：

1. **字符串/变长列引入字典编码**（对标 Ladybug `StringColumn`）。
   当前所有 String/FixedString/低基数枚举类属性都是长度前缀 + postcard 裸存，空间与扫描解码效率差。低基数字段（label 类、状态类）用 dictionary + index 列可显著降低内存并提升比较/过滤性能；`FixedString` 也应改为定宽列而非变长 payload。

2. **向量化扫描与列式批量读**（对标 Ladybug ValueVector / Neug Arrow）。
   当前扫描路径大量逐行 `get_at_ts` 解码成 `Value`（见 `decode_column_values_at_ts` 的手写分派），CPU 分支与分配开销大。建议：
   - 扩展 `ColumnValues` 的类型覆盖面，让定长列 scan 直接产出原生类型批量数组（跳过 `Value` 装箱）；
   - executor 侧算子改为按 batch（如 2048 行）消费，配合 zone map 谓词下推（当前 zone map 已存在，但需确认扫描路径真正利用了它剪枝）。

3. **压缩子系统**（对标 Ladybug compression）。
   chunk_encoding 目前覆盖有限；定长列应支持 bit-packing / RLE / 常量块等轻量压缩，并利用列统计在 checkpoint 时选择编码。

4. **统一 MVCC 版本链的 GC 与 checkpoint 协同**。
   Ladybug 在 checkpoint 时将 update_info 折叠进持久列，本项目 version_chains 懒增长，长事务 + 频繁更新会膨胀（已有 `value_payload_bytes` 记账与 gc_manager，但需确保压缩/checkpoint 时真正折叠 before-image 并回收空间，而不只依赖 tombstone watermark）。

5. **边的 bundled 形态判断自动化**。
   本项目已支持 bundled/unbundled 迁移，但触发条件（`record_form`）是手动/离线的。可借鉴 Neug 的 Compact：基于属性宽度与访问模式统计，在 compaction 时自动选择 bundled（窄边）或 unbundled（宽边），并做在线迁移。

6. **顶点表 schema 演进的状态机化**。
   边表已有 add/drop/rename 列状态机，顶点侧 `set_schema` 是直接替换（`vertex_table/core.rs:761`）。应把边表的模式复用到顶点列，避免 schema 切换瞬间读写不一致。

7. **统计信息与代价估计增强**。
   Ladybug 的 ColumnChunkStats（HLL 基数、min/max）服务于 join/scan 计划；本项目 ColumnStats 较薄，建议补 per-column 基数（HLL）与 NDV，供查询引擎下推和 join 顺序决策。

8. **风险提示：避免照搬 Ladybug 的复杂度**。
   Ladybug 的 NodeGroup/Segment/ShadowFile 体系是为磁盘型数据库设计的；本项目定位轻量单节点，不应引入 buffer pool + page manager 全套，只需吸收其"列统计 + 向量化 + 字典编码"三个点。

---

## 6. 结论

- Ladybug 胜在**磁盘体系完整性与向量化列存**，是本项目在编码（字典）、压缩、向量化上的最佳参照。
- Neug 胜在**简洁与内存效率**（mmap container、typed column、bundled CSR 思想），是本项目在"防止过度工程化"上的参照；其 bundled/unbundled CSR 双形态与本项目的 `BundledCsr` 同源。
- 本项目的差异化优势是**完整的行级 MVCC 快照读**与**工程化的事务/DDL 设施**（staging batch、状态机 DDL、WAL + 增量 checkpoint），这在两个参考实现中都不完备。
- 最值得投入的改进依次为：字典编码、向量化批量扫描、压缩、MVCC-GC/checkpoint 协同、bundled 形态自动迁移。
