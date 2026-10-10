# 问题：写路径 commit gate 多线程扩展性坍塌（且 bench 自带结论与数据矛盾）

- 状态：新建（待优化）
- 类型：性能缺陷（写扩展性）+ bench 输出自洽性
- 复现：`cargo bench -p linkrs-storage --bench commit_gate_bench`（2026-10-09）

## 实测数据（statements per thread = 4000）

| threads | wall ms | stmts/sec | gate wait ms | gate share |
|--------:|--------:|----------:|-------------:|-----------:|
| 1 | 53.55 | 74,703 | 0.06 | 0.12% |
| 4 | 1,200.48 | 13,328 | 1,766.22 | 36.78% |
| 8 | 7,002.66 | 4,570 | 29,931.88 | 53.43% |
| 16 | 64,322.45 | 995 | 748,225.38 | 72.70% |

- 单线程 74.7k stmts/s，16 线程反而 **995 stmts/s（总吞吐跌 75 倍）**，
  gate 等待占总时长 72.7%——瓶颈明确在 commit gate 串行化本身。
- **bench 输出自相矛盾**：数据下方打印
  `result: gate wait share < 5% at N=16 -> sharding not justified`，
  与上一行 72.70% 直接冲突。verdict 判定逻辑（阈值比较对象算错）需一并修复，
  否则会误导容量决策。

注意：本环境 cgroup 配额 4 核，16 线程本身超订阅；但 gate wait share 随线程数单调增长
（0.12% → 72.7%）表明串行化是主因，非仅 CPU 不足（4 线程时吞吐已跌至 1 线程的 18%）。
建议开发板真机复核绝对数值。

## 对比参照

同一日志中 staging-commit 基准（uniform/20000 边、49 组）commit 仅 21.48ms，
说明批提交路径本身不慢；问题集中在高频小语句竞争 gate 的场景。

## 修复方向

- 定位 gate 实现的临界区（staging.rs 的 capacity contract 注释所指对象），
  评估按 key/hash 分片 gate 或改用无锁 staging 提交；
- 修复 commit_gate_bench 的 verdict 计算与文案；
- 在真机重测后更新容量契约文档。
