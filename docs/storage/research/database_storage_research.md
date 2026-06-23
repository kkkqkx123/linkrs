# 数据库存储架构对比调查报告

## 一、调查概述

本报告调研了主流数据库的存储架构实现，包括 RocksDB、DuckDB、SQLite、Neo4j 和 Apache Arrow，为 GraphDB 的节点存储优化提供参考。

---

## 二、各数据库存储架构分析

### 2.1 RocksDB (KV存储)

#### 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                      Client API                         │
├─────────────────────────────────────────────────────────┤
│                   Column Family                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │
│  │  MemTable   │  │  MemTable   │  │  MemTable   │    │
│  │  (SkipList) │  │  (SkipList) │  │  (SkipList) │    │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘    │
│         │                │                │            │
│  ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐    │
│  │ Immutable   │  │ Immutable   │  │ Immutable   │    │
│  │  MemTable   │  │  MemTable   │  │  MemTable   │    │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘    │
├─────────┼────────────────┼────────────────┼───────────┤
│         │                │                │           │
│  ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐   │
│  │   SSTable   │  │   SSTable   │  │   SSTable   │   │
│  │   Level 0   │  │   Level 1   │  │   Level N   │   │
│  └─────────────┘  └─────────────┘  └─────────────┘   │
│                                                        │
│  ┌─────────────────────────────────────────────────┐  │
│  │              Block Cache (LRU)                   │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

#### 核心设计

| 组件 | 设计要点 |
|------|----------|
| **MemTable** | 内存中的跳表结构，支持并发写入，默认64MB |
| **SSTable** | Sorted String Table，有序键值对文件，支持压缩 |
| **Column Family** | 类似表空间的概念，隔离不同数据 |
| **Compaction** | 后台合并压缩，减少读放大 |
| **Block Cache** | LRU缓存热点数据块 |

#### SSTable 文件格式

```
<beginning_of_file>
[data block 1]      <- 键值对数据，可压缩
[data block 2]
...
[data block N]
[meta block: filter block]    <- Bloom Filter
[meta block: index block]     <- 数据块索引
[meta block: compression dictionary]
[metaindex block]
[Footer]                       <- 文件元信息
<end_of_file>
```

#### 压缩策略

```cpp
// 多级压缩配置
options.compression = kLZ4Compression;           // 默认级别
options.bottommost_compression = kZSTD;          // 底层重压缩
options.compression_per_level = {kNoCompression, kSnappy, kLZ4, kZSTD};
```

#### 关键启示

1. **LSM-Tree 架构**：写入性能优异，适合写密集场景
2. **分层压缩**：不同层级使用不同压缩算法
3. **Block Cache**：细粒度缓存，减少内存占用
4. **Bloom Filter**：快速判断键是否存在，减少磁盘IO

---

### 2.2 DuckDB (列式OLAP)

#### 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                    SQL Interface                        │
├─────────────────────────────────────────────────────────┤
│                 Query Planner                          │
├─────────────────────────────────────────────────────────┤
│              Vectorized Execution                      │
│  ┌─────────────────────────────────────────────────┐  │
│  │  Data Chunk (2048 rows)                         │  │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐           │  │
│  │  │ Vector  │ │ Vector  │ │ Vector  │           │  │
│  │  │ (col 1) │ │ (col 2) │ │ (col 3) │           │  │
│  │  └─────────┘ └─────────┘ └─────────┘           │  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                 Columnar Storage                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Column 1: [v1, v2, v3, ...] + Validity Mask    │  │
│  │ Column 2: [v1, v2, v3, ...] + Validity Mask    │  │
│  │ Column 3: [v1, v2, v3, ...] + Validity Mask    │  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│              Compression Layer                         │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐     │
│  │  RLE    │ │ Dict    │ │ BitPack │ │  FSST   │     │
│  └─────────┘ └─────────┘ └─────────┘ └─────────┘     │
└─────────────────────────────────────────────────────────┘
```

#### Vector 数据结构

```c
// DuckDB Vector 结构
typedef struct {
    void* data;              // 数据指针
    uint64_t* validity;      // NULL 位图 (每个值1 bit)
    duckdb_logical_type type;
    idx_t size;
} duckdb_vector;

