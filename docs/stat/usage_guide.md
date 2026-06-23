# GraphDB 指标使用指南

## 快速开始

本指南介绍如何在 GraphDB 中使用指标系统，包括记录指标、查询指标和集成监控系统。

---

## 一、记录指标

### 1.1 基本用法

#### 计数器（Counter）

```rust
use metrics::counter;

// 记录简单计数
counter!("graphdb_query_total").increment(1);

// 记录带标签的计数
counter!("graphdb_error_by_type_total", "type" => "timeout").increment(1);

// 增加任意值
counter!("graphdb_rows_processed_total").increment(100);
```

#### 仪表（Gauge）

```rust
use metrics::gauge;

// 增加
gauge!("graphdb_active_connections").increment(1.0);

// 减少
gauge!("graphdb_active_connections").decrement(1.0);

// 设置值
gauge!("graphdb_memory_used_bytes").set(1024.0 * 1024.0);
```

#### 直方图（Histogram）

```rust
use metrics::histogram;
use std::time::Duration;

let duration = Duration::from_millis(50);

// 记录时间（秒为单位）
histogram!("graphdb_query_duration_seconds").record(duration.as_secs_f64());

// 记录大小（字节为单位）
histogram!("graphdb_response_size_bytes").record(1024.0);
```

### 1.2 命名规范

**格式**：`graphdb_<模块>_<操作>_<类型>`

**示例**：
```rust
// ✅ 正确的命名
counter!("graphdb_query_total")
counter!("graphdb_storage_scan_duration_seconds")
counter!("graphdb_executor_rows_processed_total")

// ❌ 错误的命名
counter!("queryCount")           // 缺少前缀
counter!("graphdb_Query_Total")  // 应该使用小写
counter!("graphdb/query/total")  // 应该使用下划线
```

**类型后缀**：
- `_total`: 计数器（单调递增）
- `_seconds`: 时间（秒）
- `_bytes`: 大小（字节）
- 无前缀：仪表（可增减）

### 1.3 使用标签

```rust
// 单个标签
counter!("graphdb_error_total", "type" => "timeout").increment(1);

// 多个标签
counter!("graphdb_request_total", 
         "method" => "GET",
         "status" => "success").increment(1);

// 动态标签
let query_type = "MATCH";
counter!("graphdb_query_total", "type" => query_type).increment(1);
```

**标签使用建议**：
- ✅ 使用有限的标签值（避免基数爆炸）
- ✅ 使用有意义的标签名
- ❌ 避免使用高基数的标签（如用户 ID）

---

## 二、查询指标

### 2.1 通过 HTTP 端点查询

**启动 Telemetry 服务器**：

```rust
use graphdb::api::telemetry;

// 启动 Telemetry 服务器（默认端口：9090）
telemetry::start_server("127.0.0.1:9090").await?;
```

**查询指标**：

```bash
# 获取所有指标（Plain Text 格式）
curl http://localhost:9090/metrics

# 获取所有指标（JSON 格式）
curl http://localhost:9090/metrics?format=json

# 过滤特定前缀的指标
curl http://localhost:9090/metrics?prefix=graphdb_query
```

### 2.2 Plain Text 格式示例

```
# TYPE graphdb_query_total counter
graphdb_query_total 1000

# TYPE graphdb_query_duration_seconds histogram
graphdb_query_duration_seconds_count 1000
graphdb_query_duration_seconds_sum 50.5
graphdb_query_duration_seconds_min 0.001
graphdb_query_duration_seconds_max 2.5
graphdb_query_duration_seconds_p50 0.045
graphdb_query_duration_seconds_p95 0.15
graphdb_query_duration_seconds_p99 0.5

# TYPE graphdb_active_connections gauge
graphdb_active_connections 50
```

### 2.3 JSON 格式示例

```json
{
  "counters": [
    ["graphdb_query_total", 1000],
    ["graphdb_error_total", 5]
  ],
  "gauges": [
    ["graphdb_active_connections", 50.0],
    ["graphdb_memory_used_bytes", 104857600.0]
  ],
  "histograms": [
    [
      "graphdb_query_duration_seconds",
      {
        "count": 1000,
        "sum": 50.5,
        "min": 0.001,
        "max": 2.5,
        "p50": 0.045,
        "p95": 0.15,
        "p99": 0.5
      }
    ]
  ],
  "timestamp": 1681555200
}
```

---

## 三、内置指标

### 3.1 查询指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_query_total` | Counter | 总查询数 |
| `graphdb_query_duration_seconds` | Histogram | 查询延迟 |
| `graphdb_query_active` | Gauge | 活跃查询数 |
| `graphdb_query_match_total` | Counter | MATCH 查询数 |
| `graphdb_query_create_total` | Counter | CREATE 查询数 |
| `graphdb_query_update_total` | Counter | UPDATE 查询数 |
| `graphdb_query_delete_total` | Counter | DELETE 查询数 |

