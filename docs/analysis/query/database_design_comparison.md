# 数据库存储结构与查询方式设计分析

> 基于 Context7 查询的主流数据库设计（Neo4j、Dgraph、DuckDB、Apache Arrow、SurrealDB、LMDB），
> 结合 linkrs 的实际场景，分析采用何种存储结构和查询方式作为基础更合适。

---

## 1. 主流数据库设计方案概览

### 1.1 Neo4j — 原生图存储（Native Graph）

| 维度 | 方案 |
|------|------|
| **存储结构** | **固定大小记录文件**：节点记录（15 bytes）、关系记录（包含首尾节点指针）、属性链（动态 Property Chain） |
| **核心思想** | **Index-Free Adjacency**：关系记录中直接存储源/目标节点的物理位置（Record ID），遍历时 O(1) 跳转，无需索引查找 |
| **存储文件** | 独立的 `nodestore.db`、`relstore.db`、`propstore.db`、`labelstore.db` |
| **属性存储** | 动态 Property Chain：节点/关系记录指向第一个 Property 记录，属性以单向链表链接，每个 Property 包含 `(prev, next, key_id, value)` |
| **图遍历** | 从起始节点出发，沿着关系记录中的指针直接到达相邻节点 — 内存/磁盘上的指针跳转 |
| **查询引擎** | Cypher → ANTLR 解析 → Logical Plan → Cost-Based Optimizer → Pipeline Executor |
| **特点** | 写入友好（固定大小记录），遍历极快（指针跳转），但属性访问需链式寻址 |

### 1.2 Dgraph — KV-Based 图存储

| 维度 | 方案 |
|------|------|
| **存储结构** | 基于 **Badger（LSM-Tree KV Store）**，所有数据表示为 Key-Value |
| **核心思想** | **Posting List**：对每个 `(Subject, Predicate)` 维护一个有序的 `(Value, UID)` 列表 — 本质是倒排索引 |
| **数据模型** | RDF 三元组 `(Subject, Predicate, Object)`，Predicate 预定义类型 |
| **图遍历** | 通过 Posting List 求交/并集实现遍历，使用 min-heap 合并多个 posting list |
| **查询引擎** | GraphQL+ → DQL（Dgraph Query Language） |
| **特点** | 水平可扩展、分布式 ACID、适合大规模图数据。但遍历效率依赖 KV 查询延迟，不如原生图快 |

### 1.3 DuckDB — 列式 OLAP

| 维度 | 方案 |
|------|------|
| **存储结构** | **Row Group → Column Segments**：数据水平分区为 Row Group（~2⁶⁴ rows），每个 Row Group 内独立列存储 |
| **列式布局** | 每列包含类型特定的 Segment（如 `DOUBLE`、`INTEGER`、`VALIDITY`），独立压缩 |
| **执行模型** | **Push-Based Vectorized Execution**：算子间传递 Vector 而非单行 |
| **Vector 格式** | 支持 Flat、Dictionary、Constant、Sequence 四种物理布局 |
| **Vector 大小** | 固定 2048 tuples |
| **DataChunk** | 水平切片，包含多个 Vector，是算子间数据交换的基本单位 |
| **压缩** | 轻量级列式压缩（RLE、字典、Delta、FSST 等），自动为持久化表启用 |
| **MVCC** | Optimistic Concurrency Control，适合读密集型分析负载 |
| **特点** | 列式压缩高、向量化执行高效、适合分析型查询。但点查和图形遍历不擅长 |

### 1.4 Apache Arrow — 标准化列式内存格式

| 维度 | 方案 |
|------|------|
| **物理布局** | **Validity Bitmap + Data Buffer(s)**：每个列由 1 个位图（null 标记）+ N 个数据缓冲区组成 |
| **定长类型** | 单数据缓冲区，平坦数组访问 |
| **变长类型** | Offsets Buffer（int32/int64）+ Data Buffer |
| **嵌套类型** | Struct → 子数组；List → Offsets + Child Array |
| **RecordBatch** | 列的集合，跨语言零拷贝交换 |
| **特点** | 工业标准、SIMD 友好、零拷贝跨进程通信。但 Arrow 是内存格式，非存储引擎 |