// 标准向量大小: 2048 行
#define STANDARD_VECTOR_SIZE 2048
```

#### 压缩算法

| 算法 | 适用场景 | 压缩比 |
|------|----------|--------|
| **Constant Encoding** | 所有值相同 | 极高 |
| **RLE** | 连续重复值 | 高 |
| **Bit Packing** | 小范围整数 | 中 |
| **Dictionary** | 低基数字符串 | 高 |
| **FSST** | 字符串压缩 | 中高 |
| **ALP** | 浮点数 | 高 |
| **Zstd** | 通用压缩 | 高 |

#### Dictionary Vector 示例

```
原始数据: ["apple", "banana", "apple", "cherry", "banana", "apple"]

Dictionary Vector:
┌─────────────────┐
│ Dictionary      │  Selection
│ [0] "apple"     │  [0, 1, 0, 2, 1, 0]
│ [1] "banana"    │
│ [2] "cherry"    │
└─────────────────┘

压缩后: 3个唯一字符串 + 6个索引 (vs 6个完整字符串)
```

#### 关键启示

1. **向量化执行**：批量处理2048行，CPU缓存友好
2. **Validity Mask**：使用位图而非bool数组，节省内存
3. **延迟解压**：Dictionary Vector 可在压缩状态下执行查询
4. **多种压缩**：根据数据特征自动选择最优压缩算法

---

### 2.3 SQLite (嵌入式关系型)

#### 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                    SQL Engine                          │
├─────────────────────────────────────────────────────────┤
│                   B-Tree Layer                         │
│  ┌─────────────────────────────────────────────────┐  │
│  │              Table B+Tree                       │  │
│  │     ┌───┐                                       │  │
│  │     │Root│                                      │  │
│  │     └─┬─┘                                       │  │
│  │   ┌───┴───┐                                     │  │
│  │   │       │                                     │  │
│  │  ┌▼─┐    ┌▼─┐    Interior Nodes                │  │
│  │  │  │    │  │    (keys + child pointers)       │  │
│  │  └┬─┘    └┬─┘                                     │  │
│  │   │       │                                       │  │
│  │  ┌▼─┐    ┌▼─┐    Leaf Pages                      │  │
│  │  │C1│    │C2│    (cell data)                     │  │
│  │  └──┘    └──┘                                     │  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                    Page Layer                          │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Page Header | Cell Pointers | Free Space | Cells│  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                    File Layer                          │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Header | Page 1 | Page 2 | ... | Page N         │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

#### 页面结构

```
┌────────────────────────────────────────────────────┐
│ Page Header (8-12 bytes)                          │
│ ├─ Page Type (1 byte): table/index interior/leaf  │
│ ├─ First Freeblock (2 bytes)                      │
│ ├─ Cell Count (2 bytes)                           │
│ ├─ Cell Content Start (2 bytes)                   │
│ └─ Fragmented Bytes (1 byte)                      │
├────────────────────────────────────────────────────┤
│ Cell Pointer Array                                │
│ ├─ Pointer 1 (2 bytes) → Cell 1                   │
│ ├─ Pointer 2 (2 bytes) → Cell 2                   │
│ └─ ...                                            │
├────────────────────────────────────────────────────┤
│ Free Space (unallocated)                          │
├────────────────────────────────────────────────────┤
│ Cell Content Area (grows from bottom)             │
│ ├─ Cell N (variable size)                         │
│ ├─ ...                                            │
│ └─ Cell 1                                         │
└────────────────────────────────────────────────────┘
```

#### Record 格式

```
Record Format:
┌─────────────────────────────────────────────────────┐
│ Header                                              │
│ ├─ Header Size (varint)                            │
│ ├─ Serial Type 1 (varint)                          │
│ ├─ Serial Type 2 (varint)                          │
│ └─ ...                                             │
├─────────────────────────────────────────────────────┤
│ Body                                                │
│ ├─ Value 1 (variable bytes)                        │
│ ├─ Value 2 (variable bytes)                        │
│ └─ ...                                             │
└─────────────────────────────────────────────────────┘

