# Encoding & Compression 分阶段修改方案

## 1. 目标

基于 `docs/analysis/zstd-fsst-usage.md` 与 `docs/analysis/serialization-analysis.md` 中识别的问题，解决编码层与持久化层割裂、物理压缩粒度过粗、ALP 缺少异常处理、编码选择策略不足等核心缺陷。

总体目标：

- 编码层实现结构化持久化，编码元数据自包含
- 物理压缩从文件级改为页级，支持随机读取和增量更新
- ALP 实现数学无损压缩
- 编码统计信息持久化，服务查询优化
- 补齐编码选择策略、运行时监控、安全性等方面的短板

本项目处于开发阶段，不要求保留旧接口兼容层。每个阶段完成后应直接删除被替代的接口和路径。

## 2. 实施原则

1. 正确性优先于性能优化。
2. 每个阶段先补失败测试，再修改实现。
3. 物理压缩粒度变更必须有明确版本标记。
4. 编码元数据持久化格式需自描述、可扩展。
5. 不引入新的序列化框架，优先复用现有 postcard + serde。
6. 每阶段完成后运行 storage 单元测试、集成测试和 workspace compile check。

## 3. 阶段概览

| 阶段 | 主题 | 优先级 | 前置条件 |
| --- | --- | --- | --- |
| 0 | 建立回归测试基线 | P0 | 无 |
| 1 | 编码持久化与 FSST 符号表序列化 | P0 | 阶段 0 |
| 2 | 页级物理压缩替换文件级压缩 | P0 | 阶段 0 |
| 3 | ALP 异常处理与统计持久化 | P1 | 阶段 1 |
| 4 | 编码选择策略与运行时优化 | P1 | 阶段 1 |
| 5 | 安全性与格式边界检查 | P2 | 阶段 2 |

阶段 1 和阶段 2 在完成阶段 0 后可以独立开发。

## 5. 阶段 1：编码持久化与 FSST 符号表序列化

### 5.1 目标

解决编码层与持久化层割裂问题。所有编码类型应具备结构化持久化能力，编码元数据自包含在存储格式中。

### 5.2 问题分析

当前编码结构是纯内存对象，没有 `serialize`/`deserialize` 方法。Flush 时仅写入原始字节 + 1 字节 `encoding_type` 标记，编码元数据（字典表、ALP factor、FSST 符号表、RLE runs）走通用二进制路径，缺乏结构化格式。加载时通过 `deferred_encodings` 重建，pipeline 未完全闭环。

### 5.3 修改内容

#### 5.3.1 定义编码元数据 trait

在 `crates/graphdb-storage/src/storage/encoding/mod.rs` 中：

```rust
pub trait Encoding: Send + Sync {
    fn encode(&mut self, values: &[Value]) -> StorageResult<()>;
    fn decode(&self) -> StorageResult<Vec<Value>>;
    fn memory_usage(&self) -> usize;
    fn encoding_type(&self) -> EncodingType;
    /// 序列化编码元数据到 writer，返回写入的字节数
    fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize>;
    /// 从 reader 反序列化编码元数据
    fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> where Self: Sized;
}
```

#### 5.3.2 FSST 符号表持久化

修改 `crates/graphdb-storage/src/storage/encoding/fsst.rs`：

- 为 `FsstSymbolTable` 实现 `serialize_meta` / `deserialize_meta`：
  - 格式: `[2 bytes symbol_count LE] [for each: 1 byte len][bytes][1 byte code]`
- 修复 `fsst.rs:142` 的 code 溢出风险：
  ```rust
  for (idx, (ngram, _freq)) in ngrams.into_iter().enumerate() {
      if idx >= max_symbols.min(SYMBOL_TABLE_SIZE) - 1 { break; }
      let code = (idx + 1) as u8;
      self.table.insert(ngram, code);
  }
  ```

#### 5.3.3 各编码实现持久化

