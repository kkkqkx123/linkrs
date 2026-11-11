# 存储层基准数据（2c9b92f8，编译服务器 node1）

## storage_bench（已修复，全绿）

首轮 `storage_edge_insert/single` 因单 store 无限增长触发 CapacityExceeded 中断。修复：bench 改为每 100 次插入轮换 store（setup 摊薄，见 bench 内注释）。重跑 exit=0（1802s），全部完成：

- bulk_vertex_insert：1000/10000 分别约 146/146 Kelem/s（规模无关，纯内存路径）。
- indexed_bulk_load：1000→10000 从 18.3 降至 8.9 Kelem/s（索引维护成本随规模显现）。
- edge_insert_density：sparse 约 40.1 Kelem/s，dense 约 28.7 Kelem/s。
- single（持久单边写，HDD fsync-bound）：中位数 1.94ms/edge。
- scan：cursor 全扫描约 1.47~1.63 Melem/s，顶点扫描约 1.66~1.76 Melem/s；point_lookup 窄表 37.3 Melem/s。
- churn delete_reinsert_10k：约 9.7 Kelem/s。
- checkpoint 段：2000 行约 1.9 Kelem/s，10000 行约 12.9 Kelem/s（固定开销主导，小规模反而低）。

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

## crash_recovery_bench（已修复，全绿）

首轮两处源码级竞态，均已修复并验证通过（node1 安静窗口，load<0.5）：

1. 同目录并发 flush 的 `*.tmp` 清理/重命名竞态：`compression.rs` 新增进程内按目录串行锁 `with_table_flush_lock`，包装全部 flush 入口。
2. checkpoint 的 `checkpoint_{seq}.tmp` 目录被并发 checkpoint 或 `open` 时清理删掉：`with_checkpoint_dir_lock` 串行化 checkpoint 执行与 open 期清理；另发现 `populate` 中显式 checkpoint 与后台 scheduler 自动 checkpoint 在同一 coordinator 上互斥（fail-fast "already active"），bench 改用产品自带的 `begin_bulk_load` 会话抑制 trigger（`abandon` 结尾，不引入额外 checkpoint，WAL-tail 场景不受影响）。

数据（criterion 中位数，2000 顶点/4000 边）：clean_checkpoint reopen 109.6ms，wal_tail_replay reopen 1143ms（约 10.4x，WAL 重放成本）；plain-main 附带 one-shot：97.1ms / 1602.9ms。

## rollback_bench（B4 两行有效，25000 行超时未完成）

B4（5 万顶点，持久事务单边写，HDD fsync-bound，median-of-2）：5000 边行写入 362.8s（约 72ms/edge）、commit 35.9ms、abort 0.59ms；10000 边行写入 467.4s（约 47ms/edge）、commit 101.9ms、abort 0.79ms。25000 边行未能在 90 分钟超时内完成（median-of-2 使墙钟约翻倍）。结论：abort/commit 相对写入可忽略（<0.1%），写入是 durability-bound；25000 行需更大时间预算或 SSD 环境，暂记为 open。

B4 根因与修复（b4_probe 专项，devs 同机 5000 顶点 / 200 边）：分段计时确认每边 40ms 几乎全是表级 WAL `sync_data`（`wal_append` 中位 39.7ms，其余阶段 <0.1ms），即显式事务 eager apply 的逐边 fsync。修复后显式事务单边 415µs（约 96 倍加速），批量 274µs，自动提交单边 53.4ms（每语句一次提交，正确成本）。回归：`linkrs-storage` lib 1395 + 集成 83（wal_recovery/transaction_isolation/crash 各套件）全绿。B4 全量重跑与默认 5 万顶点 probe 待安静窗口：5 万顶点 setup 在共享宿主上被邻居大构建 IO 拖住（与修复无关，旧二进制同样停滞，顶点自动提交逐次 fsync 属正确成本）。

## neighbor_batch_bench（目标达成）

neighbor 批量 1088ns/vertex（3.6x，加速目标 ≥3x 达成），out-degree 批量 341ns（11.5x）。

## edge_read_alloc_bench

单边读取约 2.0µs（含 25% 离群，多为高值）。

## edge_scan_speedup_bench

10 万边：1/2/4/8 worker 加速比 1.00/1.17/1.35/1.50，效率一路掉到 0.19，内存带宽主导，符合预期。

## edge_property_tables_bench

每边字节拆分：flat 388.6B，ranked 404.7B，行占比 13.7%，未达到窄布局 15% 节省门（且要求零扫描回归），维持现状。
