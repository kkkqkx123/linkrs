# linkrs 与 ladybug 数据序列化方案对比分析

## 一、linkrs 序列化方案

### 1.1 存储数据序列化

linkrs 的存储数据序列化采用**分层组合**策略，总共四个独立的序列化层面：

**编码层（列内压缩）** — `crates/graphdb-storage/src/storage/encoding/`

在内存中对列数据做语义级压缩，提供五种编码：

| 编码类型 | 适用场景 | 核心数据结构 |
|---------|---------|------------|
| Dictionary | 低基数字符串 | `HashMap<Arc<str>, u32>` 字典 + `Vec<u32>` 索引数组 |
| RLE | 连续重复值（整型/布尔） | `Vec<RleRun<T>>` 游程序列 |
| BitPacking | 小范围整数 | `BitVec` 位向量 + `min_value` 基准 |
| FSST | 长字符串、高基数 | 静态符号表（2-8 字节子串映射） |
| ALP | 浮点数 | 乘 10^k 转整数 + BitPacking（无异常表） |

编码选择通过 `CompressionSelector` 自动决策：分析 `ColumnStats`（基数比、游程比、数值范围）后选取最优编码。所有编码都是**纯内存结构**，没有 `serialize`/`deserialize` trait 实现。

**物理压缩层** — `crates/graphdb-storage/src/storage/compression.rs`

采用**文件级 zstd 压缩**：先写出完整的二进制文件，再对整个文件做 zstd 压缩，格式为 `[1 byte marker (0x01)] [4 bytes CRC32] [4 bytes compressed_len] [compressed data]`。物理压缩完全独立于编码层，编码和 zstd 压缩是串行的两个阶段。

**持久化格式层** — 各 `persistence.rs`

以 VertexTable 为例，flush 流程将多个组件写入独立文件：
- `meta.bin`: 表元信息 + schema JSON（serde_json）
- `id_indexer.bin`: 自定义二进制格式（`IdKey` 序列化）
- `columns.bin`: 列名、原始数据字节（`get_flush_data()` 返回 data/offsets/bitmap）、编码类型标记（1 字节 `EncodingType::to_u8()`）
- `timestamps.bin`: 时间戳数组

每列在 flush 时写入 `encoding_type` 标记位。加载时通过 `deferred_encodings` 机制延迟重建编码结构。当前实现在 flush 未完成 deferred encoding 时会打印警告。

**索引键编码** — `OrderedCodec` + `KeyCodec`

为索引树提供保序二进制键编码。核心技术：
- 类型标签体系（1 字节 tag，按标签值大小定义类型间排序）
- 整型: XOR 翻转符号位转无符号偏序
- 浮点: IEEE 754 全序编码（处理 NaN、正负零）
- 变长类型: 逃逸终结符方案（`0x00` 编码为 `0x00 0x01`，终结符 `0x00 0x00`）
- 键拼接: `[space_id: u64 LE] [key_type: u8] [index_name] [encoded(values)...] [entity_ref]`

字节序即语义序，范围扫描无需反序列化。

### 1.2 中间数据序列化

**查询溢出（Spill）** — `spill.rs`

用于查询执行器（HashJoin/Aggregate/Distinct/ExternalSort）的磁盘溢出：

- 文件格式: `[RunHeader: 40 bytes magic+checksum+metadata] [Row: 8 bytes length LE + postcard 编码 Vec<Value>]...`
- 行数据使用 `postcard` 紧凑二进制格式
- Header 使用手写二进制 + FNV-1a 64-bit 校验码
- 每行独立长度前缀帧，可逐行读取
- `DiskQuota` 2 GiB 默认配额管理
- schema_fingerprint 用于读取端校验列结构

**事务/同步数据** — WAL + Outbox + Checkpoint Manifest

- WAL: 完全手写 LE 二进制，32KB 块对齐，CRC32 校验，LSN 链
- Outbox: `serde` derive + `postcard` 编码
- Checkpoint Manifest: `serde` derive + `postcard` 编码
- Undo Log: `postcard` 编码

### 1.3 序列化基础设施

