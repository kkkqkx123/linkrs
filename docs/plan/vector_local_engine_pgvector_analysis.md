# 内置向量引擎方案对 pgvector 的效仿分析

> 状态：分析文档（2026-08-17）。
>
> 分析对象：[vector_local_engine_plan.md](vector_local_engine_plan.md)（内置向量引擎设计方案，
> 自研 pgvector 风格）。
> 参照实现：pgvector v0.8.6（本仓库当前拉取的源码版本）。
>
> 本分析回答一个问题：`vector_local_engine_plan.md` 声明"取 pgvector 设计思想"，
> 具体效仿了 pgvector 的哪些做法，哪些地方又因项目自身存储范式而刻意未照搬。

## 0. 结论摘要

| 项 | 结论 |
|----|------|
| 效仿的性质 | 效仿的是**架构决策层**的思想，而非源码级实现细节 |
| 效仿的核心做法 | 索引是存储上的可选二级结构、无索引时精确扫描为默认、删除走"数据删除 + 后台清理"、IVFFlat 的 k-means 采样训练与逐插入 list 分配、聚类漂移与重建的 recall 教训、L2/Cosine/Dot 三种距离度量、定长稠密 f32 存储、元数据页、WAL 保证向量写与事务原子 |
| 明确未照搬 | VACUUM 物理删除（改为 tombstone + 压缩）、依赖 PostgreSQL 的索引页存储（改为自建 mmap 文件）、手动 REINDEX（改为自动漂移监测）、SQL WHERE 过滤（改为结构化 VectorFilter，Qdrant 风格）、PostgreSQL 缓冲锁并发（改为 parking_lot 锁） |
| 参照物的双重来源 | 方案中过滤与类型体系（must/must_not/should）来自 Qdrant，存储与索引骨架来自 pgvector；本分析只覆盖 pgvector 部分 |

## 1. 效仿点总览

以下逐条列出设计方案中与 pgvector 一致的决策，并标注 pgvector 侧的出处
（README 行号与源码文件/函数）。

| # | 设计文档决策（出处） | pgvector 对应做法（出处） |
|---|---------------------|--------------------------|
| 1 | 索引是存储引擎上的二级结构、无索引时精确扫描（§0、§3.3） | 向量是普通列，默认精确搜索，索引可选（README §Indexing；ivfflat.h / hnsw.h） |
| 2 | 删除走正常行删除路径，不进入索引在线修复（§0、§3.4） | 删除是普通行删除，索引条目由 VACUUM 延迟清理（ivfvacuum.c `ivfflatbulkdelete`、hnswvacuum.c） |
| 3 | Tier 1 k-means 在采样子集上训练（§3.3） | 块采样 + reservoir sampling（同 ANALYZE），目标 50 样本/list、至少 10000（ivfbuild.c `SampleRows`/`ComputeCenters`） |
| 4 | 列表分配随插入逐个进行，无需整库重扫（§3.3） | 插入时计算到全部 list 中心的距离，归入最近 list（ivfinsert.c `FindInsertPage`） |
| 5 | Tier 1 probe 搜索（§3.3） | 用 pairing heap 选出 probes 个最近 list 再扫描（ivfscan.c `GetScanLists`/`GetScanItems`；README L342 建议 `sqrt(lists)`） |
| 6 | 聚类漂移导致 recall 下降，需重建调度（§8） | "数据不足建索引会导致低 recall"的告诫与手动 REINDEX（README L338-342、FAQ；ivfbuild.c 数据不足 NOTICE） |
| 7 | 距离度量 L2/Cosine/Dot（§3.2、Phase A） | `vector_l2_ops`/`vector_ip_ops`/`vector_cosine_ops` 三个 opclass（sql/vector.sql；vector.c 三种距离函数） |
| 8 | SIMD 距离核（Phase A，AVX2 + 朴素对照） | target_clones 自动向量化距离循环（vector.c `VECTOR_TARGET_CLONES`），未用显式 intrinsics |
| 9 | 定长稠密行主序 f32 槽位存储（§3.2 vectors.bin） | `Vector` 为定长 varlena：`float x[]`，`4*dim+8` 字节（vector.h；README L960） |
| 10 | meta.bin 存维度/距离度量/索引层级/聚类中心（§3.2） | `IvfflatMetaPageData`（magic/version/dimensions/lists）+ list 页存中心 `IvfflatListData.center`；`HnswMetaPageData` 同理（ivfflat.h、hnsw.h） |
| 11 | 近似索引下过滤是候选集后过滤（§3.5 Tier 0） | 近似索引扫描后再应用 WHERE 过滤（README L450 "filtering is applied after the index is scanned"） |
| 12 | HNSW 若实现则走 tombstone + 定期重建（预留 Tier 2、§9） | HNSW 元素带 `deleted` 标志，由 vacuum 清理（hnsw.h `HnswElementData.deleted`；hnswvacuum.c） |
| 13 | 向量写与图事务提交原子一致，崩溃恢复回放 WAL（§3.6） | 完全基于 PostgreSQL WAL，支持复制与 PITR（README FAQ "pgvector uses the write-ahead log"；索引页修改走 GenericXLog） |

