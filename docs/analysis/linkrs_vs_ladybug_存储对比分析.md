# linkrs 与 ladybug 存储模块对比及 linkrs 设计缺陷分析

> 分析对象：
> - **linkrs** — `https://github.com/kkkqkx123/linkrs`（Rust，单节点图数据库，CSR + MVCC，Pre-v1.0 活跃开发）
> - **ladybug** — `https://github.com/kkkqkx123/ladybug`（C++，嵌入式图数据库，Kuzu 的复刻/重命名版本，列式磁盘存储 + CSR + 向量化查询）
>
> 方法说明：本文为**定性（架构级）对比**，基于对两个仓库存储层源码的逐文件阅读（linkrs 约 6.5 万行 Rust 存储代码、ladybug 约 240 个 C++ 存储文件）。由于两者构建成本极高（Rust 工具链 + C++ 大型依赖），**未在本环境实际编译运行基准**，性能结论均源于设计机制推断，而非实测数据。

---

## 1. 项目定位与成熟度对比

| 维度 | linkrs | ladybug |
|------|--------|---------|
| 实现语言 | Rust | C++（Kuzu 复刻） |
| 部署形态 | 单节点 / 可服务端（HTTP、gRPC、嵌入式、C-API） | 嵌入式 / serverless，可编译到 WASM |
| 数据库内核来源 | 从零自研 | 继承自 Kuzu（成熟研究级代码库） |
| 目标负载 | 偏 OLTP + 时序（time-travel） | 偏 OLAP 复杂分析（LDBC SNB 类负载） |
| 成熟度 | Pre-v1.0，明确"不保证向后兼容" | 已有完整 CI、测试套件、文档体系 |
| 查询执行 | 流式执行器 | 向量化（vectorized）+ 因子化（factorized）+ morsel 并行 |

**核心背景差异**：ladybug 是 Kuzu 的重命名分支，继承了经过学术与工业验证的存储与执行设计；linkrs 是近期从零构建的 Pre-v1.0 项目。这决定了两者在"工程完成度"上不处于同一阶段——对比时应区分"架构意图"与"实际落地"，这是后续缺陷分析的重要前提。

---

## 2. 存储模块架构对比

### 2.1 linkrs 存储架构

存储引擎是一个 **LSM 风格的多版本 CSR 存储**：

- 每个边类型按 `(src_label, dst_label, edge_label)` 三元组被切成多个 **edge partition**（`EdgeStore`），每个 partition 对应一个磁盘目录。
- 每个 `EdgeStore` 内部维护两套结构：**可变的 delta CSR**（`out_csr` / `in_csr`，即 `CsrVariant`）与一组 **不可变的冻结段**（`out_segments` / `in_segments`，即 `CsrSegment` 列表）。
- 写入先落到 delta CSR；达到阈值后 `freeze` 把 delta 物化为不可变段；段过多时 `merge`；删除用 **MVCC tombstone**。
- 顶层协调者 `GraphDataStore`（`engine/data_store.rs`）用 `RwLock<HashMap<EdgeTableKey, EdgeStore>>` 管理所有边表；`PersistenceCoordinator` 负责 checkpoint 刷盘。

目录布局：`data/vertices/`、`data/edges/<src>_<dst>_<edge>/`（`meta.bin` / `out_csr.bin` / `in_csr.bin` / `properties.bin`，zstd 分页压缩）、`wal/`、`checkpoint/`。

### 2.2 ladybug 存储架构

存储引擎是 **单文件页寻址的列式存储 + CSR 邻接**：

- `StorageManager` 持有单一主数据文件 `dataFH`，所有列、CSR、索引、溢出数据都作为**页范围**存放于这一个文件内，由 `PageManager` / `FreeSpaceManager` 全局分配回收（较新 Kuzu 的"单文件"布局）。
- 一个 `RelTable`（关系表）按方向维护多个 `RelTableData`，CSR header 由 `offset` + `length` 两列构成（`CSRHeaderColumns`）；给定 bound node 的 offset，`offset[offset]` + `length[offset]` 定位其在 `nbr_id` / 属性列中的边行区间——这就是图遍历的"join index"。
- 顶点/边属性均为**列式**（`Column` 类，每属性一列），嵌套类型（LIST/STRUCT/STRING）递归拆为子列 + overflow 文件。
- 内存管理由 `BufferManager`（受 Umbra/vmcache 启发）统一负责：虚拟内存映射 + `MADV_DONTNEED` 释放物理页 + 页状态机 + 乐观无锁读。