### 1.5 SurrealDB — 多模型（Document + Graph）

| 维度 | 方案 |
|------|------|
| **存储结构** | **文档存储在 KV Store（RocksDB/TiKV）上**，数据以 JSON Document 形式存放 |
| **架构** | **Compute/Storage 分离**：查询引擎（Rust 实现）与存储层解耦 |
| **图模型** | Edge 作为一等记录（文档嵌入关系指针），支持递归图遍历 syntax |
| **查询** | SurrealQL，支持 SQL-like + 图遍历语法 |
| **特点** | 多模型灵活、Rust 实现、嵌入/分布式均可。但文档存储的列式压缩效率低 |

### 1.6 LMDB — 内存映射 KV Store

| 维度 | 方案 |
|------|------|
| **存储结构** | **mmap 基 B+Tree**：使用操作系统虚拟内存管理持久化数据 |
| **事务模型** | **Copy-On-Write MVCC**：写事务创建 B+Tree 的新版本，读者读旧版本 |
| **并发** | 单写者多读者（Single Writer, Multiple Readers），写不阻塞读，读不阻塞写 |
| **数据访问** | 零拷贝读取（直接访问 mmap 内存）、序列化写入 |
| **特点** | 极致读性能、嵌入式友好、事务安全。但 mmap 内存管理复杂、写放大 |

---

## 2. 对比分析

### 2.1 存储结构

| 方案 | 空间效率 | 写入吞吐 | 点查延迟 | 范围扫描 | 图遍历 |
|------|----------|----------|----------|----------|--------|
| Neo4j 固定记录 | 低（填充/对齐浪费） | 高 | O(1) | 差 | **O(1)/hop** |
| Dgraph Posting List | 中（倒排索引开销） | 中（LSM 写放大） | O(log N) | 好 | O(k * log N) |
| DuckDB 列式 | **极高**（列压缩） | 低（列式写入慢） | 差（需解压整列） | **极好** | 不支持 |
| LMDB B+Tree | 中（B+Tree 内部节点） | 中（COW 写放大） | **O(log N)** | 好 | 需应用层模拟 |
| Arrow 内存 | 极高（无序列化） | N/A（内存格式） | O(1) | 极好 | N/A |
| **linkrs 当前（CSR+ColumnStore）** | 高（CSR 紧密 + 列编码） | 中（CSR 追加/分段合并） | O(1) 邻居 + O(K) 属性 | 好（列式扫描） | **O(1)/hop** |

### 2.2 图遍历能力

- **Neo4j 的 Index-Free Adjacency** 是最适合图遍历的设计，但固定大小记录空间浪费大。
- **CSR（Compressed Sparse Row）** 本质上是指针跳转的压缩形式——用 offset 数组代替指针，空间更紧凑，遍历复杂度同为 O(degree)。
- **Dgraph 的 Posting List** 的遍历需要 KV 查询，延迟较高。
- **DuckDB/Arrow 不支持原生图遍历**。

**结论**：linkrs 当前的 CSR 边存储方案在遍历效率上对标 Neo4j 的 Index-Free Adjacency，同时空间效率更高。这是正确的选择。

### 2.3 查询执行模型

| 方案 | 模型 | 数据交换单位 | 适合负载 |
|------|------|-------------|----------|
| Neo4j | Pull-based Pipeline | 行（单行流） | OLTP + 图遍历 |
| DuckDB | **Push-based Vectorized** | Vector（2048 列值） | OLAP |
| SurrealDB | Pull-based | 文档 | 混合 |
| linkrs 当前 | Pull-based Streaming | **DataChunk（Vec<Vec<Value>>）** | 混合 |

**关键观察**：
- DuckDB 的 Push-Based Vectorized 模型不适合图遍历——图遍历逐顶点展开，无法预知 2048 条结果。
- Neo4j 的行式 Pipeline 更适合图遍历，因为每次 expand 产生的结果不确定。
- DuckDB 的 Vector 格式（Flat/Dictionary/Constant/Sequence）值得借鉴：对于重复值和常量可以零拷贝传递。
- Arrow 的 Validity Bitmap 是列式格式的基础设施，linkrs 当前无对应。