### 3.2 存储指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_storage_scan_total` | Counter | 存储扫描次数 |
| `graphdb_storage_scan_duration_seconds` | Histogram | 扫描延迟 |
| `graphdb_storage_cache_hits_total` | Counter | 缓存命中数 |
| `graphdb_storage_cache_misses_total` | Counter | 缓存未命中数 |

### 3.3 执行器指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_executor_rows_processed_total` | Counter | 处理的行数 |
| `graphdb_executor_memory_used_bytes` | Gauge | 内存使用量 |

### 3.4 错误指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_error_total` | Counter | 错误总数 |
| `graphdb_error_by_type_total{type}` | Counter | 按类型分类的错误数 |

### 3.5 同步系统指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_sync_transactions_committed_total` | Counter | 提交的事务数 |
| `graphdb_sync_transactions_rolled_back_total` | Counter | 回滚的事务数 |
| `graphdb_sync_index_operations_total` | Counter | 索引操作总数 |
| `graphdb_sync_index_operations_insert_total` | Counter | 索引插入数 |
| `graphdb_sync_index_operations_update_total` | Counter | 索引更新数 |
| `graphdb_sync_index_operations_delete_total` | Counter | 索引删除数 |
| `graphdb_sync_retry_attempts_total` | Counter | 重试尝试数 |
| `graphdb_sync_retry_successes_total` | Counter | 重试成功数 |
| `graphdb_sync_retry_failures_total` | Counter | 重试失败数 |
| `graphdb_sync_dead_letter_queue_size` | Gauge | 死信队列大小 |
| `graphdb_sync_active_transactions` | Gauge | 活跃事务数 |
| `graphdb_sync_compensation_attempts_total` | Counter | 补偿操作数 |

### 3.6 全文搜索指标

| 指标名称 | 类型 | 描述 |
|----------|------|------|
| `graphdb_fulltext_index_ops_total` | Counter | 索引操作数 |
| `graphdb_fulltext_indexed_docs_total` | Counter | 索引文档数 |
| `graphdb_fulltext_search_ops_total` | Counter | 搜索操作数 |
| `graphdb_fulltext_search_duration_seconds` | Histogram | 搜索延迟 |
| `graphdb_fulltext_queue_size` | Gauge | 队列大小 |
| `graphdb_fulltext_cache_hits_total` | Counter | 缓存命中数 |
| `graphdb_fulltext_cache_misses_total` | Counter | 缓存未命中数 |

---

## 四、集成监控系统

### 4.1 集成 Prometheus

**步骤 1：配置 Prometheus**

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'graphdb'
    static_configs:
      - targets: ['localhost:9090']
    metrics_path: '/metrics'
    scrape_interval: 15s
```

**步骤 2：启动 Prometheus**

```bash
prometheus --config.file=prometheus.yml
```

**步骤 3：访问 Prometheus UI**

打开浏览器访问：http://localhost:9090/graph

**查询示例**：
```promql
# 查询速率
rate(graphdb_query_total[1m])

# 查询延迟百分位
histogram_quantile(0.95, rate(graphdb_query_duration_seconds_bucket[1m]))

# 查询错误率
rate(graphdb_error_total[1m]) / rate(graphdb_query_total[1m])
```

### 4.2 集成 Grafana

**步骤 1：添加 Prometheus 数据源**

1. 打开 Grafana
2. 进入 Configuration → Data Sources
3. 点击 "Add data source"
4. 选择 "Prometheus"
5. 配置 URL: http://localhost:9090
6. 点击 "Save & Test"

**步骤 2：创建仪表板**

1. 进入 Dashboard → Create → New Dashboard
2. 点击 "Add new panel"
3. 编写 PromQL 查询
4. 配置可视化选项
5. 点击 "Save"

**示例仪表板**：
- 查询速率和延迟
- 存储性能指标
- 错误率和类型分布
- 系统资源使用

### 4.3 集成 OpenTelemetry（未来）

```rust
// TODO: OpenTelemetry 集成示例
use opentelemetry::global;

// 配置 OpenTelemetry
let tracer = global::tracer("graphdb");

// 创建 span
let span = tracer.span_builder("query_execution").start(&tracer);

// 记录指标
// ...
```

---

## 五、最佳实践

### 5.1 指标设计

**DO（推荐）**：
- ✅ 使用有意义的指标名称
- ✅ 使用统一的命名规范
- ✅ 使用标签进行维度划分
- ✅ 记录关键业务指标
- ✅ 设置合理的告警阈值

**DON'T（避免）**：
- ❌ 使用无意义的名称
- ❌ 混用不同的命名风格
- ❌ 滥用标签（导致基数爆炸）
- ❌ 记录过多细节指标
- ❌ 忽略告警配置

### 5.2 性能优化

**批量记录**：
```rust
// ✅ 批量记录
for i in 0..100 {
    // 处理数据
}
counter!("graphdb_rows_processed_total").increment(100);

