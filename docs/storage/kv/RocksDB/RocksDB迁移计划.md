# RocksDB 迁移计划

## 概述

本文档详细说明将 GraphDB 项目从 sled 迁移到 RocksDB 的完整计划，包括文件结构分析、编译要求、实施步骤等。

---

## 1. 当前存储模块分析

### 1.1 文件结构

```
src/storage/
├── mod.rs                          # 模块导出
├── storage_engine.rs               # 存储引擎 trait 定义
├── native_storage.rs              # 当前 sled 实现（需要替换）
├── test_mock.rs                  # 测试 mock
└── iterator/                    # 迭代器模块
    ├── mod.rs
    ├── default_iter.rs
    ├── get_neighbors_iter.rs
    ├── prop_iter.rs
    └── sequential_iter.rs
```

### 1.2 核心接口

**StorageEngine Trait** (`storage_engine.rs`):
- 节点操作：`insert_node`, `get_node`, `update_node`, `delete_node`
- 边操作：`insert_edge`, `get_edge`, `delete_edge`
- 扫描操作：`scan_all_vertices`, `scan_vertices_by_tag`, `scan_all_edges`
- 批量操作：`batch_insert_nodes`, `batch_insert_edges`
- 事务操作：`begin_transaction`, `commit_transaction`, `rollback_transaction`

### 1.3 当前实现特点

**native_storage.rs** (sled 实现):
- 使用多个 Tree 隔离数据（nodes, edges, schema, indexes）
- 实现 LRU 缓存（vertex_cache, edge_cache）
- 支持事务管理（active_transactions）
- 使用 bincode 序列化

---

## 2. RocksDB Windows MSVC 编译要求

### 2.1 系统要求

**Windows + MSVC 工具链**:

#### 必需软件
- **Visual Studio 2019 或更高版本**
  - 包含 C++ 编译器（MSVC）
  - 支持 C++17 或更高标准

- **CMake** 3.15 或更高版本
  - 用于编译 RocksDB C++ 库
  - 下载：https://cmake.org/download/

- **Perl**（可选）
  - RocksDB 构建脚本可能需要
  - Strawberry Perl 或 ActivePerl

#### 可选但推荐
- **Ninja**（替代 Make）
  - 更快的构建速度
  - 下载：https://github.com/ninja-build/ninja/releases

### 2.2 环境变量配置

#### 方法 1：使用 Developer Command Prompt

```powershell
# 打开 "x64 Native Tools Command Prompt for VS 2019"
# 或
# 打开 "x86 Native Tools Command Prompt for VS 2019"
```

#### 方法 2：手动设置环境变量

```powershell
# 设置 Visual Studio 路径
$env:VSCMD_ARG_tgt_arch="x64"
$env:VSCMD_ARG_pref=host=x64

# 添加 CMake 到 PATH
$env:Path += ";C:\Program Files\CMake\bin"

# 添加 Perl 到 PATH（如果需要）
$env:Path += ";C:\Strawberry\perl\bin"
```

### 2.3 编译依赖

**Rust 侧**:
- `rocksdb` crate 会自动处理 C++ 库编译
- 使用 `librocksdb-sys` 作为 FFI 绑定
- 需要 `cc` crate 来编译 C++ 代码

**系统侧**:
- **Windows SDK**
  - Windows 10 SDK 或更高版本
  - 包含必要的头文件和库

- **C++ 标准库**
  - MSVC 标准库
  - Visual Studio 运行时库

### 2.4 编译选项

**Cargo.toml 配置**:
```toml
[dependencies]
rocksdb = "0.24.0"
```

**编译时特性**:
- `static-link`: 静态链接 RocksDB（默认）
- `snappy`: 启用 Snappy 压缩支持
- `lz4`: 启用 LZ4 压缩支持
- `zstd`: 启用 ZSTD 压缩支持
- `zlib`: 启用 Zlib 压缩支持

**推荐配置**:
```toml
[dependencies]
rocksdb = { version = "0.24.0", features = ["lz4", "zstd"] }
```

### 2.5 常见编译问题

#### 问题 1：找不到 C++ 编译器

**错误信息**:
```
error: failed to run custom build command for `librocksdb-sys`
Caused by:
  process didn't exit successfully: `cc` exited with status 1
```

**解决方案**:
1. 安装 Visual Studio 2019 或更高版本
2. 使用 "x64 Native Tools Command Prompt" 打开终端
3. 确保 `cl.exe` 在 PATH 中

#### 问题 2：CMake 版本过低

**错误信息**:
```
CMake Error at CMakeLists.txt:XX (VERSION_LESS):
  CMake 3.15 or higher is required
```

**解决方案**:
1. 下载并安装 CMake 3.15 或更高版本
2. 添加 CMake 到 PATH
3. 重新运行 `cargo build`