**结论**：行式 DataChunk 对图遍历是合适的。但如果能引入 DuckDB 式的 Vector 类型系统（少量列式优化）而不改变整体 pull-based 模型，可以有选择地加速分析型算子。

### 2.4 属性存储

| 方案 | 方式 | 投影下推 | 压缩 |
|------|------|----------|------|
| Neo4j | Property Chain（链表） | 不支持 | 无 |
| Dgraph | Posting List 内嵌值 | 天然支持（predicate 级） | 无 |
| **linkrs 当前 ColumnStore** | **列式 Vec<u8> 平坦数组** | **支持（已有 API）** | **RLE/Dict/FSST/Bitpack/ALP** |
| DuckDB | Column Segment | 天然支持 | 多种列式压缩 |

linkrs 的 ColumnStore 在所有方案中属于最先进的一档——列式布局 + 多种编码 + 已有 `PropertyBatchReader` API。**当前最大的问题是 SourceOperator 没有使用它做投影下推**（已在 columnar_vectorization_analysis.md 中详细分析）。

### 2.5 事务与并发

| 方案 | 模型 | 适合 linkrs？ |
|------|------|---------------|
| Neo4j | Page-level lock + MVCC | 可，但复杂度高 |
| Dgraph/Badger | **LSM-Tree + MVCC** | 适合写多读少 |
| LMDB | **COW B+Tree + mmap** | 最适合嵌入式（单节点） |
| DuckDB | Optimistic MVCC | 读多写少分析场景 |
| linkrs 当前 | 待评估 | — |

**分析**：linkrs 是单节点嵌入式数据库，LMDB 风格的 COW B+Tree + mmap 是最自然的模型：
- 单写者多读者 = 嵌入式场景的实际模型（一个进程写，多个进程/线程读）
- mmap 零拷贝读取 = 极致读性能
- COW B+Tree 的事务 ACID = 简化持久化实现

**但** LMDB 的固定 mmap 大小和序列化写入是约束。如果 linkrs 需要支持大规模写入，ROCKSDB-style LSM 可能更合适。不过从当前测试使用的数据集大小来看，LMDB 足够。

### 2.6 数据表示（Value 类型系统）

| 方案 | 实现 | 内存效率 |
|------|------|----------|
| Neo4j | Java Object + Property Chain | 低 |
| Dgraph | Protocol Buffer + Posting List | 中 |
| DuckDB | **Vector 物理类型（非 boxed）** | **高** |
| Arrow | **Buffer<T> 平坦数组** | **极高** |
| linkrs | **Value enum（29 变体）+ Vec<Vec<Value>>** | 低（堆分配、枚举 tag 开销） |

linkrs 的 `Value` 类型使用 29 变体 enum + 堆分配（`Box<T>`），与 DuckDB/Arrow 的平坦数组相比，内存效率差距大。

**但是**，Arrow 的 `Buffer<T>` 要求列式访问——每列是同构数组。图数据库的 property 集是异构的（每个顶点可能有不同属性）。Schema-on-read 模式下，Arrow-style 列式要求在扫描时就知道所有属性，这在动态图场景中不现实。

**合适的折中**：对已知 schema 的顶点类型（Tag），走 Arrow-style 列式路径；对动态属性（如 JSON/Map），保持 Value enum。

---

## 3. 综合推荐

### 3.1 存储结构 — 保持并优化当前方案

```
CSR 边存储 + ColumnStore 属性存储
```

**理由**：
- CSR 是 Index-Free Adjacency 的压缩形式，遍历效率 O(degree)，对标 Neo4j
- ColumnStore 的列式属性存储和多种编码与 DuckDB/Arrow 在同一水平
- 已有的 `PropertyBatchReader` 支持但未使用批量投影读取——补上这个缺口即可
- 不需要改变整体架构

**引入建议**：
1. 在 ColumnStore 层面引入 **Validity Bitmap**（替代 `Value::Null` 变体）——Arrow 格式的标准做法
2. 为 ColumnStore 查询路径支持 **projection pushdown**（利用已有 `PropertyBatchReader`）

### 3.2 查询执行模型 — 保持行式，选择性列式化

```
Pull-based Streaming DataChunk（行式）
+ 列式优化 for 分析型算子（Aggregate/Sort/Join）
```

