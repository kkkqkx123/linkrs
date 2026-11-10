# 存储层基准数据（2c9b92f8，编译服务器 node1）

## storage_bench（部分失败，bulk 数据有效）

indexed_bulk_load/5000 中位数 376ms（13.3 Kelem/s），/10000 中位数 1.17s（8.6 Kelem/s）。随后 `storage_edge_insert/single` 报 CapacityExceeded 中断，属容量配置问题（bulk 阶段已把预置空间占满），bulk 段数据有效，单边插入段需修容量配置后重跑。

## storage_read_baseline

窄表 1 万行：全扫描 8.7ms（115 万行/s），投影扫描 1.69x；随机点查约 1.6µs/op。投影在窄表上提升明确。

## storage_workflow_bench

混合读写流：点查 1.5~3.1µs，批量段 166ms~1.24s（规模相关），全绿。

## ingest_bench（写入归因，决策明确）

- batch_insert_vertices：50k 行 CPU 249ms vs WAL 272ms，CPU 占比 91.6%（大批量下 CPU 主导）。
- batch_insert_edges：50k 行 CPU 208ms vs WAL 219ms，CPU 占比 94.9%。
- 小批量（2000 行）顶点写入 CPU 占比超 100%（内存路径反而慢于持久路径，属固定开销主导）。
- 单提交持久化每边约 40µs，内存单提交每边约 46µs。

结论（与 bench 内置 decision 一致）：点写延迟是 durability-bound，应让调用方批量而非加顶点级锁或分片并行。

## commit_gate_bench

1/2/4 线程吞吐 39908/20543/10784 stmts/s，门等待占比 0.3%/30%/50%。指导：4 线程下门等待超 20% 即应走 batch_insert_edges 或 auto-commit 组，门分片只在真 8 核以上数据出来后再议。

## csr_perf_bench

插入：uniform d=16 约 3.0M edges/s，power-law 约 6.2M，batch put 约 15.9M（单条约 4.4M）；扫描 3~12ns/edge（caller-buffer 路径最快 2.9ns）；路由：宽索引 49.5ns/lookup，窄扫描 24.6ns/lookup。

## wal_bench

batch_512 写入约 7.9ms；5000 条目顺序重放约 50.6µs。

## crash_recovery_bench（确定性失败）

`clean_checkpoint` 场景 reopen 时 checkpoint rename 失败（`groups_manifest.tmp` 缺失），QUICK 与 FULL 均复现，与环境无关，属源码级问题，需修后重跑。

## rollback_bench（部分数据 + 被干扰中断）

B4（5 万顶点）：5000 边行写入 219.8s（约 44ms/edge，持久单边写）、commit 37.9ms、abort 0.5ms。10000 边行进行中因同机 buildroot 争抢（/tmp 所在根分区 util 95%）过慢，31 分钟后主动终止。结论：持久单边写成本极高（与 ingest 单提交 40µs/edge 同量级印证），需在安静窗口重跑完整 B4。

## neighbor_batch_bench（目标达成）

neighbor 批量 1088ns/vertex（3.6x，加速目标 ≥3x 达成），out-degree 批量 341ns（11.5x）。

## edge_read_alloc_bench

单边读取约 2.0µs（含 25% 离群，多为高值）。

## edge_scan_speedup_bench

10 万边：1/2/4/8 worker 加速比 1.00/1.17/1.35/1.50，效率一路掉到 0.19，内存带宽主导，符合预期。

## edge_property_tables_bench

每边字节拆分：flat 388.6B，ranked 404.7B，行占比 13.7%，未达到窄布局 15% 节省门（且要求零扫描回归），维持现状。
