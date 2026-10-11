# P1 [工程] bench 覆盖健康度与基础设施缺口

## 本次覆盖健康度（2026-10-11）

| 包 | 结果 | 说明 |
|---|---|---|
| simvec | 8 bench OK | 部分大规模单项用 `-- --sample-size 10` 降采样 |
| linkrs-core | 1 bench OK | serde 基线 |
| linkrs-fulltext | 1 bench OK* | 需本地补丁（见 `fulltext-numbers-and-infra.md`） |
| linkrs-storage | **EXIT=124 超时（bench 超时，非挂死）** | 超时 kill 发生在 `edge_scan_speedup_bench` 内部（N=1/3 未跑完，ingest 未启动）；`ingest_bench` 全量直跑可完成，原 P0 指控已删除 |
| linkrs-transaction | 2 bench OK | 基线健康 |
| linkrs-query | **EXIT=101 panic** | collector.rs:458 panic（P1，见 `query-parallel-collector-panic.md`） |

6 包中 **2 包整包失败**（storage 超时、query panic），并行扩展类覆盖存在缺口。

## 基础设施缺口

### 1. 子模块未 pin commit（复现性）
`tantivy` 子模块未固定，导致 fulltext 编译随上游漂移。所有子模块（tantivy / simvec / llm-suite）应固定 `rev` / commit。

### 2. criterion 无 quick 模式（工程效率）
- `ivf_build_time/build/100000` 默认 100 采样需 **955–998 s（~16 min）**（`bench_results/linkrs-simvec.log:283-296`）；
- `hnsw_build/sequential/10000` 需 ~1252 s。
- 建议在各包 `benches/bench_group.rs` 统一入口支持 `LINKRS_BENCH_QUICK=1`（降 `sample_size` + 缩 `measurement_time`）：CI / 本地迭代用 quick 档，发版基线用全量档。

### 3. bench 契约脱节（更早一轮记录，仍待修）
既有 `docs/issue/` 记录 5 个 bench 因"主键必须镜像顶点 id"校验 panic（storage/query 多 bench），spill 目录生命周期交错互删（parallel_scale warmup ENOENT），rollback 约 524 次迭代后 `InvalidTimestamp`。这些是"数据生成 vs 存储校验"的系统性脱节，本次 query 的 collector panic 是同类覆盖缺口的复现。（原 storage ingest 挂死指控经复核不成立，已删除，见 `README.md` 结论摘要第 1 条。）
- 建议引入统一 bench 数据生成 helper（PK 列镜像 vid），让 `cargo bench --workspace` 全绿成为 **CI 门禁**。

### 4. 代理依赖
ghproxy（仓库/子模块）+ rsproxy（crates.io）是离线/受限网络环境跑通的前提，脚本见工作区 `bench_scripts/`；CI 需保留代理或预置镜像。

### 5. 默认规模与公开数据集基线
合成 micro-bench 的绝对数值在不同机器（核数、cgroup 配额、磁盘）之间难以对比，
本轮已把各包默认规模降至 smoke 档（storage ingest 最大 50k、edge_scan 100k 边、
rollback 最大 25k 边、commit_gate 最大 4 线程 ×500 语句；query parallel/traversal
20k 顶点；simvec ivf 10k 点、hnsw 构建最大 2k 点、scan 最大 100k 点），目标是
`cargo bench` 在受限机器上能正常跑完、只做回归守门。正式的大规模基准测试不再
用合成数据加码：待基础设施就绪后，直接从权威的公开数据集照搬基线（图侧如
LDBC  Datagen/SNB 数据集与查询集，向量侧如 ANN-Benchmarks 的公开 corpus 与
recall 评估），以可复现的第三方数据为准。

## 建议（汇总）

1. `cargo bench --workspace` 全绿 → CI 门禁；
2. 统一 bench 数据生成 helper，消除 PK 镜像等契约脱节；
3. 引入 `LINKRS_BENCH_QUICK=1` quick 档；
4. **pin 所有子模块 commit**（tantivy 优先级最高）。
5. 默认规模保持 smoke 档（回归守门用），大规模基线待正式基准时从权威公开数据集照搬，不再扩大合成数据规模。

## 严重级
**P1（工程效率 / 复现性）**

## 数据来源
各 `bench_results/*.log`；`bench_results/linkrs-simvec.log:283-296`；既有 `docs/issue/*`（2026-10-09）
