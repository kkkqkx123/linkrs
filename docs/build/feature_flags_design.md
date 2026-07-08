# 条件编译与 Feature Flags 设计文档

## 概述

本文档描述了 GraphDB 项目的条件编译系统设计，包括 feature flags 的配置、使用场景以及最佳实践。

## 设计目标

1. **模块化**：允许用户根据需求选择功能组件
2. **依赖优化**：仅编译实际需要的依赖项，减少编译时间和二进制大小
3. **清晰的 API 边界**：通过 feature flags 明确区分不同的使用场景
4. **向后兼容**：保持默认配置的稳定性

## Feature Flags 配置

### 完整配置列表

```toml
[features]
default = ["server"]

# Server API (HTTP/Web interface)
server = ["graphdb-api/server", "graphdb-config/server"]

# Full-text search support
fulltext-search = ["graphdb-search/fulltext-search", "graphdb-sync/fulltext-search"]

# Optional Chinese tokenizer support for full-text search
jieba = ["graphdb-search/jieba"]

# Vector search support
qdrant = [
    "graphdb-api/qdrant",
    "graphdb-config/qdrant",
    "graphdb-sync/qdrant",
    "graphdb-query/qdrant",
]

# Embedded API for standalone/embedded usage (Rust API only)
embedded = ["graphdb-api/embedded", "graphdb-config/embedded"]

# gRPC API
grpc = ["graphdb-api/grpc"]
```

### Feature 依赖关系图

```
default (server)
├── server
│   ├── graphdb-api/server
│   │   ├── axum
│   │   ├── tower
│   │   ├── tower-http
│   │   ├── http
│   │   ├── sqlx
│   │   └── async-trait
│   └── graphdb-config/server
├── fulltext-search
│   ├── graphdb-search/fulltext-search
│   └── graphdb-sync/fulltext-search
├── jieba
│   └── graphdb-search/jieba
├── qdrant
│   ├── graphdb-api/qdrant
│   ├── graphdb-config/qdrant
│   ├── graphdb-sync/qdrant
│   └── graphdb-query/qdrant
├── embedded
│   ├── graphdb-api/embedded
│   └── graphdb-config/embedded
└── grpc
    └── graphdb-api/grpc

embedded (standalone)
└── (no additional dependencies)
```

## 使用场景

### 场景 1：默认服务器模式（推荐用于生产环境）

**用途**：完整的 HTTP/Web 服务器，包含所有功能

**编译命令**：
```bash
cargo build --release
# 或显式指定
cargo build --release --features server
```

**包含组件**：
- ✅ HTTP API 服务器（Axum）
- ✅ Web 管理界面后端
- ✅ 用户认证与权限管理
- ✅ 批处理接口
- ✅ 全文搜索（BM25 + Inverted Index）
- ✅ 可选的向量搜索集成（启用 `qdrant` 时）
- ❌ C API（不包含）

**适用场景**：
- 部署为独立服务
- 通过 HTTP API 访问数据库
- 需要 Web 管理界面

---

### 场景 2：仅嵌入式 Rust 库

**用途**：作为 Rust 库直接嵌入应用程序，无需网络功能

**编译命令**：
```bash
cargo build --release --no-default-features --features embedded
```

**包含组件**：
- ✅ Embedded Rust API（类似 SQLite 的使用方式）
- ❌ HTTP 服务器
- ❌ C API
- ❌ 网络相关依赖

**代码示例**：
```rust
use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 打开数据库
    let db = GraphDatabase::open("my_database")?;
    
    // 创建会话
    let mut session = db.session()?;
    
    // 切换到空间
    session.use_space("test_space")?;
    
    // 执行查询
    let result = session.execute("MATCH (n) RETURN n")?;
    
    Ok(())
}
```

**适用场景**：
- 桌面应用程序
- 嵌入式设备
- 单机应用
- 测试和开发环境

---

### 场景 3：向量检索模式（Qdrant）

**用途**：启用 GraphDB 的向量索引与相似度搜索。

**编译命令**：
```bash
cargo build --release --features qdrant
```

**包含组件**：
- ✅ 向量客户端配置与管理
- ✅ Qdrant HTTP / gRPC 适配
- ✅ 向量索引同步协调器
- ❌ 本地 llama.cpp embedding provider

**适用场景**：
- 需要向量索引、相似度搜索和外部向量库集成
- 需要与 Qdrant 服务端协同工作
- 需要通过 `vector-client` 统一管理向量集合

**说明**：
- 当前 `vector-client` 仅保留 Qdrant 相关引擎实现
- embedding 侧只保留 OpenAI-compatible HTTP provider

---

### 场景 4：混合模式（Embedded + Server + Qdrant）

**用途**：同时提供嵌入式 API 和 HTTP 服务

**编译命令**：
```bash
cargo build --release --features embedded,server,qdrant
```

**包含组件**：
- ✅ 所有 Embedded API 功能
- ✅ 所有 Server API 功能
- ✅ 向量索引和外部向量库集成
- ❌ 不包含任何 C API 绑定

**适用场景**：
- 需要本地嵌入 + 远程访问双重模式
- 开发调试工具
- 需要同时使用文本检索和向量检索

---

## 条件编译在代码中的使用

### 模块级别条件编译

```rust
// src/api/mod.rs
pub mod core;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "embedded")]
pub mod embedded;

#[cfg(feature = "qdrant")]
pub mod vector;
```

