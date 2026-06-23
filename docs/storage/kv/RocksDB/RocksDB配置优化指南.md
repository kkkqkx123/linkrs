# RocksDB 配置优化指南

## 概述

本文档详细介绍 RocksDB 的配置选项和优化策略，针对 GraphDB 场景提供最佳实践配置建议。

---

## 1. 基础配置

### 1.1 必要配置

```rust
use rocksdb::{DB, Options};

let mut opts = Options::default();
opts.create_if_missing(true);  // 数据库不存在时创建
```

### 1.2 推荐基础配置

```rust
use rocksdb::{DB, Options, DBCompressionType};

let mut opts = Options::default();
opts.create_if_missing(true);
opts.set_compression(DBCompressionType::Lz4);  // 使用 LZ4 压缩
```

---

## 2. 内存配置

### 2.1 块缓存（Block Cache）

块缓存用于缓存从磁盘读取的数据块，减少磁盘 I/O。

#### 配置选项

```rust
use rocksdb::Options;

let mut opts = Options::default();

// 设置块缓存大小（字节）
opts.set_block_cache_size(256 * 1024 * 1024);  // 256MB
```

#### GraphDB 推荐配置

**小型数据库（< 1GB）**:
```rust
opts.set_block_cache_size(128 * 1024 * 1024);  // 128MB
```

**中型数据库（1GB - 10GB）**:
```rust
opts.set_block_cache_size(512 * 1024 * 1024);  // 512MB
```

**大型数据库（> 10GB）**:
```rust
opts.set_block_cache_size(1024 * 1024 * 1024);  // 1GB
```

#### 缓存分片

```rust
// 设置块缓存分片位数（2^6 = 64 个分片）
opts.set_table_cache_num_shard_bits(6);
```

**推荐**: 根据并发线程数调整，通常设置为 6-8。

### 2.2 写缓冲区（Write Buffer）

写缓冲区用于缓存写入操作，减少磁盘 I/O。

#### 配置选项

```rust
use rocksdb::Options;

let mut opts = Options::default();

// 设置每个写缓冲区的大小（字节）
opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB

// 设置最大写缓冲区数量
opts.set_max_write_buffer_number(32);

// 设置触发合并的最小写缓冲区数量
opts.set_min_write_buffer_number_to_merge(4);
```

#### GraphDB 推荐配置

**写入密集型**:
```rust
opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
opts.set_max_write_buffer_number(32);
opts.set_min_write_buffer_number_to_merge(4);
```

**读取密集型**:
```rust
opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
opts.set_max_write_buffer_number(16);
opts.set_min_write_buffer_number_to_merge(2);
```

**读写均衡**:
```rust
opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
opts.set_max_write_buffer_number(24);
opts.set_min_write_buffer_number_to_merge(3);
```

---

## 3. 压缩配置

### 3.1 压缩算法选择

RocksDB 支持多种压缩算法：

| 算法 | 压缩率 | CPU 开销 | 速度 | 推荐场景 |
|------|----------|-----------|------|----------|
| None | 无 | 无 | 最快 | 不需要压缩 |
| Snappy | 中等 | 低 | 快 | 通用场景 |
| Zlib | 高 | 高 | 慢 | 存储空间敏感 |
| LZ4 | 中等 | 低 | 快 | 推荐默认 |
| ZSTD | 高 | 中等 | 中等 | 平衡场景 |

### 3.2 全局压缩配置

```rust
use rocksdb::{Options, DBCompressionType};

let mut opts = Options::default();

// 设置全局压缩算法
opts.set_compression(DBCompressionType::Lz4);
```

### 3.3 分层压缩配置

不同层可以使用不同的压缩算法：

```rust
use rocksdb::{Options, DBCompressionType};

let mut opts = Options::default();

// 为不同层设置不同的压缩算法
// L0: 不压缩（频繁写入）
// L1-L2: Snappy（中等压缩）
// L3+: LZ4（高压缩）
opts.set_compression_per_level(&[
    DBCompressionType::None,      // L0
    DBCompressionType::None,      // L1
    DBCompressionType::Snappy,    // L2
    DBCompressionType::Snappy,    // L3
    DBCompressionType::Lz4,       // L4
    DBCompressionType::Lz4,       // L5
    DBCompressionType::Lz4,       // L6
]);
```

### 3.4 GraphDB 推荐配置

```rust
use rocksdb::{Options, DBCompressionType};

let mut opts = Options::default();

// 使用 LZ4 作为默认压缩
opts.set_compression(DBCompressionType::Lz4);

// 或者使用分层压缩
opts.set_compression_per_level(&[
    DBCompressionType::None,      // L0: 不压缩，快速写入
    DBCompressionType::None,      // L1: 不压缩
    DBCompressionType::Snappy,    // L2: 轻量压缩
    DBCompressionType::Lz4,       // L3+: 高压缩
]);
```

