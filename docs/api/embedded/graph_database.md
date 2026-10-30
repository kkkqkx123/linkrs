## GraphDatabase

数据库主结构体，作为嵌入式 API 的主要入口点。

### 创建数据库

```rust
use linkrs::api::embedded::{GraphDatabase, DatabaseConfig};
use std::time::Duration;

// 打开文件数据库
let db = GraphDatabase::open("/path/to/database")?;

// 创建内存数据库
let db = GraphDatabase::open_in_memory()?;

// 使用自定义配置
let config = DatabaseConfig::file("/path/to/database")
    .with_cache_size(128)
    .with_timeout(Duration::from_secs(60))
    .with_wal(true)
    .with_sync_mode(SyncMode::Normal);
let db = GraphDatabase::open_with_config(config)?;
```

### 方法

#### session()
创建新会话。

```rust
pub fn session(&self) -> CoreResult<Session<S>>
```

#### execute()
执行简单查询（便捷方法）。

```rust
pub fn execute(&self, query: &str) -> CoreResult<QueryResult>
```

#### execute_with_params()
执行参数化查询（便捷方法）。

```rust
pub fn execute_with_params(
    &self,
    query: &str,
    params: HashMap<String, Value>
) -> CoreResult<QueryResult>
```

#### create_space()
创建图空间（便捷方法）。

```rust
pub fn create_space(&self, name: &str, space_config: SpaceConfig) -> CoreResult<()>
```

#### drop_space()
删除图空间（便捷方法）。

```rust
pub fn drop_space(&self, name: &str) -> CoreResult<()>
```

#### list_spaces()
列出所有图空间（便捷方法）。

```rust
pub fn list_spaces(&self) -> CoreResult<Vec<String>>
```

---

