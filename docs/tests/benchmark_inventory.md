# 性能基准测试清单

## 1. 总览

一手基准约 30 个，分散在 6 个包内，全部 `harness = false`，统一经 `cargo bench -p <包> --bench <名>` 运行。两种形态：Criterion 形态（统计 harness，预热 1s、测量 10s）与 plain-main 形态（手写 main 取中位数）。共享 `bench_group.rs`（QUICK 冒烟开关）与 `results_report.rs`（落盘 `benches/results/<名>/`）。

全部基准当前为冒烟量级（万级顶点/行），定位是防回归而非权威吞吐。回归门：`olap_e2e_bench` 的 Q1~Q5、`traversal_perf_bench` 两项阈值（锚定 1 跳、非锚定 2 跳），源码侧由 `threshold_registry/bench_ref.rs` 绑定。

server / API 层（HTTP、gRPC、并发）暂无基准，列为后续阶段，不在本次执行范围。

## 2. 清单

### linkrs-query（7 个）

| 基准 | 形态 | 测量内容 |
|---|---|---|
| query_bench | Criterion | 查询主基准：数据访问、路径遍历、聚合、大顶点扫描、计数、边密度、混合读写 B1~B7 |
| query_stage_bench | Criterion | 阶段拆分：parse / bind / plan+optimize / execute 各阶段耗时 |
| traversal_perf_bench | plain-main | 遍历路径性能，含两项回归门 |
| parallel_scale_bench | plain-main | 1/2/4/8 worker 并行加速比与效率，20k 顶点 |
| olap_e2e_bench | Criterion | 合成无标度图上的端到端 OLAP，Q1~Q5 为回归门 |
| executor_bench | Criterion | 执行器微基准：表达式求值、累积路径、SIMD 对比 |
| columnar_probe_bench | Criterion | 列式探针门：种子计数、扁平投影 |

### linkrs-storage（13 个）

| 基准 | 形态 | 测量内容 |
|---|---|---|
| storage_bench | Criterion | 存储读写主基准 |
| storage_read_baseline | plain-main | 宽表全扫描 vs 投影扫描基线 |
| storage_workflow_bench | Criterion | 存储工作流 |
| ingest_bench | plain-main | 写入归因：内存侧 vs WAL 侧，批量 vs 单提交 |
| commit_gate_bench | plain-main | 提交门争用：staging hold 时间与门等待占比 |
| csr_perf_bench | plain-main | CSR 插入吞吐、扫描带宽、内存 |
| wal_bench | Criterion | WAL 独立基准：fsync 延迟基线与持久化成本 |
| crash_recovery_bench | Criterion | 崩溃恢复/重启延迟 |
| rollback_bench | plain-main | 大事务回滚成本 |
| neighbor_batch_bench | plain-main | 批量邻居读访问器 |
| edge_read_alloc_bench | Criterion | 边属性读路径分配计数 |
| edge_scan_speedup_bench | plain-main | 边表扫描并行加速比 |
| edge_property_tables_bench | plain-main | 边属性表决策套件 |

### linkrs-transaction（2 个）、core（1 个）、fulltext（1 个）

| 基准 | 形态 | 测量内容 |
|---|---|---|
| transaction_bench | Criterion | 事务操作/MVCC/认证 |
| txn_conflict_bench | Criterion | 多客户端写写冲突 |
| serde_bench | Criterion | 核心类型序列化 |
| fulltext_bench | Criterion | 全文索引（需 `--features fulltext`） |

### simvec（9 个，向量距离与索引）

Criterion：distance、hnsw_build、hnsw_ingest、ivf、vector_scan、concurrent_search、persist_crc；plain-main：alloc_stats、concurrent_write。

## 3. 运行方式

冒烟验证用 `QUICK=1`（秒级）；正式数据用全量（分钟级/套件）。按包执行，包内套件串行，避免相互干扰。编译服务器上绑定 NUMA node1、内存同节点，node0 留给系统，见 `remote_bench_server_design.md`。编译环境与同步规范见 `compile_server_build_env.md`。

## 4. 数据归档约定

原始输出与 `results/*.txt` 回传后，按功能落盘到 `docs/archive/perf/`，一功能一文件或一目录：query、storage、transaction、vector（simvec）、fulltext、core-serde。文件名含日期与提交短哈希，跨版本对比只允许同核集、同内存节点、同构建方式。
