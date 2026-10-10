# 问题：查询计划基数估计严重偏差 + 顶点全扫描吞吐过低

- 状态：新建（待优化）
- 类型：性能缺陷（执行层 / 优化器）
- 复现：`cargo bench -p linkrs-query --bench traversal_perf_bench`（2026-10-09，
  100k 顶点 / 300k 边，两跳计数）

## 实测数据（计划 profile，累计时间）

| 算子 | rows 实际 | est_rows | 累计 time |
|---|---:|---:|---:|
| 0 StorageScanVertex | 100,000 | 1 | 57.7ms |
| 1 Filter | 100,000 | 0.5 | 59.1ms |
| 2 ExpandAll(id_only) | 300,000 | 40,000 | 194.2ms |
| 3 Flatten | 300,000 | 40,000 | 266.3ms |
| 4 ExpandAll(count_only) | 147 | 80,000 | 414.1ms |
| 6 Aggregate | 1 | 8,000 | 414.9ms |

## 问题点

1. **基数估计系统性偏差**：
   - Filter est_rows=0.5，实际 100,000（选择性被低估 20 万倍）；
   - ExpandAll est 40,000，实际 300,000（7.5 倍）；
   - 第二跳 ExpandAll est 80,000，实际 147（544 倍反向偏差）。
   该质量足以破坏 join/expand 顺序与 buffer 预算决策。
2. **顶点全扫描过慢**：StorageScanVertex 100k 行 57.7ms ≈ **1.7M 行/s**；
   同环境下 CSR 边扫描为 1.2-1.4 亿边/s（csr_perf_bench），相差两个数量级。
   与 BENCHMARK_REPORT 中"q3/q4 在行存执行模型下逐行物化 Value"的既知问题一致——
   顶点扫描路径尚未列式化。
3. 两跳计数端到端 415ms（100k/300k 图），作为 OLAP 基础负载偏慢。

## 修复方向

- Filter 选择性：接入已有的 column stats（min/max/HLL，linkrs-storage column_stats
  已具备）替代 0.5 默认值；ExpandAll 估计用出度统计。
- 顶点扫描列式化/向量化（批量解码代替逐行 Value 物化），与边扫描
  （caller-buffer/visitor 模式，1-6ns/edge）对齐。
- 修复后重跑 traversal_perf_bench 与 olap_e2e（q3 111ms 是当前最慢 OLAP 查询，
  同属聚合+逐行物化路径，预期一并受益）。
