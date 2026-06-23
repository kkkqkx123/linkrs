# Metrics 架构设计

## 1. 现状分析

### 1.1 当前架构总览

```
┌─────────────────────────────────────────────────────────────────────┐
│                        graphdb (src/)                               │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    core::stats 体系                           │   │
│  │  StatsManager │ QueryMetrics │ QueryProfile                  │   │
│  │  LatencyHistogram │ SlowQueryLogger │ ErrorStatsManager      │   │
│  │  AggregatedStatsManager                                      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                           │                                         │
│                           ▼                                         │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    search:: 模块                              │   │
│  │  SearchEngine trait → Adapters → crates/inversearch          │   │
│  │                              → crates/bm25                   │   │
│  │  FulltextIndexManager (引擎管理)                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
┌──────────────────────┐    ┌──────────────────────┐
│  crates/inversearch  │    │    crates/bm25        │
│  ┌────────────────┐  │    │  ┌────────────────┐   │
│  │ StorageMetrics │  │    │  │ StorageMetrics │   │
│  │ (内部实现细节)  │  │    │  │ (redis.rs 内)  │   │
│  │ MetricsCollector│  │    │  └────────────────┘   │
│  │ OperationTimer  │  │    │                       │
│  └────────────────┘  │    │                       │
└──────────────────────┘    └──────────────────────┘
```

### 1.2 各模块 Metrics 现状

#### crates/inversearch

| 文件                            | 内容                                                   | 状态           |
| ------------------------------- | ------------------------------------------------------ | -------------- |
| `src/storage/common/metrics.rs` | `StorageMetrics`, `MetricsCollector`, `OperationTimer` | 保留，内部实现 |
| `src/storage/file.rs`           | 使用 `MetricsCollector` 跟踪文件操作                   | 保留           |
| `src/storage/memory.rs`         | 使用 `MetricsCollector` 跟踪内存操作                   | 保留           |
| `src/storage/redis.rs`          | 独立的 `StorageMetrics` 结构体                         | 保留           |

特点：

- 纯内部实现，无外部依赖（仅使用 `std::sync::atomic`）
- `StorageMetrics` 通过 `StorageInterface` trait 的 `get_operation_stats()` 暴露
- 粒度：存储层操作计数、延迟、错误

#### crates/bm25

| 文件                        | 内容                                             | 状态 |
| --------------------------- | ------------------------------------------------ | ---- |
| `src/storage/redis.rs`      | 独立的 `StorageMetrics` 结构体                   | 保留 |
| `src/api/server/metrics.rs` | 仅 `init_logging()`（tracing_subscriber 初始化） | 保留 |

特点：

- 无独立的 metrics 模块
- `StorageMetrics` 仅存在于 redis.rs 中，与 inversearch 的版本重复
- 无 `metrics` crate 依赖

#### src/ (graphdb 主 crate)

| 模块                              | 内容                                 | 状态 |
| --------------------------------- | ------------------------------------ | ---- |
| `core/stats/manager.rs`           | `StatsManager` + `MetricType` 枚举   | 保留 |
| `core/stats/metrics.rs`           | `QueryMetrics`（查询阶段耗时）       | 保留 |
| `core/stats/profile.rs`           | `QueryProfile`（详细查询画像）       | 保留 |
| `core/stats/latency_histogram.rs` | `LatencyHistogram`（延迟百分位）     | 保留 |
| `core/stats/slow_query_logger.rs` | `SlowQueryLogger`（慢查询日志）      | 保留 |
| `core/stats/error_stats.rs`       | `ErrorStatsManager`（错误统计）      | 保留 |
| `core/stats/aggregated_stats.rs`  | `AggregatedStatsManager`（聚合统计） | 保留 |
| `search/`                         | 搜索模块，无 metrics 采集            | 缺失 |

### 1.3 关键发现

1. **crates/inversearch 和 crates/bm25 的 StorageMetrics 是重复的**：两个 crate 各自定义了几乎相同的 `StorageMetrics` 结构体，但互不共享。