目录布局：单一 `databasePath` 主文件 + `databasePath.wal` + `databasePath.wal.checkpoint` + `databasePath.shadow` + 检查点锁文件。

### 2.3 架构关键差异一览

| 维度 | linkrs | ladybug |
|------|--------|---------|
| 物理组织 | 多目录多文件（每 partition 独立目录） | 单文件页寻址 |
| 属性存储 | 边属性**行存**（`PropertyTable`），顶点属性**列存** | 顶点/边属性**统一列存** |
| CSR 邻接 | "6 变体"自适应（`Multiple/Single/MultiSingle/Labeled/None` + 不可变 `Csr`） | 单一密度自适应 CSR + 每节点 `NodeCSRIndex`（顺序/稀疏自适应） |
| MVCC 模型 | 真多版本（每条边 `create_ts/delete_ts` + tombstone，支持 time-travel） | 单版本 + delta（`VersionInfo`/`UpdateInfo` + 事务私有 `local_storage` + undo buffer） |
| 崩溃一致性 | checkpoint = temp 目录 + rename + fsync 提交栅栏 | checkpoint = ShadowFile 影子页 + 冻结 WAL |
| 缓冲管理 | 无独立 buffer manager；段 `residency` 落盘 + Moka 记录缓存 | vmcache 风格 `BufferManager`（全局页帧 + 乐观读） |
| 索引 | BTreeMap 内存索引 + 重建崩溃测试 | HASH（主键）+ ART（自适应基数树，替代 B+ 树） |

---

## 3. 存储模块逐项技术对比

### 3.1 数据组织与目录布局

- **linkrs**：按 `(src,dst,edge)` partition 拆分，每个 partition 一个目录，文件为 `meta/out_csr/in_csr/properties` 四件套 + zstd 分页压缩。优势是 partition 级隔离清晰；劣势是目录/文件数量随 schema 组合爆炸，元数据管理分散。
- **ladybug**：单文件 + 全局页分配器。优势是 IO 路径统一、易做全局空间复用与缓冲；劣势是所有随机 IO 汇聚到同一文件句柄，需要成熟的 `PageManager` 避免碎片。

### 3.2 CSR 邻接实现

- **linkrs 的"6 种变体"**：针对不同的边基数特征选择不同结构——`Single`（一对一，O(1) 槽位、时间覆盖语义）、`MultiSingle`（有界多边，定长内存）、`Multiple`（通用，主块 + overflow chunk 两级避免高 degree 顶点整块拷贝）、`Labeled`（按 label 分组过滤）、`None`（占位）、不可变 `Csr`（冻结段，连续内存、cache 友好）。设计意图是"按数据特征选最优结构"，理念先进。
- **ladybug 的"密度自适应 CSR"**：结构统一，但每个 bound node 用 `NodeCSRIndex`（`isSequential` 连续区间 or 显式 `rowIndices`）紧凑表达；checkpoint 时用 `PackedCSRInfo` calibrator 树按叶子区域密度重分布（`redistributeCSRRegions`），密集节点连续、稀疏节点不浪费。配合 Arrow 零拷贝 CSR 扫描（`docs/design_arrow_csr_zero_copy.md`）。

> 对比：linkrs 用"结构分化"应对异构度，ladybug 用"统一结构 + 密度自适应 + 零拷贝"应对。ladybug 的零拷贝 Arrow 扫描对分析查询更友好；linkrs 的 `MutableCsr` 两级结构（主块 + overflow）在极端高 degree 顶点上会产生碎片。

### 3.3 属性存储（行 vs 列）

- **linkrs**：边属性是**行存**（`PropertyTable`，`records: Vec<Option<PropertyRecord>>`），设计理由是"遍历边时整行属性一次性读"。顶点属性是**列存**（`ColumnStore` + ALP/bitpacking/dictionary/fsst/rle 编码）。即"边行存、顶点列存"的混合。
- **ladybug**：顶点与边属性**统一列存**，每列独立 `Column` + 每列段压缩 + min/max 元数据，min/max 同时服务"原地更新判定"与"谓词下推（列裁剪）"。