## 2. 逐条分析

### 2.1 索引是二级结构，精确扫描是默认（对应 #1）

设计方案 §0 明确："索引是存储引擎上的二级结构、删除走正常行删除路径、
无索引时精确扫描"。这与 pgvector 的核心定位完全一致：pgvector 中向量只是
一张普通表的普通列，加不加索引完全由用户决定；默认路径是精确最近邻搜索
（README："By default, pgvector performs exact nearest neighbor search,
which provides perfect recall"）。

设计方案的 Tier 0（SIMD flat scan + rayon 并行）对应 pgvector 的
"无索引 seq scan"：pgvector 依靠 PostgreSQL 原生并行表扫描与 sort 算子完成
精确 top-K，linkrs 依靠项目既有的 `-C target-cpu=x86-64-v3`（AVX2）自动
向量化与 rayon 数据并行。两边都是"不加索引也能用，只是数据量上来后变慢"。

### 2.2 删除走行删除 + 后台清理（对应 #2、#12）

pgvector 的删除语义是设计决策的核心参照：

- IVFFlat 删除：`DELETE` 只删行，索引条目不即时移除，VACUUM 时
  `ivfflatbulkdelete` 遍历各 list 页，把对应 heap TID 的条目
  `PageIndexMultiDelete` 物理删除（ivfvacuum.c）。
- HNSW 删除：元素带 `deleted` 位（hnsw.h `HnswElementData.deleted`），
  vacuum 时标记删除并修复邻居连接（hnswvacuum.c）。

两种索引都**不提供在线删除图修复**——这正是设计方案 §2 表格判断
hnsw-rs/hnswlib-rs 不适合的原因，也是自研方案"tombstone + 定期压缩"的来源：
删除只做标记与摘除，空间回收交给后台清理，与 pgvector 的"行删除 +
VACUUM 清理"是同构决策。设计方案 §3.4 还专门注明"无图结构损坏问题
（区别于 HNSW），无需 REINDEX 语义"，即 Tier 0/1 的删除比 pgvector 的
HNSW 更简单（无图需要修复）。

实现上的差异见 §3.1。

### 2.3 IVFFlat 的 k-means 采样训练与 list 机制（对应 #3、#4、#5、#6）

设计方案 §3.3 直接点名"pgvector 同款思路"的有两处，实际对应 pgvector
IVFFlat 的完整构建与查询链路：

- **采样训练**：pgvector 构建时用 `BlockSampler` + reservoir sampling
  做块级随机采样（与 ANALYZE 相同逻辑，ivfbuild.c `SampleRows`），样本量
  目标"50 samples per list，至少 10000"（`ComputeCenters`）；随后用
  kmeans++ 初始化中心并迭代（ivfkmeans.c `InitCenters`，README Thanks
  引用 kmeans++ 论文）。设计方案"k-means 在采样子集上训练"即此思路。
- **逐插入分配**：pgvector 插入时 `FindInsertPage` 逐个计算新向量到全部
  list 中心的距离并归入最近 list（ivfinsert.c），构建时的整体分配用
  tuplesort 按 list 排序后批量写页。设计方案"列表分配随插入逐个进行，
  无需整库重扫"与之一致。
- **probe 搜索**：pgvector 查询时先算查询向量到所有 list 中心的距离，
  用 pairing heap 保留 probes 个最近 list，再只扫描这些 list 并排序
  （ivfscan.c `GetScanLists`/`GetScanItems`）。设计方案 Tier 1 的 probe
  搜索即此机制；README 建议 probes 从 `sqrt(lists)` 起步（默认 1，
  ivfflat.h `IVFFLAT_DEFAULT_PROBES`），设计方案未定参数，实施时可沿用。
- **漂移教训**：pgvector 文档反复强调 recall 与建索引时机的关系
  （README L338-342 三条要领、FAQ "Drop the index until the table has
  more data"、ivfbuild.c 数据不足 NOTICE "This will cause low recall"），
  重建依赖用户手动 `REINDEX`。设计方案把这一教训自动化：漂移超阈值
  （约 10%）自动重建。这是"效仿结论、改进手段"的典型——pgvector 的
  IVFFlat 同样面临"数据持续插入导致中心点漂移、recall 下降"，但把重建
  责任完全交给用户。

### 2.4 距离度量与 SIMD 距离核（对应 #7、#8）