- **Dictionary**: 序列化 `HashMap<Arc<str>, u32>` → `[4 bytes count LE][for each: 4 bytes len LE][bytes][4 bytes index LE]`
- **RLE**: 序列化 `Vec<RleRun<T>>` → `[4 bytes count LE][for each run: count + value]`
- **BitPacking**: 序列化 `min_value` + `bit_width` + `BitVec` 字节
- **ALP**: 序列化 `factor` + `exception_count` + 异常表（详见阶段 3）

#### 5.3.4 修改 flush 流程

修改各 `persistence.rs` 中的 flush 逻辑：

1. flush 时调用 `encoding.serialize_meta()` 写入编码元数据到独立区域
2. 移除 `deferred_encodings` 警告，编码结构在 load 时直接通过 `deserialize_meta()` 还原
3. 加载时不再需要重新扫描原始数据重建编码

#### 5.3.5 消除 `deferred_encodings` 机制

编码持久化完成后，`deferred_encodings` 不再是必须的。移除相关警告和重建逻辑。

### 5.4 验收标准

- 所有编码类型通过 serialize → deserialize round-trip 测试
- FSST 符号表持久化后编码结果与内存版本一致
- VertexTable flush/load 端到端测试通过，不再触发 deferred encoding 警告
- `cargo test --package graphdb-storage` 全绿

### 5.5 影响范围

- `crates/graphdb-storage/src/storage/encoding/*.rs`
- `crates/graphdb-storage/src/storage/*/persistence.rs`

## 6. 阶段 2：页级物理压缩替换文件级压缩

### 6.1. 目标

将物理压缩从文件级改为页级，支持随机页读取和增量更新，消除大表全量解压瓶颈。

### 6.2 问题分析

当前 `compression.rs` 对完整文件做 zstd 压缩。读取任意一列需解压整个文件；修改一行需解压 → 修改 → 重新压缩整个文件。大表场景下开销显著。

### 6.3 修改内容

#### 6.3.1 定义页格式

```
PageHeader:
  [4 bytes magic: "PGZC"]
  [2 bytes page_size LE]        // 页内数据大小（不含 header）
  [1 bytes compression_type]    // 0x00=未压缩, 0x01=zstd
  [4 bytes crc32 LE]           // 页内数据 CRC32
  [4 bytes compressed_len LE]  // 压缩后长度（若未压缩则等于 page_size）

PageData:
  [compressed_len bytes]        // 压缩数据或原始数据
```

页大小可配置，默认 64KB（对齐 `LBUG_PAGE_SIZE` 思路）。

#### 6.3.2 实现 PageWriter / PageReader

在 `crates/graphdb-storage/src/storage/compression.rs` 中新增：

```rust
pub struct PageWriter {
    page_size: usize,
    compression_level: i32,
}

impl PageWriter {
    pub fn new(page_size: usize, level: i32) -> Self;
    pub fn write_page(&mut self, writer: &mut impl Write, data: &[u8]) -> StorageResult<()>;
    pub fn flush_page(&mut self) -> StorageResult<()>;
}

pub struct PageReader {
    page_size: usize,
}

impl PageReader {
    pub fn new(page_size: usize) -> Self;
    pub fn read_page(&mut self, reader: &mut impl Read) -> StorageResult<Vec<u8>>;
    pub fn skip_page(&mut self, reader: &mut impl Read) -> StorageResult<()>;
}
```

#### 6.3.3 文件头格式

每个列数据文件开头增加全局 header：

```
ColumnFileHeader:
  [8 bytes magic: "GRPHDCOL"]
  [2 bytes version LE]          // 当前为 1
  [2 bytes page_size LE]        // 页大小
  [4 bytes page_count LE]       // 总页数
  [4 bytes total_rows LE]       // 总行数
  [bytes reserved: 32]          // 保留字段
```

#### 6.3.4 修改 flush 流程

1. flush 时使用 `PageWriter` 按页写入数据
2. 不足一页的数据在 flush 末尾 partial write
3. 更新 ColumnStats 中的 page_count
4. 加载时通过 `PageReader` 按需读取指定页

#### 6.3.5 Shadow File 机制

通过写入 shadow file 再原子 rename 保证崩溃安全：

1. flush 时先写入 `.tmp` 文件
2. 写完后 rename 为正式文件名
3. 启动时检测残留 `.tmp` 文件并清理