2. **搜索模块缺少 metrics 采集**：`src/search/` 中的 `SearchEngine` trait 和 `FulltextIndexManager` 没有任何 metrics 采集。搜索操作的延迟、吞吐量、错误率均不可观测。

3. **crates 内部的 metrics 是纯内部实现**：`StorageMetrics` / `MetricsCollector` 仅使用 `std::sync::atomic`，无外部依赖，适合保留在 crate 内部。

4. **主 crate 已有完整的 stats 基础设施**：`StatsManager` + `MetricType` 枚举 + `LatencyHistogram` 等，可直接扩展用于搜索 metrics。

---

## 2. 决策：统一在 src/ 中实现

### 2.1 决策结论

**搜索相关的业务 metrics 统一在 `src/` 中实现，crates 内部的存储 metrics 保留为内部实现细节。**

### 2.2 决策依据

| 维度         | 统一在 src/                                    | 分散在各 crate                         |
| ------------ | ---------------------------------------------- | -------------------------------------- |
| **职责边界** | ✅ 搜索 metrics 是 graphdb 的业务关注点        | ❌ crate 应聚焦核心搜索算法            |
| **依赖管理** | ✅ 复用已有的 `core::stats` 基础设施           | ❌ 需为每个 crate 引入 metrics 依赖    |
| **一致性**   | ✅ 统一使用 `MetricType` 枚举和 `StatsManager` | ❌ 各 crate 各自实现，风格不一致       |
| **观测粒度** | ✅ 适配器层可观测 search/index/delete 全操作   | ⚠️ 只能观测 crate 内部，缺少业务上下文 |
| **独立使用** | ⚠️ 独立使用 crate 时无 metrics                 | ✅ 独立使用时自带 metrics              |
| **改造成本** | ✅ 只需在 adapter 层添加 decorator             | ❌ 需修改两个 crate 的公共 API         |

**关键判断**：crates/inversearch 和 crates/bm25 是**嵌入式库**，它们被 graphdb 通过 adapter 模式使用。metrics 应该在 adapter 层（即 `src/search/`）采集，而不是侵入到库内部。

### 2.3 例外：StorageMetrics 保留

`crates/inversearch` 中的 `StorageMetrics` / `MetricsCollector` / `OperationTimer` 作为**内部实现细节**保留，原因：

- 无外部依赖（仅 `std::sync::atomic`）
- 用于存储层自身的性能诊断
- 通过 `StorageInterface::get_operation_stats()` 暴露，不影响公共 API
- 与 graphdb 的业务 metrics 处于不同抽象层级

---

## 3. 目标架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        graphdb (src/)                               │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                    core::stats 体系                          │   │
│  │  StatsManager (扩展 MetricType)                             │   │
│  │  ├─ 查询 metrics (已有)                                     │   │
│  │  ├─ 搜索 metrics (新增) ← ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│   │
│  │  └─ 同步 metrics (已有)                                     │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                           │                                         │
│                           ▼                                         │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                    search:: 模块                             │   │
│  │                                                             │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │  MetricsSearchEngine (Decorator)                     │   │   │
│  │  │  - 包装 SearchEngine trait                           │   │   │
│  │  │  - 自动记录 search/index/delete 延迟和计数            │   │   │
│  │  │  - 通过 StatsManager 记录                            │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                           │                                    │   │
│  │                    ┌──────┴──────┐                             │   │
│  │                    ▼             ▼                              │   │
│  │  ┌────────────────────┐  ┌────────────────────┐                │   │
│  │  │ Bm25SearchEngine   │  │ InversearchEngine  │                │   │
│  │  │ (adapter)          │  │ (adapter)          │                │   │
│  │  └────────┬───────────┘  └────────┬───────────┘                │   │
│  └───────────┼───────────────────────┼────────────────────────────┘   │
│              ▼                       ▼                                 │
│     crates/bm25              crates/inversearch                       │
│     (StorageMetrics 内部)    (StorageMetrics 内部)                    │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.1 核心设计：MetricsSearchEngine Decorator