#### 问题 3：链接错误

**错误信息**:
```
error: linking with `cc` failed
  = note: ld.exe: error: cannot find -lrocksdb
```

**解决方案**:
1. 清理构建缓存：`cargo clean`
2. 重新构建：`cargo build --release`
3. 如果问题持续，删除 `target` 目录后重试

#### 问题 4：内存不足

**错误信息**:
```
error: process didn't exit successfully: `cc` exited with status 1
  Caused by: out of memory
```

**解决方案**:
1. 关闭其他应用程序
2. 增加虚拟内存（如果使用虚拟机）
3. 使用 `cargo build --release` 而不是 `cargo build`

### 2.6 编译优化

#### 加速编译

```powershell
# 使用多线程编译
$env:CARGO_BUILD_JOBS=8
cargo build --release

# 或使用 cargo 的并行构建
cargo build --release --jobs 8
```

#### 减少编译时间

```powershell
# 使用编译缓存
cargo install sccache

# 设置环境变量
$env:RUSTC_WRAPPER=sccache

# 编译
cargo build --release
```

---

## 3. 需要修改的文件

### 3.1 必须修改的文件

| 文件 | 操作 | 优先级 |
|------|------|--------|
| `Cargo.toml` | 移除 sled，添加 rocksdb | 高 |
| `src/storage/native_storage.rs` | 完全重写为 RocksDB 实现 | 高 |
| `src/storage/mod.rs` | 更新导出（如果需要） | 中 |

### 3.2 可能需要修改的文件

| 文件 | 操作 | 优先级 |
|------|------|--------|
| `src/storage/iterator/*.rs` | 检查是否依赖 sled 特定 API | 中 |
| `src/main.rs` | 检查是否直接使用 sled | 低 |
| `src/lib.rs` | 检查是否直接使用 sled | 低 |

### 3.3 需要创建的文件

| 文件 | 用途 | 优先级 |
|------|------|--------|
| `src/storage/rocksdb_storage.rs` | RocksDB 存储引擎实现 | 高 |
| `src/storage/rocksdb_config.rs` | RocksDB 配置管理（可选） | 中 |

---

## 4. 迁移步骤

### 4.1 准备阶段

#### 步骤 1：备份当前代码

```powershell
# 创建备份分支
git checkout -b backup-sled
git push origin backup-sled

# 创建迁移分支
git checkout -b migrate-to-rocksdb
```

#### 步骤 2：安装依赖

```powershell
# 确保已安装 Visual Studio 2019 或更高版本
# 确保已安装 CMake 3.15 或更高版本
# （可选）安装 Ninja 以加速编译
```

#### 步骤 3：更新 Cargo.toml

```toml
[dependencies]
# 移除
# sled = "0.34.7"

# 添加
rocksdb = { version = "0.24.0", features = ["lz4"] }
```

### 4.2 实现阶段

#### 步骤 4：创建 RocksDB 存储引擎

创建新文件 `src/storage/rocksdb_storage.rs`：

```rust
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType};
use crate::core::{Direction, Edge, StorageError, Value, Vertex};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct RocksDBStorage {
    db: DB,
    db_path: String,
    vertex_cache: Arc<Mutex<lru::LruCache<Vec<u8>, Vertex>>>,
    edge_cache: Arc<Mutex<lru::LruCache<Vec<u8>, Edge>>>,
    active_transactions: Arc<Mutex<HashMap<TransactionId, TransactionBatches>>>,
}

impl RocksDBStorage {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref().to_string_lossy().to_string();

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_compression(DBCompressionType::Lz4);

        let cfs = vec![
            ColumnFamilyDescriptor::new("nodes", Options::default()),
            ColumnFamilyDescriptor::new("edges", Options::default()),
            ColumnFamilyDescriptor::new("schema", Options::default()),
            ColumnFamilyDescriptor::new("indexes", Options::default()),
        ];

        let db = DB::open_cf_descriptors(&db_opts, &db_path, cfs)
            .map_err(|e| StorageError::DbError(e.to_string()))?;

        Ok(Self {
            db,
            db_path,
            vertex_cache: Arc::new(Mutex::new(lru::LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())),
            edge_cache: Arc::new(Mutex::new(lru::LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())),
            active_transactions: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}
```

#### 步骤 5：实现核心方法

实现 `StorageEngine` trait 的所有方法：

- `insert_node`
- `get_node`
- `update_node`
- `delete_node`
- `insert_edge`
- `get_edge`
- `delete_edge`
- `scan_all_vertices`
- `scan_all_edges`
- `begin_transaction`
- `commit_transaction`
- `rollback_transaction`

#### 步骤 6：更新模块导出

修改 `src/storage/mod.rs`：