> 对比：ladybug 的统一列存 + 谓词下推对分析查询（只扫需要的列）更优；linkrs 的边行存在做"仅取少数边属性"的分析扫描时浪费 IO。但 linkrs 的行存在"边遍历顺带取全部属性"的 OLTP 场景下也有合理性。

### 3.4 MVCC 与并发模型

- **linkrs（真多版本）**：每条边带 `create_ts/delete_ts`，删除用 `MVCCManager` 的 tombstone（热 HashMap + 冷排序 Vec + bloom 过滤）；支持 `export_snapshot` 做 **time-travel**；快照隔离（SI）。代价：存储放大、tombstone GC 复杂度高（热冷分层 + 引用计数 `min_active_snapshot_ts`）。
- **ladybug（单版本 + delta）**：磁盘只保留一份已提交版本；未提交写入事务私有 `local_storage` + 内存 `VersionInfo`/`UpdateInfo`；回滚靠丢弃 `LocalWAL` + `undoBuffer`。代价：**不支持 time-travel**，但读路径极便宜（无版本链追逐），实现简单。

> 对比：这是两种成熟的 MVCC 范式。linkrs 的多版本换取了时序查询能力，但落地时（见第 4 节）存在并发瓶颈；ladybug 的单版本 + delta 在读多写少的分析负载上更轻。

### 3.5 WAL 与崩溃恢复

- **linkrs**：`WalManager` 支持 LSN 管理、group commit、按 `DurabilityLevel` 提交；WAL 截断带 `truncation_barrier_lsn` 屏障（不被索引追上）；checkpoint 用 **temp 目录 + rename + fsync** 提交栅栏 + 可注入故障点（`PersistenceFaultPoint`）。这是较用心的崩溃安全设计。
- **ladybug**：逻辑级 WAL（redo 记录）+ 每事务 `LocalWAL`；checkpoint 用 **ShadowFile 影子页**（脏页先写影子，成功后再覆盖回原文件）+ 冻结 WAL（`.wal.checkpoint`）；崩溃恢复 `dryReplay` 定位最后 checkpoint/commit 偏移并重放逻辑操作。

> 对比：两者崩溃安全设计都较完整。linkrs 用"发布式 checkpoint + 故障注入测试"，ladybug 用"影子页原子性 + 逻辑重放"。ladybug 的影子页方案避免了对数据页的原地双写。

### 3.6 缓存 / 缓冲管理

- **linkrs**：无独立 buffer manager；段级缓冲由 `CsrSegment.residency`（内存/落盘 spill，CAS 状态机 + seqlock 乐观读）承担；记录级用 **Moka（TinyLFU 近似 LRU）** 按版本缓存顶点/ID 索引。
- **ladybug**：**vmcache 风格 `BufferManager`**——大虚拟区间 mmap + `MADV_DONTNEED` 释放物理页 + 页状态机（EVICTED→LOCKED→MARKED→UNLOCKED）+ 乐观无锁读 + 环形淘汰队列。这是查询性能的核心支柱。

> 对比：ladybug 有系统化的全局页缓冲与内存映射，linkrs 的缓冲更"局部化"（段 spill + 记录缓存），缺少统一的大页缓冲框架。

### 3.7 索引

- **linkrs**：`index_engine` / `index_manager` 基于内存 `BTreeMap` + chunk 缓冲池 + manifest + GC，有重建崩溃测试（`GenerationFaultPoint`）。文档提及其支持全文（tantivy/BM25）与向量（Qdrant 外部服务）。
- **ladybug**：C++ 引擎内仅 `HASH`（主键等值）+ `ART`（字符串/范围，替代 B+ 树）。FTS/向量不在 C++ 引擎内（README 提到的全文/向量是上层/其他语言绑定能力）。

> 对比：ladybug 用 ART 替代 B+ 树是现代图库的常见选择（前缀压缩好）；linkrs 的索引实现更偏"基础内存结构 + 崩溃测试"，成熟度相对早期。