linkrs 使用了**三种不同的序列化框架**，按使用场景分布：

| 框架 | 使用场景 | 特点 |
|------|---------|------|
| 手写二进制 | WAL、OrderedCodec、持久化格式 | 紧凑、确定布局、高性能 |
| `serde` + `postcard` | Outbox、Checkpoint、Spill 行、Undo Log、Index Manifest | 开发效率高、紧凑二进制 |
| `serde_json` | Schema 定义 | 可读性好 |

这三种框架之间没有共同的抽象接口。每种序列化方式独立实现 encode/decode 逻辑，各自管理字节序、长度前缀、校验等基础设施。

---

## 二、ladybug 序列化方案

### 2.1 存储数据序列化

ladybug 采用**统一序列化抽象 + 页级物理压缩**的架构。

**序列化抽象层** — `common/serializer/`

定义了一组统一的接口：

```
Writer (抽象类)
  ├── write(data, size)           // 原始字节写入
  ├── getSize() / clear() / flush() / sync()
  └── 子类: BufferWriter, BufferedFile, InMemFileWriter

Reader (抽象类)
  ├── read(data, size)            // 原始字节读取
  ├── finished() / skip()
  └── 子类: BufferReader, BufferedFile...

Serializer<T>
  ├── serializeValue(T)           // 基本类型
  ├── serializeVector<T>()        // 容器类型
  ├── serializeMap<K,V>()
  ├── serializeOptionalValue<T>()
  └── serializeUnorderedSet<T>()

Deserializer<T>
  ├── deserializeValue(T)         // 基本类型
  ├── deserializeVector<T>()      // 容器类型
  ├── beginReadLimit(size)        // 安全边界读取
  ├── skipReadLimit()             // 格式向前兼容
  └── storageVersion              // 格式版本管理
```

所有需要序列化的类型（WAL 记录、Value、ValueVector、Catalog 条目、查询计划等）都实现 `serialize(Serializer&)` / `static deserialize(Deserializer&)` 方法。这是一种**侵入式序列化**设计，每个类型自行控制序列化格式。

**页级物理压缩层** — `storage/compression/` + `storage/table/`

列数据的物理存储采用**分段分析 + 逐页压缩**的架构：

```
ColumnChunk (逻辑列块)
  └── ColumnChunkData[] (数据段)
       ├── append(value)           // 内存追加
       ├── finalize()              // 分析 min/max，选择压缩算法
       ├── flush(PageAllocator)    // 分页压缩写入
       └── CompressionMetadata     // 每段独立的压缩参数

CompressionType: UNCOMPRESSED | INTEGER_BITPACKING | BOOLEAN_BITPACKING | CONSTANT | ALP
```

核心特点：
- **分段决策**: 同一列的不同 `ColumnChunkData` 段可以使用不同压缩策略
- **统计感知**: `CompressionMetadata` 存储 min/max，支持谓词下推
- **页级写入**: 压缩按 `LBUG_PAGE_SIZE` 页边界分块，支持随机页读取
- **幕写机制**: 通过 `ShadowFile` 先写 shadow 再交换，保证崩溃安全

ALP 浮点编码具有完整的异常处理机制：
- 分析阶段：采集共同指数 `exp` 和因子 `fac`
- 编码阶段：将浮点编码为整数，无法无损编码的值写入异常表
- 读取阶段：先批量解压整数，再遍历异常表修补

**索引键编码**

使用 ART (Adaptive Radix Tree) 索引，键编码在 `art_index.cpp` 中实现。同时有 `order_by_key_encoder` 用于排序时生成保序键。

### 2.2 中间数据序列化

**WAL 日志层** — `storage/wal/`

WAL 序列化完全基于 Serializer/Deserializer 抽象：

```
WALRecord (基类)
  ├── serializeWithLength(Serializer&, const WALRecord&)
  │     → 长度前缀 + 序列化内容
  ├── deserialize(Deserializer&)
  └── 子类 (17 种记录类型):
       TableInsertionRecord, NodeUpdateRecord, CreateIndexRecord,
       CommitRecord, CheckpointRecord, ...
```

