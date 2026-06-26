# Linkrs 性能改进方案分析

> 基于 LadybugDB 对比分析，定位 Linkrs 的性能短板并提出可落地的改进方案
> 分析日期：2026-06-25
> 更新时间：2026-06-26（与 `44ab47d` 提交实现同步）

---

## 目录

1. [架构演进：EdgeStore 枚举分层](#1-架构演进edgeStore-枚举分层)
2. [邻接遍历性能改进](#2-邻接遍历性能改进)
3. [并行查询执行](#3-并行查询执行)
4. [BufferManager 与数据换页](#4-buffermanager-与数据换页)
5. [列级压缩与存储优化](#5-列级压缩与存储优化)
6. [索引持久化与磁盘友好](#6-索引持久化与磁盘友好)
7. [查询 Pipeline 与算子框架优化](#7-查询-pipeline-与算子框架优化)
8. [GDS / 图算法框架](#8-gds--图算法框架)
9. [优先级排序与实施路线图](#9-优先级排序与实施路线图)

---

## 1. 架构演进：EdgeStore 枚举分层

### 1.1 设计背景

根据 `linkrs_time_travel_runtime_design.md` 的分析，Linkrs 采用 **运行时枚举分发** 方案替代编译期 feature flag 或 trait object：

```rust
pub enum EdgeStore {
    TimeTravel(TimeTravelEdgeStore),  // 多段 CSR，完整 Time-Travel
    Simple(SimpleEdgeStore),          // 单段 CSR，零历史开销
}
```

**核心设计原则：**
- 一个二进制文件服务所有 Space 和 Edge Type
- 每个 Edge Type 独立决定是否启用 Time-Travel
- 入口处分发一次（`EdgeStore::create()`），内部通过 `match` 分发
- 当前时间查询零额外开销（Simple 变体）

### 1.2 架构对比

| 维度 | LadybugDB | Linkrs (改进后) |
|------|-----------|----------------|
| 边存储 | 单段 CSR | **双模式**：TimeTravel（多段）/ Simple（单段） |
| 时间旅行 | 不支持 | **Edge Type 级可选** |
| 当前查询 | 单段 CSR O(degree) | Simple: 单段 CSR O(degree) / TimeTravel: 快照缓存 O(degree) |
| 历史查询 | N/A | TimeTravel: 多段遍历 + 稀疏索引 |

### 1.3 已实施内容（P0）

提交 `44ab47d` 已完成以下核心实现：

| 组件 | 文件 | 说明 |
|------|------|------|
| `EdgeStore` 枚举 | `edge_table/mod.rs` | `TimeTravel` + `Simple` 双变体，match 分发 |
| `TimeTravelEdgeStore` | `edge_table/core.rs` | 多段 CSR + 稀疏顶点索引 + 快照缓存 |
| `SimpleEdgeStore` | `edge_table/simple.rs` | 单段 CSR，轻量删除追踪 |
| 稀疏顶点索引 | `edge_table/core.rs` | `sparse_vertex_index_out/in: HashMap<u32, Vec<usize>>` |
| 快照缓存 | `edge_table/core.rs` | `current_snapshot_out/in: Option<Csr>`，ts=MAX 快速路径 |

---

## 2. 邻接遍历性能改进

### 2.1 问题定位

**TimeTravelEdgeStore 的遍历瓶颈：**
- 邻接遍历需合并 `Mutable CSR + 多个 Immutable Segment`
- 对每个源节点遍历所有 segment，然后用 `HashSet` 去重
- 当 segment 数量达到阈值（默认 50）时，遍历开销显著

**SimpleEdgeStore 的优势：**
- 单段 CSR，无 segment 遍历，零合并开销
- 适用于不需要 Time-Travel 的 Edge Type（如高频 OLTP 边）

### 2.2 已实施的优化（P0）

#### 2.2.1 稀疏顶点索引（Sparse Vertex Index）

**实现位置：** `TimeTravelEdgeStore` 结构体

```rust
pub struct TimeTravelEdgeStore {
    // ... 现有字段 ...
    
    /// 稀疏顶点索引：vid → segment 索引列表
    /// 仅对不可变 segment 构建，避免对所有 segment 的无效扫描
    pub sparse_vertex_index_out: HashMap<u32, Vec<usize>>,
    pub sparse_vertex_index_in: HashMap<u32, Vec<usize>>,
    
    /// 当前时间快照缓存：ts=MAX 查询时跳过 segment 遍历
    pub current_snapshot_out: Option<Csr>,
    pub current_snapshot_in: Option<Csr>,
    pub snapshot_dirty: bool,
}
```

**构建与更新策略：**

| 方法 | 触发时机 | 复杂度 |
|------|----------|--------|
| `rebuild_sparse_vertex_indices()` | freeze/merge 后全量重建 | O(total_edges) |
| `append_sparse_index_out/in()` | 新增 segment 增量更新 | O(new_segment_edges) |
| `rebuild_current_snapshot()` | freeze/merge 后重建快照 | O(total_edges) |

**查询路径优化：**

```rust
fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
    if ts == u32::MAX && !self.snapshot_dirty && self.current_snapshot_out.is_some() {
        // 快速路径：使用快照缓存 + Mutable CSR，避免 segment 遍历
        self.merged_edges_of_current(&self.out_csr, src)
    } else {
        // 标准路径：稀疏索引过滤 + segment 遍历
        self.merged_edges_of(&self.out_csr, &self.out_segments, 
                            Some(&self.sparse_vertex_index_out), src, ts)
    }
}
```

#### 2.2.2 当前时间快照缓存

**设计原理：**
- 对于 `ts = MAX` 查询（占大多数 OLTP 场景），维护合并后的单一 CSR
- 快照在 freeze/merge 后重建，查询时无需遍历 segment
- 快照脏标记（`snapshot_dirty`）避免不必要的重建

**效果：**
- TimeTravelEdgeStore 的 ts=MAX 查询从 O(N_segments × degree) 降至 O(degree)
- 与 SimpleEdgeStore 性能对齐，同时保留 Time-Travel 能力

### 2.3 后续优化方向（P1）

#### 2.3.1 边遍历去重优化

**现状：** `HashSet<EdgeId>` 做 O(E * hash) 去重。

**改进方向：** 利用 segment 时间戳单调性，从新到旧遍历，首次出现即有效。

**注意：** 此优化对 SimpleEdgeStore 不适用（单段 CSR 无需去重）。

#### 2.3.2 邻接缓存（可选）

对于高频访问的热点顶点，可添加 LRU 缓存。**仅适用于 TimeTravelEdgeStore 的 ts=MAX 查询路径。**

### 2.4 预期效果

| 场景 | 现状 | 改进后 | 加速比(估) |
|------|------|--------|------------|
| SimpleEdgeStore (任意 ts) | N/A | 单段 CSR，零合并 | 基准 |
| TimeTravelEdgeStore ts=MAX (50 segments) | 扫描 50 个 segment | 快照缓存 + Mutable CSR | ~10-50x |
| TimeTravelEdgeStore ts<MAX (稀疏顶点) | 扫描 50 个 segment | 稀疏索引过滤后 1-2 个 | ~10-50x |
| TimeTravelEdgeStore ts<MAX (密集顶点) | 扫描 50 个 segment | 稀疏索引 + HashSet 去重 | ~3-5x |

---

## 3. 并行查询执行

### 3.1 问题定位

Linkrs 的查询执行以单线程为主：
- `GraphDataStore` 内部的 `VertexTable` 和 `EdgeTable` 被 `RwLock` 保护
- `Executor::execute()` 是同步阻塞调用，无任务分解/调度机制

### 3.2 改进方向

#### 3.2.1 数据分区（Partition）

**现状：** 单个 VertexTable / EdgeTable 是单体结构。

**改进方向：** 按顶点 ID 范围分区，多分区可并行扫描。

**注意：** 分区设计需考虑 `EdgeStore` 枚举的分发开销。建议在 `EdgeStore` 内部对 `TimeTravelEdgeStore` 和 `SimpleEdgeStore` 分别实现分区策略。

#### 3.2.2 并行 HashJoin

**现状：** HashTable 单线程构建。

**改进方向：** Build-Probe 分离模式，并行构建哈希表。

#### 3.2.3 异步任务调度器

引入简单的 `TaskScheduler`，将 Scan 任务分片调度。

### 3.3 预期效果

| 场景 | 现状 | 改进后 (8 核) |
|------|------|---------------|
| 全表扫描 (1M vertices) | 单线程 ~500ms | 8 线程并行 ~70ms |
| Cross-Join | 单线程 O(n*m) | 并行 Build + Probe |
| Graph Traversal (BFS) | 单线程逐层扩展 | 分区并行扩展 |

---

## 4. BufferManager 与数据换页

### 4.1 问题定位

- 全部数据在内存：`VertexTable.columns` 是 `Vec<Box<dyn ColumnStorage>>`
- 大数据集 OOM：超过可用内存时只能依赖操作系统 swap

### 4.2 改进方向

#### 4.2.1 Page 抽象（高优先级，大改动）

将 Column 数据切分为固定大小的 Page，通过 PageCache 管理。

#### 4.2.2 mmap 过渡方案（低侵入）

将 `FixedWidthColumn` 的 `Vec<u8>` 替换为内存映射文件。

#### 4.2.3 Cold/Hot 分离

利用现有 `TieredTombstoneManager` 的分层思想，扩展到数据存储。

### 4.3 预期效果

| 场景 | 现状 | 改进后 |
|------|------|--------|
| 100GB 数据 / 32GB 内存 | OOM 或不可用 | 冷数据自动换出到磁盘 |
| 大范围扫描 | 快速 (全内存) | 可能变慢 (I/O 换入) |

---

## 5. 列级压缩与存储优化

### 5.1 问题定位

- `ColumnEncoding` / `FsstColumn` 等编码在列级别可选，但压缩不是默认开启
- LadybugDB 默认启用多种压缩算法

### 5.2 改进方向

#### 5.2.1 压缩选择器增强

增加按数据类型自动选择压缩策略的逻辑。

#### 5.2.2 运行时可变编码

允许在 compaction/flush 时重新评估并切换编码。

#### 5.2.3 NodeGroup 级别压缩

借鉴 LadybugDB 的 NodeGroup 概念，按批压缩。

### 5.3 预期效果

| 场景 | 现状 | 改进后 | 存储节省 |
|------|------|--------|----------|
| Int 列 (小范围) | 4 bytes/值 | Bitpack 1 byte/值 | ~75% |
| Float 列 | 4 bytes/值 | FSC 2-3 bytes/值 | ~25-50% |
| String 列 (字典) | 原始字符串 | Dict/FSST | ~30-70% |

---

## 6. 索引持久化与磁盘友好

### 6.1 问题定位

- `IdIndexer` (`HashMap`) 和 `VertexIndexManager` (`BTreeMap`) 全部在内存
- 大数据量下恢复时需从文件反序列化全部索引到内存

### 6.2 改进方向

#### 6.2.1 IdIndexer Page 化

使用 `DiskHashMap` 或 LSM-tree 风格的持久化索引。

#### 6.2.2 BTreeMap 索引分层

借鉴 SSTable 思路，将 BTreeMap 转化为不可变的有序段。

### 6.3 预期效果

| 场景 | 现状 | 改进后 |
|------|------|--------|
| 100M 顶点 | 100M * ~40 bytes = 4GB 内存 | 缓存 + 磁盘 ~512MB 缓存 |
| 冷启动加载 | 反序列化全部 4GB | 仅加载热数据，按需换入 |

---

## 7. 查询 Pipeline 与算子框架优化

### 7.1 问题定位

- 现有 `Executor` 采用 `execute()` 返回 `Vec<Value>` 的物化模式
- LadybugDB 采用 volcano-style pull-based pipeline

### 7.2 改进方向

#### 7.2.1 流式 Chunk 接口

```rust
pub struct DataChunk {
    columns: Vec<ColumnVector>,
    sel_vector: Option<SelectionVector>,
    size: usize,
}

#[async_trait]
pub trait StreamingExecutor: Send {
    async fn open(&mut self) -> DBResult<()>;
    async fn next(&mut self) -> DBResult<Option<DataChunk>>;
    async fn close(&mut self) -> DBResult<()>;
}
```

#### 7.2.2 Pipeline 调度器

### 7.3 预期效果

| 场景 | 现状 (全物化) | 改进后 (流式) |
|------|---------------|---------------|
| LIMIT 10 / 大表 | 扫描全部数据到内存 | 扫描 10 行即停止 |
| 大表排序 + TopK | 全部排序 | 堆排序 O(N log K) |

---

## 8. GDS / 图算法框架

### 8.1 问题定位

- Linkrs 有 `MultiShortestPathNode` / `BFSShortestNode` 等计划节点，但实现待确认
- LadybugDB 有完整的 GDS 框架

### 8.2 改进方向

#### 8.2.1 通用 GDS 框架

```rust
pub trait VertexCompute: Send + Sync {
    fn begin_on_table(&mut self, table_id: LabelId) -> bool;
    fn vertex_compute(&mut self, vid: u32, table_id: LabelId);
    fn copy(&self) -> Box<dyn VertexCompute>;
}

pub struct GdsExecutor {
    compute: Box<dyn VertexCompute>,
    graph: Arc<GraphStorage>,
    num_threads: usize,
}
```

#### 8.2.2 BFS 并行化

### 8.3 预期效果

| 场景 | 现状 | 改进后 |
|------|------|--------|
| 单源 BFS | 单线程逐层扩展 | 分区并行扩展 |
| PageRank | 未实现 | 并行迭代 |

---

## 9. 优先级排序与实施路线图

### 9.1 优先级矩阵

| 序号 | 改进项目 | 预期收益 | 实现难度 | 代码侵入 | 优先级 | 状态 |
|------|----------|----------|----------|----------|--------|------|
| A1 | EdgeStore 枚举分层 | 高 (架构基础) | 中 | 中 | **P0** | ✅ 已完成 |
| A2 | 稀疏顶点索引 | 高 (遍历减速) | 低 | 中 | **P0** | ✅ 已完成 |
| A3 | 当前时间快照缓存 |高 (ts=MAX 加速) | 中 | 中 | **P0** | ✅ 已完成 |
| A4 | SimpleEdgeStore 实现 | 高 (零开销) | 中 | 中 | **P0** | ✅ 已完成 |
| A5 | 数据分区 + 并行扫描 | 高 (多核利用) | 中 | 大 | **P1** | 待实施 |
| A6 | 并行 HashJoin | 高 | 中 | 中 | **P1** | 待实施 |
| A7 | 列级压缩选择器增强 | 中 | 低 | 低 | **P1** | 待实施 |
| A8 | 流式 Chunk 接口 | 高 (内存优化) | 高 | 大 | **P1** | 待实施 |
| A9 | IdIndexer 持久化换页 | 中 | 高 | 大 | **P2** | 待实施 |
| A10 | PageCache 抽象 | 高 | 高 | 大 | **P2** | 待实施 |
| A11 | GDS 框架 | 中 | 高 | 中 | **P2** | 待实施 |
| A12 | BTreeMap 分层索引段 | 中 | 中 | 中 | **P2** | 待实施 |
| A13 | BFS 并行化 | 中 | 低 | 低 | **P2** | 待实施 |
| A14 | 邻接缓存 | 低 | 低 | 低 | **P3** | 待实施 |

### 9.2 推荐实施顺序

#### 阶段 1：P0 已完成 ✅

| 任务 | 改动范围 | 核心文件 | 状态 |
|------|----------|----------|------|
| EdgeStore 枚举架构 | edge_table 层 | `edge_table/mod.rs` | ✅ 完成 |
| SimpleEdgeStore 实现 | edge_table 层 | `edge_table/simple.rs` | ✅ 完成 |
| 稀疏顶点索引 | TimeTravelEdgeStore | `edge_table/core.rs` | ✅ 完成 |
| 快照缓存 | TimeTravelEdgeStore | `edge_table/core.rs` | ✅ 完成 |
| ts=MAX 快速路径 | TimeTravelEdgeStore | `edge_table/core.rs` | ✅ 完成 |

#### 阶段 2：查询引擎增强 (P1，4-8 周)

| 任务 | 改动范围 | 核心文件 |
|------|----------|----------|
| 流式 Chunk 接口 | query executor 层 | `executor/base.rs`, `executor/mod.rs` |
| Pipeline 调度器 | query 层 | `query_pipeline_manager.rs` |
| 并行 HashJoin | executor/join | `executor/relational_algebra/join/` |
| 列级压缩选择器 | storage/encoding | `encoding/mod.rs`, `encoding/compression.rs` |
| 数据分区架构 | GraphDataStore + VertexTable/EdgeTable | `engine/data_store.rs`, `vertex/vertex_table/core.rs` |

#### 阶段 3：持久化与索引优化 (P2，4-6 周)

| 任务 | 改动范围 | 核心文件 |
|------|----------|----------|
| IdIndexer 缓存层 | storage/vertex | `vertex/id_indexer.rs` |
| PageCache 抽象 | storage 层 (新模块) | `storage/cache/page_cache.rs` |
| 列存储 mmap 支持 | storage/vertex | `vertex/column_store.rs` |
| BTreeMap 分层索引段 | storage/index | `index/vertex_index_manager.rs` |
| GDS 框架 | query 层 (新模块) | `executor/gds/` |

---

## 总结

Linkrs 在**架构层面**的核心短板归纳为三点：

1. **缺乏并行机制**：数据不分区、查询不分任务、无法利用多核
2. **数据全在内存**：无换页机制，大数据集受限
3. **邻接遍历需多段合并**：无稀疏索引加速，高 segment 数时退化

**P0 改进已完成（提交 `44ab47d`）：**
- ✅ EdgeStore 枚举分层（TimeTravel + Simple）
- ✅ 稀疏顶点索引（TimeTravelEdgeStore）
- ✅ 当前时间快照缓存（TimeTravelEdgeStore）
- ✅ SimpleEdgeStore 实现（单段 CSR，零历史开销）

**后续重点方向：**
- P1：并行查询执行 + 流式 Chunk 接口 + 列级压缩
- P2：持久化换页 + GDS 框架

Linkrs 的核心优势（Time-Travel + MVCC + 属性索引）已通过 EdgeStore 枚举得到保留，同时 SimpleEdgeStore 为不需要历史查询的场景提供了零开销路径。