Serial Types:
┌────────┬───────────────────────────────────────────┐
│ Type   │ Meaning                                   │
├────────┼───────────────────────────────────────────┤
│ 0      │ NULL (0 bytes)                            │
│ 1      │ 8-bit signed int (1 byte)                 │
│ 2      │ 16-bit signed int (2 bytes)               │
│ 3      │ 24-bit signed int (3 bytes)               │
│ 4      │ 32-bit signed int (4 bytes)               │
│ 5      │ 48-bit signed int (6 bytes)               │
│ 6      │ 64-bit signed int (8 bytes)               │
│ 7      │ IEEE 754 float (8 bytes)                  │
│ 8      │ Integer 0 (0 bytes)                       │
│ 9      │ Integer 1 (0 bytes)                       │
│ N>=12  │ BLOB of (N-12)/2 bytes                    │
│ N>=13  │ String of (N-13)/2 bytes                  │
└────────┴───────────────────────────────────────────┘
```

#### 溢出页处理

```
当记录超过页面容量时:
┌──────────────────┐
│ Primary Page     │
│ ├─ Partial Data  │
│ └─ Overflow Ptr ─┼──→ ┌──────────────────┐
└──────────────────┘    │ Overflow Page 1  │
                        │ ├─ Next Ptr ─────┼──→ ┌──────────────────┐
                        │ └─ Data          │    │ Overflow Page 2  │
                        └──────────────────┘    │ ├─ Next Ptr = 0   │
                                                │ └─ Data          │
                                                └──────────────────┘
```

#### 关键启示

1. **Varint 编码**：紧凑存储小整数，减少空间占用
2. **Cell Pointer Array**：支持二分查找，无需移动数据
3. **溢出页链表**：处理大记录，不浪费页面空间
4. **Serial Type**：类型信息嵌入数据，自描述格式

---

### 2.4 Neo4j (原生图数据库)

#### 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                    Cypher Query                        │
├─────────────────────────────────────────────────────────┤
│                   Query Engine                         │
├─────────────────────────────────────────────────────────┤
│                    Graph API                           │
├─────────────────────────────────────────────────────────┤
│                  Record Storage                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │
│  │ Node Store  │  │ Rel Store   │  │ Prop Store  │    │
│  │             │  │             │  │             │    │
│  │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │    │
│  │ │Node 1   │ │  │ │Rel 1    │ │  │ │Prop 1   │ │    │
│  │ │Node 2   │ │  │ │Rel 2    │ │  │ │Prop 2   │ │    │
│  │ │...      │ │  │ │...      │ │  │ │...      │ │    │
│  │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │    │
│  └─────────────┘  └─────────────┘  └─────────────┘    │
├─────────────────────────────────────────────────────────┤
│                   Page Cache                           │
├─────────────────────────────────────────────────────────┤
│                   File System                         │
│  neostore.nodestore.db                                │
│  neostore.relationshipstore.db                        │
│  neostore.propertystore.db                            │
└─────────────────────────────────────────────────────────┘
```

#### 节点存储格式

```
Node Record (固定大小):
┌────────────────────────────────────────────────────────┐
│ Byte 0-3: First Relationship ID (4 bytes)             │
│ Byte 4-7: First Property ID (4 bytes)                 │
│ Byte 8: Labels (variable, bit-packed)                 │
│ Byte 9: Flags (in-use, etc.)                          │
└────────────────────────────────────────────────────────┘

特点:
- 固定大小记录，O(1) 随机访问
- 关系和属性通过链表连接
- 标签使用位图压缩
```

