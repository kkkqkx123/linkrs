# 观察项汇总（2026-10-09 bench，待真机复核）

本文件收录未升级为独立 issue 的观察项。环境：EPYC 9K65，**cgroup CPU 配额 4 核**（32 核可见），
bench profile。并发类结论受 4 核配额混杂，绝对数值需开发板复核。

## 1. concurrent_search 8 线程反而比 4 线程慢

| threads | time | thrpt |
|---:|---:|---:|
| 1 | 47.77 ms | 5.36 Kelem/s |
| 4 | 14.39 ms | 17.79 Kelem/s（3.3x） |
| 8 | 17.80 ms | 14.38 Kelem/s（**回退 19%**） |

bench 自述期望"near-linear scaling until contention says otherwise"（concurrent_search_bench.rs）。
疑似 RwLock 读锁高频竞争或内存带宽饱和，但 **4 核配额下 8 线程超订阅是更强解释**——
在 ≥8 物理核的机器复测后再定性。若真机上仍回退，沿 adjacency 读锁路径排查。

## 2. WAL fsync 成本与 ingest 结构

- `wal_fsync/append_plus_sync_all_per_op`：**734 µs/op** vs `append_no_sync`：1.23 µs/op（596x）。
- ingest_bench（10k/100k 批）：WAL 时间占比 77.0% / 86.6%——持久化写入被 WAL 同步主导。
- `wal_sync_policy_ingest` 四种策略吞吐几乎相同（~999ms 窗口内），说明当前 ingest 路径
  可能并未真正按策略差异化同步（或批内合并已抵消），建议核对 every_write 策略是否生效。
- 1M 批挂死见 [ingest-1m-wal-hang.md](ingest-1m-wal-hang.md)。

## 3. OLAP q3（聚合 + TopN）仍是最慢查询

olap_e2e（5000 顶点 / 24,985 边，scale=1，与 BENCHMARK_REPORT 2026-10-06 基线同数据集）：

| 查询 | 本次 | 10-06 基线 |
|---|---:|---:|
| q1 two_hop_unanchored | 41.2 ms | 47.9 ms |
| q2 two_hop_anchored | 6.16 ms | 5.21 ms |
| **q3 group_aggregate_topn** | **111.2 ms** | 127.9 ms |
| q4 filtered_edge_scan | 30.9 ms | 38.8 ms |
| q5 filtered_vertex_scan | 5.90 ms | 5.73 ms |

q3 是 q1 的 2.7 倍、q5 的 19 倍，与报告"行存逐行物化 Value"判断一致，
是执行层列式化的首要受益者（见 [query-plan-estimation-and-scan.md](query-plan-estimation-and-scan.md)）。

## 4. CSR 宽行点查 miss 路径慢 8 倍

csr_perf_bench：point-lookup wide hit 20.0M ops/s，**miss 2.46M ops/s**（8.1x 差距）。
miss 路径疑似需要完整遍历 overflow chunk / tombstone 链，可考虑 bloom filter 或
per-row 状态位短路。

## 5. criterion 大规模单项无 quick 模式（工程效率）

- `ivf_build_time/build/100000` 默认 100 采样需 **8286s（2.3h）**（criterion 已自行警告
  "reduce sample count to 10"）；`hnsw_build/sequential/10000` 需 1252s。
- 本次用 `-- --sample-size 10` 降采样跑通，但 bench README/文档未提供规模/采样约定；
  建议在 bench_group helper（各包 benches/bench_group.rs 统一入口）中支持
  `LINKRS_BENCH_QUICK=1` 环境变量（降 sample_size + 缩 measurement_time），
  CI 与本地迭代用 quick 档，发版基线用全量档。

## 6. 其他小项

- `Gnuplot not found, using plotters backend`：无碍，纯提示。
- tantivy agg_bench 单包约 29 分钟（binggan，1000_segments 组），文档应标注预期时长。
- csr_perf "freeze reclaim: removed 0 edges"（before=38912 after=38912 但 mem 7508424→4321704）：
  回收边数为 0 但内存下降 3.2MB，计数器与实际释放不一致，建议核对 counters 语义。
