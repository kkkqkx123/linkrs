# LadybugDB vs Linkrs — 存储结构、遍历方式、索引对比分析

> 分析日期：2026-06-25
> LadybugDB：C++ 实现的图数据库，https://github.com/LadybugDB/ladybug
> Linkrs：Rust 实现的图数据库，https://github.com/kkkqkx123/linkrs

---

## 1. 存储结构对比

### 1.1 整体架构

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 语言 | C++ | Rust |
| 存储模型 | **单文件 page-based**（BufferManager 管理固定大小页） | **多文件 segment-based**（每个 VertexTable 独立目录，含 meta / id_indexer / columns / timestamps 等文件） |
| 存储单元 | 固定大小的 Page（`LBUG_PAGE_SIZE`），通过 `BufferManager` 实现缓存与换出 | 内存中的数据结构 + 文件序列化（flush/load），压缩可选 |
| 内存模式 | 支持纯内存运行（`inMemory`），使用 `O_PERSISTENT_FILE_IN_MEM` | 未显式区分内存/持久模式，持久化通过显式 flush 到文件 |
| 冗余机制 | **Shadow File + WAL**：ShadowFile 提供影子页机制，WAL 记录变更日志 | **WAL + Segment**：`LocalWalWriter` 记录变更，`CheckpointManager` 管理检查点 |
| 持久化格式 | 二进制 page 格式，由 `Serializer`/`Deserializer` 控制 | 自定义二进制格式，每文件开头有 magic bytes (`GRDB`) + version + section_id |

### 1.2 节点 (Vertex) 存储

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 存储结构 | **列式存储（Column）**：每个属性一列，单独压缩；列内分段（segment）存储 | **列式存储（ColumnStore）**：`FixedWidthColumn`（定长类型，O(1) 随机访问）与 `VariableWidthColumn`（变长类型如 String） |
| 行组织 | 按 **NodeGroup** 分组（`NODE_GROUP_SIZE` 节点一组），支持并行扫描 | 内部行号（row_idx）连续排列，`IdIndexer` 维护外部 ID 到内部行号的映射 |
| ID 映射 | 内部 offset 标识节点位置（`offset_t`），表 ID + offset 构成 `nodeID_t` | `IdIndexer` 提供双向映射（`IdKey` → `u32` 内部 ID 及反向），支持 Int 和 Text 类型 key |
| 空值处理 | `NullColumn` + `NullMask` | `NullBitmap`（`BitVec<u8, Lsb0>`） |
| 压缩 | 支持多种压缩算法（ALP, Bitpacking, FloatCompression 等），通过 `enableCompression` 开关 | 列级别可选 `ColumnEncoding`（`CompressionSelector` / `FsstColumn`），flush 时可选文件级压缩 |

### 1.3 关系/边 (Edge/Relation) 存储

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 核心结构 | **CSRNodeGroup**：CSR（Compressed Sparse Row）格式，每个 NodeGroup 内用 offset 数组 + length 数组 + 邻接连续数组存储 | **多层 CSR 架构**：MutableCsr（可变 △）+ Csr（不可变 Segment）+ SingleMutableCsr（单边优化） |
| 方向 | **双向 CSR**：RelTable 内部维护 `RelTableData`，按方向（FWD/BWD）分别存储 offset/length 列和邻接列 | **双向 CSR**：`out_csr` + `in_csr` 分别存储出入边 |
| 边属性 | 属性列与 CSR 平行存储在同一 NodeGroup 中 | **独立的 PropertyTable**：行式存储（Row-oriented），MVCC 版本追踪，create_ts/delete_ts |
| 边 ID | 基于 CSR 内部偏移定位 | `EdgeId` 全局计数器 + `EdgeOffset` CSR 本地偏移两种机制并存 |
| 分层策略 | 单层 CSR（内存修改 → checkpoint 持久化） | **三层架构**：(1) Mutable Delta CSR → (2) Freeze 为不可变 Segment → (3) Segment Merge（LSM 风格） |
| 边策略 | 单一 CSR 实现 | 多策略：`Multiple`（O(degree) 通用）、`Single`（O(1) 一对一关系）、`None` |
| 删除处理 | 通过 VersionRecordHandler + UndoBuffer 管理删除 | 三层删除模型：(1) 可变 CSR 物理删除 (2) Segment 合并时物理删除 (3) Tombstone GC，带 `TieredTombstoneManager` |

### 1.4 关键差异总结

**LadybugDB 的设计哲学**：单文件、page-based、列式压缩为主，节点和边数据在 NodeGroup 内紧密耦合。类似传统 RDBMS 的 page 管理思路，强在数据局部性和压缩率。

**Linkrs 的设计哲学**：多文件、LSM-style 的边存储分层架构，节点和边解耦存储。边使用类似 LSM-tree 的 delta → segment → merge 流水线，强在写吞吐和 MVCC 版本管理。

---

## 2. 遍历方式对比