#### 关系存储格式

```
Relationship Record (固定大小):
┌────────────────────────────────────────────────────────┐
│ Byte 0-3: Source Node ID                              │
│ Byte 4-7: Target Node ID                              │
│ Byte 8-11: First Property ID                          │
│ Byte 12-15: Source Node Prev Rel                      │
│ Byte 16-19: Source Node Next Rel                      │
│ Byte 20-23: Target Node Prev Rel                      │
│ Byte 24-27: Target Node Next Rel                      │
│ Byte 28-29: Relationship Type                         │
│ Byte 30: Flags                                        │
└────────────────────────────────────────────────────────┘

双向链表设计:
- 每个节点维护关系的双向链表
- O(1) 遍历节点的所有关系
- 空间换时间的设计思路
```

#### 关键启示

1. **固定大小记录**：简化内存管理，支持O(1)访问
2. **指针链表**：关系通过双向链表连接，遍历高效
3. **分离存储**：节点、关系、属性分开存储
4. **原生图存储**：无索引邻接，O(1)关系遍历

---

### 2.5 Apache Arrow (列式内存格式)

#### 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                   Application                          │
├─────────────────────────────────────────────────────────┤
│                  Arrow API                             │
├─────────────────────────────────────────────────────────┤
│                  RecordBatch                           │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Schema: [Field(name, type, nullable), ...]      │  │
│  │ Arrays: [Array1, Array2, Array3, ...]           │  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                    Array                               │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Validity Buffer (bitmap)                        │  │
│  │ Data Buffer(s)                                  │  │
│  │   ├─ Fixed: values buffer                       │  │
│  │   └─ Variable: offsets + data buffers           │  │
│  └─────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                  Buffer                               │
│  ┌─────────────────────────────────────────────────┐  │
│  │ Contiguous memory region                        │  │
│  │ 64-byte aligned, zero-copy IPC                  │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

#### 内存布局

```
Int32 Array Example:
┌─────────────────────────────────────────────────────┐
│ Validity Buffer (bitmap)                           │
│ ┌─────────────────────────────────────────────────┐│
│ │ 0xFF (all valid)                                ││
│ └─────────────────────────────────────────────────┘│
│ Data Buffer                                        │
│ ┌─────────────────────────────────────────────────┐│
│ │ [1, 2, 3, 4, 5, 6, 7, 8]                       ││
│ └─────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘

String Array Example:
┌─────────────────────────────────────────────────────┐
│ Validity Buffer (bitmap)                           │
│ Data: [0b00000101] (index 0, 2 are NULL)           │
├─────────────────────────────────────────────────────┤
│ Offsets Buffer                                     │
│ [0, 5, 5, 8, 13] (start position of each string)   │
├─────────────────────────────────────────────────────┤
│ Data Buffer                                        │
│ "hello" + "cat" + "world"                          │
└─────────────────────────────────────────────────────┘
```

#### Parquet 文件格式

```
Parquet File Structure:
┌─────────────────────────────────────────────────────┐
│ Magic Number: "PAR1"                               │
├─────────────────────────────────────────────────────┤
│ Row Group 1                                        │
│ ├─ Column Chunk 1 (Column 1 data)                  │
│ │  ├─ Page 1 (compressed)                          │
│ │  └─ Page 2 (compressed)                          │
│ ├─ Column Chunk 2 (Column 2 data)                  │
│ └─ Column Chunk 3 (Column 3 data)                  │
├─────────────────────────────────────────────────────┤
│ Row Group 2                                        │
│ └─ ...                                             │
├─────────────────────────────────────────────────────┤
│ File Metadata (schema, row counts, offsets)        │
├─────────────────────────────────────────────────────┤
│ Footer Length (4 bytes)                            │
├─────────────────────────────────────────────────────┤
│ Magic Number: "PAR1"                               │
└─────────────────────────────────────────────────────┘
```