```rust
// src/lib.rs
pub mod api;
pub mod core;
pub mod storage;
// ... 其他模块

#[cfg(feature = "server")]
pub use api::server::{session, HttpServer};
```

### 测试文件条件编译

```rust
// tests/integration_embedded_api.rs
#![cfg(feature = "embedded")]

#[test]
fn test_embedded_database() {
    // Embedded API 测试
}
```

```rust
// tests/integration_vector.rs
#![cfg(feature = "qdrant")]

#[test]
fn test_vector_support() {
    // 向量相关测试
}
```

---

## 构建产物对比

| Feature 组合 | 库类型 | 二进制 | 主要能力 |
|-------------|--------|--------|----------|
| `default` | rlib | graphdb-server | HTTP 服务器 |
| `embedded` | rlib | 无 | Embedded Rust API |
| `server` | rlib | graphdb-server | HTTP 服务器 |
| `server,qdrant` | rlib | graphdb-server | HTTP 服务器 + 向量检索 |
| `embedded,server,qdrant` | rlib | graphdb-server | Embedded + Server + 向量检索 |
| `server,grpc` | rlib | graphdb-server | HTTP 服务器 + gRPC |

---

## 依赖管理

### Optional Dependencies

以下依赖项被标记为 `optional = true`，仅在对应的 feature 启用时编译：

| 依赖 | Feature | 用途 |
|------|---------|------|
| `axum` | `server` | HTTP 框架 |
| `tower` | `server` | 服务抽象层 |
| `tower-http` | `server` | HTTP 中间件 |
| `http` | `server` | HTTP 类型定义 |
| `sqlx` | `server` | SQLite 客户端（Web 元数据存储） |
| `async-trait` | `server` | 异步 trait 支持 |
| `vector-client` | `qdrant` | 向量索引客户端 |
| `tonic` | `grpc` / `qdrant-grpc` | gRPC 客户端 |
| `prost` / `prost-types` | `grpc` / `qdrant-grpc` | protobuf 支持 |

### 平台特定依赖

```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
winapi = { version = "0.3", features = ["..."] }
```

这些依赖根据目标操作系统自动选择，无需手动配置。

---

## Build Script 行为

`crates/vector-client/build.rs` 会根据 feature flags 决定是否生成 Qdrant gRPC 的 protobuf 代码：

```rust
fn main() {
    if env::var("CARGO_FEATURE_QDRANT_GRPC").is_ok() {
        compile_qdrant_protos();
    }
    
    // 设置重新编译触发条件
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=proto/");
}
```

**环境变量检测**：
- `CARGO_FEATURE_QDRANT`：当启用 `qdrant` feature 时设置
- `CARGO_FEATURE_GRPC`：当启用 `grpc` feature 时设置
- `CARGO_FEATURE_QDRANT_GRPC`：当启用 `qdrant-grpc` feature 时设置
- `CARGO_FEATURE_EMBEDDED`：当启用 `embedded` feature 时设置
- `CARGO_FEATURE_SERVER`：当启用 `server` feature 时设置

---

## 最佳实践

### 1. 最小化依赖原则

在 `Cargo.toml` 中仅声明实际需要的 features：

```toml
# ❌ 不推荐：引入不必要的依赖
[dependencies]
graphdb = "0.1.0"  # 默认启用 server

# ✅ 推荐：明确指定需要的功能
[dependencies]
graphdb = { version = "0.1.0", default-features = false, features = ["embedded"] }
```

### 2. 条件编译代码组织

- 将 feature 特定的代码放在独立的模块中
- 使用 `#[cfg(feature = "...")]` 标记模块而非单个函数
- 为不同 feature 提供一致的公共 API（使用空实现或返回错误）

### 3. 测试策略

为不同的 feature 组合编写集成测试：

```rust
// tests/integration_embedded_api.rs
#![cfg(feature = "embedded")]

// tests/integration_server_api.rs
#![cfg(feature = "server")]

// tests/integration_vector_api.rs
#![cfg(feature = "qdrant")]
```

### 4. 文档示例

在代码示例中明确标注所需的 features：

```rust
//! ```rust,ignore
//! // 需要启用 embedded feature
//! use graphdb::api::embedded::GraphDatabase;
//! ```
```

---

## 常见问题

### Q1: 可以同时启用 `server` 和 `qdrant` 吗？

可以。这是当前最常见的生产组合之一，HTTP API 和向量检索会一起编译。

### Q2: 如何只编译库而不生成二进制文件？

```bash
cargo build --lib --no-default-features --features embedded
```

### Q3: 如何启用 gRPC？

```bash
cargo build --release --features server,grpc
```

---

## 版本历史

| 版本 | 日期 | 变更说明 |
|------|------|----------|
| 0.1.0 | 2026-04-03 | 初始版本，移除未使用的 `system_monitor` 和 `executor_internal` features |

---

## 参考文档

- [Cargo Features](https://doc.rust-lang.org/cargo/reference/features.html)
- [Conditional Compilation](https://doc.rust-lang.org/reference/conditional-compilation.html)
- [Build Scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html)
- [crate-type 字段](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#the-crate-type-field)

---

## 维护者备注

- 添加新 feature 时，确保在文档中更新依赖关系图
- 删除 feature 前，检查所有代码引用和测试文件
- 保持 feature 名称的语义清晰，避免歧义
- 定期审查 optional dependencies，移除未使用的依赖
