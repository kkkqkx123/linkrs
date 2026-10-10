# 问题：单批 1M 顶点 + 持久化存储挂死（ingest_bench 无法完成）

- 状态：新建（待修复）
- 类型：数据面挂死（P0）
- 复现：`cargo bench -p linkrs-storage --bench ingest_bench`（2026-10-09，bench profile）
- 关联现象：ingest_bench 10k/100k 批正常输出，1M 批挂死

## 问题描述

`ingest_bench` 对 1M 顶点执行单次 `batch_insert_vertices`（持久化存储，`GraphStorage::new_with_path`）
后进程挂死：

- WAL 文件停在 **70,090,953 字节**（约 66.8 MiB，接近常见 64 MiB 段边界量级），
  观察 ≥35 分钟无任何增长（19:46 → 20:14）。
- 全部 6 个线程停在 `futex_wait` / `nanosleep`，CPU 时间冻结（gdb attach 抓栈确认：
  rayon worker 全部 idle 于 `rayon_core::sleep::Sleep::sleep`，其中一个 rayon job 内在
  `std::thread::sleep` 轮询——典型逻辑等待，非 fsync I/O 阻塞，线程态为 S 而非 D）。

## 交叉证据

- `rollback_bench` 对同样 1M 顶点但**分块 100k/批**（rollback_bench.rs:59 `chunks(100_000)`）
  装载成功（约 3 分钟完成 setup），说明问题与“单批行数过大”强相关。
- `ingest_bench` 10k → T_cpu 22.98ms / T_wal 29.82ms；100k → 280.82ms / 324.28ms，
  规模外推 1M 应为秒级，实际无限挂起。

## 疑点（待开发板验证）

1. WAL 段轮转（segment rotation）在 64MiB 边界等待某个由批插入持有的锁/条件变量。
2. WAL 组提交（group commit）协调线程与批插入主线程的 backpressure 死锁
   （gdb 观察到 rayon job 内 sleep 轮询 + 其余线程 futex）。
3. 批插入路径与 checkpoint（`/tmp/.tmp*/wal/checkpoint.meta` 存在）互相等待。

## 影响

- 大批量初始装载（百万级单批）在生产路径上会永久挂起。
- ingest_bench 自身无法产出 1M 档基线，写入吞吐上限未知。

## 修复方向

- 用 debug 构建 + `RUST_BACKTRACE=1` 复跑 1M 单批，抓取挂死点栈；
  重点排查 WAL 段轮转与组提交协调（`crates/linkrs-storage` WAL 模块）。
- 为 ingest_bench 增加 watchdog（如 120s 无进展即 dump 全线程栈）便于 CI 捕获。
- 临时规避：调用方按 ≤100k/批分块（rollback_bench 的做法）。