### 3.8 压缩

- **linkrs**：针对顶点列存的编码器 ALP（浮点）、bitpacking（整数）、dictionary（低基数）、fsst（长字符串）、rle（bool/int），`EncodingSelector` 按列统计自动选优。边行存部分压缩较弱。
- **ladybug**：每列段独立压缩 `CompressionType`（INTEGER_BITPACKING / BOOLEAN_BITPACKING / CONSTANT / ALP），min/max 元数据同时服务"原地更新判定"与"谓词下推"；嵌套类型递归拆列。

> 对比：两者压缩算法集相似（ALP、bitpacking 是共识）。ladybug 的压缩与"谓词下推/原地更新"深度耦合，工程闭环更完整。

---

## 4. 各项性能定性对比

> 以下为基于架构机制的**定性推断**，非实测。

| 性能维度 | linkrs | ladybug | 定性结论 |
|----------|--------|---------|----------|
| **写入吞吐** | delta CSR 先写内存，微基准测顶点/边插入 | 单版本 + `local_storage` + 批量 `OptimisticAllocator` + WAL 组提交 | 两者均有写入优化；但 linkrs 的**整表写锁**会严重限制并发写（见 4.1） |
| **图遍历（邻接扫描）** | 可变 CSR + overflow 两级 | 密度自适应 CSR + 零拷贝 Arrow 扫描 | ladybug 零拷贝 + 密度自适应在大规模遍历上更优；linkrs 两级结构有额外间接 |
| **分析查询（多跳 join）** | 流式执行器 | 向量化 + 因子化 + morsel 并行 | **ladybug 压倒性优势**——linkrs 未见因子化/morsel 并行执行器 |
| **多核扩展性** | 未见多核查询并行；粗锁阻碍并发 | 明确 morsel 并行（`docs/morsel_parallelism.md`） | ladybug 天然多核；linkrs 当前偏单线程执行 |
| **内存效率** | Moka 缓存 + 段 spill；边行存 | vmcache BM + 列存压缩 + 谓词下推 | ladybug 全局缓冲 + 列裁剪更省内存/IO |
| **读一致性 / 时间旅行** | SI + time-travel（真多版本） | 快照隔离，无 time-travel | linkrs 独有 time-travel 能力 |
| **崩溃恢复速度** | WAL 重放 + 发布式 checkpoint | WAL 逻辑重放 + 影子页 | 均完整；ladybug 影子页避免数据页双写 |
| **存储放大** | 多版本 tombstone + 碎片 | 单版本 + delta | ladybug 更省空间；linkrs tombstone/overflow 有放大 |

### 4.1 关于 linkrs 写入并发的关键瓶颈

`GraphDataStore` 用 `RwLock<HashMap<EdgeTableKey, EdgeStore>>` 管理边表。所有写入路径（`with_edge_tables_mut`、`with_edge_partitions_mut`、`with_edge_tables_mut_result`）都取**整个 HashMap 的写锁**；所有读取取整表读锁。这意味着：

- 任何一次边写入 → 拿整个 `edge_tables` 写锁 → **所有边写操作全局串行**；
- 写锁期间 → **阻塞所有读**（RwLock 语义）；
- 不同 edge label、不同 partition 之间的写入**无法并发**。

对一个标榜"图数据库"的系统而言，这是显著的扩展性瓶颈。它实现简单、对单节点小写入量可接受，但与 ladybug 的"事务私有 local_storage + 提交时合并"相比，并发写入能力差距明显。

---

## 5. linkrs 设计缺陷与考虑不周之处

以下问题按"严重程度"排序，均基于源码核实。

### 5.1 【严重】粗粒度 catalog 锁导致写入无法并发（已核实）

`crates/graphdb-storage/src/storage/engine/data_store.rs:41-42, 445, 554-624`：
整表 `RwLock` 使跨 partition、跨 edge label 的写入相互串行且写阻塞读。**这是当前架构最突出的扩展性缺陷**，与图数据库"高并发写入"的典型需求相悖。

### 5.2 【严重】freeze 路径存在无防御 `assert!` panic（已核实）