### 2.1 节点扫描

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 扫描方式 | `ScanNodeTable` 按 NodeGroup 分批扫描，支持并行 | `VertexTable` 直接遍历内部行号空间 |
| 谓词下推 | 支持 `ColumnPredicateSet` | 通过 `BTreeMap` 属性索引做范围过滤 |
| 并行度 | `TaskScheduler` 将不同的 NodeGroup 分给不同线程 | 外部同步（`Arc<Mutex<VertexTable>>`），无内建并行扫描 |

### 2.2 邻接遍历

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 核心算法 | CSR offset + length 数组直接定位邻接列表：`O(1)` 找到起始偏移，`O(degree)` 读取邻接 | 同样 CSR 思路：`edges_of(vid)` 返回 `&[ImmutableNbr]` 切片 |
| 多段合并 | 单段 CSR 结构，无需合并 | 需合并多个 Segment 的结果（Mutable CSR + 多个 Immutable CSR），通过 segment index 做时间范围剪枝 |
| Shortest Path / BFS | `RecursiveExtend` 算子（基于 GDS 框架的 RJVertexCompute） | 未发现专用 BFS/shortest path 实现（但可通过 Cypher 查询支持） |
| 缓存 | `tryScanCachedTuples`：缓存的扫描结果重用 | `prefetch_batch()`：CPU cache locality 批量预取 |

### 2.3 索引查找

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 点查 | `IndexLookup` 算子通过 Hash/ART 索引定位节点 | `id_indexer` 通过 HashMap 实现 O(1) 外部 ID → 内部 ID 映射 |
| 索引范围扫描 | ART 索引支持范围查询（prefix-based） | BTreeMap 属性索引天然支持 Range Query |

### 2.4 关键差异总结

- **LadybugDB 的遍历优势**：单段 CSR 无需合并，遍历路径短；NodeGroup 内的数据局部性好；内建并行框架（TaskScheduler）
- **Linkrs 的遍历优势**：多策略 CSR（Single/Multiple）可按场景选最优；Segment index 可做时间范围剪枝，适合 time-travel 查询
- **Linkrs 的遍历开销**：邻接遍历需要合并可变 CSR + 多个 Segment，访存路径更长

---

## 3. 索引对比

### 3.1 索引类型

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 主键索引 | **HashIndex**（链式哈希表）和 **ART Index**（自适应基数树，支持范围查询） | `IdIndexer`（HashMap 双向映射）+ `SecondaryIndex` 未发现原生主键索引概念 |
| 属性索引 | 无内建通用属性索引（仅主键） | **`VertexIndexManager`**：基于 `BTreeMap` 的属性索引，支持前向+反向索引，MVCC 版本管理 |
| 边索引 | 无 | 未发现专门的边属性索引 |
| 辅助索引 | 无 | `GenericIndexManager<VertexIndexKeyGen>` 提供通用二级索引框架 |
| 唯一性约束 | HashIndex 处理主键唯一性 | 未显式发现 |

### 3.2 索引存储

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 索引持久化 | **磁盘 HashIndex**：使用 `DiskArray` 管理主/溢出 slot 数组 + `OverflowFile` 处理长字符串 | `IndexDataManagerImpl` 支持 flush/load，使用 `KeyCodec`（`KeyBuilder`/`KeyParser`）编解码 |
| 内存索引 | `InMemHashIndex` + `LocalHashIndex`（未提交事务的本地索引） | BTreeMap 全部在内存中，`flush_id_indexer` 和索引 flush 写入文件 |
| 索引清理 | checkpoint 时做 `vacuum` 注释为 TODO | `IndexGcManager` 后台清理 tombstone 条目 |

### 3.3 关键差异总结

- **LadybugDB 只对主键建索引**（Hash 或 ART），不提供通用属性索引——意味着按属性过滤通常需要全表扫描
- **Linkrs 提供 BTreeMap 属性索引**，支持范围查询和 MVCC，适合按属性过滤/范围扫描的场景，但所有索引都在内存中
- Linkrs 的 `GenericIndexManager` 框架更通用，易于扩展新的索引类型

---

## 4. 并发控制（MVCC / 事务）

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 事务模型 | 基于 `Transaction` 对象 + `UndoBuffer` + `LocalStorage` | 基于 MVCC Snapshot + `MVCCTable` trait + `UndoLog` |
| 隔离级别 | 通过 startTS/commitTS 实现 snapshot 隔离 | snapshot 隔离 + `SnapshotHandle` + 活跃快照跟踪 |
| 写冲突 | UndoBuffer 记录回滚信息，LocalStorage 管理未提交数据 | `ConflictManager` + 死锁预防 + `Cleaner` 模块 |
| WAL | `LocalWAL` + `WALReplayer` 实现 ARIES 风格恢复 | `LocalWalWriter` + `RecoveryManager` + `ParallelWalParser` |
| 二阶段提交 | 未发现 | 支持 `TwoPhaseCommit` |
| 垃圾回收 | Checkpoint 时处理 | `TieredTombstoneManager`（hot + cold 两层） + `IndexGcManager` + Segment Merge 时物理删除 |
| 时间旅行 | 未发现 | `EdgeTable` 的 `export_snapshot()` + Segment 时间戳可实现 time-travel 查询 |