#### 关键启示

1. **零拷贝IPC**：内存布局标准化，进程间无需序列化
2. **Validity Bitmap**：1 bit 表示 NULL，内存高效
3. **列式存储**：同类型数据连续，SIMD 友好
4. **Parquet 集成**：支持高效持久化和压缩

---

## 三、对比总结

### 3.1 存储模型对比

| 数据库 | 存储模型 | 适用场景 | 写性能 | 读性能 | 压缩支持 |
|--------|----------|----------|--------|--------|----------|
| RocksDB | LSM-Tree KV | 写密集 | ★★★★★ | ★★★☆☆ | 多级压缩 |
| DuckDB | 列式存储 | OLAP分析 | ★★☆☆☆ | ★★★★★ | 多种算法 |
| SQLite | B+Tree | 通用OLTP | ★★★★☆ | ★★★★☆ | 页级压缩 |
| Neo4j | 原生图存储 | 图遍历 | ★★★☆☆ | ★★★★★ | 有限 |
| Arrow | 列式内存 | 数据交换 | N/A | ★★★★★ | 可选 |

### 3.2 NULL 处理对比

| 数据库 | NULL 表示方式 | 内存开销 |
|--------|---------------|----------|
| GraphDB (当前) | `Vec<bool>` | 1 byte/值 |
| DuckDB | Validity Bitmap | 1 bit/值 |
| SQLite | Serial Type 0 | 0 byte |
| Arrow | Validity Bitmap | 1 bit/值 |
| Neo4j | 链表指针 | 4 bytes |

### 3.3 字符串存储对比

| 数据库 | 字符串存储方式 | 特点 |
|--------|----------------|------|
| GraphDB (当前) | 长度前缀 + 原始数据 | 简单，无压缩 |
| DuckDB | Dictionary/RLE/FSST | 自动选择最优压缩 |
| SQLite | Serial Type + UTF-8 | Varint 编码长度 |
| Arrow | Offsets + Data | 支持字典编码 |

### 3.4 ID 映射对比

| 数据库 | ID 映射方式 | 查找复杂度 |
|--------|-------------|------------|
| GraphDB (当前) | HashMap<String, u32> | O(1) 平均 |
| Neo4j | 固定偏移量 | O(1) |
| RocksDB | Bloom Filter + SST | O(log N) |
| SQLite | B-Tree | O(log N) |

---

## 四、对 GraphDB 的启示

### 4.1 可借鉴的设计

1. **Validity Bitmap** (DuckDB/Arrow)
   - 将 `Vec<bool>` 改为位图，节省 8x 内存

2. **Varint 编码** (SQLite)
   - 对小整数使用变长编码，减少存储空间

3. **字典压缩** (DuckDB)
   - 对低基数字符串列使用字典编码

4. **分层压缩** (RocksDB)
   - 内存数据不压缩，持久化数据使用 Zstd

5. **向量化处理** (DuckDB/Arrow)
   - 批量处理数据，提高 CPU 缓存命中率

### 4.2 不适合的设计

1. **LSM-Tree** (RocksDB)
   - GraphDB 是单机场景，LSM-Tree 的复杂性不必要

2. **固定大小记录** (Neo4j)
   - 属性数量可变，固定大小会浪费空间

3. **溢出页链表** (SQLite)
   - 增加实现复杂度，可简化为大对象单独存储

---

## 五、参考资料

- [RocksDB Wiki](https://github.com/facebook/rocksdb/wiki)
- [DuckDB Internals](https://duckdb.org/docs/current/internals/overview)
- [SQLite File Format](https://www.sqlite.org/fileformat.html)
- [Neo4j Graph Database Concepts](https://neo4j.com/docs/getting-started/appendix/graphdb-concepts/)
- [Apache Arrow Columnar Format](https://arrow.apache.org/docs/format/Columnar.html)