`crates/graphdb-storage/src/storage/edge/edge_table/freeze.rs:120`：
```rust
assert!(
    max_vid < vertex_capacity,
    "Vertex ID {} exceeds capacity {}",
    max_vid, vertex_capacity
);
```
当 freeze 时某顶点 ID 超出 CSR 容量会**直接 panic**（进程崩溃），而非返回错误或安全扩容。该断言依赖上层 `ensure_vertex_capacity` 永不出错——属于"把不变量检查放在了崩溃路径上"的脆弱设计。生产环境应降级为 `Result` 或自动扩容。

### 5.3 【中等】文档与实现不一致：`SimpleEdgeStore` 仅注释未实现（已核实）

`crates/graphdb-storage/src/storage/edge/edge_table.rs:5` 注释称有 `simple: SimpleEdgeStore (single CSR, no history)`，但 `EdgeStore` 枚举（`:47`）**只有 `TimeTravel` 一个变体**，无 `simple` 实现文件。说明架构设想超前于落地，存在"声明了但未交付"的模块。

### 5.4 【中等】`SingleMutableCsr` 并发/乱序时间戳会被静默拒绝

`crates/graphdb-storage/src/storage/edge/single_mutable_csr.rs`：更新按时间戳覆盖，若两笔写入 `ts` 相等或非单调，后到者被**静默拒绝**（不报错、丢更新）。该限制依赖上层 WAL/MVCC 保证时间戳单调，但底层未做防御，存在"静默数据丢失"风险。

### 5.5 【中等】碎片（zombie block）与硬编码经验值

`MutableCsr` 的两级结构（主块 + overflow chunk）在高 degree 顶点多次扩容时会产生 **zombie blocks**（不可达 overflow 块，`fragmentation_stats.rs`）。合并启发式使用的 `bytes_per_edge`（`csr_variant.rs:227`）是**经验硬编码值**（26/20/28/36），非实测；碎片回收依赖 `compact`，但 compaction 触发与回收效率未充分验证。

### 5.6 【轻微】VertexId 编码的隐式退化

`edge_table/core.rs:198-220`：VertexId 假设固定 16 字节（endpoint + rank），长度不符时**静默退化为 `(key, 0)`**，无告警。跨系统/非标准 key 下可能出现难以排查的 ID 映射错误。

### 5.7 【中等】缺少成熟的向量化/因子化分析执行器

与 ladybug 的"向量化 + 因子化 + morsel 并行"相比，linkrs 当前是**流式执行器**，未见：
- 批量向量化扫描（2048 行/批）；
- 因子化连接（semi-mask 半连接掩码）；
- 多核 morsel 并行。

这使其在分析型（多跳 join、聚合）负载上性能天花板明显低于 ladybug。**如果 linkrs 定位是 OLTP+时序，这不算缺陷；若想兼顾分析负载，则是能力缺口。**

### 5.8 【中等】基准方法论偏向微基准，缺真实分析负载验证

linkrs 的 `benches/` 以 Criterion 微基准为主（`storage_bench.rs` 测 vertex_insert / edge_insert / data_generation）。这能说明"单点插入快不快"，但**无法反映分析查询、多跳遍历、并发写入**等真实图负载性能。反观 ladybug/Kuzu，其性能叙事建立在 LDBC SNB 等标准分析基准之上（`dataset/`、`test/statements`、`test/answers`）。linkrs 尚未提供这类端到端分析基准，**性能声明缺乏第三方可复现的验证**。

### 5.9 【轻微】Pre-v1.0 激进开发带来的工程信号

- `AGENTS.md` 明确"不保证向后兼容""无需特别考虑兼容性"——适合早期探索，但意味着 schema/磁盘格式可能在未来版本破坏性变更。
- 存储引擎虽用心（WAL 屏障、checkpoint 故障注入、段 seqlock 乐观读、热冷分层 tombstone + bloom），但多处"文档超前于代码"（如 5.3）、assert 崩溃（5.2）、静默拒绝（5.4）表明**正确性边界尚未完全固化**。

---

## 6. 总结

### 6.1 两个系统本质差异

