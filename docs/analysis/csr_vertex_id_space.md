# CSR 顶点 ID 空间与扩容机制问题分析

> 本文档自包含地梳理一个存储层架构问题：边表 CSR 以"内部顶点 ID 直接作为数组下标"组织行空间，
> 导致行数与 ID 数值绑定而非与实际顶点数绑定，配合预分配策略产生数量级的空闲内存浪费，
> 极端情况下引发整机 OOM。文末给出改造方向，供与其他数据库项目对比。
>
> 背景时间：2026-07，代码位于 `crates/graphdb-storage`，单节点图数据库 linkrs。

---

## 1. 问题背景

### 1.1 触发事件

开发冷快照（ColdSnapshot）查询集成测试时发现：**只要运行任意涉及插入边的测试，整个操作系统就会冻结/崩溃**。初步怀疑是测试环境问题，但隔离验证后确认是进程内内存分配失控触发的内核 OOM：

- 在受限内存（ulimit 3GB）下运行，进程输出 `memory allocation of 536870912 bytes failed`（单次分配 512MB 即失败）；
- 崩溃在**首次边插入**时发生，早于任何冷快照逻辑，说明是热点（hot）边表路径的既有缺陷；
- 非 cold snapshot 专有问题——任何 `insert_edge` 触达的测试（包括既有 `full_lifecycle`）都会触发。

### 1.2 根因链条

1. 顶点表采用**分片哈希**（默认 8 分片）。内部 ID 由 `(分片号, 片内序号)` 编码而来；
2. 原编码把分片号放在**高 4 位**：`internal_id = shard << 28 | local_id`。外部 ID 经哈希落入任意分片后，即使片内序号是 0、1、2，内部 ID 也会高达 10 亿～19 亿；
3. 边表 CSR 以内部 ID **直接作为行下标**（`csr[row_id]`）。插入一条边时，CSR 为保证 `row = 10 亿` 可写，须把行数组扩充到 `next_power_of_two(10亿+1) = 2^31` 行；
4. 每行预分配约 292 字节（见 3.1），2^31 行即约 **150GB 分配请求** → 内核 OOM → 整机崩溃。

### 1.3 第一轮修复（已完成，仅缓解）

将编码翻转为 `internal_id = (local_id << 4) | shard`，内部 ID 与"片内序号"成正比，不再有 2^28 数量级的跳跃。OS 崩溃问题消失，全部既有测试恢复可运行。

但此修复只是把放大系数从 2^28× 降到 16×，**行数与 ID 绑定、每行固定预分配、无收缩路径**这三个结构性缺陷原样保留——当顶点规模达到十万/百万级时，空闲内存浪费仍然达到数百 MB～数 GB 量级。本文分析的对象正是这些残留缺陷。

---

## 2. 代码上下文（文件职责与关系）

### 2.1 顶点侧：外部 ID 与内部 ID 的分层

顶点侧共三层，自下而上：

- **VertexId（graphdb-core）**：统一的外部 ID 表示，定长 32 字节结构，可承载 int64 或字符串。它是用户可见的标识，与存储布局无关。
- **VertexTable + IdIndexer（vertex_table 目录）**：单分片内的顶点存储。`IdIndexer` 维护 `外部ID → 片内序号` 的双向索引，序号按插入顺序**单调递增、删除不复用**（`index = keys.len()`）。片内还有属性列存储、MVCC 版本链。
- **ShardedVertexTable（vertex_table/sharded.rs）**：8 个分片各持一个互斥锁保护的 VertexTable，按外部 ID 哈希选片，负责并发写隔离。**内部 ID 的编码/解码（encode_id/decode_id）是本文件私有函数**，全库其他代码把内部 ID 当作不透明 u32 句柄使用——这是第一轮修复能局部完成的原因。

关键语义：内部 ID 是"行号式"句柄（顶点表内部存储、边表 CSR 行号、属性偏移都引用它），但它的数值空间由"分片数 × 片内序号"决定，**从来不是稠密的**。

### 2.2 边侧：CSR 变体族与两级存储

边存储的核心文件与职责：