---

## 4. 压缩策略配置

### 4.1 压缩风格

RocksDB 支持两种压缩风格：

#### Level Compaction（默认）

```rust
use rocksdb::{Options, DBCompactionStyle};

let mut opts = Options::default();

// Level 风格（默认）
opts.set_compaction_style(DBCompactionStyle::Level);
```

**特点**:
- 多层结构（L0, L1, L2, ...）
- 每层大小按指数增长
- 适合读多写少的场景
- 写放大较高，读放大较低

#### Universal Compaction

```rust
use rocksdb::{Options, DBCompactionStyle};

let mut opts = Options::default();

// Universal 风格
opts.set_compaction_style(DBCompactionStyle::Universal);
```

**特点**:
- 所有数据在一个层中
- 适合写多读少的场景
- 写放大较低，读放大较高
- 历史数据保留更好

### 4.2 GraphDB 推荐配置

**通用场景（Level Compaction）**:
```rust
use rocksdb::{Options, DBCompactionStyle};

let mut opts = Options::default();
opts.set_compaction_style(DBCompactionStyle::Level);

// L0 层触发器
opts.set_level_zero_stop_writes_trigger(2000);
opts.set_level_zero_slowdown_writes_trigger(1600);
```

**写入密集型（Universal Compaction）**:
```rust
use rocksdb::{Options, DBCompactionStyle};

let mut opts = Options::default();
opts.set_compaction_style(DBCompactionStyle::Universal);
```

### 4.3 压缩触发器

```rust
use rocksdb::Options;

let mut opts = Options::default();

// L0 层停止写入触发器
opts.set_level_zero_stop_writes_trigger(2000);  // L0 文件数超过 2000 时停止写入

// L0 层慢化写入触发器
opts.set_level_zero_slowdown_writes_trigger(1600);  // L0 文件数超过 1600 时慢化写入

// 目标文件大小基数
opts.set_target_file_size_base(64 * 1024 * 1024);  // 64MB
```

---

## 5. 文件和 I/O 配置

### 5.1 最大打开文件数

```rust
use rocksdb::Options;

let mut opts = Options::default();

// 设置最大打开文件数
opts.set_max_open_files(10000);
```

**推荐**:
- 小型数据库: 1000-5000
- 中型数据库: 5000-10000
- 大型数据库: 10000-50000

### 5.2 同步和 WAL 配置

```rust
use rocksdb::{Options};

let mut opts = Options::default();

// 是否使用 fsync
opts.set_use_fsync(false);  // 使用 fdat而不是 fsync

// 每次同步的字节数
opts.set_bytes_per_sync(8388608);  // 819KB
```

**推荐**:
- 追求性能: `set_use_fsync(false)`
- 追求安全: `set_use_fsync(true)`

---

## 6. Bloom Filter 配置

Bloom Filter 用于减少磁盘读取，提高读取性能。

### 6.1 启用 Bloom Filter

```rust
use rocksdb::{Options};

let mut opts = Options::default();

// Bloom Filter 默认启用，无需额外配置
```

### 6.2 Bloom Filter 位数

```rust
use rocksdb::Options;

let mut opts = Options::default();

// 设置 Bloom Filter 位数（影响假阳性率）
// 默认值通常为 10
```

**推荐**:
- 小型数据库: 6-8
- 中型数据库: 10-12
- 大型数据库: 12-16

---

## 7. 列族配置

### 7.1 为不同列族设置不同配置

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType};

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

// 节点列族配置（读取密集）
let mut nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
nodes_cf_opts.set_compression(DBCompressionType::Lz4);

// 边列族配置（写入密集）
let mut edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
edges_cf_opts.set_compression(DBCompressionType::Snappy);

// 索引列族配置（读取密集）
let mut indexes_cf_opts = Options::default();
indexes_cf_opts.set_write_buffer_size(16 * 1024 * 1024);  // 16MB
indexes_cf_opts.set_compression(DBCompressionType::Lz4);