| 视角 | linkrs | ladybug |
|------|--------|---------|
| 内核血统 | 从零自研（Pre-v1.0） | Kuzu 成熟分支 |
| 存储哲学 | LSM 式多版本 CSR + 行存边属性 + 时间旅行 | 单文件列式 + 密度自适应 CSR + 单版本 delta |
| 性能取向 | OLTP + 时序查询 | OLAP 复杂分析（向量化/因子化/morsel 并行） |
| 工程完成度 | 意图先进、部分落地、存在简化与崩溃风险点 | 完整、有 CI/测试/文档体系 |
| 独特能力 | 真 time-travel、多 API 面（HTTP/gRPC/C） | 零拷贝 CSR、morsel 多核并行、ART 索引 |

### 6.2 linkrs 的核心矛盾

linkrs 的**架构意图是先进的**（6 变体 CSR、真多版本 MVCC、time-travel、WAL 屏障、checkpoint 故障注入），但**落地存在结构性短板**：

1. **并发模型拖后腿**：整表 `RwLock` 使所有边写入串行且写阻塞读（5.1），这是与"图数据库"身份最不匹配的一点。
2. **崩溃路径脆弱**：freeze 的 `assert!` panic（5.2）、`SingleMutableCsr` 静默丢更新（5.4）、VertexId 静默退化（5.6）表明错误处理未贯彻"绝不 panic/绝不静默"的原则。
3. **文档与代码脱节**：`SimpleEdgeStore` 声明未实现（5.3），部分设计停留在注释层面。
4. **分析能力缺口**：缺少向量化/因子化/morsel 并行执行器（5.7），在复杂分析负载上天花板低于 ladybug。
5. **验证不充分**：仅有微基准（5.8），缺 LDBC SNB 类标准分析负载的可复现性能证据。

### 6.3 给 linkrs 的优先改进建议

1. **（最高优先级）** 将 catalog 级整表 `RwLock` 改为 partition/stripe 级细粒度锁或 MVCC 无锁读，解除写入并发瓶颈。
2. 把 `freeze.rs:120` 的 `assert!` 改为 `Result` 返回或自动扩容，消除崩溃路径。
3. 为 `SingleMutableCsr` 的乱序时间戳加显式错误返回，杜绝静默丢更新。
4. 补齐 `SimpleEdgeStore` 或删除对应文档声明，消除文档/实现不一致。
5. 引入向量化/因子化执行与 morsel 并行，或至少在文档中明确 linkrs 的 OLTP+时序定位、不追求分析负载。
6. 增加 LDBC SNB 类端到端分析基准，使性能声明可被独立验证。

---

### 附：关键源码索引

**linkrs**
- 粗锁：`crates/graphdb-storage/src/storage/engine/data_store.rs:41-42, 554-624`
- freeze panic：`crates/graphdb-storage/src/storage/edge/edge_table/freeze.rs:120`
- SimpleEdgeStore 注释：`crates/graphdb-storage/src/storage/edge/edge_table.rs:5`
- CSR 变体：`crates/graphdb-storage/src/storage/edge/csr_variant.rs`
- MVCC：`crates/graphdb-storage/src/storage/edge/edge_table/mvcc.rs`
- WAL/checkpoint：`crates/graphdb-storage/src/storage/engine/wal_manager.rs`、`persistence_coordinator.rs`
- 属性表（行存）：`crates/graphdb-storage/src/storage/edge/property_table.rs`

**ladybug**
- 存储管理：`src/storage/storage_manager.cpp`、`src/include/storage/storage_manager.h`
- 列式：`src/include/storage/table/column.h`、`column_chunk_data.h`
- CSR：`src/include/storage/table/rel_table_data.h`、`csr_node_group.h`
- BufferManager：`src/include/storage/buffer_manager/buffer_manager.h`
- WAL/恢复：`src/storage/wal/wal_replayer.cpp`、`src/include/storage/wal/`
- 并发/MVCC：`src/include/transaction/transaction.h`、`src/include/storage/table/version_info.h`
- 索引：`src/include/storage/index/hash_index.h`、`art_index.cpp`
- 压缩：`src/include/storage/compression/compression.h`
- 设计文档：`docs/design_arrow_csr_zero_copy.md`、`docs/morsel_parallelism.md`、`docs/semi_mask_in_scan.md`