WAL 写入的是**未压缩的原始值**——WAL 追求写入速度和恢复简单性，物理压缩仅在 ColumnChunk flush 阶段发生。WAL 配有 ChecksumWriter/ChecksumReader 做完整性校验。

**ValueVector 序列化**

`ValueVector` 是查询执行过程中间数据的主要载体，支持通过 Serializer 序列化/反序列化。这使得：
- WAL 日志可以序列化插入操作的 ValueVector
- Checkpoint 可以序列化列数据的快照
- 支持跨阶段的中间结果传递

### 2.3 序列化基础设施

ladybug 采用**单一统一抽象**的设计：

- 所有序列化通过 `Serializer`/`Deserializer` 接口
- 底层 Writer/Reader 可插拔（内存 Buffer、文件、网络流）
- `storageVersion` 字段支持格式演变
- `beginReadLimit`/`skipReadLimit` 实现安全的格式向前兼容（新字段自动用零填充）
- 每个需要序列化的类型都实现 `serialize`/`deserialize` 静态方法

---

## 三、定性对比

### 3.1 架构设计

| 维度 | linkrs | ladybug |
|------|--------|---------|
| 序列化抽象 | **无统一抽象**，三种框架混用 | **统一 Serializer/Deserializer 抽象** |
| 序列化耦合 | **外部式**（编码器与数据结构分离） | **侵入式**（类型内实现 serialize 方法） |
| 物理压缩粒度 | **文件级** zstd（先写文件再压缩） | **页级** 压缩（Frame-of-Reference + FastPFOR） |
| 编码与压缩关系 | 编码（内存）→ 独立物理压缩（文件级） | 编码决策（finalize）→ 页级压缩写入（flush） |
| 格式版本管理 | 各层独立：OrderedCodec v2, WAL v2, Run v1 | Deserializer 统一 `storageVersion` |
| 向前兼容 | 各层自行处理 | `beginReadLimit`/`skipReadLimit` 零填充兜底 |

### 3.2 压缩策略

| 维度 | linkrs | ladybug |
|------|--------|---------|
| 压缩决策时机 | 编码选择（写入时）+ 全局 zstd（flush 后） | 分段分析（finalize）→ 直接压缩写入 |
| 统计持久化 | ColumnStats 仅用于选择，不持久化 | CompressionMetadata.min/max 持久化供查询优化 |
| ALP 异常处理 | 无异常表，近似无损（factor 精度取舍） | 完整异常表，精确无损 |
| 压缩可选择性 | 每个文件独立压缩，全量读取 | 页级压缩，支持随机页读取 |
| 空间效率 | 较高（zstd 全局压缩好） | 较高（语义压缩 + 页剩余空间浪费） |

### 3.3 中间数据序列化

| 维度 | linkrs | ladybug |
|------|--------|---------|
| 查询溢出方案 | postcard 紧凑二进制 + header + checksum | 主要依赖 WAL + ValueVector serialize |
| 溢出数据校验 | FNV-1a 64-bit | 通过 Serializer 层 CRC 或传输层保证 |
| 磁盘配额 | 专用 `DiskQuota` (2 GiB 默认) | 未观察到独立配额机制 |

### 3.4 优缺点定性

**linkrs 的优势：**

1. 编码层设计灵活：五种专用编码器各自独立，CompressionSelector 自动决策，易于扩展新编码
2. postcard 开发效率高：serde derive 零样板代码，对配置/元数据/同步事件序列化非常便利
3. OrderedCodec 设计精巧：字节序即语义序，范围扫描零反序列化开销
4. 文件级 zstd 实现简单：整体压缩效果好，仅需一个 API 调用

**ladybug 的优势：**

1. 统一序列化抽象：所有序列化走同一套接口，代码一致性高，易于审查和维护
2. 页级压缩：支持随机页读取，大表无需全量解压
3. 分段压缩决策：同一列不同区段的压缩策略独立，适应数据特征变化
4. 统计与应用耦合：CompressionMetadata 既服务压缩效率又服务查询优化
5. 格式向前兼容：Deserializer 的 readLimit 机制 + storageVersion 确保格式演进安全
6. ALP 精确异常处理：真无损浮点压缩