let cfs = vec![
    ColumnFamilyDescriptor::new("nodes", nodes_cf_opts),
    ColumnFamilyDescriptor::new("edges", edges_cf_opts),
    ColumnFamilyDescriptor::new("indexes", indexes_cf_opts),
];

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
```

### 7.2 GraphDB 列族推荐配置

#### nodes 列族

```rust
let mut nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
nodes_cf_opts.set_max_write_buffer_number(16);
nodes_cf_opts.set_compression(DBCompressionType::Lz4);
```

#### edges 列族

```rust
let mut edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
edges_cf_opts.set_max_write_buffer_number(32);
edges_cf_opts.set_compression(DBCompressionType::Snappy);
```

#### schema 列族

```rust
let mut schema_cf_opts = Options::default();
schema_cf_opts.set_write_buffer_size(16 * 1024 * 1024);  // 16MB
schema_cf_opts.set_max_write_buffer_number(8);
schema_cf_opts.set_compression(DBCompressionType::Lz4);
```

#### indexes 列族

```rust
let mut indexes_cf_opts = Options::default();
indexes_cf_opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
indexes_cf_opts.set_max_write_buffer_number(16);
indexes_cf_opts.set_compression(DBCompressionType::Lz4);
```

---

## 8. 性能优化配置

### 8.1 点查询优化

```rust
use rocksdb::Options;

let mut opts = Options::default();

// 优化点查询
opts.optimize_for_point_lookup(1024);  // 优化缓存大小
```

### 8.2 统计配置

```rust
use rocksdb::{DB, Options, PerfStatsLevel};

let mut opts = Options::default();

// 启用性能统计
DB::set_perf_stats(PerfStatsLevel::EnableAll)?;

let db = DB::open(&opts, path)?;

// 获取统计信息
if let Some(stats) = db.get_property("rocksdb.stats") {
    println!("Stats: {}", stats);
}
```

---

## 9. GraphDB 完整配置示例

### 9.1 小型 GraphDB（< 1GB）

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType};

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

// 全局配置
db_opts.set_block_cache_size(128 * 1024 * 1024);  // 128MB
db_opts.set_max_open_files(5000);
db_opts.set_compression(DBCompressionType::Lz4);
db_opts.set_use_fsync(false);

// 列族配置
let nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
nodes_cf_opts.set_max_write_buffer_number(12);

let edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
edges_cf_opts.set_max_write_buffer_number(16);

let cfs = vec![
    ColumnFamilyDescriptor::new("nodes", nodes_cf_opts),
    ColumnFamilyDescriptor::new("edges", edges_cf_opts),
];

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
```

### 9.2 中型 GraphDB（1GB - 10GB）

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType};

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

// 全局配置
db_opts.set_block_cache_size(512 * 1024 * 1024);  // 512MB
db_opts.set_max_open_files(10000);
db_opts.set_compression(DBCompressionType::Lz4);
db_opts.set_use_fsync(false);
db_opts.set_bytes_per_sync(8388608);  // 819KB

// 列族配置
let nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
nodes_cf_opts.set_max_write_buffer_number(16);

let edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
edges_cf_opts.set_max_write_buffer_number(24);

let schema_cf_opts = Options::default();
schema_cf_opts.set_write_buffer_size(16 * 1024 * 1024);  // 16MB
schema_cf_opts.set_max_write_buffer_number(8);

let indexes_cf_opts = Options::default();
indexes_cf_opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
indexes_cf_opts.set_max_write_buffer_number(16);

let cfs = vec![
    ColumnFamilyDescriptor::new("nodes", nodes_cf_opts),
    ColumnFamilyDescriptor::new("edges", edges_cf_opts),
    ColumnFamilyDescriptor::new("schema", schema_cf_opts),
    ColumnFamilyDescriptor::new("indexes", indexes_cf_opts),
];

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
```

### 9.3 大型 GraphDB（> 10GB）

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType};

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

// 全局配置
db_opts.set_block_cache_size(1024 * 1024 * 1024);  // 1GB
db_opts.set_max_open_files(20000);
db_opts.set_compression(DBCompressionType::Lz4);
db_opts.set_use_fsync(false);
db_opts.set_bytes_per_sync(8388608);  // 819KB

// 压缩触发器
db_opts.set_level_zero_stop_writes_trigger(2000);
db_opts.set_level_zero_slowdown_writes_trigger(1600);
db_opts.set_target_file_size_base(64 * 1024 * 1024);  // 64MB

// 列族配置
let nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
nodes_cf_opts.set_max_write_buffer_number(24);

let edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(256 * 1024 * 1024);  // 256MB
edges_cf_opts.set_max_write_buffer_number(32);

let schema_cf_opts = Options::default();
schema_cf_opts.set_write_buffer_size(32 * 1024 * 1024);  // 32MB
schema_cf_opts.set_max_write_buffer_number(12);

let indexes_cf_opts = Options::default();
indexes_cf_opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
indexes_cf_opts.set_max_write_buffer_number(24);

let cfs = vec![
    ColumnFamilyDescriptor::new("nodes", nodes_cf_opts),
    ColumnFamilyDescriptor::new("edges", edges_cf_opts),
    ColumnFamilyDescriptor::new("schema", schema_cf_opts),
    ColumnFamilyDescriptor::new("indexes", indexes_cf_opts),
];

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
```