- **mutable_csr.rs — MutableCsr（默认策略）**：可变 CSR，两级布局。每行（顶点）有：`adj_offsets`（主块在邻居数组中的起始位置）、`degrees`（活跃度数）、`primary_capacities`（主块槽数）、`overflow_chunks`（溢出块链表，超出主块容量时按 4096 条/块追加）。`ensure_vertex_capacity` 在 `min_capacity` 超出当前行数时以 `next_power_of_two` 一次跳到 2 的幂。
- **single_mutable_csr.rs — SingleMutableCsr（单边策略）**：每顶点最多一条出边的简化变体，行数组直接物化 `Nbr` 元素，同样的 `next_power_of_two` 扩容。
- **multi_single_mutable_csr.rs / labeled_mutable_csr.rs**：另两种策略变体，结构思想相同。
- **csr_variant.rs — CsrVariant**：策略枚举包装（Multiple / Single / MultiSingle / Labeled / None），统一所有变体的接口（insert/get/delete/scan/序列化）。
- **csr.rs — Csr（冻结段格式）**：只读 CSR，`offsets + edges` 紧凑布局，由 `from_nbr_entries` 从 `(src, Nbr)` 列表构建，`vertex_capacity` 由调用方传入。

### 2.3 边表主体与生命周期模块

- **edge_table/core.rs — TimeTravelEdgeStore**：一条边类型的完整存储，持 `out_csr`（按源顶点）+ `in_csr`（按目标顶点）两个 CsrVariant，加 MVCC 分段（segments）、稀疏顶点索引（哪些段含某顶点）、属性表、版本历史等。所有外部操作入口在此层做统一处理，再分派给 CSR。
- **edge_table/segment.rs / freeze.rs**：增量数据从可变 CSR 冻结为只读段的流程。**冻结时段的 `vertex_capacity` 直接继承可变 CSR 的行数**——问题随之复制到段文件。
- **edge_table/merge.rs / compaction.rs / free_space.rs**：段合并与碎片回收。合并时按条目列表重建紧凑 CSR。
- **edge_table/mvcc.rs / snapshot.rs**：时间旅行查询与当前时刻合并快照（预合并 CSR，同样以内部 ID 为行号）。
- **edge_table/edge_table.rs — EdgeStore**：`TimeTravelEdgeStore` 的 Arc 包装与公开接口。

### 2.4 上下游数据流

- **写入路径**：`GraphStorage::insert_edge` → 顶点表解析出 src/dst 内部 ID → `TimeTravelEdgeStore::insert_edge(src_internal, dst_internal, ...)` → `MutableCsr::insert_edge` → 行号不足则 `ensure_vertex_capacity` 扩容。**外部 ID 多大并不直接决定行号**，决定行号的是内部 ID 的数值。
- **查询路径**：`edges_of / get_edge / scan` 以内部 ID 为行号访问 CSR，`get_edge` 用 `VertexId` 比较邻居，`nbr_to_edge_record` 再解码回外部 ID。内部 ID 在此全程不透明。
- **冻结/合并路径**：freeze 继承行数；merge 按 `(src, Nbr)` 条目重建，行数为调用方传入的最大 ID。
- **冷快照导出路径**：从 CSR 迭代全部边，写 `.lkcs` 文件，文件内 CSR 行数同样来自顶点容量。行空间浪费会直接落到磁盘文件尺寸上。

### 2.5 问题在代码中的具体落点（各文件职责小结）

| 文件 | 职责 | 与本问题相关的行为 |
|---|---|---|
| `vertex_table/sharded.rs` | 分片顶点表、内部 ID 编码 | `(local<<4)\|shard` 编码，ID 非稠密（16× 放大） |
| `vertex_table/id_indexer.rs` | 外部 ID ↔ 片内序号 | 序号单调递增，删除不复用（空洞永久化） |
| `edge/mutable_csr.rs` | 可变 CSR（默认） | `next_power_of_two` 跳变；每行预分配 4 主槽 + 空溢出 Vec（~292B/行）；compact 不缩行 |
| `edge/single_mutable_csr.rs` | 单边 CSR | 每行直接物化 64B 元素；同样的幂等扩容 |
| `edge/csr_variant.rs` | 策略分派 | 5 种变体各自实现同样的行空间逻辑 |
| `edge/csr.rs` | 冻结段 CSR | 行数由调用方传入，继承放大 |
| `edge_table/freeze.rs` | 增量冻结为段 | 段容量继承可变 CSR 行数 |
| `edge_table/core.rs` | 边表主体 | out_csr/in_csr 双 CSR；上游内部 ID 直接流入 |

---

## 3. 量化分析

### 3.1 每行固定成本（MutableCsr，实测 Nbr = 64 字节）

