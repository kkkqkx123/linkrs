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
| linkrs-storage | 13 | **EXIT=124 超时（bench 超时，非数据面挂死，见下）** | commit gate 1→16 线程吞吐跌 65×；边扫描并行加速比≈1.0 |
| linkrs-transaction | 2 | OK | begin_read 2.12µs，write_set/build 线性偏超线性 |
| linkrs-query | 6 | **EXIT=101 panic** | parallel_scale 在 `collector.rs:458` panic；并行效率崩溃 |

`*` linkrs-fulltext 因上游 `tantivy` 子模块未 pin commit、fork 默认分支移除了 `Bm25Params`/`IndexSettings.bm25_params`，
沙箱做了 import 替换+去字段的本地临时补丁才编译通过（补丁未纳入任何 patch，根因修复应为 pin 子模块 commit，见 `fulltext-numbers-and-infra.md`）。

## 发现索引（按严重级）

> 注（2026-10-11 复核）：原 P0 `storage-ingest-wal-hang.md` 已删除，见下文结论摘要第 1 条。
> `linkrs-storage` 的 EXIT=124 是 bench 总耗时超过外层超时（edge_scan 未跑完即被 kill，
> ingest 根本未启动），且 `ingest_bench` 全量（10k/100k/1M）直跑可完成（1M 持久批约 53s），
> 离线批路径不经过组提交等待，不存在 WAL 挂死证据。P0 级别空缺。

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

1. **P0 空缺：EXIT=124 是 bench 超时而非数据面挂死**：`linkrs-storage.log` 止于 `edge_scan_speedup_bench` 的 N=1/3 分区（3 分区只跑完 1 个），
   按 cargo bench 字母序 ingest 在 edge_scan 之后，ingest 缺席只能证明 kill 发生在 edge_scan 内部，不能作为 ingest 挂死的证据；
   直接重跑 `ingest_bench` 全量 10k/100k/1M 全部完成（本机 1M 持久批顶点 leg 约 53s，CPU/WAL 约 54s/53s），无挂死；
   且离线批路径（`new_with_path`，无事务上下文）走 `append_entry_buffered`（纯内存缓冲 + 原子 LSN，无 fsync/轮转/condvar 等待）
   与 `VersionManager::commit_ordered`（非阻塞 reserve+publish），疑似的“段边界轮转 / 组提交死锁”在此路径上根本不可达；
   引用的 `bench_results/*.exit` 与 `docs/issue/ingest-1m-wal-hang.md` 在本仓库均不存在，本次亦无栈（原文自述“未深入定位，无栈”），故 P0 指控无负载证据，予以删除。
   真因是 bench 工作量超出外层 25 分钟超时（edge_scan 3 分区 × 建 1–2M 顶点 + 1M 边 + 52 次全表扫描 + ingest 1M × 3 轮），后续应拆分慢 bench / 缩小默认规模 / 延长超时，而非修 WAL。
2. **写扩展性崩溃（P2）是最大性能改进抓手**：commit gate 在 16 线程时吞吐跌 65×、gate 等待占 57%，且与 bench 自带 "review sharding" 文案自相矛盾——写路径的集中式 gate 是瓶颈，应评估细粒度分片锁或批提交。
3. **查询并行不仅没加速反而更慢**：并行效率在 8 worker 时跌到 0.04–0.12，且并行扩展 bench 直接 panic，说明并行执行框架（collector/writer 生命周期、分区 fallback）有正确性+扩展性双重缺陷。
4. **向量算子是"看起来 SIMD 实则没吃满"**：avx512 路径基本无收益甚至回退，批量 WAL upsert 与低选择性 filter 存在反常回归，是 simvec 热路径最该修的两处。
5. **复现性隐患**：tantivy 子模块未 pin commit，导致 fulltext 编译随上游漂移——必须用 `[patch]`/rev 固定子模块，否则 bench 不可复现。
