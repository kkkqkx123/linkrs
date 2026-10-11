# P2 [storage] 写路径 commit gate 扩展性崩溃

## 现象（实测）

`commit_gate_bench` 在 `statements per thread = 4000, threads = [1, 4, 8, 16]` 下：

| threads | wall ms | stmts/sec | gate wait ms | acq | gate share |
|---:|---:|---:|---:|---:|---:|
| 1 | 55.76 | 71738 | 0.06 | 4000 | 0.10% |
| 4 | 1138.27 | 14056 | 1331.08 | 16000 | 29.23% |
| 8 | 6810.17 | 4699 | 31574.02 | 32000 | 57.95% |
| 16 | 58106.70 | 1101 | 533895.31 | 64000 | 57.43% |

数据来源：`bench_results/linkrs-storage.log:1397-1403`

- 1→16 线程吞吐 **71738 → 1101 stmts/s（≈65× 下跌）**；墙时 55.76ms → 58106.70ms（≈1042× 恶化）。
- gate 等待占比从 0.10% 单调升到 **57.4%**，且 `acq`（gate 获取次数）= `threads × 4000` 随线程数线性累积，gate wait ms 从 0.06 → 533895（≈9×10⁶×）。
- bench 自带 verdict：`gate wait share 57.43% at N=16 (>= 5%) -> review sharding`（`linkrs-storage.log:1403`）。
- bench 自带 guidance：`gate-wait share above 20%: prefer batch_insert_edges or begin_auto_commit_group over per-statement auto-commit`（`:1410` 附近）。

## 自相矛盾（bench 文案缺陷）

verdict 指向 "review sharding"（在 gate 之上分片），guidance 却指向 "用 batch_insert_edges / begin_auto_commit_group 绕开逐语句提交"（在 gate 之下）。两者是**不同层级**的修复方向，bench 输出把它们并列，易误导读者。单线程 gate wait 仅 0.10%，说明 gate 单笔逻辑开销极小，瓶颈是**并发竞争**而非单笔成本。

## 影响

高并发写吞吐不增反崩：16 线程比 1 线程慢约 50×（墙时）。这对写入密集场景是直接的生产风险。

## 疑似根因

写路径存在**集中式 commit gate**（单一 acquire 点）。所有线程在 gate 上串行排队，等待时间随线程数近线性累积，gate 成为全局串行瓶颈。

## 建议

1. 优先落实 bench guidance：写路径默认改走 `batch_insert_edges` / `begin_auto_commit_group`，避免逐语句 auto-commit（单线程本就 0.10% gate wait，批提交可大幅降低 gate 获取频率）。
2. 若业务必须逐语句提交，评估 gate 之下的**细粒度分片锁 / sharded commit queue**，把全局 gate 拆成按分区/分片的局部 gate。
3. 修正 `commit_gate_bench` 自带 verdict/guidance 自相矛盾，给出单一明确结论，避免误导后续优化方向。

## 严重级
**P2（性能）** —— 最大性能改进抓手，但属可规避（批提交路径）的瓶颈，非数据面故障。

## 数据来源
`bench_results/linkrs-storage.log:1384-1410`
