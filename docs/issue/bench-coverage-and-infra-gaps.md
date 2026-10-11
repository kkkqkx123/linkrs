# P1 [工程] bench 覆盖健康度与基础设施缺口

## 本次覆盖健康度（2026-10-11）

| 包 | 结果 | 说明 |
|---|---|---|
| simvec | 8 bench OK | 部分大规模单项用 `-- --sample-size 10` 降采样 |
| linkrs-core | 1 bench OK | serde 基线 |
| linkrs-fulltext | 1 bench OK* | 需本地补丁（见 `fulltext-numbers-and-infra.md`） |
| linkrs-storage | **EXIT=124 挂死** | ingest WAL 挂死（P0，见 `storage-ingest-wal-hang.md`） |
| linkrs-transaction | 2 bench OK | 基线健康 |
| linkrs-query | **EXIT=101 panic** | collector.rs:458 panic（P1，见 `query-parallel-collector-panic.md`） |

6 包中 **2 包整包失败**（storage 挂死、query panic），并行扩展类覆盖存在缺口。

## 基础设施缺口

### 1. 子模块未 pin commit（复现性）
`tantivy` 子模块未固定，导致 fulltext 编译随上游漂移。所有子模块（tantivy / simvec / llm-suite）应固定 `rev` / commit。

### 2. criterion 无 quick 模式（工程效率）
- `ivf_build_time/build/100000` 默认 100 采样需 **955–998 s（~16 min）**（`bench_results/linkrs-simvec.log:283-296`）；
- `hnsw_build/sequential/10000` 需 ~1252 s。
- 建议在各包 `benches/bench_group.rs` 统一入口支持 `LINKRS_BENCH_QUICK=1`（降 `sample_size` + 缩 `measurement_time`）：CI / 本地迭代用 quick 档，发版基线用全量档。

### 3. bench 契约脱节（更早一轮记录，仍待修）
既有 `docs/issue/` 记录 5 个 bench 因"主键必须镜像顶点 id"校验 panic（storage/query 多 bench），spill 目录生命周期交错互删（parallel_scale warmup ENOENT），rollback 约 524 次迭代后 `InvalidTimestamp`。这些是"数据生成 vs 存储校验"的系统性脱节，本次 query 的 collector panic 与 storage 的 ingest 挂死是同类覆盖缺口的复现。
- 建议引入统一 bench 数据生成 helper（PK 列镜像 vid），让 `cargo bench --workspace` 全绿成为 **CI 门禁**。

### 4. 代理依赖
ghproxy（仓库/子模块）+ rsproxy（crates.io）是离线/受限网络环境跑通的前提，脚本见工作区 `bench_scripts/`；CI 需保留代理或预置镜像。

## 建议（汇总）

1. `cargo bench --workspace` 全绿 → CI 门禁；
2. 统一 bench 数据生成 helper，消除 PK 镜像等契约脱节；
3. 引入 `LINKRS_BENCH_QUICK=1` quick 档；
4. **pin 所有子模块 commit**（tantivy 优先级最高）。

## 严重级
**P1（工程效率 / 复现性）**

## 数据来源
各 `bench_results/*.exit`；`bench_results/linkrs-simvec.log:283-296`；既有 `docs/issue/*`（2026-10-09）
