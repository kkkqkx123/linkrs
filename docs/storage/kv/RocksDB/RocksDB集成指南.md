# RocksDB 集成到 Rust 项目指南

## 概述

本文档详细介绍如何将 RocksDB 集成到 Rust 项目中，包括依赖配置、基本使用、列族管理等。

---

## 1. 添加依赖

### 1.1 Cargo.toml 配置

在项目的 `Cargo.toml` 中添加 rocksdb 依赖：

```toml
[dependencies]
rocksdb = "0.24.0"
```

### 1.2 依赖说明

rocksdb 0.24.0 的依赖包括：

**运行时依赖**:
- `libc`: 系统调用接口
- `librocksdb-sys`: RocksDB C++ 库的 Rust 绑定
- `serde`: 可选，用于序列化支持

**开发依赖**:
- `bincode`: 序列化/反序列化
- `tempfile`: 临时文件测试
- `pretty_assertions`: 测试断言
- `trybuild`: 编译时测试

### 1.3 编译要求

由于 RocksDB 是 C++ 库的 Rust 绑定，需要：

- **C++ 编译器**: 支持 C++17 或更高版本
- **CMake**: 用于编译 RocksDB C++ 库
- **链接器**: 支持静态链接

**Windows 用户**:
- 需要 Visual Studio 2019 或更高版本
- 需要安装 C++ 构建工具

**Linux 用户**:
- 需要 `build-essential` 或类似工具链
- 可能需要安装 `libsnappy-dev` 等压缩库

---

## 2. 基本使用

### 2.1 打开数据库

#### 默认配置打开

```rust
use rocksdb::{DB, Options};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "path/to/rocksdb";

    let db = DB::open_default(path)?;

    Ok(())
}
```

#### 自定义配置打开

```rust
use rocksdb::{DB, Options};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "path/to/rocksdb";
    let mut opts = Options::default();
    opts.create_if_missing(true);

    let db = DB::open(&opts, path)?;

    Ok(())
}
```

### 2.2 基本操作

#### 插入数据

```rust
use rocksdb::{DB, Options};

let db = DB::open_default("path/to/db")?;

db.put(b"my_key", b"my_value")?;
```

#### 读取数据

```rust
match db.get(b"my_key")? {
    Some(value) => println!("Found value: {:?}", value),
    None => println!("Key not found"),
}
```

#### 删除数据

```rust
db.delete(b"my_key")?;
```

### 2.3 错误处理

```rust
use rocksdb::{DB, Options, Error};

fn main() -> Result<(), Error> {
    let db = DB::open_default("path/to/db")?;

    db.put(b"key", b"value")?;

    match db.get(b"key") {
        Ok(Some(value)) => println!("Value: {:?}", value),
        Ok(None) => println!("Not found"),
        Err(e) => eprintln!("Error: {}", e),
    }

    Ok(())
}
```

---

## 3. 列族（Column Families）

### 3.1 为什么需要列族

列族允许在同一个 RocksDB 实例中创建多个独立的数据集，每个列族：

- 有独立的配置选项
- 有独立的压缩策略
- 有独立的缓存设置
- 数据完全隔离

**对于 GraphDB 的好处**:
- `nodes`: 存储节点数据
- `edges`: 存储边数据
- `schema`: 存储模式定义
- `indexes`: 存储索引数据

### 3.2 创建列族

#### 单个列族

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options};

let path = "path/to/db";

let mut cf_opts = Options::default();
cf_opts.set_max_write_buffer_number(16);

let cf = ColumnFamilyDescriptor::new("nodes", cf_opts);

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

let db = DB::open_cf_descriptors(&db_opts, path, vec![cf])?;
```

#### 多个列族

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options};

let path = "path/to/db";

let mut db_opts = Options::default();
db_opts.create_missing_column_families(true);
db_opts.create_if_missing(true);

let cfs = vec![
    ColumnFamilyDescriptor::new("nodes", Options::default()),
    ColumnFamilyDescriptor::new("edges", Options::default()),
    ColumnFamilyDescriptor::new("schema", Options::default()),
    ColumnFamilyDescriptor::new("indexes", Options::default()),
];

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
```

### 3.3 列族操作

#### 使用列族

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options};

let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;

let cf = db.cf_handle("nodes").expect("Column family not found");

db.put_cf(cf, b"node_1", b"node_data")?;
let value = db.get_cf(cf, b"node_1")?;
```

#### 列族管理

```rust
use rocksdb::{DB, Options};