```rust
// src/search/metrics.rs
pub struct MetricsSearchEngine {
    inner: Arc<dyn SearchEngine>,
    stats_manager: Arc<StatsManager>,
    engine_type: EngineType,
    space_id: u64,
    index_name: String,
}

impl MetricsSearchEngine {
    /// 记录搜索操作
    async fn record_search(&self, query: &str, limit: usize,
                           start: Instant, result: &Result<Vec<SearchResult>, SearchError>) {
        let latency_ms = start.elapsed().as_millis() as u64;
        self.stats_manager.record_search(
            self.space_id, &self.index_name,
            self.engine_type, latency_ms, result.is_ok()
        );
    }

    /// 记录索引操作
    async fn record_index(&self, doc_id: &str, start: Instant, result: &Result<(), SearchError>) {
        let latency_ms = start.elapsed().as_millis() as u64;
        self.stats_manager.record_index_operation(
            self.space_id, &self.index_name,
            self.engine_type, latency_ms, result.is_ok()
        );
    }
}
```

### 3.2 StatsManager 扩展

```rust
// src/core/stats/manager.rs 扩展 MetricType
pub enum MetricType {
    // ... 已有类型 ...

    // 搜索 metrics (新增)
    NumSearchQueries,
    NumSearchErrors,
    SearchLatencyMs,
    NumIndexOperations,
    NumIndexErrors,
    IndexLatencyMs,
    NumDeleteOperations,
    NumDeleteErrors,
    DeleteLatencyMs,
    SearchResultCount,
    SearchCacheHitCount,
    SearchCacheMissCount,
}
```

### 3.3 搜索统计信息

```rust
// src/core/stats/search_stats.rs (新增)
pub struct SearchStatsCollector {
    /// 按引擎类型统计
    by_engine: DashMap<EngineType, EngineSearchStats>,
    /// 按 space 统计
    by_space: DashMap<u64, SpaceSearchStats>,
}

pub struct EngineSearchStats {
    pub total_queries: AtomicU64,
    pub total_errors: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub total_index_ops: AtomicU64,
    pub total_delete_ops: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
}
```

---

## 4. 各 crate 的 metrics 处理

### 4.1 crates/inversearch

| 组件                                      | 处理方式 | 说明                    |
| ----------------------------------------- | -------- | ----------------------- |
| `StorageMetrics`                          | **保留** | 内部实现，无外部依赖    |
| `MetricsCollector`                        | **保留** | 用于存储层性能诊断      |
| `OperationTimer`                          | **保留** | RAII 风格定时器         |
| `StorageInterface::get_operation_stats()` | **保留** | 通过 trait 暴露统计信息 |

**不需要做的改动**：

- 不添加 `metrics` crate 依赖
- 不修改公共 API
- 不添加 Prometheus 端点

### 4.2 crates/bm25

| 组件                       | 处理方式 | 说明              |
| -------------------------- | -------- | ----------------- |
| `redis.rs::StorageMetrics` | **保留** | 内部实现          |
| `api/server/metrics.rs`    | **保留** | 仅 tracing 初始化 |

**建议改进**：

- 将 `redis.rs` 中的 `StorageMetrics` 提取为独立模块，避免与 inversearch 重复
- 当前优先级低，可在后续统一 storage 层时处理

### 4.3 数据流

```
用户请求
    │
    ▼
FulltextIndexManager::search()
    │
    ▼
MetricsSearchEngine::search()  ← 记录 metrics
    │
    ├─► 记录 latency 到 StatsManager
    ├─► 记录 result_count
    ├─► 记录 error (如果有)
    │
    ▼
Bm25SearchEngine::search()  /  InversearchEngine::search()
    │
    ▼
crates/bm25  /  crates/inversearch  (内部 StorageMetrics 继续工作)
```