**理由**：
- 图遍历本质上是逐顶点的，行式模型更自然
- DuckDB 的 Push-based Vectorized 模型不适用于图遍历（不确定性展开）
- 但 Aggregate/Sort/Join 等分析型算子可以从列式处理获益
- DataChunk 已有 `get_column()` API——只是没有实际使用

**引入建议**：
1. 对 `PartialAggregate` 和 `Sort`（已 Spillable）优先使用列式访问
2. 引入 DuckDB-style 的 `Vector` 概念作为 `DataChunk` 的内部表示选项——DataChunk 既可以行式也可以列式，算子根据自身特性选择最优视图
3. 不为图遍历算子（Expand、Traverse、ShortestPath）引入列式

### 3.3 数据表示 — 引入平坦列缓冲

```
逐步将 Vec<Vec<Value>> 替换为 Column 数组 + 原子访问器
```

**推荐参考：Apache Arrow Validity + Data Buffer + Offset Buffer 三缓冲方案**

具体来说：
- 定长类型（Int, Float, Double, Bool, Date）：`Validity Bitmap + Data Buffer<T>`，O(1) 随机访问
- 变长类型（String, Blob, List）：`Validity Bitmap + Offset Buffer + Data Buffer`
- 复合类型（Vertex, Edge, Path, Map）：嵌套 Column 结构
- Value enum 保留用于动态类型场景

**但这个改造规模大，优先级低**，应在 profile 证明 `Vec<Vec<Value>>` 是瓶颈后再推进。

### 3.4 事务模型 — 借鉴 LMDB

```
LMDB-style 单写多读 + COW B+Tree + mmap
```

**理由**：
- 单节点嵌入式 = LMDB 的最佳场景
- 零拷贝读取对图遍历友好（频繁随机点查）
- ACID 事务保证

**但**：linkrs 当前的事务层设计需要独立评估，不在本分析范围内。

---

## 4. 方案对比总结

| 维度 | Neo4j | Dgraph | DuckDB | Arrow | LMDB | **推荐** |
|------|-------|--------|--------|-------|------|----------|
| 边存储 | 固定记录指针 | Posting List KV | N/A | N/A | N/A | **CSR（已有）** |
| 属性存储 | Property Chain | Posting List 内嵌 | Column Segments | Buffer<T> | B+Tree Value | **ColumnStore（已有）+ Validity Bitmap** |
| 执行模型 | 行式 Pipeline | 行式 | **向量化 Push** | N/A | N/A | **行式 + 选择性列式** |
| 数据交换 | 单行流 | 行批 | **DataChunk(Vector)** | RecordBatch | N/A | **DataChunk（增强 Vector 支持）** |
| 事务 | Page Lock + MVCC | LSM MVCC | Optimistic MVCC | N/A | **COW B+Tree** | **LMDB 风格** |
| 数据类型 | Java Object | Protobuf | **物理类型（非 boxed）** | **Buffer<T>** | Bytes | **逐步引入 Buffer<T>** |
| 投影下推 | 不支持 | 支持 | **原生** | 原生 | 不支持 | **启用已有 PropertyBatchReader** |

---

## 5. 实施路线建议

### Phase A（当前，低成本）
1. 投影下推到 ColumnStore（利用已有 `PropertyBatchReader` 和 `ScanOptions`）——已在 columnar_vectorization_analysis.md 中建议
2. 在 ColumnStore 中引入 Validity Bitmap（替代部分 `Value::Null` 场景）

### Phase B（中成本，profile 驱动）
3. 为 DataChunk 引入内部列式布局选项（`DataChunk::columns: Vec<ColumnVec>` 可选，与 `rows: Vec<Vec<Value>>` 共存）
4. PartialAggregate 使用列式累积路径
5. HashJoin 的 build 侧使用列式 key（而非 `Vec<Value>` + Equality）

### Phase C（高成本，profile 驱动）
6. 用 Arrow 三缓冲布局（Validity + Data + Offset）逐步替换 `Vec<Vec<Value>>`
7. 评估 LMDB 风格的事务层

Phase A 即时可行，Phase B/C 需要 profile 证据。