| 结构 | 每行字节 |
|---|---|
| adj_offsets | 4 |
| degrees | 4 |
| primary_capacities | 4 |
| overflow_chunks（空 Vec 指针三件套） | 24 |
| 主块预物化槽位（4 × 64B） | 256 |
| **合计** | **~292B/行，与度数无关** |

SingleMutableCsr 每行 64B；冻结段 Csr 每行 4B（offset）但 edges 紧凑。

### 3.2 实测行数跳变（修复后）

| 场景 | 行数 |
|---|---|
| 空表初始容量 | 4,096 |
| 插入 1 条边 @ 内部 ID 1,000,000 | 1,048,576（2^20，一条边） |
| 插入 100k 条边 @ `(v<<4)\|5` | 2,097,152（2^21） |

### 3.3 放大的四层来源

1. **ID 非稠密**：`(local<<4)\|shard` 编码 → 行数 ≤ 16 × 片内最大序号；
2. **幂等扩容**：`next_power_of_two` → 尾段 ≤ 2×；
3. **固定预分配**：~292B/行，零度顶点同样占满；
4. **无收缩**：删除仅标 delete_ts；compact 合并溢出块但行数不变；IdIndexer 序号不复用。

组合效果：100 万顶点规模下，单个边表空行成本即达数百 MB 量级，且随写入历史单调增长。

---

## 4. 设计思路（供对比研究）

### 4.1 核心判断

本问题的本质是**两类身份混用**：内部 ID 既是"顶点记录句柄"（允许稀疏、可编码分片信息），又被当作"CSR 行号"（要求稠密、紧凑、可收缩）。任何数据库只要把"用户可见 ID / 逻辑句柄"与"物理行号"混为一谈，都会出现同类问题。主流图数据库（Neo4j 的 record ID、NebulaGraph 的 vertex ID、DuckDB 的 row group 内 dense id）普遍的做法是：**逻辑句柄 → 物理位置之间显式建映射，物理布局只对稠密行号负责**。

### 4.2 候选方案

**方案 A：行号与内部 ID 解耦，边表持有稠密行映射**
边表维护 `内部ID → 稠密行号` 哈希表与反查数组；CSR 尺寸 == 活顶点数；冻结/合并时行号重排。
- 根治全部四层放大；冷文件尺寸同步收缩；ID 编码未来再调整时不牵动边表。
- 代价：每次 CSR 访问多一次哈希查找；改动覆盖全部热点路径（估计 800~1200 行）。

**方案 B：保留 ID 索引 CSR，局部缓解**
① 主块预分配归零，全走既有 overflow chunk 机制（按需分配）→ 每行 292B→36B；
② 幂等扩容改线性/比例步进；③ 删除时回收片内序号。
- 改善 3~5 倍，但 16× 放大仍在，治标不治本。

**方案 C：内部 ID 完全稠密化**
去掉分片位，顶点表发全局稠密序号，并发隔离改用其他手段（原子计数器/批处理）。
- 效果接近 A，但放弃分片写并发隔离；且仍缺显式映射层，未来灵活性低。

### 4.3 推荐与验证路径

推荐 **方案 A**，理由：
1. 一次性根治，不依赖 ID 编码细节，对后续 ID 方案演进免疫；
2. freeze / merge / 冷导出已有"按条目重建 CSR"的路径，行号重排可复用同一套逻辑；
3. 映射成本（~12B/顶点）远低于被消除的空行成本（~292B/行）。

验证路径建议：
- 先做微基准：`MutableCsr` 插入 10k / 100k / 1M 条边，对比方案 A 与现状的 `vertex_capacity`、实际 RSS、GC/分配耗时；
- 再在 `TimeTravelEdgeStore` 层验证查询路径（edges_of / get_edge / scan / 时间旅行）在行号翻译下的正确性；
- 最后跑全量测试回归 + 冷快照导出/加载 roundtrip。

### 4.4 与其他数据库的对比点（待研究）

- Neo4j：固定大小记录 + record ID 指针跳转（index-free adjacency）——行号即物理槽位，如何回收？
- NebulaGraph：vertex ID 显式映射，part 内稠密；
- DuckDB：row group 内 dense id，逻辑 id 与物理位置分离；
- SurrealDB / Dgraph：KV 层以 key 承载 ID，无固定行数组问题；
- LMDB/其他 LSM：天然稀疏，无此类放大。

对比时的关键问题：**目标数据库是否允许"行号空洞"？删除后的空间如何回收？行数组（若存在）的扩容策略与每行成本如何？**
