# GraphDB 扩展配置管理设计文档（务实版）

## 1. 核心结论

**配置热重载不现实，不需要实现。**

原因：

1. **Qdrant 配置无法热重载** - host/port 更改需要重新连接，等同于重启
2. **全文检索配置无法热重载** - 索引路径、分词器等一旦确定不能更改
3. **实际可热重载的配置极少** - 只有日志级别等少数参数
4. **单机数据库重启成本低** - 不需要复杂的配置重载机制

## 2. 保留的改进

### 2.1 简单的参数验证

在 `client.rs` 中直接实现 `validate()` 方法：

```rust
impl VectorClientConfig {
    /// Validate configuration at startup
    pub fn validate(&self) -> Result<(), String> {
        self.connection.validate()?;
        self.timeout.validate()?;
        self.retry.validate()?;
        Ok(())
    }
}

impl ConnectionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("connection.host cannot be empty".to_string());
        }
        if self.port == 0 || self.port > 65535 {
            return Err("connection.port must be in range [1, 65535]".to_string());
        }
        Ok(())
    }
}
```

**优点**：

- ✅ 启动时检查配置合法性
- ✅ 代码简单，无运行时开销
- ✅ 保持类型安全

### 2.2 命名规范（文档层面）

统一命名规范，但不强制代码结构改变：

```
# 向量检索（网络相关）
vector.enabled
vector.engine
vector.connection.host
vector.connection.port
vector.timeout.request_timeout_secs
vector.retry.max_retries

# 全文检索（本地存储）
fulltext.enabled
fulltext.engine
fulltext.index_path
fulltext.bm25.k1
fulltext.inversearch.resolution
```

## 3. 配置结构（保持现状）

### 3.1 向量检索配置

```rust
// crates/vector-client/src/config/client.rs
pub struct VectorClientConfig {
    pub enabled: bool,
    pub engine: EngineType,
    pub connection: ConnectionConfig,
    pub timeout: TimeoutConfig,
    pub retry: RetryConfig,
}

pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
    pub api_key: Option<String>,
    pub connect_timeout_secs: u64,
    pub http_port: Option<u16>,
}
```

### 3.2 全文检索配置

```rust
// crates/bm25/src/config/mod.rs
pub struct Bm25Config {
    pub k1: f32,
    pub b: f32,
    pub avg_doc_length: f32,
    pub field_weights: FieldWeights,
}

// crates/inversearch/src/config/mod.rs
pub struct EmbeddedConfig {
    pub index_path: Option<PathBuf>,
    pub resolution: usize,
    pub tokenize: TokenizeMode,
    // ...
}
```

## 4. 配置文件示例

```toml
# graphdb.toml

[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/graphdb"

# 向量检索配置
[vector]
enabled = true
engine = "Qdrant"

[vector.connection]
host = "localhost"
port = 6333
use_tls = false

[vector.timeout]
request_timeout_secs = 30
search_timeout_secs = 60
upsert_timeout_secs = 30

[vector.retry]
max_retries = 3
initial_delay_ms = 100

# 全文检索配置
[fulltext]
enabled = true
engine = "bm25"
index_path = "data/fulltext"

[fulltext.bm25]
k1 = 1.2
b = 0.75
```

## 5. 使用方式

### 5.1 启动时加载配置

```rust
// 加载配置
let config = Config::load("config.toml")?;

// 验证配置（启动时检查）
config.vector.validate()?;
config.fulltext.bm25.validate()?;

// 使用配置初始化
let vector_client = VectorClient::new(&config.vector)?;
```

### 5.2 配置修改

**修改配置后需要重启服务**：

```bash
# 1. 修改 config.toml
vim config.toml

# 2. 重启服务
systemctl restart graphdb
# 或
./graphdb-server restart
```

## 6. 为什么不需要热重载

### 6.1 Qdrant 配置

| 参数    | 能否热重载 | 原因                     |
| ------- | ---------- | ------------------------ |
| host    | ❌         | 需要重新建立 TCP 连接    |
| port    | ❌         | 需要重新建立 TCP 连接    |
| use_tls | ❌         | 需要重新建立 TLS 连接    |
| api_key | ❌         | 需要重新认证             |
| timeout | ⚠️         | 可以修改，但只影响新请求 |

### 6.2 全文检索配置

| 参数       | 能否热重载 | 原因                     |
| ---------- | ---------- | ------------------------ |
| index_path | ❌         | 索引文件已打开，无法更改 |
| resolution | ❌         | 需要重建索引             |
| tokenize   | ❌         | 需要重建索引             |
| k1/b       | ⚠️         | 可以修改，但只影响新查询 |

### 6.3 单机数据库的特点

- **重启成本低**：没有分布式协调问题
- **状态简单**：不需要保存复杂的运行时状态
- **用户可控**：用户可以自主决定重启时机

## 7. 总结

### 7.1 实际改进

1. **添加简单验证**：启动时检查配置合法性
2. **统一命名规范**：文档层面统一，不强制代码结构
3. **保持现状**：不引入复杂的配置管理机制

### 7.2 删除的过度设计

1. ❌ ~~reloadable.rs~~ - 配置热重载不现实
2. ❌ ~~validation.rs~~ - 重复定义，直接在 client.rs 中实现
3. ❌ ~~复杂的扩展注册机制~~ - 只有2-3个扩展，不需要
4. ❌ ~~参数上下文控制~~ - 单机部署不需要

### 7.3 务实的原则

- **保持简单**：不过度设计
- **类型安全**：充分利用 Rust 的类型系统
- **零成本抽象**：不引入不必要的运行时开销
- **解决实际问题**：只改进真正需要改进的地方

对于 GraphDB 这样的单机图数据库，**简单重启比复杂的配置热重载更可靠、更易维护**。