#### 6.3.6 随机读取 API

为 ColumnStore 增加按页读取接口：

```rust
impl ColumnStore {
    pub fn read_page(&self, page_idx: usize) -> StorageResult<Vec<u8>>;
    pub fn read_rows_in_range(&self, start: usize, end: usize) -> StorageResult<Vec<Value>>;
}
```

### 6.4 验收标准

- 页写入 → 页读取 round-trip 测试通过
- 随机页读取测试：读取第 N 页不触发全量解压
- Shadow file 崩溃恢复测试：写至中途 panic 后重启可自动清理
- 大表（>10MB）读取单页性能显著优于文件级解压
- `cargo test --package graphdb-storage` 全绿

### 6.5 影响范围

- `crates/graphdb-storage/src/storage/compression.rs`
- `crates/graphdb-storage/src/storage/*/persistence.rs`
- `crates/graphdb-storage/src/storage/vertex/column_store.rs`

## 7. 阶段 3：ALP 异常处理与统计持久化

### 7.1 目标

ALP 实现数学无损压缩（含异常表），统计信息持久化服务查询优化。

### 7.2 问题分析

当前 ALP 实现寻找最优 `10^k` 因子后 round 为整数再 BitPacking，无法精确表示的值做近似取舍。这不是数学无损的。缺少异常表机制。

ColumnStats 仅用于运行时编码选择，不持久化。查询优化器无法利用 min/max 做谓词下推。

### 7.3 修改内容

#### 7.3.1 ALP 异常表

修改 `crates/graphdb-storage/src/storage/encoding/alp.rs`：

```rust
pub struct AlpEncoding {
    pub factor: i32,                  // 10^k
    pub bitpacking: BitPackingEncoding,
    pub exceptions: Vec<ExceptionEntry>,  // 异常表
}

pub struct ExceptionEntry {
    pub row_idx: u32,                 // 异常值所在行号
    pub original_value: f64,          // 原始值
}

fn encode(&mut self, values: &[Value]) -> StorageResult<()> {
    for (idx, value) in values.iter().enumerate() {
        let scaled = value * 10f64.powi(self.factor);
        let rounded = scaled.round();
        if (rounded / 10f64.powi(self.factor) - value).abs() < EPSILON {
            // 无损编码
            self.bitpacking.push(rounded as i64);
        } else {
            // 写入异常表，BitPacking 占位 0
            self.exceptions.push(ExceptionEntry { row_idx: idx as u32, original_value: *value });
            self.bitpacking.push(0);
        }
    }
}

fn decode(&self) -> StorageResult<Vec<Value>> {
    let integers = self.bitpacking.decode()?;
    let mut result: Vec<f64> = integers.iter()
        .map(|&i| i as f64 / 10f64.powi(self.factor))
        .collect();
    // 遍历异常表修补
    for exc in &self.exceptions {
        result[exc.row_idx as usize] = exc.original_value;
    }
    Ok(result.into_iter().map(Value::Float64).collect())
}
```

#### 7.3.2 统计信息持久化

在 ColumnStats 中增加持久化字段，flush 时写入 meta 文件：

```rust
pub struct PersistentColumnStats {
    pub min_value: Option<Value>,
    pub max_value: Option<Value>,
    pub null_count: u64,
    pub distinct_count: Option<u64>,  // 可选，近似值
    pub encoding_type: EncodingType,
    pub compressed_size: u64,
    pub raw_size: u64,
}
```

#### 7.3.3 服务查询优化

为查询层暴露统计信息读取接口：

```rust
impl ColumnStore {
    pub fn stats(&self) -> &PersistentColumnStats;
}
```

查询优化器可据此做范围裁剪。

### 7.4 验收标准

- ALP 编码 → 解码后与原始数据逐值相等（bit-exact）
- 含异常值的浮点序列 round-trip 测试通过
- 统计信息 flush → load 后一致
- 查询优化器可利用 stats 做范围裁剪（最小可用接口）
- `cargo test --package graphdb-storage` 全绿

### 7.5 影响范围

