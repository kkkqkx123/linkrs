# P2 [query] 查询并行效率随 worker 数崩溃（E(n) 跌到 0.04–0.12）

## 现象（实测）

`parallel_scale_bench` 各查询的并行效率（E(n) = 实际加速 / 理想线性加速，1.0=完美）：

| 查询 | 1w E | 2w E | 4w E | 8w E | 8w storage R |
|---|---:|---:|---:|---:|---:|
| Q1 scan+filter+agg | 1.00 | 0.72 | 0.26 | 0.08 | 723% |
| Q2 scan+groupby | 1.00 | 1.09 | 0.51 | 0.15 | 653% |
| Q3 2-hop traversal | 1.00 | 0.36 | 0.11 | 0.04 | 1173% |
| E1 union multi-scan | 1.00 | 0.93 | 0.46 | 0.12 | 732% |

数据来源：`bench_results/linkrs-query.log:3376-3404`

- **并行不仅没线性加速，反而随 worker 数严重退化**：8 线程效率仅 0.04–0.12。
- E1 union 8 线程墙时 **836244µs vs 单线程 103387µs = 8.1× 变慢**（worker 越多越慢）。
- "storage R" 列（存储回退比例）随 worker 数从 100%+ 升到 1173%——并行化把存储扫描**放大**而非分摊。

## 影响

查询并行执行框架不仅没带来扩展收益，还显著拖慢。在多核真机上这会是查询层的主要瓶颈。

## 疑似根因

两种可能叠加，需真机区分：
1. **cgroup 4 核配额混杂**：8 worker 跑在 4 核配额上超订阅，调度竞争加剧（本环境 caveat，见 README 环境说明）。
2. **框架缺陷（更强信号）**：效率跌到 0.04 远超 4 核超订阅可解释；"storage R 随 worker 上升" 指向**每个 worker 重复全量存储回退扫描**，而非分区并行——即并行框架未真正分片，反而重复工作。E2 阶段还直接 panic（见 `query-parallel-collector-panic.md`），说明并行执行路径整体不成熟。

## 建议

1. **真机 ≥8 物理核复测**，排除 cgroup 4 核混杂（绝对数值需复核，但 0.04 效率强烈指向框架缺陷）。
2. 排查 "storage R 随 worker 上升" 的根因：每个 worker 是否重复全量 scan？分区 fallback 是否真正并行化？
3. 先修 `collector.rs:458` panic，再谈并行效率优化。

## 严重级
**P2（性能）** —— 方向受 cgroup caveat，但效率崩溃幅度远超环境可解释，强烈建议按框架缺陷排查。

## 数据来源
`bench_results/linkrs-query.log:3376-3404`