设计方案 Phase A 的距离核为 L2/Cosine/Dot，与 pgvector 的
`vector_l2_ops`/`vector_ip_ops`/`vector_cosine_ops` 一一对应；现有
`DistanceMetric` 枚举（Cosine/Euclid/Dot，见 vector-engine-design.md）
也是这三者。

pgvector 的实现要点值得借鉴：

- 三种距离都是单循环标量实现（vector.c `VectorL2SquaredDistance`、
  `VectorInnerProduct`、`VectorCosineSimilarity`），靠
  `target_clones("default","fma")` 让编译器为不同 CPU 特性生成多版本
  自动向量化，而非手写 intrinsics；
- cosine 是"单循环同时累加点积与两个范数，返回 `1 - similarity`"
  （vector.c `VectorCosineSimilarity`），避免两次遍历；
- 索引层为 cosine 使用 spherical k-means（kmeans 距离用归一化向量），
  设计方案如做 cosine 的 Tier 1，同样需要处理"聚类中心是否归一化"问题。

设计方案"AVX2 距离核 + 朴素实现对照测试"对应 pgvector 的"自动向量化 +
回归测试"思路，但载体不同（目标 CPU 编译选项 vs target_clones）。

### 2.5 定长稠密存储与元数据页（对应 #9、#10）

- **定长稠密 f32**：pgvector 的 `Vector` 是 `varlena 头 + int16 dim +
  float x[]`，单精度、不压缩、行主序，`4*dim+8` 字节定长
  （README L960；vector.h）。设计方案 `vectors.bin` 的"mmap 稠密行主序
  f32 定长槽位"是同一思想：向量数组不压缩、定长、连续，便于随机寻址与
  SIMD 批处理。差异是载体：pgvector 内嵌在堆行中（可 TOAST、可
  `SET STORAGE PLAIN` 强制内联），linkrs 用独立 mmap 数组文件。
- **元数据页**：pgvector 每个索引有 metapage 存 `magic/version/
  dimensions/lists`（IVFFlat）或 `magic/version/m/efConstruction/
  entry point`（HNSW），list 本身也在页上存中心向量
  （`IvfflatListData.center`）。设计方案 `meta.bin` 存"维度、距离度量、
  索引层级配置、Tier 1 聚类中心"是同样的"元数据与数据分离"结构。

### 2.6 过滤是索引扫描后的后处理（对应 #11）

设计方案 §3.5 的 Tier 0 post-filter（top-K 超采后按 `VectorFilter` 后
过滤）标注为"Qdrant 同款 post-filter 语义"，但 pgvector 的近似索引
同样是后过滤：README 明确 "With approximate indexes, filtering is
applied *after* the index is scanned"，且因此引入 iterative index scans
（自动扩大扫描量直至凑够结果）。即"近似索引 + 后过滤 + 候选超采"是
pgvector 与 Qdrant 的共同语义，设计方案的 Tier 0/1 同时吸收了双方。

pgvector 还提供"精确过滤优先"的思路（过滤列建 B-tree 走精确最近邻、
partial index、分区），设计方案未采纳（保留在结构化 `VectorFilter`
体系内），见 §3.4。

### 2.7 事务原子性与 WAL（对应 #13）

设计方案 §3.6 的目标是"向量写与图事务提交原子一致"：提交时同步追加
`wal.bin`（txn id + ops）并应用内存索引，崩溃恢复时幂等回放。这与
pgvector 的持久化承诺同构——pgvector 完全依赖 PostgreSQL WAL
（README FAQ："pgvector uses the write-ahead log (WAL), which allows
for replication and point-in-time recovery"），索引页修改走
GenericXLog 记录。差异仅在载体：pgvector 复用现成的数据库 WAL 体系，
linkrs 需要自建 wal.bin（设计方案 §3.6 的关键简化：删除重试/DLQ/
circuit breaker 后，本地同步 + WAL 即可达到同等原子性）。

## 3. 未照搬的差异点

以下为设计方案的刻意偏离，均有项目存储范式的理由。