---

## 四、linkrs 设计考虑不够全面的方面

### 4.1 缺少统一序列化抽象

linkrs 的序列化分散在三种独立框架中（手写二进制、serde+postcard、serde_json+serde），每种有各自的字节序约定、长度编码、错误处理模式。这带来以下问题：

- **维护成本**: 新增序列化场景时需重复实现基础编码逻辑
- **一致性风险**: WAL 用 LE，OrderedCodec 用 BE，容易混淆
- **格式演进困难**: 没有统一的版本标记和向前兼容机制（ladybug 的 `storageVersion` + `readLimit` 模式更安全）

### 4.2 编码层与持久化层割裂

编码层（Dictionary/RLE/BitPacking/FSST/ALP）和持久化层之间的契约不够清晰：

- 编码结构是**纯内存对象**，没有 `serialize`/`deserialize` 方法
- Flush 时通过 `get_flush_data()` 取原始字节数组 + encoding_type 标记，但编码的**元数据**（字典表、ALP factor、FSST 符号表、RLE runs 结构）走的是列数据的通用二进制路径，没有结构化的格式
- 加载时通过 `deferred_encodings` 重建编码，在 flush 时如果仍有未应用的 deferred encoding 会打印警告——说明 pipeline 未完全融合

这与 ladybug 形成对比：ladybug 的 ColumnChunkData 在 flush 时直接调用 `compressionAlg->compressNextPage()` 将数据按页压缩写入，读取时从页数据直接解压，编码格式完全自包含在页数据中。

### 4.3 缺少页级/段级压缩

linkrs 的物理压缩是**文件级**的：先写完整文件，再用 zstd 压缩整个文件再写回。这导致：

- **无法随机读取**: 读取任意一列或任意一页都需要解压整个文件
- **无法增量更新**: 修改一行数据需要解压整个文件、修改、再压缩整个文件
- **大表不友好**: 顶点表/边属性表较大时，全量解压开销显著

ladybug 的页级压缩通过 `LBUG_PAGE_SIZE` 分页、每页独立压缩，可以：
- 随机读取指定页
- 增量更新单页（通过 ShadowFile）
- 仅解压需要的页

### 4.4 ALP 实现缺少异常处理

linkrs 的 ALP 实现采用简化方案：寻找最优 `10^k` 因子，将浮点值乘以因子后 round 为整数，再做 BitPacking 压缩。这种方式存在的问题：

- **非精确无损**: 当浮点值无法在给定因子下精确表示为整数时（如 `0.1 * 10 = 1.0` 对某些精度丢失），会做近似取舍
- **无异常表**: 无法编码的值直接 round 近似，而非存储为异常值单独管理

ladybug 的 ALP 实现包含完整的异常处理：编码时检测无法无损往返的值，将其写入独立的异常 Chunk；读取时先批量解压，再遍历异常表修补。这是真正的**数学无损**压缩。

### 4.5 缺少压缩统计持久化

linkrs 的 `ColumnStats` 仅用于 `CompressionSelector` 做编码选择（运行时使用），但**不持久化**。这意味着：

- 查询优化器无法利用 min/max 做谓词下推或范围裁剪
- 加载后无法获得列的基数、空值数、数据范围等统计信息，需重新全量扫描

ladybug 的 `CompressionMetadata` 同时存储 `min`/`max`/`compressionType`，既指导压缩又供查询引擎使用。

### 4.6 编码选择策略的局限性

`CompressionSelector` 使用固定的启发式阈值（如 `cardinality_ratio < 0.5`、`avg_length >= 20.0`），存在以下问题：

- **阈值硬编码**: 不同硬件环境（SSD vs HDD）、数据分布下最优阈值可能不同
- **无反馈学习**: 不根据实际压缩率调整选择策略
- **无成本模型**: 不考虑不同编码的解码开销差异（如 FSST 解码比 Dictionary 慢）
- **字符串跳过逻辑存在矛盾**: `select_string_encoding` 中先判断 `cardinality_ratio < 0.5` 尝试字典、再判断 `avg_length >= 20.0 && cardinality_ratio > 0.5` 尝试 FSST、最后 `skip_high_cardinality_short_strings` 直接跳过。当 cardinality_ratio 在 0.5~0.8 且 avg_length < 20 时三段逻辑都跳过，返回 None，但实际上这段数据可能有压缩价值

