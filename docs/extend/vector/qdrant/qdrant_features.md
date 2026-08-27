# Qdrant 功能概述

> 分析日期: 2026-04-06
> 来源: Qdrant官方文档和Rust Client文档
>
> **注意**: 本说明基于之前整理的qdrant特性文档， vendored gRPC proto 完全是client模块自己构建时生成的，不代表qdrant的完整功能。qdrant后续版本引入新距离度量时文档可能滞后。

---

## 目录

- [1. 核心功能](#1-核心功能)
- [2. 向量索引](#2-向量索引)
- [3. 距离度量](#3-距离度量)
- [4. 量化配置](#4-量化配置)
- [5. Payload过滤](#5-payload过滤)
- [6. 高级特性](#6-高级特性)

---

## 1. 核心功能

### 1.1 向量存储与检索

Qdrant是一个高性能的向量数据库，核心功能包括：

| 功能 | 描述 |
|------|------|
| 向量存储 | 存储高维向量数据，支持稠密向量和稀疏向量 |
| 相似度搜索 | 基于向量相似度的快速检索 |
| Payload存储 | 每个向量点可关联JSON格式的元数据 |
| 过滤搜索 | 支持基于Payload的条件过滤 |

### 1.2 数据模型

```
Collection (集合)
├── Points (点)
│   ├── ID (唯一标识)
│   ├── Vector (向量数据)
│   └── Payload (元数据)
└── Configuration (配置)
    ├── Vector Params (向量参数)
    ├── HNSW Config (索引配置)
    └── Quantization (量化配置)
```

---

## 2. 向量索引

### 2.1 HNSW索引

Qdrant使用HNSW（Hierarchical Navigable Small World）算法进行向量索引：

| 参数 | 默认值 | 描述 |
|------|--------|------|
| `m` | 16 | 每个节点的连接数，影响召回率和内存 |
| `ef_construct` | 100 | 构建时的搜索范围，影响索引质量 |
| `full_scan_threshold` | 10000 | 触发全扫描的阈值 |

```rust
use qdrant_client::qdrant::HnswConfigDiffBuilder;

let hnsw_config = HnswConfigDiffBuilder::default()
    .m(16)
    .ef_construct(100)
    .full_scan_threshold(10000);
```

### 2.2 Filterable HNSW

Qdrant扩展了HNSW图，支持在向量搜索时高效应用Payload过滤：

- 标准HNSW索引在高选择性过滤时性能下降
- Filterable HNSW通过额外边解决此问题
- 自动应用于已索引的Payload字段

### 2.3 Payload索引

```rust
// 创建Payload索引
client
    .create_payload_index(
        "my_collection",
        "category",
        PayloadSchemaType::Keyword,
        None,  // 可选的索引参数
    )
    .await?;
```

Payload索引类型：
- `Keyword`: 关键字索引，用于精确匹配
- `Integer`: 整数索引，用于范围查询
- `Float`: 浮点数索引，用于范围查询
- `Text`: 全文索引，用于文本搜索
- `Bool`: 布尔索引

---

## 3. 距离度量

### 3.1 支持的距离度量

| 度量 | 公式 | 适用场景 |
|------|------|---------|
| Cosine | 1 - (A·B)/(‖A‖‖B‖) | 文本嵌入、归一化向量 |
| Euclidean | ‖A-B‖ | 图像特征、物理距离 |
| Dot Product | A·B | 推荐系统、已归一化向量 |
| Manhattan | Σ\|Ai-Bi\| | 稀疏向量 |

### 3.2 Rust配置

```rust
use qdrant_client::qdrant::{Distance, VectorParamsBuilder};

// Cosine距离
VectorParamsBuilder::new(768, Distance::Cosine)

// Euclidean距离
VectorParamsBuilder::new(768, Distance::Euclid)

// Dot Product
VectorParamsBuilder::new(768, Distance::Dot)

// Manhattan距离
VectorParamsBuilder::new(768, Distance::Manhattan)
```

---

## 4. 量化配置

### 4.1 标量量化 (Scalar Quantization)

将float32向量压缩为int8：

```rust
use qdrant_client::qdrant::ScalarQuantizationBuilder;

let quantization = ScalarQuantizationBuilder::default()
    .quantile(0.99)    // 量化分位数
    .always_ram(true); // 始终保持在内存中
```

**优势**：
- 内存占用减少4倍
- 搜索速度提升
- 召回率轻微下降

### 4.2 乘积量化 (Product Quantization)

更高压缩比的量化方法：

```rust
use qdrant_client::qdrant::ProductQuantizationBuilder;

let quantization = ProductQuantizationBuilder::default()
    .compression(ProductQuantizationCompression::X4);
```

### 4.3 二进制量化 (Binary Quantization)

适用于高维向量的极端压缩：

```rust
use qdrant_client::qdrant::BinaryQuantizationBuilder;

let quantization = BinaryQuantizationBuilder::default();
```

---

## 5. Payload过滤

### 5.1 过滤条件类型

```rust
use qdrant_client::qdrant::{Condition, Filter, Range};

// 精确匹配
Condition::matches("category", "tutorial".to_string())

// 范围过滤
Condition::range("views", Range {
    gte: Some(1000.0),
    lte: Some(5000.0),
    ..Default::default()
})

// 数组包含
Condition::matches("tags", "rust".to_string())

// 空值检查
Condition::is_empty("optional_field")

// 地理位置过滤
Condition::geo_radius("location", GeoRadius {
    center: GeoPoint { lat: 52.5, lon: 13.4 },
    radius: 1000.0,
})
```

### 5.2 逻辑组合

```rust
// AND条件
Filter::must([condition1, condition2])

// OR条件
Filter::should([condition1, condition2])

// NOT条件
Filter::must_not([condition1])

// 复杂组合
Filter::must([
    Filter::should([condition1, condition2]),
    Filter::must_not([condition3]),
])
```

---

## 6. 高级特性

### 6.1 多向量支持

一个点可以包含多个命名向量：

```rust
use qdrant_client::qdrant::VectorsConfigBuilder;

let mut config = VectorsConfigBuilder::default();
config.add_named_vector_params("image", VectorParamsBuilder::new(512, Distance::Dot).build());
config.add_named_vector_params("text", VectorParamsBuilder::new(768, Distance::Cosine).build());
```

### 6.2 稀疏向量

支持稀疏向量存储和检索：

```rust
use qdrant_client::qdrant::SparseVectorParamsBuilder;

// 创建支持稀疏向量的集合
client
    .create_collection(
        CreateCollectionBuilder::new("sparse_collection")
            .sparse_vectors_config([("text-sparse".to_string(), SparseVectorParamsBuilder::default())])
    )
    .await?;
```

### 6.3 多向量 (Multivector)

支持一个点包含多个向量用于相似度计算：

```rust
// 配置多向量比较器
VectorParamsBuilder::new(128, Distance::Cosine)
    .multivector_config(MultivectorConfig {
        comparator: Comparator::MaxSim,
    })
```

### 6.4 快照与备份

```rust
// 创建快照
client.create_snapshot("my_collection").await?;

// 列出快照
let snapshots = client.list_snapshots("my_collection").await?;

// 恢复快照
client.recover_snapshot("my_collection", "snapshot_name").await?;
```

### 6.5 别名管理

```rust
// 创建别名
client.create_alias("my_collection", "alias_name").await?;

// 列出别名
let aliases = client.list_aliases("my_collection").await?;

// 删除别名
client.delete_alias("alias_name").await?;
```

---

## 性能特性

| 特性 | 描述 |
|------|------|
| 水平扩展 | 支持分片和分布式部署 |
| 持久化 | WAL + 快照保证数据安全 |
| 内存优化 | 量化、磁盘存储 |
| 并发处理 | 异步API，高吞吐量 |

---

## 7. 监控与遥测

Qdrant 提供两种指标获取方式，均可通过 HTTP REST API 访问。

### 7.1 Prometheus 指标端点 `/metrics`

返回 Prometheus/OpenMetrics 格式的文本，适合对接 Prometheus + Grafana。

```
GET http://localhost:6333/metrics
```

**核心 API 响应指标：**

| 指标名 | 类型 | 含义 |
|--------|------|------|
| `rest_responses_total` | counter | REST API 响应总数（含 method/endpoint/status 标签） |
| `rest_responses_fail_total` | counter | REST API 失败响应数 |
| `rest_responses_avg_duration_seconds` | gauge | 平均响应时间 |
| `rest_responses_duration_seconds` | histogram | 响应时间分布直方图 (v1.8+) |
| `grpc_responses_total` | counter | gRPC API 响应总数 |
| `grpc_responses_fail_total` | counter | gRPC API 失败响应数 |
| `grpc_responses_avg_duration_seconds` | gauge | gRPC 平均响应时间 |
| `grpc_responses_duration_seconds` | histogram | gRPC 响应时间分布 (v1.8+) |

**Per-collection 模式** (v1.18+)：

```
GET http://localhost:6333/metrics?per_collection=true
```

启用后 `rest_responses_total` 增加 `collection` 标签：

```
rest_responses_total{method="POST",endpoint="/collections/{collection_name}/points/search",status="200",collection="my-collection"} 42
```

**已计量的 REST 端点白名单：**

```
/collections/{collection_name}/points          (upsert)
/collections/{collection_name}/points/search   (search)
/collections/{collection_name}/points/delete   (delete)
/collections/{collection_name}/points/scroll   (scroll)
/collections/{collection_name}/points/payload  (set payload)
/collections/{collection_name}/points/payload/delete
/collections/{collection_name}/index           (create payload index)
```

**其他指标：** `collections_total`, `app_info`, `cluster_enabled`, `memory_active_bytes` 等。

### 7.2 遥测端点 `/telemetry`

返回结构化 JSON，包含更丰富的 per-collection 和 per-shard 统计。

```
GET http://localhost:6333/telemetry?details_level=1
```

**响应结构（关键部分）：**

```json
{
  "result": {
    "requests": {
      "rest": {
        "responses": {
          "POST /collections/{collection_name}/points/search": {
            "200": {
              "count": 42,
              "fail_count": 0,
              "avg_duration_micros": 1234.5,
              "total_duration_micros": 51849,
              "min_duration_micros": 800.0,
              "max_duration_micros": 3200.0,
              "last_responded": "2024-01-15T09:30:00Z"
            }
          },
          "PUT /collections/{collection_name}/points": { "...": "..." },
          "POST /collections/{collection_name}/points/delete": { "...": "..." },
          "POST /collections/{collection_name}/points/scroll": { "...": "..." },
          "POST /collections/{collection_name}/points/payload": { "...": "..." },
          "POST /collections/{collection_name}/points/payload/delete": { "...": "..." }
        },
        "per_collection_responses": {
          "my-collection": {
            "POST /collections/{collection_name}/points/search": {
              "200": { "count": 10, "fail_count": 0, "total_duration_micros": 12000 }
            }
          }
        }
      },
      "grpc": {
        "responses": {
          "/qdrant.Points/Search": { "0": { "count": 42, "fail_count": 0, "total_duration_micros": 51849 } },
          "/qdrant.Points/Upsert": { "...": "..." },
          "/qdrant.Points/Delete": { "...": "..." },
          "/qdrant.Points/Scroll": { "...": "..." },
          "/qdrant.Points/SetPayload": { "...": "..." },
          "/qdrant.Points/DeletePayload": { "...": "..." }
        }
      }
    },
    "collections": {
      "collections": [
        {
          "id": "my-collection",
          "shards": [
            {
              "remote": [
                {
                  "searches": { "count": 10, "fail_count": 0, "total_duration_micros": 12000 },
                  "updates": { "count": 50, "fail_count": 0, "total_duration_micros": 25000 }
                }
              ]
            }
          ]
        }
      ]
    }
  }
}
```

**`OperationStats` 字段：**

| 字段 | 类型 | 含义 |
|------|------|------|
| `count` | integer | 操作总数 |
| `fail_count` | integer \| null | 失败操作数 |
| `avg_duration_micros` | double \| null | 最近 128 次操作的加权平均耗时（微秒） |
| `total_duration_micros` | uint64 \| null | 累计总耗时（微秒） |
| `min_duration_micros` | double \| null | 最小耗时 |
| `max_duration_micros` | double \| null | 最大耗时 |
| `last_responded` | string \| null | 最后一次响应时间 |

**gRPC 端点白名单：**

```
/qdrant.Points/Search
/qdrant.Points/SearchBatch
/qdrant.Points/Upsert
/qdrant.Points/Delete
/qdrant.Points/Scroll
/qdrant.Points/SetPayload
/qdrant.Points/DeletePayload
/qdrant.Points/ClearPayload
/qdrant.Points/Count
/qdrant.Points/Recommend
/qdrant.Points/Query
```

### 7.3 端点名到 GraphDB 指标的映射

| Qdrant REST 端点 | Qdrant gRPC 端点 | GraphDB `MetricType` |
|---|---|---|
| `POST .../points/search` | `/qdrant.Points/Search` | `VectorSearchOps`, `VectorSearchErrors`, `VectorSearchLatencyMs` |
| `PUT .../points` | `/qdrant.Points/Upsert` | `VectorUpsertOps`, `VectorUpsertErrors`, `VectorUpsertLatencyMs` |
| `POST .../points/delete` | `/qdrant.Points/Delete` | `VectorDeleteOps`, `VectorDeleteErrors`, `VectorDeleteLatencyMs` |
| `POST .../points/scroll` | `/qdrant.Points/Scroll` | (scroll 不单独计量) |
| `POST .../points/payload` | `/qdrant.Points/SetPayload` | (payload 操作暂不计量) |
| `POST .../points/payload/delete` | `/qdrant.Points/DeletePayload` | (payload 操作暂不计量) |

**注意：** `total_duration_micros` 是累计值，采样器通过计算相邻两次采样的差值得到区间增量。`avg_duration_micros` 是最近 128 次操作的滑动窗口均值，不适合做区间差分。

### 7.4 集成方式

GraphDB 通过 `VectorMetricsSampler` 每 10 秒调用 `GET /telemetry` 获取全局统计数据，
将 `rest.responses` 和 `grpc.responses` 中的 `count`/`fail_count`/`total_duration_micros`
差分后写入 `StatsManager`。Embedding 指标由 `EmbeddingService` 自行记录，
不属于 Qdrant 服务端指标范围。

---

## 与GraphDB集成要点

1. **集合命名规范**: 使用 `space_{space_id}_{tag}_{field}` 格式
2. **Payload设计**: 存储顶点ID、标签、属性引用
3. **同步机制**: 复用现有SyncManager处理向量更新
4. **错误处理**: 统一错误类型，支持重试机制
