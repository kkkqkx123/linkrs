# P0 [storage] ingest 路径 WAL 持久化挂死（整包 EXIT=124）

## 现象（实测）

- `linkrs-storage` 整包 `cargo bench` 退出码 **EXIT=124（超时）**。`bench_results/linkrs-storage.exit` = `EXIT=124`。
- 本次 `linkrs-storage.log` 中 `commit_gate_bench` / `crash_recovery_bench` / `csr_perf_bench` / `edge_property_tables_bench` / `edge_read_alloc_bench` / `edge_scan_speedup_bench` 均**正常产出**，但 **ingest 类 bench 未出现在日志中**——进程在 ingest 阶段挂死，被外层超时 kill，故未捕获任何 ingest 指标。
- 日志末尾止于 `edge_scan_speedup_bench`（`:1552-1564`），之后无输出。

## 与既有分析的关联

更早一轮 bench（2026-10-09，见既有 `docs/issue/ingest-1m-wal-hang.md`）记录：单批 1M 顶点 + 持久化存储时 **WAL 文件停止增长、全部线程 futex 等待、零 CPU ≥35 分钟**。`linkrs-storage` 本次整包超时挂死，与该现象**一致并复现**。

> 注意：本分析不重复既有文档内容，仅确认该 P0 在 2026-10-11 运行中再次触发（以 EXIT=124 形式），且本次未深入定位（进程被超时杀死，无栈）。

## 影响

**P0 —— 数据面挂死**：大数据批写入在 WAL 持久化路径可挂死，是生产不可用风险，严重度最高。

## 疑似根因（沿用既有分析，本次未深入）

WAL 段边界轮转 / 组提交协调在单批 1M 顶点时死锁或活锁，所有写入线程在 futex 上等待、零进度。

## 建议

1. **最高优先级**：在开发板用 `RUST_BACKTRACE=1` + debug 构建复跑 `cargo bench -p linkrs-storage --bench <ingest bench>`，并在挂死时抓 `gdb` 栈，定位 WAL 持久化路径的等待点（段边界轮转 / 组提交协调）。
2. 加超时保护 + 结构化错误，避免 ingest 无限挂死（至少应 fail-fast 而非死等）。
3. 修复后回归，确认 EXIT=124 不再出现，再处理 P2 性能项。

## 严重级
**P0（数据面挂死）**

## 数据来源
- `bench_results/linkrs-storage.exit` = `EXIT=124`
- `bench_results/linkrs-storage.log`（末尾止于 `:1564`，ingest 未产出）
- 交叉参考既有 `docs/issue/ingest-1m-wal-hang.md`（2026-10-09 记录的 1M WAL 挂死栈证据）
