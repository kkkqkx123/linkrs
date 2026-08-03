# GraphDB

[English Version](README.md)

一个用 Rust 实现的**轻量级单节点图数据库**，专注于本地部署。受 NebulaGraph 数据模型启发（空间、标签、边类型、属性），但构建为独立的 Rust 工作空间。

> **状态**: Pre-v1.0，活跃开发中，不保证向后兼容。

---

## 特性

- **图数据模型** — 空间、顶点标签、边类型及带类型的属性
- **Cypher 兼容查询语言** — MATCH、RETURN、CREATE，流式执行
- **CSR 存储引擎** — 压缩稀疏行邻接存储，5 种变体 + 不可变 Csr，MVCC，压缩
- **全文搜索** — 基于 tantivy 的 BM25，支持 jieba 中文分词
- **向量搜索** — 通过 Qdrant 外部服务实现 HNSW 索引（余弦、欧几里得、点积）
- **多 API 接口**：
  - **HTTP** (axum) — REST API，通用使用
  - **gRPC** (tonic/prost) — 77+ RPC，高性能访问
  - **嵌入式 API** — 直接作为 Rust 库集成
  - **C API** — 类似 SQLite 的外语绑定（cbindgen）
- **Web 管理界面** — React + TypeScript 仪表盘 (graphdb-studio)
- **CLI 客户端** — 交互式 REPL (graphdb-cli)
- **全面基准测试** — 基于 Criterion.rs 的性能测量

---

## 架构

```
crates/
├── graphdb-core          # 核心类型、错误、数据结构
├── graphdb-config        # 配置管理
├── graphdb-search        # 全文搜索 (tantivy/BM25)
├── graphdb-sync          # 同步原语
├── graphdb-transaction   # 事务管理 (MVCC)
├── graphdb-migration     # 模式/数据迁移
├── graphdb-storage       # CSR 存储引擎
├── graphdb-query         # 查询解析器、优化器、流式执行器
├── graphdb-api           # HTTP、gRPC、嵌入式、C API
└── vector-client         # Qdrant 向量搜索客户端
```

依赖流向: `core → config → search → sync → transaction → storage → query → api`

---

## 快速开始

### 环境要求

- Rust 1.88.0+
- Cargo 1.88.0+

### 构建与运行

> **SIMD 编译选项（Phase 0）**：`.cargo/config.toml` 以
> `-C target-cpu=x86-64-v3`（AVX2，Haswell+ 2013 年后 CPU）编译，
> 获得编译器自动向量化收益（已验证 autovectorization 基准约 3.46x）。
> 不支持 AVX2 的 CPU 可回退基线目标：
> `RUSTFLAGS="-C target-cpu=x86_64" cargo build --release`
> （或删除 `[target.x86_64-unknown-linux-gnu]` 配置段）。

```shell
# 构建服务端
cargo build --release

# 启动服务端（默认端口 9758）
cargo run --release -- serve

# 执行单条查询
cargo run --release -- query "CREATE TAG person(name string, age int)"

# 使用自定义配置启动
cargo run --release -- serve -c /path/to/config.toml
```

### 特性标志

| 特性 | 说明 |
|------|------|
| `server` (默认) | HTTP/管理服务端 |
| `fulltext-search` | 全文搜索引擎 |
| `jieba` | 中文分词 |
| `qdrant` | 通过 Qdrant 进行向量搜索 |
| `grpc` | gRPC 服务端 |
| `c_api` | C 语言 API 绑定 |
| `embedded` | 嵌入式数据库模式 |

```shell
cargo build --release --features "server,fulltext-search,grpc,c_api"
```

### 快速检查

```shell
cargo check --workspace --all-features
cargo clippy --all-targets --all-features
cargo test --lib
```

---

## 配置

通过 `config.toml` 管理配置：

- **`[database]`** — 主机、端口（默认 9758）、存储路径、最大连接数
- **`[transaction]`** — 默认超时（30s）、最大并发、两阶段提交开关
- **`[log]`** — 日志级别、目录、文件、轮转
- **`[auth]`** — 鉴权开关、默认凭据、会话超时
- **`[grpc]`** — gRPC 端口（9669）、保活、超时
- **`[vector]`** — 向量搜索引擎连接、超时、重试
- **`[optimizer]`** — 查询优化器设置
- **`[monitoring]`** — 指标、缓存、慢查询阈值

---

## 项目结构

| 路径 | 说明 |
|------|------|
| `crates/` | 10 个子 crate（8 个核心 + migration + vector-client） |
| `src/` | 根 crate：服务端二进制、C API、库重新导出 |
| `frontend/` | graphdb-studio：React + TypeScript Web 界面 |
| `graphdb-cli/` | 交互式 CLI 客户端 |
| `proto/` | gRPC protobuf 定义 |
| `tests/` | 集成测试 + C API 测试 + E2E 测试 |
| `benches/` | Criterion.rs 基准测试 |
| `docs/` | 架构、存储、查询、API 文档 |
| `include/` | C 头文件（cbindgen 生成） |

---

## API 接口

### HTTP API (axum)
默认端口 `9758`。RESTful 接口，支持所有图操作。

### gRPC API (tonic)
端口 `9669`。77+ 个 RPC，涵盖健康检查、认证、会话、查询、模式、批量操作、向量索引和配置。

### 嵌入式 API
直接作为 Rust 库使用：

```rust
use graphdb::api::Database;
let db = Database::open("path/to/data")?;
db.execute("CREATE TAG person(name string)")?;
```

### C API
SQLite 风格接口。包含 `include/graphdb.h` 并链接 `libgraphdb`：

```c
graphdb *db;
graphdb_open("path/to/data", &db);
graphdb_execute(db, "CREATE TAG person(name string)", NULL, NULL);
graphdb_close(db);
```

---

## 查询语言

GraphDB 支持兼容 Cypher 的查询语言：

```cypher
CREATE TAG person(name string, age int);
CREATE EDGE knows(since date);

CREATE (:person {name: "Alice", age: 30});
CREATE (:person {name: "Bob", age: 25});
MATCH (a:person)-[:knows]->(b:person) RETURN a.name, b.name;
```

---

## CLI 客户端

```shell
cd graphdb-cli
cargo run -- --host localhost --port 9758
```

交互式 REPL，支持语法高亮、历史记录、CSV 导出和分页。

---

## Web 界面 (graphdb-studio)

```shell
cd frontend
npm install
npm run dev
```

React + TypeScript 仪表盘，支持图可视化（Cytoscape）、Ant Design 组件和国际化。

---

## 基准测试

```shell
cargo bench
```

基于 Criterion.rs 的基准测试涵盖：存储吞吐量、事务性能、查询解析/遍历、全文/向量搜索、API 延迟和端到端工作流。

---

## 贡献指南

- 代码：仅英文，遵循 Rust 标准格式（`cargo fmt`）
- 文档：代码内英文，文档中文
- 生产代码中不使用 `unwrap`（测试中可使用 `expect`）
- 减少 `dyn` 使用，优先使用具体类型
- 详细约定参见 `AGENTS.md`

---

## 许可证

Apache-2.0