// ❌ 逐条记录
for i in 0..100 {
    // 处理数据
    counter!("graphdb_rows_processed_total").increment(1);
}
```

**减少标签**：
```rust
// ✅ 使用有限的标签
counter!("graphdb_request_total", "method" => "GET").increment(1);

// ❌ 使用高基数标签
counter!("graphdb_request_total", "user_id" => user_id).increment(1);
```

### 5.3 告警配置

**示例告警规则**：

```yaml
# alerting_rules.yml
groups:
  - name: graphdb
    rules:
      - alert: HighErrorRate
        expr: rate(graphdb_error_total[1m]) / rate(graphdb_query_total[1m]) > 0.05
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "GraphDB 错误率过高"
          description: "错误率超过 5%，当前值：{{ $value }}"

      - alert: HighQueryLatency
        expr: histogram_quantile(0.95, rate(graphdb_query_duration_seconds_bucket[1m])) > 1.0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "GraphDB 查询延迟过高"
          description: "P95 延迟超过 1 秒，当前值：{{ $value }}秒"

      - alert: HighActiveConnections
        expr: graphdb_active_connections > 100
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "GraphDB 活跃连接数过高"
          description: "活跃连接数超过 100，当前值：{{ $value }}"
```

---

## 六、故障排查

### 6.1 指标未记录

**检查清单**：
1. 确认 `TelemetryRecorder` 已设置为全局 recorder
2. 确认指标名称正确
3. 确认调用了 `increment()` 或 `record()` 方法
4. 检查是否有编译错误

**调试方法**：
```rust
// 手动检查 recorder
use metrics::recorder;

let recorder = metrics::recorder();
println!("Recorder: {:?}", recorder);
```

### 6.2 内存占用过高

**可能原因**：
- 直方图数据过多
- 标签组合过多（基数爆炸）

**解决方案**：
```rust
// 启用直方图清理
telemetry::cleanup_histograms(1000);  // 保留最多 1000 个条目

// 减少标签使用
// ❌ 避免
counter!("graphdb_request_total", "user_id" => user_id, "request_id" => request_id);

// ✅ 推荐
counter!("graphdb_request_total", "method" => "GET");
```

### 6.3 性能下降

**可能原因**：
- 指标记录过于频繁
- DashMap 冲突过多

**解决方案**：
```rust
// 批量记录
let mut batch_size = 0;
for item in items {
    // 处理数据
    batch_size += 1;
    
    if batch_size >= 100 {
        counter!("graphdb_items_processed_total").increment(batch_size as u64);
        batch_size = 0;
    }
}

// 处理剩余
if batch_size > 0 {
    counter!("graphdb_items_processed_total").increment(batch_size as u64);
}
```

---

## 七、示例代码

### 7.1 完整示例

```rust
use graphdb::api::telemetry;
use metrics::{counter, gauge, histogram};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 启动 Telemetry 服务器
    telemetry::start_server("127.0.0.1:9090").await?;
    
    // 模拟查询处理
    process_query().await;
    
    // 保持服务器运行
    tokio::time::sleep(Duration::from_secs(60)).await;
    
    Ok(())
}

async fn process_query() {
    let start = std::time::Instant::now();
    
    // 记录查询开始
    counter!("graphdb_query_total").increment(1);
    gauge!("graphdb_query_active").increment(1.0);
    
    // 模拟查询执行
    tokio::time::sleep(Duration::from_millis(50)).await;
    
    // 记录查询完成
    let duration = start.elapsed();
    histogram!("graphdb_query_duration_seconds").record(duration.as_secs_f64());
    gauge!("graphdb_query_active").decrement(1.0);
    
    // 记录处理结果
    counter!("graphdb_rows_processed_total").increment(100);
}
```

### 7.2 自定义指标

```rust
use metrics::{counter, histogram};

/// 记录自定义业务指标
pub fn record_custom_metric(event_type: &str, value: f64) {
    // 记录事件数
    counter!("graphdb_custom_event_total", "type" => event_type).increment(1);
    
    // 记录事件值
    histogram!("graphdb_custom_value", "type" => event_type).record(value);
}

// 使用示例
record_custom_metric("user_login", 1.0);
record_custom_metric("data_import", 1024.0);
```

---

## 八、参考资源

### 8.1 相关文档

- [架构文档](architecture.md) - 详细的架构说明
- [迁移总结](migration_summary.md) - 迁移过程和经验

### 8.2 外部资源

- [metrics crate 文档](https://docs.rs/metrics/)
- [Prometheus 文档](https://prometheus.io/docs/)
- [Grafana 文档](https://grafana.com/docs/)

---

**文档版本**：1.0  
**最后更新**：2026-04-15  
**维护者**：GraphDB Team