---

## 10. 监控和调优

### 10.1 启用统计

```rust
use rocksdb::{DB, Options, PerfStatsLevel};

let mut opts = Options::default();
DB::set_perf_stats(PerfStatsLevel::EnableAll)?;

let db = DB::open(&opts, path)?;
```

### 10.2 关键指标监控

```rust
use rocksdb::{DB, Options};

let db = DB::open_default("path/to/db")?;

// 缓存命中率
let cache_hits = db.get_int_property("rocksdb.block.cache.hit")?;
let cache_misses = db.get_int_property("rocksdb.block.cache.miss")?;
if let (Some(hits), Some(misses)) = (cache_hits, cache_misses) {
    let hit_rate = hits as f64 / (hits + misses) as f64;
    println!("Cache hit rate: {:.2}%", hit_rate * 100.0);
}

// Bloom Filter 效果
let bloom_useful = db.get_int_property("rocksdb.bloom.filter.useful")?;
let bloom_full = db.get_int_property("rocksdb.bloom.filter.full.positive")?;
if let (Some(useful), Some(full)) = (bloom_useful, bloom_full) {
    let effectiveness = useful as f64 / (useful + full) as f64;
    println!("Bloom filter effectiveness: {:.2}%", effectiveness * 100.0);
}

// 写入放大
let bytes_written = db.get_int_property("rocksdb.bytes.written")?;
let keys_written = db.get_int_property("rocksdb.number.keys.written")?;
if let (Some(written), Some(keys)) = (bytes_written, keys_written) {
    let write_amplification = written as f64 / (keys as f64 * 100.0);
    println!("Write amplification: {:.2}x", write_amplification);
}
```

### 10.3 调优建议

**缓存命中率低**:
- 增加块缓存大小
- 检查查询模式
- 考虑使用 Bloom Filter

**写入性能差**:
- 增加写缓冲区大小
- 增加最大写缓冲区数量
- 考虑使用 Universal Compaction

**读取性能差**:
- 增加块缓存大小
- 优化压缩算法
- 检查压缩触发器设置

**磁盘空间增长快**:
- 启用压缩
- 调整压缩策略
- 手动触发压缩

---

## 11. 配置检查清单

### 11.1 初始部署

- [ ] 设置适当的块缓存大小
- [ ] 配置写缓冲区大小和数量
- [ ] 启用压缩（推荐 LZ4）
- [ ] 设置最大打开文件数
- [ ] 配置列族隔离
- [ ] 启用性能统计

### 11.2 性能调优

- [ ] 监控缓存命中率
- [ ] 监控 Bloom Filter 效果
- [ ] 监控写入放大
- [ ] 根据工作负载调整配置
- [ ] 定期检查统计信息

### 11.3 生产环境

- [ ] 配置适当的同步策略
- [ ] 设置压缩触发器
- [ ] 配置备份策略
- [ ] 设置监控告警
- [ ] 制定灾难恢复计划

---

## 12. 常见问题和解决方案

### 12.1 内存占用过高

**问题**: RocksDB 占用过多内存

**解决方案**:
- 减少块缓存大小
- 减少写缓冲区大小
- 减少最大写缓冲区数量

### 12.2 写入性能下降

**问题**: 写入速度逐渐变慢

**解决方案**:
- 检查压缩是否阻塞写入
- 增加写缓冲区大小
- 考虑使用 Universal Compaction
- 手动触发压缩

### 12.3 读取性能下降

**问题**: 读取速度变慢

**解决方案**:
- 增加块缓存大小
- 检查缓存命中率
- 优化压缩算法
- 检查 Bloom Filter 配置

### 12.4 磁盘空间增长快

**问题**: 磁盘空间占用增长过快

**解决方案**:
- 启用压缩
- 调整压缩策略
- 手动触发压缩
- 检查压缩触发器设置

---

## 13. 参考资料

- [RocksDB 官方文档](https://github.com/facebook/rocksdb)
- [RocksDB 性能调优](https://github.com/facebook/rocksdb/wiki/Performance-Tuning)
- [RocksDB 配置选项](https://github.com/facebook/rocksdb/wiki/Option-String-and-Env-Variable)
- [RocksDB 压缩](https://github.com/facebook/rocksdb/wiki/Compression)
