# Qdrant 配置说明

> 分析日期: 2026-04-06
> 依赖: qdrant-client v1.7+

---

## 目录

- [1. 客户端配置](#1-客户端配置)
- [2. 集合配置](#2-集合配置)
- [3. 向量配置](#3-向量配置)
- [4. HNSW配置](#4-hnsw配置)
- [5. 量化配置](#5-量化配置)
- [6. 优化器配置](#6-优化器配置)
- [7. GraphDB集成配置](#7-graphdb集成配置)

---

## 1. 客户端配置

### 1.1 连接参数

```rust
pub struct QdrantClientConfig {
    /// Qdrant服务地址
    pub url: String,
    /// API密钥（可选）
    pub api_key: Option<String>,
    /// 请求超时时间
    pub timeout: Duration,
    /// 连接超时时间
    pub connect_timeout: Duration,
    /// 压缩编码
    pub compression: Option<CompressionEncoding>,
    /// 保持连接
    pub keep_alive: bool,
}

impl Default for QdrantClientConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:6334".to_string(),
            api_key: None,
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            compression: None,
            keep_alive: true,
        }
    }
}
```

### 1.2 配置示例

```toml
# config.toml
[vector]
engine = "qdrant"

[vector.qdrant]
url = "http://localhost:6334"
api_key = ""
timeout_secs = 30
connect_timeout_secs = 10
compression = "gzip"  # none, gzip, deflate
keep_alive = true
```

---

## 2. 集合配置

### 2.1 基本集合参数

| 参数 | 类型 | 默认值 | 描述 |
|------|------|--------|------|
| `vectors` | VectorParams | 必需 | 向量配置 |
| `shard_number` | u32 | 1 | 分片数量 |
| `on_disk_payload` | bool | false | Payload存储在磁盘 |
| `replication_factor` | u32 | 1 | 副本数量 |

### 2.2 Rust配置

```rust
use qdrant_client::qdrant::{
    CreateCollectionBuilder,
    VectorParamsBuilder,
    Distance,
    WalConfigDiffBuilder,
};

let collection_config = CreateCollectionBuilder::new("my_collection")
    .vectors_config(VectorParamsBuilder::new(768, Distance::Cosine))
    .shard_number(1)
    .on_disk_payload(false)
    .wal_config(WalConfigDiffBuilder::default()
        .wal_capacity_mb(32)
        .wal_segments_ahead(0)
    );
```

---

## 3. 向量配置

### 3.1 单向量配置

```rust
pub struct VectorParams {
    /// 向量维度
    pub size: u64,
    /// 距离度量
    pub distance: Distance,
    /// HNSW配置（可选）
    pub hnsw_config: Option<HnswConfigDiff>,
    /// 量化配置（可选）
    pub quantization_config: Option<QuantizationConfig>,
    /// 是否存储在磁盘
    pub on_disk: Option<bool>,
}

// Rust使用
VectorParamsBuilder::new(768, Distance::Cosine)
    .on_disk(false);
```

### 3.2 多向量配置

```rust
use qdrant_client::qdrant::VectorsConfigBuilder;

let mut vectors_config = VectorsConfigBuilder::default();

// 添加命名向量
vectors_config.add_named_vector_params(
    "embedding",
    VectorParamsBuilder::new(768, Distance::Cosine).build()
);
vectors_config.add_named_vector_params(
    "image",
    VectorParamsBuilder::new(512, Distance::Dot).build()
);
```

### 3.3 稀疏向量配置

```rust
use qdrant_client::qdrant::SparseVectorParamsBuilder;

let sparse_config = SparseVectorParamsBuilder::default()
    .index(SparseIndexConfig {
        full_scan_threshold: 10000,
        ..Default::default()
    });
```

---

## 4. HNSW配置

### 4.1 参数说明

| 参数 | 类型 | 默认值 | 描述 |
|------|------|--------|------|
| `m` | u64 | 16 | 每层连接数 |
| `ef_construct` | u64 | 100 | 构建时搜索范围 |
| `full_scan_threshold` | u64 | 10000 | 全扫描阈值 |
| `max_indexing_threads` | u64 | 0 | 索引线程数(0=自动) |
| `on_disk` | bool | false | 存储在磁盘 |
| `payload_m` | u64 | 16 | Payload索引连接数 |

### 4.2 性能调优建议

| 场景 | m | ef_construct | 说明 |
|------|---|--------------|------|
| 高召回率 | 32-64 | 200-400 | 更高内存消耗 |
| 平衡 | 16-32 | 100-200 | 推荐默认 |
| 高吞吐 | 8-16 | 50-100 | 更低延迟 |

### 4.3 Rust配置

```rust
use qdrant_client::qdrant::HnswConfigDiffBuilder;

let hnsw_config = HnswConfigDiffBuilder::default()
    .m(16)
    .ef_construct(100)
    .full_scan_threshold(10000)
    .max_indexing_threads(0)
    .on_disk(false)
    .payload_m(16);
```

---

## 5. 量化配置

### 5.1 标量量化

```rust
use qdrant_client::qdrant::ScalarQuantizationBuilder;

let scalar_quant = ScalarQuantizationBuilder::default()
    .quantile(0.99)      // 量化分位数
    .always_ram(true);   // 始终在内存中
```

| 参数 | 默认值 | 描述 |
|------|--------|------|
| `quantile` | 0.99 | 用于计算量化范围的分位数 |
| `always_ram` | true | 量化数据始终在内存中 |

### 5.2 乘积量化

```rust
use qdrant_client::qdrant::{
    ProductQuantizationBuilder,
    ProductQuantizationCompression,
};

let product_quant = ProductQuantizationBuilder::default()
    .compression(ProductQuantizationCompression::X4)
    .always_ram(true);
```

| 压缩比 | 内存节省 | 召回率影响 |
|--------|----------|-----------|
| X4 | 75% | 轻微下降 |
| X8 | 87.5% | 中等下降 |
| X16 | 93.75% | 明显下降 |
| X32 | 96.875% | 显著下降 |

### 5.3 二进制量化

```rust
use qdrant_client::qdrant::BinaryQuantizationBuilder;

let binary_quant = BinaryQuantizationBuilder::default()
    .always_ram(true);
```

适用于高维向量（如OpenAI embeddings），内存节省约32倍。

---

## 6. 优化器配置

### 6.1 参数说明

| 参数 | 默认值 | 描述 |
|------|--------|------|
| `deleted_threshold` | 0.2 | 触发段合并的删除比例 |
| `vacuum_min_vector_count` | 1000 | 最小向量数触发清理 |
| `default_segment_number` | 0 | 默认段数量(0=自动) |
| `indexing_threshold` | 10000 | 创建索引的阈值 |
| `memmap_threshold` | 20000 | 使用mmap的阈值 |

### 6.2 Rust配置

```rust
use qdrant_client::qdrant::OptimizersConfigDiffBuilder;

let optimizers_config = OptimizersConfigDiffBuilder::default()
    .deleted_threshold(0.2)
    .vacuum_min_vector_count(1000)
    .default_segment_number(0)
    .indexing_threshold(10000)
    .memmap_threshold(20000)
    .max_optimization_threads(1);
```

---

## 7. GraphDB集成配置

### 7.1 配置文件结构

```toml
# graphdb.toml
[vector]
enabled = true
engine = "qdrant"

[vector.qdrant]
# 连接配置
url = "http://localhost:6334"
api_key = ""
timeout_secs = 30
connect_timeout_secs = 10

# 默认向量配置
default_vector_size = 768
default_distance = "cosine"  # cosine, euclidean, dot

# HNSW配置
[vector.qdrant.hnsw]
m = 16
ef_construct = 100
full_scan_threshold = 10000

# 量化配置
[vector.qdrant.quantization]
enabled = false
type = "scalar"  # scalar, product, binary
quantile = 0.99

# 同步配置
[vector.sync]
mode = "async"  # sync, async, off
batch_size = 100
retry_count = 3
retry_delay_ms = 100
```

### 7.2 Rust配置结构

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    pub enabled: bool,
    pub engine: String,
    pub qdrant: QdrantConfig,
    pub sync: VectorSyncConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub default_vector_size: usize,
    pub default_distance: String,
    pub hnsw: HnswConfig,
    pub quantization: QuantizationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswConfig {
    pub m: usize,
    pub ef_construct: usize,
    pub full_scan_threshold: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfig {
    pub enabled: bool,
    pub quant_type: String,
    pub quantile: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSyncConfig {
    pub mode: String,
    pub batch_size: usize,
    pub retry_count: usize,
    pub retry_delay_ms: u64,
}

impl QdrantConfig {
    pub fn to_client_config(&self) -> qdrant_client::Qdrant {
        let mut builder = qdrant_client::Qdrant::from_url(&self.url)
            .timeout(Duration::from_secs(self.timeout_secs))
            .connect_timeout(Duration::from_secs(self.connect_timeout_secs));
        
        if let Some(ref api_key) = self.api_key {
            builder = builder.api_key(Some(api_key.clone()));
        }
        
        builder.build().expect("Failed to build Qdrant client")
    }
}
```

### 7.3 集合命名规范

```
格式: space_{space_id}_{tag_name}_{field_name}

示例:
- space_1_Document_embedding
- space_1_User_profile_vector
- space_2_Image_feature
```

### 7.4 Payload结构设计

```rust
// 向量点Payload结构
pub struct VectorPayload {
    // 顶点标识
    pub vertex_id: i64,           // 顶点ID
    pub tag_name: String,         // 标签名
    pub space_id: u64,            // 空间ID
    
    // 属性引用
    pub properties: HashMap<String, Value>,  // 关键属性
    
    // 元数据
    pub created_at: i64,          // 创建时间
    pub updated_at: i64,          // 更新时间
}
```

---

## 环境变量支持

```bash
# Qdrant连接
QDRANT_URL=http://localhost:6334
QDRANT_API_KEY=your-api-key

# 超时配置
QDRANT_TIMEOUT_SECS=30
QDRANT_CONNECT_TIMEOUT_SECS=10

# 默认向量配置
QDRANT_DEFAULT_VECTOR_SIZE=768
QDRANT_DEFAULT_DISTANCE=cosine
```
