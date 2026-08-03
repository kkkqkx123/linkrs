# Phase 3 验证实测数据归档（Parallel & Storage Validation）

- 执行日期：2026-08-03
- 执行环境：AMD Ryzen 7 8845HS（16 核），Linux，release/bench profile（含 Phase 0 `x86-64-v3` rustflags），内存存储
- 验证设施：`benches/storage_read_baseline.rs`（P3.1）、`benches/parallel_scale_bench.rs`（P3.2，走完整执行链路 + `EXPLAIN ANALYZE FORMAT = DOT` 逐算子耗时）
- 结论摘要：**P3.1 达标立项；P3.2 未达标（并行链路未接通，无法测得加速）**，详见 `docs/issue/parallel-execution-dead-chain.md`

## 1. P3.1 存储层读取基线

数据：宽表（8 列：BigInt×5 + String×3）/ 窄表（2 列），内存存储；扫描取 7 次中位。

| 负载 | 场景 | 行数 | 耗时 | 吞吐 |
|------|------|------|------|------|
| 宽表 | full cursor scan（批 256） | 10k | 22.1 ms | 452k rows/s |
| 宽表 | projected scan（1 列，批 256） | 10k | 4.5 ms | 2.22M rows/s |
| 宽表 | full scan（批 4096） | 10k | 40.7 ms | 245k rows/s |
| 宽表 | full scan（批 256） | 100k | 284 ms | 352k rows/s |
| 宽表 | projected scan（批 256） | 100k | 62.5 ms | 1.60M rows/s |
| 窄表 | full scan（批 256） | 10k | 6.9 ms | 1.46M rows/s |
| 窄表 | projected scan | 10k | 1.4 ms | 7.34M rows/s |
| 窄表 | full scan（批 256） | 100k | 83.1 ms | 1.20M rows/s |
| 窄表 | projected scan | 100k | 25.5 ms | 3.92M rows/s |
| 窄表 | 随机 get_vertex（10k 次，种子固定） | 100k | 15.0 ms | 1.50 µs/op |
| 窄表 | 随机 get_vertex_projected | 100k | 11.6 ms | 1.16 µs/op |

投影/全行加速比（列式属性块 PoC）：

| 负载 | 10k | 100k |
|------|-----|------|
| 宽表 8 列扫描 | **4.91x** | **4.54x** |
| 窄表 2 列扫描 | **5.04x** | **3.26x** |
| 窄表随机访问 | — | 1.29x（查找主导，投影无收益） |

要点：

- PoC 门槛（≥1.5x）达标；`VertexTable`/`ColumnStore` 列式布局已存在（`graphdb-storage/src/storage/vertex/vertex_table/core.rs`），收益来自既有列式存储的投影路径
- 批 4096 反而慢于批 256（全扫 100k 宽表 245k vs 352k rows/s）：行解码/物化成本主导，非批大小主导

## 2. P3.1 端到端存储读占比 R

方法：`EXPLAIN ANALYZE FORMAT = DOT`，R = Σ(StorageScanVertices/GetVertices/AppendVertices/Expand 等存储类算子 exec_duration) / 根算子 exec_duration（逐算子时间为累积语义，取根节点为分母）。

数据：100k 顶点（`value`/`group_id` 属性）+ 300k 边（每顶点 3 条），串行执行。

| 查询 | 形态 | R |
|------|------|---|
| Q1 | `MATCH (n:Node) WHERE n.value < 50000 RETURN count(n)` | **33%** |
| Q2 | `MATCH (n:Node) RETURN n.group_id, count(*)` | **50%** |
| Q3 | `MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)` | **92%** |

结论：表扫描类查询存储读占 1/3~1/2（其余为查询侧逐行 `Value` 处理）；图遍历型存储读占 92%（Expand/邻居读取主导）。

## 3. P3.2 并行加速曲线

方法：每个 workers 配置独立 `OptimizerEngine`（`PartitioningConfig { enabled, vertex_id_range=0..100000, min_rows_per_partition=100000/workers, max_partitions=workers, max_workers=workers }`）+ 独立 stats（`collect_statistics(space, force=true)`）；每配置 3 次预热 + 11 次计时取中位；`EXPLAIN ANALYZE`（TABLE + DOT 双格式）采集 `requested/actual_workers`、`parallel_work/wall_time_us`、`fallback_reason`、逐算子耗时。

数据：100k 顶点 + 300k 边（同一份存储，跨配置复用）。

| 查询 | workers | actual | T(n) 中位 | E(n) | work/wall |
|------|---------|--------|-----------|------|-----------|
| Q1 scan+filter+agg | 1 | 1 | 905 ms | 1.00 | 0 / 0 |
| Q1 | 2 | **1** | 933 ms | 0.97 | 0 / 0 |
| Q1 | 4 | **1** | 947 ms | 0.96 | 0 / 0 |
| Q1 | 8 | **1** | 922 ms | 0.98 | 0 / 0 |
| Q2 scan+groupby | 1 | 1 | 647 ms | 1.00 | 0 / 0 |
| Q2 | 2 | **1** | 644 ms | 1.00 | 0 / 0 |
| Q2 | 4 | **1** | 650 ms | 1.00 | 0 / 0 |
| Q2 | 8 | **1** | 654 ms | 0.99 | 0 / 0 |
| Q3 1-hop 遍历 | 1 | 1 | 5.15 s | 1.00 | 0 / 0 |
| Q3 | 2 | **1** | 4.99 s | 1.03 | 0 / 0 |
| Q3 | 4 | **1** | 5.32 s | 0.97 | 0 / 0 |
| Q3 | 8 | **1** | 5.82 s | 0.89 | 0 / 0 |

结论：

- **所有配置 `actual_workers = 1`，`parallel_work/wall_time_us = 0`：端到端从未发生并行执行**。E(n) ∈ [0.89, 1.03] 为噪声（含每查询创建未使用的 `MorselWorkerPool` 的开销）
- 并行加速比**未得到任何验证**（功能不存在，无法测量），根因与修复方案见 `docs/issue/parallel-execution-dead-chain.md`
- Q3 图遍历型按规划器白名单预期不分区（fallback），仅作对照基线

## 4. 附带实测（独立问题，见 docs/issue）

| 项 | 数据 | 问题文档 |
|----|------|----------|
| 批量顶点插入 | 200k 顶点 510s（块 10k：1.2s → 块 170k：358s，单调恶化）；批量边插入 600k 仅 1.6s（线性） | `docs/issue/vertex-batch-insert-quadratic.md` |
| 无锚 2-hop 遍历 | 100k×3 边，无锚 2-hop count 单次 >5 min（终止）；锚定 1-hop 单次 5.15s | `docs/issue/traversal-query-pathology.md` |
| 查询侧行处理 | Q1 100k 行 ≈ 905ms（0.11M rows/s）vs 原始 cursor 全扫 352k rows/s、投影扫 1.6M rows/s | `docs/plan/columnar-phase3-*`（A1 列块消费） |

## 5. 环境说明与数据可信度

- 采样环境存在并发负载（多个开发进程 + 1 个 100% CPU 后台进程），T(n) 存在 5~15% 抖动；E(n) 结论（无加速）不受影响，因为 all-config 均 serial
- 后续并行重测须固定 CPU（`taskset`）并独占核心，见 `docs/plan/columnar-phase3-parallel-storage-verification-design.md` §验证方案