```rust
pub mod iterator;
pub mod rocksdb_storage;  // 新增
pub mod storage_engine;

pub use iterator::*;
pub use rocksdb_storage::*;  // 新增
pub use storage_engine::*;

pub use crate::core::StorageError;

#[cfg(test)]
pub use test_mock::*;
```

### 4.3 测试阶段

#### 步骤 7：编译检查

```powershell
# 清理旧构建
cargo clean

# 编译检查
cargo check

# 完整编译
cargo build --release
```

#### 步骤 8：运行测试

```powershell
# 运行所有测试
cargo test

# 运行特定测试
cargo test storage

# 运行集成测试
cargo test --test integration
```

#### 步骤 9：性能基准测试

```powershell
# 运行性能测试
cargo test --release --test bench

# 或使用 criterion
cargo bench
```

### 4.4 清理阶段

#### 步骤 10：删除 sled 代码

```powershell
# 删除旧文件
Remove-Item src/storage/native_storage.rs

# 检查是否有其他 sled 引用
Select-String -Path src -Pattern "sled" -Recurse

# 清理构建缓存
cargo clean
```

#### 步骤 11：最终验证

```powershell
# 重新编译
cargo build --release

# 运行所有测试
cargo test

# 检查文档是否需要更新
# 检查 README 是否需要更新
```

---

## 5. 配置建议

### 5.1 初始配置

```rust
let mut db_opts = Options::default();
db_opts.create_if_missing(true);
db_opts.create_missing_column_families(true);
db_opts.set_compression(DBCompressionType::Lz4);
db_opts.set_block_cache_size(512 * 1024 * 1024);  // 512MB
db_opts.set_max_open_files(10000);
```

### 5.2 列族配置

```rust
let nodes_cf_opts = Options::default();
nodes_cf_opts.set_write_buffer_size(64 * 1024 * 1024);  // 64MB
nodes_cf_opts.set_compression(DBCompressionType::Lz4);

let edges_cf_opts = Options::default();
edges_cf_opts.set_write_buffer_size(128 * 1024 * 1024);  // 128MB
edges_cf_opts.set_compression(DBCompressionType::Snappy);
```

---

## 6. 风险和缓解

### 6.1 技术风险

| 风险 | 影响 | 缓解措施 |
|------|--------|----------|
| 编译失败 | 阻塞开发 | 使用预编译二进制或 Docker |
| 性能下降 | 影响用户体验 | 充分测试和调优 |
| 数据损坏 | 数据丢失 | 实现备份和恢复机制 |
| API 不兼容 | 需要大量修改 | 保持 trait 接口不变 |

### 6.2 项目风险

| 风险 | 影响 | 缓解措施 |
|------|--------|----------|
| 迁移时间过长 | 延误其他功能 | 分阶段实施 |
| 测试覆盖不足 | 生产环境问题 | 增加测试用例 |
| 文档缺失 | 维护困难 | 同步更新文档 |

---

## 7. 时间估算

| 阶段 | 预估时间 |
|------|----------|
| 准备阶段 | 1-2 天 |
| 实现阶段 | 5-10 天 |
| 测试阶段 | 3-5 天 |
| 清理阶段 | 1-2 天 |
| **总计** | **10-19 天** |

---

## 8. 验收标准

### 8.1 功能验收

- [ ] 所有 StorageEngine trait 方法实现
- [ ] 所有现有测试通过
- [ ] 性能不低于 sled 实现
- [ ] 内存使用合理
- [ ] 磁盘使用合理

### 8.2 性能验收

- [ ] 写入性能 ≥ sled 的 90%
- [ ] 读取性能 ≥ sled 的 90%
- [ ] 批量操作性能 ≥ sled 的 90%
- [ ] 事务性能 ≥ sled 的 90%

### 8.3 质量验收

- [ ] 无编译警告
- [ ] 无运行时错误
- [ ] 内存泄漏检查通过
- [ ] 代码审查通过

---

## 9. 回滚计划

### 9.1 回滚触发条件

- 编译失败且无法修复
- 性能严重下降（< sled 的 50%）
- 发现严重 bug 且无法快速修复
- 项目需求变更

### 9.2 回滚步骤

```powershell
# 切换回 sled 分支
git checkout backup-sled

# 恢复 Cargo.toml
git checkout backup-sled -- Cargo.toml

# 重新编译
cargo build --release

# 验证功能
cargo test
```

---

## 10. 参考资料

- [RocksDB 集成指南](./RocksDB集成指南.md)
- [RocksDB 配置优化指南](./RocksDB配置优化指南.md)
- [RocksDB 官方文档](https://github.com/facebook/rocksdb)
- [rust-rocksdb API 文档](https://docs.rs/rocksdb/)