let db = DB::open_default("path/to/db")?;

// 列出所有列族
let cfs = DB::list_column_families(&Options::default(), "path/to/db")?;
for cf in cfs {
    println!("Column family: {:?}", String::from_utf8(cf)?);
}

// 创建新列族
db.create_cf("new_cf", &Options::default())?;

// 删除列族
db.drop_cf("old_cf")?;
```

---

## 4. 事务支持

### 4.1 WriteBatch 批量操作

```rust
use rocksdb::{DB, WriteBatchWithTransaction};

let db = DB::open_default("path/to/db")?;

let mut batch = WriteBatchWithTransaction::default();
batch.put(b"key1", b"value1");
batch.put(b"key2", b"value2");
batch.put(b"key3", b"value3");

db.write(batch)?;
```

### 4.2 事务数据库

```rust
use rocksdb::{DB, TransactionDB, Options, TransactionDBOptions};

let path = "path/to/db";

let mut db_opts = Options::default();
db_opts.create_if_missing(true);

let mut txn_db_opts = TransactionDBOptions::default();

let db = TransactionDB::open(&db_opts, &txn_db_opts, path)?;

// 使用事务
let txn = db.transaction();
txn.put(b"key", b"value");
txn.commit()?;
```

---

## 5. 读写选项

### 5.1 读选项

```rust
use rocksdb::{DB, ReadOptions};

let db = DB::open_default("path/to/db")?;

let mut read_opts = ReadOptions::default();
read_opts.set_verify_checksums(true);  // 验证校验和
read_opts.set_fill_cache(true);         // 填充缓存

let value = db.get_opt(b"key", &read_opts)?;
```

### 5.2 写选项

```rust
use rocksdb::{DB, WriteOptions};

let db = DB::open_default("path/to/db")?;

let mut write_opts = WriteOptions::default();
write_opts.set_sync(false);          // 异步写入
write_opts.set_disable_wal(false);     // 启用 WAL

db.put_opt(b"key", b"value", &write_opts)?;
```

---

## 6. 性能监控

### 6.1 启用统计

```rust
use rocksdb::{DB, Options, PerfStatsLevel};

let mut opts = Options::default();
opts.create_if_missing(true);

// 设置性能统计级别
DB::set_perf_stats(PerfStatsLevel::EnableAll)?;

let db = DB::open(&opts, path)?;
```

### 6.2 获取统计信息

```rust
use rocksdb::{DB, Options};

let db = DB::open_default("path/to/db")?;

// 获取数据库属性
if let Some(stats) = db.get_property("rocksdb.stats") {
    println!("Database stats: {}", stats);
}

// 获取整数属性
if let Some(num_keys) = db.get_int_property("rocksdb.estimate-num-keys") {
    println!("Estimated keys: {}", num_keys);
}

// 获取内存使用统计
if let Ok(mem_stats) = DB::get_memory_usage_stats() {
    println!("Memory usage: {:?}", mem_stats);
}
```

### 6.3 统计指标

RocksDB 提供丰富的统计指标：

**缓存相关**:
- `rocksdb.block.cache.hit`: 块缓存命中
- `rocksdb.block.cache.miss`: 块缓存未命中
- `rocksdb.bloom.filter.useful`: Bloom Filter 有效过滤

**压缩相关**:
- `rocksdb.compaction.key.drop.new`: 压缩删除新条目
- `rocksdb.compaction.key.drop.obsolete`: 压缩删除过期条目

**读写相关**:
- `rocksdb.number.keys.written`: 写入的键数量
- `rocksdb.number.keys.read`: 读取的键数量

---

## 7. GraphDB 集成示例

### 7.1 定义存储结构

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options};
use std::sync::Arc;

pub struct RocksDBStorage {
    db: Arc<DB>,
}

impl RocksDBStorage {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self, rocksdb::Error> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);

        let cfs = vec![
            ColumnFamilyDescriptor::new("nodes", Options::default()),
            ColumnFamilyDescriptor::new("edges", Options::default()),
            ColumnFamilyDescriptor::new("schema", Options::default()),
            ColumnFamilyDescriptor::new("indexes", Options::default()),
        ];

        let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;

        Ok(Self { db: Arc::new(db) })
    }

    pub fn insert_node(&self, key: &[u8], value: &[u8]) -> Result<(), rocksdb::Error> {
        let cf = self.db.cf_handle("nodes").expect("nodes cf not found");
        self.db.put_cf(cf, key, value)
    }

    pub fn get_node(&self, key: &[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error> {
        let cf = self.db.cf_handle("nodes").expect("nodes cf not found");
        self.db.get_cf(cf, key)
    }

    pub fn insert_edge(&self, key: &[u8], value: &[u8]) -> Result<(), rocksdb::Error> {
        let cf = self.db.cf_handle("edges").expect("edges cf not found");
        self.db.put_cf(cf, key, value)
    }

    pub fn get_edge(&self, key: &[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error> {
        let cf = self.db.cf_handle("edges").expect("edges cf not found");
        self.db.get_cf(cf, key)
    }
}
```

