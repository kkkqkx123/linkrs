## 配置类型

### DatabaseConfig

```rust
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: Option<PathBuf>,       // 数据库路径
    pub cache_size_mb: usize,        // 缓存大小（MB）
    pub default_timeout: Duration,   // 默认超时
    pub enable_wal: bool,            // 是否启用 WAL
    pub sync_mode: SyncMode,         // 同步模式
}
```

#### 构造方法

```rust
// 内存数据库配置
let config = DatabaseConfig::memory();

// 文件数据库配置
let config = DatabaseConfig::file("/path/to/db");

// 使用路径创建配置
let config = DatabaseConfig::new("/path/to/db");
```

#### 链式配置方法

```rust
let config = DatabaseConfig::memory()
    .with_cache_size(128)
    .with_timeout(Duration::from_secs(60))
    .with_wal(false)
    .with_sync_mode(SyncMode::Full);
```

### TransactionConfig

```rust
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    pub timeout: Option<Duration>,      // 事务超时时间
    pub read_only: bool,                // 是否只读
    pub durability: DurabilityLevel,    // 持久性级别
}
```

#### 构造方法

```rust
let config = TransactionConfig::new()
    .read_only()
    .with_timeout(Duration::from_secs(60))
    .with_durability(DurabilityLevel::Relaxed);
```

### BatchConfig

```rust
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub batch_size: usize,           // 批次大小
    pub auto_commit: bool,           // 是否自动提交
    pub continue_on_error: bool,     // 是否忽略错误继续处理
    pub max_errors: Option<usize>,   // 最大错误数量
}
```

---