---

## 5. 实现计划

### 阶段一：基础设施（src/core/stats/ 扩展）

| 任务                        | 文件                    | 说明             |
| --------------------------- | ----------------------- | ---------------- |
| 新增 `MetricType` 枚举值    | `manager.rs`            | 搜索相关指标类型 |
| 新增 `SearchStatsCollector` | `search_stats.rs`（新） | 搜索统计收集器   |
| 注册到 `StatsManager`       | `manager.rs`            | 统一管理         |

### 阶段二：Decorator 实现（src/search/）

| 任务                          | 文件                      | 说明                      |
| ----------------------------- | ------------------------- | ------------------------- |
| 实现 `MetricsSearchEngine`    | `search/metrics.rs`（新） | 包装 `SearchEngine` trait |
| 集成到 `FulltextIndexManager` | `search/manager.rs`       | 创建引擎时自动包装        |
| 更新 `SearchEngineFactory`    | `search/factory.rs`       | 支持创建带 metrics 的引擎 |

### 阶段三：API 暴露

| 任务                        | 说明                          |
| --------------------------- | ----------------------------- |
| 扩展 `/api/statistics` 端点 | 返回搜索 metrics              |
| 添加搜索统计到现有响应      | 复用已有的 statistics handler |

### 阶段四：crate 内部优化（低优先级）

| 任务                                          | 说明         |
| --------------------------------------------- | ------------ |
| 统一 bm25 和 inversearch 的 `StorageMetrics`  | 提取公共定义 |
| 评估是否需要在 crate 层添加更细粒度的 metrics | 根据实际需求 |

---

## 6. 与现有架构的关系

### 6.1 复用现有基础设施

```
现有 StatsManager ──► 扩展 MetricType ──► 搜索 metrics
现有 LatencyHistogram ──► 搜索延迟百分位
现有 ErrorStatsManager ──► 搜索错误统计
现有 SlowQueryLogger ──► 搜索慢操作日志（可选）
```

### 6.2 不修改的部分

| 模块                                               | 原因                              |
| -------------------------------------------------- | --------------------------------- |
| `crates/inversearch/src/storage/common/metrics.rs` | 内部实现，无外部依赖              |
| `crates/bm25/src/storage/redis.rs::StorageMetrics` | 内部实现                          |
| `src/core/stats/metrics.rs` (QueryMetrics)         | 查询 metrics，与搜索 metrics 正交 |
| `src/core/stats/profile.rs` (QueryProfile)         | 查询画像，与搜索 metrics 正交     |

### 6.3 分层职责

| 层级                                  | 负责的 metrics                    | 实现方式                               |
| ------------------------------------- | --------------------------------- | -------------------------------------- |
| **graphdb 业务层** (src/)             | 搜索延迟、QPS、错误率、缓存命中率 | `StatsManager` + `MetricsSearchEngine` |
| **adapter 层** (src/search/adapters/) | 无（透传）                        | 由 `MetricsSearchEngine` 统一处理      |
| **crate 内部** (crates/)              | 存储操作计数、延迟                | `StorageMetrics` / `MetricsCollector`  |

---

## 7. 与参考设计的对照

| 参考设计组件            | 本项目方案                                    | 状态        |
| ----------------------- | --------------------------------------------- | ----------- |
| MetricsRegistry         | StatsManager（已有）                          | ✅ 复用     |
| Counter/Gauge/Histogram | MetricValue + LatencyHistogram（已有）        | ✅ 复用     |
| 领域层 (Search)         | MetricsSearchEngine Decorator                 | 📝 新增     |
| 领域层 (Storage)        | crates 内部的 StorageMetrics                  | ✅ 保留     |
| RAII 定时器             | crates/inversearch 的 OperationTimer          | ✅ 保留     |
| API 端点                | 扩展 `/api/statistics`                        | 📝 新增     |
| 标签系统                | 通过 space_id + engine_type + index_name 区分 | 📝 简化方案 |