---

## 5. 性能定性判断

### 5.1 点查（Point Lookup）

| 场景 | LadybugDB | Linkrs | 结论 |
|------|-----------|--------|------|
| 按主键查节点 | 磁盘 HashIndex / ART Index，可能涉及 I/O | IdIndexer 内存 HashMap O(1) | **Linkrs 更快**（全内存） |
| 查属性 | 需全表扫描（无二级索引） | BTreeMap 属性索引 O(log n) | **Linkrs 更快** |
| 第一次查询（冷启动） | 需从磁盘加载索引页 | 需从文件反序列化整个索引 | **接近**，均为 I/O bound |

### 5.2 邻接遍历

| 场景 | LadybugDB | Linkrs | 结论 |
|------|-----------|--------|------|
| 单跳遍历 | CSR 单段 O(degree)，压缩列可直接从 page 读取 | 需合并 mutable + 多个 segment | **LadybugDB 更快** |
| 多跳遍历 | GDS 框架 + 并行 NodeGroup 扫描 | 需多段合并，并行能力弱 | **LadybugDB 更快** |
| 高扇出节点 | CSR offset+length 定位，局部性好 | 同样 CSR，但段合并增加开销 | **LadybugDB 更快** |

### 5.3 写入

| 场景 | LadybugDB | Linkrs | 结论 |
|------|-----------|--------|------|
| 单条写入 | 直接写 LocalStorage + UndoBuffer，commit 时写 WAL | 写 Mutable CSR + WAL | **接近** |
| 批量写入 | 受 page 管理、压缩和 checkpoint 限制 | Delta → Freeze → Merge 流水线，顺写为主 | **Linkrs 可能更快**（LSM 风格） |
| 边删除 | 标记删除，checkpoint 时回收 | Tombstone 写入，合并时物理回收 | **Linkrs 更灵活但 LadybugDB 回收更及时** |

### 5.4 范围查询

| 场景 | LadybugDB | Linkrs | 结论 |
|------|-----------|--------|------|
| 按主键范围 | ART Index 支持 | IdIndexer 仅 ID 映射不支持范围 | **LadybugDB 更快** |
| 按属性范围 | 无索引，全表扫描 | BTreeMap 索引 O(log n + k) | **Linkrs 显著更快** |

### 5.5 内存占用

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 索引 | 大部分在磁盘，BufferManager 按需缓存 | IdIndexer + BTreeMap 索引全部在内存 |
| 边 | CSR 结构在内存，通过 BufferManager 换页 | Mutable CSR + 多个 Segment 都常驻内存 |
| 总体 | **对大数据的友好度更高**（可在有限内存下工作） | **小到中数据集性能更优**，大数据集需更多内存 |

### 5.6 并行度

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 扫描并行 | TaskScheduler 多线程调度，NodeGroup 级别并行 | 无内建并行扫描 |
| Join 并行 | Parallel HashJoin（Build 和 Probe 分离） | 未发现 |
| 总体 | **并行能力显著更强** | **当前以单线程为主** |

---

## 6. 综合总结

### LadybugDB 的核心优势
1. **成熟的 page 管理 + BufferManager**：可在有限内存下高效运行大数据集
2. **强并行查询能力**：TaskScheduler + Pipeline 分解 + 并行 HashJoin
3. **ART 索引**：主键上支持范围查询
4. **列式压缩**：多种高级压缩算法，存储效率高
5. **单段 CSR 遍历**：无需段合并，邻接遍历路径短

### Linkrs 的核心优势
1. **LSM-style 边存储**：Delta → Freeze → Merge 流水线，写优化好
2. **属性索引（BTreeMap）**：支持按属性过滤和范围查询，MVCC 管理
3. **灵活的 CSR 策略**：Multiple/Single/None 按边类型选择最优结构
4. **完整的 MVCC 框架**：Snapshot 管理 + 分层 Tombstone 回收 + Time-travel 查询
5. **模块化 Rust 架构**：crate 划分清晰，事务/存储/同步/查询各层解耦

### 适用场景建议

| 场景 | 推荐 | 理由 |
|------|------|------|
| 复杂分析查询（多跳、聚合） | LadybugDB | 并行能力强，列式压缩好 |
| 高并发简单点查/属性过滤 | Linkrs | 内存索引 O(1)/O(log n)，MVCC 友好 |
| 大容量数据（超过内存） | LadybugDB | BufferManager 按需换页 |
| 高频写入 + 边操作 | Linkrs | LSM-style 分层写入流水线 |
| Time-travel / 历史查询 | Linkrs | Segment 快照 + MVCC 时间戳 |
| 路径查询 / Shortest Path | LadybugDB | 内建 GDS 框架和 RecursiveExtend |