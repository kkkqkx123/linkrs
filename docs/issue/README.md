# Linkrs Bench 测试问题分析（2026-10-09）

> 执行方式：gh-proxy 拉取仓库（主仓库 + 3 个子模块 + llm-suite 嵌套子模块），rsproxy 全局 cargo 代理，
> 依次运行各包含 bench 的包（simvec、linkrs-core、linkrs-fulltext、linkrs-storage、linkrs-transaction、
> linkrs-query、tantivy 子工作区）。原始日志见 `bench_results/`（phase2.log / phase3.log / simvec.log）。

## 环境说明（影响并发类结论解读）

| 项 | 值 |
|---|---|
| CPU | AMD EPYC 9K65（宿主 192 核，**cgroup 配额 4 核**，32 核可见） |
| 内存 | 123 GB |
| 工具链 | rustc/cargo 1.93.0（stable）；bench profile（优化构建） |
| 代理 | rsproxy-sparse（依赖拉取正常）；gh-proxy（仓库/子模块） |

**重要**：`num_cpus/available_parallelism` 在本环境返回 4，而 `nproc` 返回 32。
所有多线程扩展性结论（concurrent_search、commit_gate）都受 4 核配额混杂，需在开发板/真机复核。

## 执行矩阵

| 包 | bench 数 | 成功 | 失败/挂死 | 说明 |
|---|---|---|---|---|
| simvec | 8 | 8 | 0 | ivf_bench 以 `--sample-size 10` 运行（默认 100 采样单项需 2.3h） |
| linkrs-core | 1 | 1 | 0 | |
| linkrs-fulltext | 1 | 0 | 1 | 需 `--features fulltext`；开启后 warmup 阶段 panic |
| linkrs-storage | 13 | 7 | 6 | ingest 挂死（已终止）；5 个 panic（详见各 issue） |
| linkrs-transaction | 2 | 2 | 0 | |
| linkrs-query | 6 | 4 | 2 | parallel_scale（spill 目录）、query_bench（PK 镜像） |
| tantivy 根包 | 14 | 14 | 0 | 逐 `--bench` 运行（workspace 级被 lib test 编译失败与 jitexpr 阻断）；agg_bench 约 29 分钟最重 |
| tantivy 子包 | bitpacker/columnar/sstable/stacker | 4 | 0 | columnar 含 9 个 bench 目标 |
| tantivy-common | 1 | 0 | — | exit=0 但 `0 measured`——bench 目标被 libtest 自动发现，binggan 代码永不执行（静默空跑） |
| jitexpr | — | — | — | 依赖 cranelift 0.134.4 需 rustc 1.94，1.93 拒绝解析（无法编译） |

## 缺陷索引（按严重级）

### P0 — 挂死 / 数据面阻塞

- [ingest-1m-wal-hang.md](ingest-1m-wal-hang.md)：**单批 1M 顶点 + 持久化存储挂死**。
  WAL 文件 70,090,953 字节停止增长，全部线程 futex 等待、零 CPU ≥35 分钟（已抓 gdb 栈佐证）。

### P1 — bench/库契约破裂（bench 无法运行 → 覆盖缺口）

- [bench-pk-mirror-fails.md](bench-pk-mirror-fails.md)：**5 个 bench** 因“主键必须镜像顶点 id”校验 panic
  （storage_bench:706、storage_workflow_bench:27、query_bench:70、edge_scan_speedup_bench:88、
  storage_read_baseline:93 列名不匹配）。BENCHMARK_REPORT 中 olap 基准修过同类问题，但其余 bench 未跟进。
- [rollback-bench-invalid-timestamp.md](rollback-bench-invalid-timestamp.md)：~524 次迭代后
  `commit_ordered` 报 `InvalidTimestamp(524)`，指向 MVCC 写槽位 settle 契约与 bench 用法的冲突。
- [spill-manager-dir-lifecycle.md](spill-manager-dir-lifecycle.md)：SpillManager Drop 无条件
  `remove_dir_all`，同 query_id 生命周期交错互删 spill 目录 → parallel_scale_bench warmup 即 ENOENT。
- [fulltext-bench-defects.md](fulltext-bench-defects.md)：`create_index` 在迭代内无清理（warmup 二轮
  panic `IndexAlreadyExists`）；且默认 features 下 bench 是惰性占位（静默零覆盖）。
- [tantivy-bench-infra.md](tantivy-bench-infra.md)：lib test 缺 `Bm25Params` 导入编译失败；
  jitexpr 需要 rustc 1.94；tantivy-common 的 binggan bench 被 libtest harness 空跑。

### P2 — 性能问题（实测数据支撑）

- [commit-gate-write-scaling-collapse.md](commit-gate-write-scaling-collapse.md)：写路径 commit gate
  1→16 线程 74.7k→995 stmts/s（gate wait share 72.7%）；且 bench 自带 verdict 文案与数据自相矛盾。
- [avx512-manhattan-single-accumulator.md](avx512-manhattan-single-accumulator.md)：avx512 距离核
  Manhattan 用单累加器（avx2 版是双累加器），实测 4.6 vs 18.2 Gelem/s（512d）。
- [hnsw-alloc-churn.md](hnsw-alloc-churn.md)：10k 点 HNSW 构建分配 1.85GB/4.4M 次；
  过滤搜索 miss 路径 645MB。热路径每次 search_layer 新建 HashSet/BinaryHeap。
- [query-plan-estimation-and-scan.md](query-plan-estimation-and-scan.md)：基数估计严重偏差
  （est 0.5 → 实际 100%；est 40k → 实际 300k）；顶点全扫描 ~1.7M 行/s，比边扫描慢两个数量级。

### P3 — 观察项（待真机复核 / 工程效率）

- [observational-notes.md](observational-notes.md)：concurrent_search 8 线程退化（cgroup 混杂待复核）、
  WAL fsync 596x 差距与 ingest WAL 占比、OLAP q3 聚合 TopN 仍最慢、wide point-lookup miss 路径
  8x 差距、criterion 大规模单项无 quick 模式（ivf 单项默认需 2.3h）。

## 结论摘要

1. **覆盖健康度差**：34 个声明 bench 目标中 9 个在当前 HEAD 无法运行或空跑（5 个 panic、
   1 个挂死、1 个 warmup panic、1 个静默空跑、jitexpr 不可编译），
   且失败是"数据生成 vs 存储校验契约"的系统性脱节——建议引入统一的 bench 数据生成 helper
   （PK 列镜像 vid）并让 `cargo bench --workspace` 全绿成为 CI 门禁。
2. **最严重的是数据面挂死**（P0），1M 单批写入在 WAL 持久化路径可复现挂死，建议优先在开发板
   用 `RUST_BACKTRACE=1` + debug 构建定位（疑点：WAL 段边界轮转 / 组提交协调）。
3. 写扩展性（commit gate 16 线程吞吐跌 75 倍）与向量构建分配（1.85GB churn）是两个最大的性能改进抓手。