- `crates/graphdb-storage/src/storage/encoding/alp.rs`
- `crates/graphdb-storage/src/storage/encoding/selector.rs`
- `crates/graphdb-storage/src/storage/*/persistence.rs`

## 8. 阶段 4：编码选择策略与运行时优化

### 8.1 目标

改进编码选择策略的灵活性和智能性，解决 RLE 性能瓶颈、字典排序不稳定性、FSST 动态更新缺失等问题。

### 8.2 修改内容

#### 8.2.1 编码选择策略改进

修改 `crates/graphdb-storage/src/storage/encoding/selector.rs`：

1. **配置化阈值**: 将 `string_min_rows`、`avg_length_threshold`、`cardinality_ratio_threshold` 改为从配置读取
2. **修复选择逻辑矛盾**: 当 `cardinality_ratio ∈ (0.5, 0.8)` 且 `avg_length < 20` 时应回退到 Dictionary 而非直接跳过
3. **跳过逻辑修复**: `skip_high_cardinality_short_strings` 改为尝试 Dictionary 编码

#### 8.2.2 RLE 解码优化

修改 `crates/graphdb-storage/src/storage/encoding/rle.rs`：

```rust
pub struct RleEncoding<T: Clone + PartialEq> {
    runs: Vec<RleRun<T>>,
    cumulative_counts: Vec<usize>,  // 前缀和数组，支持二分查找
}

impl<T: Clone + PartialEq> RleEncoding<T> {
    pub fn get(&self, row_idx: usize) -> Option<T> {
        match self.cumulative_counts.binary_search(&(row_idx + 1)) {
            Ok(idx) | Err(idx) if idx < self.runs.len() => Some(self.runs[idx].value.clone()),
            _ => None,
        }
    }
}
```

`get()` 从 O(n_runs) 优化为 O(log n_runs)。

#### 8.2.3 字典编码排序稳定性

修改 `crates/graphdb-storage/src/storage/encoding/dictionary.rs`：

- 字典重建时按字符串字典序排序，确保相同输入始终产生相同索引映射
- 提供 `sorted_entries()` 方法用于确定性序列化

#### 8.2.4 FSST 动态重建

为 `FsstColumn` 增加重建接口：

```rust
impl FsstColumn {
    pub fn rebuild(&mut self, new_strings: &[String]) -> StorageResult<()> {
        // 合并旧符号表覆盖的新样本
        // 重新训练并替换编码
    }
}
```

触发策略：当新增数据量超过已编码数据的 20% 时自动重建，阈值可配置。

#### 8.2.5 压缩率监控

为编码选择器和 flush 流程增加统计信息：

```rust
pub struct CompressionMetrics {
    pub encoding_type: EncodingType,
    pub raw_bytes: u64,
    pub encoded_bytes: u64,
    pub compression_ratio: f64,
    pub encode_time_us: u64,
    pub decode_time_us: u64,
}
```

可选通过日志或 metrics 接口暴露。

#### 8.2.6 Spill 格式压缩

修改 `crates/graphdb-query/src/executor/spill.rs`：

- 行数据写入时增加可选 zstd 压缩层
- RunHeader 增加 1 byte compression_type 标记
- 默认启用压缩，极小数据跳过

### 8.3 验收标准

- 配置化阈值可通过配置修改并生效
- RLE `get()` 在 1000+ runs 场景下性能提升显著（benchmark）
- 字典编码 round-trip 后二进制表示稳定（相同输入相同输出）
- FSST 动态重建后编码率不退化
- Spill 数据压缩后磁盘占用减少
- `cargo test --package graphdb-storage` 和 `cargo test --package graphdb-query` 全绿

### 8.4 影响范围

- `crates/graphdb-storage/src/storage/encoding/selector.rs`
- `crates/graphdb-storage/src/storage/encoding/rle.rs`
- `crates/graphdb-storage/src/storage/encoding/dictionary.rs`
- `crates/graphdb-storage/src/storage/encoding/fsst.rs`
- `crates/graphdb-storage/src/storage/encoding/alp.rs`
- `crates/graphdb-query/src/executor/spill.rs`

## 9. 阶段 5：安全性与格式边界检查

### 9.1 目标