| # | 维度 | pgvector 做法 | 设计方案做法 | 原因 |
|---|------|--------------|--------------|------|
| 1 | 删除的空间回收 | VACUUM 物理删除条目（IVFFlat 即时物理删；HNSW tombstone + vacuum） | tombstone 位图 + 阈值（约 20%）触发压缩，memcpy 存活行 | mmap 定长槽位数组没有页内空洞概念，物理删除=整文件重写，故用位图延迟到压缩；pgvector 的页式存储删除条目即释放页空间 |
| 2 | 向量与业务数据的关系 | 向量列与业务行同表存储 | 向量存独立 collection 目录，经 `keys.bin` 做 slot↔PointId 双向映射 | 向量挂接在图顶点/边上，是图的附属索引而非行内列 |
| 3 | 聚类漂移后的重建 | 用户手动 `REINDEX` | 自动漂移监测（约 10% 阈值）+ 重建调度 | 单机嵌入式产品要求零人工运维 |
| 4 | 过滤表达 | SQL WHERE + B-tree/partial index/分区 | 结构化 `VectorFilter`（must/must_not/should/min_should），Tier 1 预留 per-list 预过滤 | 类型体系继承自 Qdrant 路径（vector-engine-design.md），需保持 qdrant 路径兼容；per-list 预过滤是 pgvector 没有的增强 |
| 5 | 精确扫描并行 | PostgreSQL 并行 seq scan（`max_parallel_workers_per_gather`） | rayon 数据并行 | 项目无 PostgreSQL 的并行执行框架，rayon 是现有依赖 |
| 6 | 并发模型 | buffer lock / LWLock / 页级锁体系 | 每 collection 一把 `parking_lot::RwLock`，mmap 读路径无锁 | 单机单进程内嵌场景，无需跨后端锁；pgvector 需服务多后端并发访问 |
| 7 | HNSW 层级 | 完整 HNSW（多层图 + 在线插入 + vacuum 修复） | 仅预留 Tier 2，且"不自研删除期间的图修复"，走 tombstone + 定期重建 | 图数据库边增删高频，HNSW 在线删除是主要风险点（§2 表格），故把 HNSW 降级为可选 |

## 4. 结论与实施建议

### 4.1 结论

设计方案对 pgvector 的效仿集中于**架构决策层**，共 13 个一致点（§1），
可归为五组：

1. **索引定位**：索引是可选二级结构，默认精确扫描，删除不依赖索引在线修复；
2. **IVFFlat 机制**：采样子集 k-means 训练、逐插入 list 分配、probe 搜索、
   漂移重建教训（并自动化）；
3. **数据与存储形态**：三种距离度量、定长稠密 f32、元数据与数据分离；
4. **过滤语义**：近似索引后过滤 + 候选超采；
5. **事务持久化**：向量写与业务事务经 WAL 原子落盘、崩溃可恢复。

差异点（§3）源于两个约束：linkrs 的存储范式是自建 mmap 文件而非
PostgreSQL 页式存储（决定了删除用 tombstone + 压缩、锁模型简化），
以及向量挂接在图顶点/边上而非表行（决定了独立 collection 与
slot↔PointId 映射）。

### 4.2 实施建议（供 Phase A/B 参考）

1. **采样参数**：Tier 1 k-means 采样可直接沿用 pgvector 的
   "50 samples/list、至少 10000"（ivfbuild.c `ComputeCenters`），
   样本数对构建时间影响很大；
2. **probe 语义**：probes 默认值与 `sqrt(lists)` 建议可对齐 pgvector
   （README L342），并保留可配置项；
3. **cosine 的 Tier 1**：若实现 cosine 距离的 IVFFlat，需像 pgvector
   一样用归一化后的向量做 spherical k-means，避免中心点在球面外的偏差；
4. **post-filter 文档化**：把"低选择性过滤下 post-filter 性能差"的语义
   写入文档（设计方案 §8 已列为风险），pgvector 的 iterative scan 思路
   （自动扩大扫描量）可作为后续 Tier 1 per-list 预过滤之外的备选；
5. **压缩触发阈值**：tombstone 20% 阈值（§3.2）与 pgvector 的
   vacuum 时机（`vacuum_cost_limit` 节流）作用相同，实施时按写入频率
   调参即可。

## 5. 附：参照实现版本与关键位置

- pgvector 版本：v0.8.6（`git clone https://github.com/pgvector/pgvector`）。
- 关键源码位置：
  - 精确搜索默认值：README L197-199；
  - 距离函数：src/vector.c `VectorL2SquaredDistance` / `VectorInnerProduct` /
    `VectorCosineSimilarity`（target_clones 见 L43）；
  - Vector 存储结构：src/vector.h `Vector`；
  - IVFFlat 构建与采样：src/ivfbuild.c `SampleRows` / `ComputeCenters` /
    `InsertTuples`；
  - k-means 初始化：src/ivfkmeans.c `InitCenters`；
  - IVFFlat 插入：src/ivfinsert.c `FindInsertPage`；
  - IVFFlat 查询：src/ivfscan.c `GetScanLists` / `GetScanItems`；
  - IVFFlat 删除：src/ivfvacuum.c `ivfflatbulkdelete`；
  - IVFFlat 元数据与 list 结构：src/ivfflat.h `IvfflatMetaPageData` /
    `IvfflatListData`；
  - HNSW 删除标志：src/hnsw.h `HnswElementData.deleted`；
  - HNSW vacuum：src/hnswvacuum.c；
  - 过滤语义：README L424-466（Filtering 一节）。