### 7.2 使用示例

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = RocksDBStorage::new("path/to/graphdb")?;

    let node_key = b"node_123";
    let node_data = b"node_data_here";

    storage.insert_node(node_key, node_data)?;

    if let Some(data) = storage.get_node(node_key)? {
        println!("Found node: {:?}", data);
    }

    Ok(())
}
```

---

## 8. 清理和关闭

### 8.1 自动关闭

RocksDB 实现了 `Drop` trait，会在离开作用域时自动关闭：

```rust
{
    let db = DB::open_default("path/to/db")?;

    db.put(b"key", b"value")?;

}  // db 在这里自动关闭
```

### 8.2 手动销毁

```rust
use rocksdb::{DB, Options};

let path = "path/to/db";

let _ = DB::destroy(&Options::default(), path);
```

---

## 9. 测试

### 9.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rocksdb::{DB, Options};
    use tempfile::TempDir;

    #[test]
    fn test_basic_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let path = temp_dir.path();

        let db = DB::open_default(path).expect("Failed to open DB");

        db.put(b"key", b"value").expect("Failed to put");

        let value = db.get(b"key").expect("Failed to get");
        assert_eq!(value, Some(b"value".to_vec()));

        db.delete(b"key").expect("Failed to delete");

        let value = db.get(b"key").expect("Failed to get");
        assert_eq!(value, None);
    }

    #[test]
    fn test_column_families() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let path = temp_dir.path();

        let cfs = vec![
            ColumnFamilyDescriptor::new("cf1", Options::default()),
            ColumnFamilyDescriptor::new("cf2", Options::default()),
        ];

        let mut db_opts = Options::default();
        db_opts.create_missing_column_families(true);
        db_opts.create_if_missing(true);

        let db = DB::open_cf_descriptors(&db_opts, path, cfs)
            .expect("Failed to open DB");

        let cf1 = db.cf_handle("cf1").expect("cf1 not found");
        db.put_cf(cf1, b"key", b"value1").expect("Failed to put");

        let cf2 = db.cf_handle("cf2").expect("cf2 not found");
        db.put_cf(cf2, b"key", b"value2").expect("Failed to put");

        let value1 = db.get_cf(cf1, b"key").expect("Failed to get");
        assert_eq!(value1, Some(b"value1".to_vec()));

        let value2 = db.get_cf(cf2, b"key").expect("Failed to get");
        assert_eq!(value2, Some(b"value2".to_vec()));
    }
}
```

---

## 10. 常见问题

### 10.1 编译错误

**问题**: `linking with cc failed`

**解决方案**:
- 确保安装了 C++ 编译器
- Windows: 安装 Visual Studio C++ 构建工具
- Linux: `sudo apt-get install build-essential`

### 10.2 运行时错误

**问题**: `IO error: While opening a file for lock`

**解决方案**:
- 确保数据库路径存在
- 检查文件权限
- 确保没有其他进程正在使用数据库

### 10.3 性能问题

**问题**: 读取/写入性能不佳

**解决方案**:
- 调整块缓存大小
- 调整写缓冲区大小
- 启用压缩
- 查看统计信息，识别瓶颈

---

## 11. 下一步

集成完成后，建议：

1. 阅读 [RocksDB 配置优化文档](./RocksDB配置优化指南.md)
2. 根据实际工作负载调整配置
3. 启用性能监控
4. 进行性能基准测试

---

## 12. 参考资料

- [RocksDB 官方文档](https://github.com/facebook/rocksdb)
- [rust-rocksdb API 文档](https://docs.rs/rocksdb/)
- [RocksDB 性能调优指南](https://github.com/facebook/rocksdb/wiki/Performance-Tuning)