为手写二进制序列化路径增加统一的格式边界安全检查机制，防止格式演进时的解析错误。

### 9.2 问题分析

当前 linkrs 的手写二进制（WAL、持久化格式）各自实现长度校验，缺少类似 ladybug `Deserializer::beginReadLimit` / `skipReadLimit` 的统一安全读取机制。格式演进时需每层各自处理向后兼容。

### 9.3 修改内容

#### 9.3.1 定义安全读取 trait

在 `crates/graphdb-storage/src/storage/` 新增 `safe_read.rs`：

```rust
pub struct BoundedReader<'a> {
    inner: &'a mut dyn Read,
    remaining: usize,
}

impl<'a> BoundedReader<'a> {
    pub fn new(inner: &'a mut impl Read, limit: usize) -> Self;
    pub fn read_exact(&mut self, buf: &mut [u8]) -> StorageResult<()>;
    pub fn skip_all(&mut self) -> StorageResult<()>;
    pub fn remaining(&self) -> usize;
}

pub trait SafeSerializable {
    fn serialize(&self, writer: &mut impl Write) -> StorageResult<()>;
    fn deserialize(reader: &mut BoundedReader) -> StorageResult<Self> where Self: Sized;
}
```

#### 9.3.2 应用到关键格式

- WAL 记录读取时使用 `BoundedReader`
- 列数据文件 page header 解析使用边界检查
- Postcard 解码增加长度前缀校验

#### 9.3.3 格式版本管理

在关键文件格式中增加 version 字段：

```
文件格式头部增加: [2 bytes format_version LE]
```

版本不匹配时返回明确的 StorageError::UnsupportedVersion。

### 9.4 验收标准

- 所有手写二进制路径通过边界读取测试
- 格式版本不匹配时返回错误而非 panic
- `cargo clippy --all-targets --all-features` 无新增 warning

### 9.5 影响范围

- `crates/graphdb-storage/src/storage/`（新增 safe_read.rs）
- `crates/graphdb-transaction/src/transaction/wal/writer/` 和 reader

## 10. 依赖关系与并行性

```
阶段 0 ──┬── 阶段 1 (编码持久化) ──┬── 阶段 3 (ALP + 统计)
         │                         │
         └── 阶段 2 (页级压缩) ─────┴── 阶段 5 (安全性)
                                   │
                                   └── 阶段 4 (策略优化)
```

- 阶段 1 和 2 可并行开发
- 阶段 3 依赖 阶段 1（编码 trait 稳定）
- 阶段 4 依赖 阶段 1（编码 trait 稳定）
- 阶段 5 依赖 阶段 2（页格式确定）

## 11. 问题优先级对照

| 原始问题 | 来源文档 | 解决阶段 |
| --- | --- | --- |
| 编码层与持久化层割裂 | serialization 4.2 | 阶段 1 |
| FSST 符号表持久化 | zstd-fsst 问题 1 | 阶段 1 |
| FSST code 溢出风险 | zstd-fsst 代码质量 | 阶段 1 |
| 文件级 → 页级压缩 | serialization 4.3 | 阶段 2 |
| ALP 缺少异常处理 | serialization 4.4 | 阶段 3 |
| 统计信息不持久化 | serialization 4.5 | 阶段 3 |
| 编码选择策略局限 | serialization 4.6 | 阶段 4 |
| RLE 解码性能瓶颈 | serialization 4.7 | 阶段 4 |
| 字典排序不稳定性 | serialization 4.10 | 阶段 4 |
| FSST 动态更新缺失 | zstd-fsst 问题 2 | 阶段 4 |
| 压缩率监控缺失 | zstd-fsst 问题 5 | 阶段 4 |
| Spill 格式缺少压缩 | serialization 4.9 | 阶段 4 |
| 缺少格式边界安全检查 | serialization 4.8 | 阶段 5 |
| 硬编码阈值 | zstd-fsst 问题 3 | 阶段 4 |
| 键编码类型覆盖不完整 | serialization 4.11 | 后续（非本方案范围） |
| 统一序列化抽象 | serialization 4.1 | 后续（非本方案范围，需跨 crate 整体设计） |