### 4.7 RLE 解码性能瓶颈

RLE 的 `get(row_idx)` 实现为 O(n_runs) 的线性扫描：

```rust
pub fn get(&self, row_idx: usize) -> Option<Value> {
    let mut cumulative = 0;
    for run in &self.runs {
        cumulative += run.count;
        if row_idx < cumulative {
            return Some(run.value.clone());
        }
    }
    None
}
```

对于大压缩比场景（n_runs 很小），这是可接受的。但对于压缩率不理想时产生大量 run 的情况（如交替值），每次随机访问都是 O(n)。缺少索引加速结构（如前缀和数组 + 二分查找）。

### 4.8 缺少格式边界安全检查

linkrs 的手写二进制序列化（WAL、持久化格式）各自实现了长度校验，但缺少类似 ladybug `Deserializer::beginReadLimit` / `skipReadLimit` 的**统一安全读取机制**：

- ladybug 的 readLimit 确保解序列化不会读超过记录的声明边界，剩余未识别的字段（新版本新增）自动用零填充
- linkrs 各格式自行管理边界，没有统一的"记录边界"概念，格式演进的向后兼容需要每层各自处理

### 4.9 溢出格式缺少压缩

Spill 的 Run 文件格式使用 postcard 编码行数据，但没有压缩：
- 大数据量溢出时磁盘占用可能很大
- postcard 虽然比 JSON 紧凑，但仍然比压缩后的二进制大得多
- 缺少类似 WAL 的压缩 flag 或可选压缩层

### 4.10 字典编码的字典排序不稳定性

Dictionary 的 `StringDictionary` 使用 `HashMap<Arc<str>, u32>` 做反向映射，字典的值列表按插入顺序排列。当从磁盘加载重建字典时，如果加载顺序不同（或并发插入不同），可能导致同一组字符串产生不同的索引映射。这会使生成的二进制表示不稳定，影响：
- 增量同步 diff 效率
- Checksum 一致性校验

ladybug 的压缩方案在页数据中直接存储压缩参数（min/max/offset/bitWidth），解码是**确定性的**，不依赖内存中的字典顺序。

### 4.11 键编码的类型覆盖不完整

`OrderedCodec` 仅支持 22 种 Value 类型中的 12 种标量类型，不支持 Vertex、Edge、Path、List、Map、Set、Geography、Vector、DataSet、Json、JsonB、Interval。对于这些类型：

- 无法建立基于 OrderedCodec 的索引
- 编码时会返回错误
- 与 ladybug 对比，ladybug 的 Serializer 对所有 LogicalType 都有序列化支持（包括 LIST、STRUCT、MAP、UNION、NODE、REL 等复杂类型）

---

## 五、总结

linkrs 在序列化方面采用了务实的"够用"策略：用 Rust 生态（serde/postcard/serde_json）快速搭建元数据和中间数据的序列化，用手写二进制处理性能敏感的 WAL 和索引键路径。其编码层（Dictionary/RLE/BitPacking/FSST/ALP）体现了对列存储压缩的良好理解。在初期开发阶段，这种"各层各自选择最合适的工具"的策略是合理的。

ladybug 展示了一个更为成熟的序列化体系设计：统一抽象、页级压缩、格式版本管理、向前兼容、统计-查询耦合。这些设计决策体现了更长的工程积累和对数据库序列化全生命周期（写入、随机读、增量更新、格式演进、查询优化）的全局考量。

linkrs 当前最核心的不足集中在两个方向：**物理压缩粒度（文件级而非页级）限制了随机读取和增量更新能力**，以及**编码层与持久化层的耦合不够紧密（缺乏结构化的编码元数据持久化格式，编码 pipeline 未完全闭环）**。其余方面在当前单机小规模部署定位下属于合理的取舍，随着系统规模和复杂度增长需要逐步补齐。
