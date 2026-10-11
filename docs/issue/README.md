# Linkrs 性能测试分析（2026-10-11 实测）

> 本文档是基于 **2026-10-11** 在沙箱中实跑的 `bench_results/*.log` 撰写的新分析。
> 它与仓库内既有的 `docs/issue/`（落款 2026-10-09、引用 `phase2.log`/`phase3.log` 等早期日志）**不是同一份**；
> 既有 `docs/issue/` 是更早一轮 bench 的产物，本分析不重复其内容，仅针对本次 2026-10-11 数据给出独立结论。
> 配套 patch：`linkrs_analysis_20261011.patch`（仅含 `docs/bench-report-2026-10-11/` 目录，可直接 `git apply` 到已应用过 `linkrs_testdata.patch` 的仓库，路径不冲突）。

## 测试环境（影响并发类结论）

| 项 | 值 |
|---|---|
| CPU | AMD EPYC 9K65（192 核宿主，**cgroup CPU 配额 4 核**，日志自报 "4 visible cores"） |
| 工具链 | rustc/cargo 1.93.0（stable），`bench` profile（优化构建） |
| 代理 | rsproxy-sparse（crates.io）+ ghproxy（仓库/子模块） |
| 采样 | 多数 bench 用 `-- --sample-size 10` 降采样（criteria 默认 100 采样在大规模单项上需数十分钟到数小时） |

> **并发类结论的强 caveat**：本环境 cgroup 仅 4 核配额，但 `nproc` 可见 32 核。
> 凡涉及多线程扩展性的结论（commit gate、edge scan、query parallel、concurrent_search）均受 4 核配额混杂，
> 绝对数值请在 ≥8 物理核的真机/开发板复核；但**趋势方向**（如 gate 等待占比单调上升、并行效率崩溃）仍有参考价值。

## 执行矩阵（本次 2026-10-11）

| 包 | bench 数 | 结果 | 关键现象 |
|---|---|---|---|
| simvec | 8 | 全 OK | avx512 无收益、upsert_wal 批量反慢、filter 选择性反慢 |
| linkrs-core | 1 | OK | serde 基线（serialize_vertex 280ns） |
| linkrs-fulltext | 1 | OK* | 需本地补丁才能编译；index_build 100/500 均 ~31ms（零缩放可疑） |
| linkrs-storage | 13 | **EXIT=124 超时挂死** | commit gate 1→16 线程吞吐跌 65×；边扫描并行加速比≈1.0；ingest WAL 挂死复现 |
| linkrs-transaction | 2 | OK | begin_read 2.12µs，write_set/build 线性偏超线性 |
| linkrs-query | 6 | **EXIT=101 panic** | parallel_scale 在 `collector.rs:458` panic；并行效率崩溃 |

`*` linkrs-fulltext 因上游 `tantivy` 子模块未 pin commit、fork 默认分支移除了 `Bm25Params`/`IndexSettings.bm25_params`，
沙箱做了 import 替换+去字段的本地临时补丁才编译通过（补丁未纳入任何 patch，根因修复应为 pin 子模块 commit，见 `fulltext-numbers-and-infra.md`）。

## 发现索引（按严重级）

### P0 — 数据面挂死
- [storage-ingest-wal-hang.md](storage-ingest-wal-hang.md)：`linkrs-storage` 整包 bench `EXIT=124`（超时挂死），与更早一轮记录的 1M 顶点 WAL 持久化挂死一致，本次复现但未捕获 ingest 指标（进程在超时前被杀）。

### P1 — 正确性 / bench 基础设施破裂
- [query-parallel-collector-panic.md](query-parallel-collector-panic.md)：`parallel_scale_bench` 在 `collector.rs:458` panic `writer must exist`，整包 `EXIT=101`，并行扩展类覆盖缺口。
- [fulltext-numbers-and-infra.md](fulltext-numbers-and-infra.md)：fulltext 编译依赖 tantivy 子模块 API，未 pin commit 导致默认分支漂移；index_build 100/500 零缩放（疑似固定开销主导）。

### P2 — 性能问题（本次实测数据支撑）
- [storage-write-path-gate-collapse.md](storage-write-path-gate-collapse.md)：写路径 commit gate 1→16 线程吞吐 71738→1101 stmts/s（**65× 崩溃**），gate 等待占比 0.1%→57.4%，bench 自带 verdict 与数据自相矛盾。
- [storage-edge-scan-no-parallel-speedup.md](storage-edge-scan-no-parallel-speedup.md)：边扫描并行加速比 E(n) 在 1→8 worker 间稳定在 1.00–1.02，零加速。
- [query-parallel-efficiency.md](query-parallel-efficiency.md)：查询并行效率随 worker 数崩溃（Q1 8 线程 E=0.08、Q3 E=0.04、E1 union 8 线程墙时 836ms vs 单线程 103ms）。
- [simvec-avx512-no-gain.md](simvec-avx512-no-gain.md)：avx512 距离核在 384/768 维**慢于** avx2，仅 1536 维勉强打平——avx512 路径基本无收益。
- [simvec-upsert-wal-batch-regression.md](simvec-upsert-wal-batch-regression.md)：`upsert_wal/batch_100` 单元素比 `single` 慢 **~9.8×**，批量反而更慢。
- [simvec-filter-selectivity-inversion.md](simvec-filter-selectivity-inversion.md)：`filter_selectivity` 1%/10%/50% 选择性比 100% 慢 **~11×**，低选择性路径异常慢。
- [simvec-hnsw-alloc-churn.md](simvec-hnsw-alloc-churn.md)：HNSW 构建 10k 点分配 ~168MB；`workers_4` 比顺序构建快 3.0×——并行构建收益明显，但热路径分配抖动大。

### 信息 / 基线（健康）
- [baseline-benches-core-transaction.md](baseline-benches-core-transaction.md)：core serde 与 transaction 基线健康，给出可回归对比的基准值。
- [bench-coverage-and-infra-gaps.md](bench-coverage-and-infra-gaps.md)：覆盖健康度与 bench 基础设施缺口汇总（含 criterion 无 quick 模式）。

## 结论摘要

1. **最严重的是数据面挂死（P0）**：`linkrs-storage` 整包在 `cargo bench` 下超时挂死（EXIT=124），与更早一轮 1M WAL 挂死一致。优先在开发板用 `RUST_BACKTRACE=1` + debug 构建定位 WAL 段边界轮转 / 组提交协调。
2. **写扩展性崩溃（P2）是最大性能改进抓手**：commit gate 在 16 线程时吞吐跌 65×、gate 等待占 57%，且与 bench 自带 "review sharding" 文案自相矛盾——写路径的集中式 gate 是瓶颈，应评估细粒度分片锁或批提交。
3. **查询并行不仅没加速反而更慢**：并行效率在 8 worker 时跌到 0.04–0.12，且并行扩展 bench 直接 panic，说明并行执行框架（collector/writer 生命周期、分区 fallback）有正确性+扩展性双重缺陷。
4. **向量算子是"看起来 SIMD 实则没吃满"**：avx512 路径基本无收益甚至回退，批量 WAL upsert 与低选择性 filter 存在反常回归，是 simvec 热路径最该修的两处。
5. **复现性隐患**：tantivy 子模块未 pin commit，导致 fulltext 编译随上游漂移——必须用 `[patch]`/rev 固定子模块，否则 bench 不可复现。
