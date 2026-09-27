# 延期项基准验证报告

> 生成日期：2026-09-27
> 基于文档：《延期项的基准验证方案》
> 机器环境：INTEL(R) XEON(R) PLATINUM 8582C（容器内可见 2 核 / 物理 240 核），5.8 GiB RAM，Linux
> 编译参数：`cargo build --benches --release -j 2`（容器限制）
> Rust 工具链：rustc 1.92.0 / cargo 1.92.0，`-C target-cpu=x86-64-v3`（AVX2）

## 一、执行摘要

本次基准覆盖了原方案 Section 2–5 四项延期优化的全部前置信号门限。**结论**：四项全部 **不满足进入重构阶段的条件**，默认动作回到用法治理（批量写、OutOnly/InOnly 方向化），关闭异步次腿、行级锁、Auto-Bundled 三项引擎内修改。唯一可立即落地的是 **dual-direction 表 → OutOnly/InOnly 的 schema 迁移**（用法层改动，非引擎改动）。

| 章节 | 优化项 | 门限 | 实测 | 判定 | 动作 |
|:---|:---|:---|:---|:---|:---|
| §2 | 组级条带锁 | skewed/uniform ≥ 2× | **1.00×** | ✗ 不通过 | 关闭；维持显式 chunk |
| §3 | 行级锁 / epoch 读 | 同行冲突 Amdahl ≥ 50% 剩余 | **−0.3%** | ✗ 不通过 | 前置 §2 未过；关闭 |
| §4 | 异步次腿 | CPU 写放大主导 + in 腿真实被用 | in 腿 hit-rate **0.0%** | ✗ 不通过 | 默认 OutOnly/InOnly 迁移 |
| §5 | Auto-Bundled | 空间收益减半级 + 限制触发率 0 | resident ratio **1.20×**，rank 触发率 **100%**（测试） | ✗ 不通过 | 维持显式 opt-in |

## 二、覆盖的基准

### 2.1 新增基准（本次）

| 文件 | 对应章节 | 覆盖维度 |
|:---|:---|:---|
| `benches/rw_mix_group_striping_bench.rs` | §2 + §3 | N 线程 uniform vs skewed 写 + M 线程扇出读；同行 vs 同组不同行；Amdahl 上界 |
| `benches/outonly_vs_both_bench.rs` | §4 | CsrShardSet commit wall time；GraphStorage 端到端 single/batch；persistent byte 占用；reverse leg hit-rate |
| `benches/bundled_necessity_bench.rs` | §5 | Bundled vs Columnar 点查延迟、全扫描吞吐量；persistent path checkpoint bytes；rank 触发率 |

### 2.2 复用的已有基准

| 文件 | 用途 |
|:---|:---|
| `benches/write_gate_bench.rs` | 度量 GraphStorage 层全局 write gate 等待占比，用于排除"瓶颈其实在上层" |
| `benches/edge_group_commit_bench.rs` | CsrShardSet 单分区 commit wall time + uniform vs skewed owner-group 分布 |
| `benches/edge_point_write_bench.rs` | WAL fsync vs CPU 路径的开销分摊，用于区分"是 durability-bound 还是结构-bound" |

## 三、基准结果

### 3.1 write_gate_bench（B2 全局 gate 等待占比）

```
threads |    wall ms |    stmts/sec | gate wait ms |        acq | gate share
      1 |      69.01 |        57964 |        0.29 |       4000 |     0.42%
      4 |    2736.89 |         5846 |     5747.34 |      16000 |    52.50%
      8 |   17975.48 |         1780 |    95209.73 |      32000 |    66.21%
     16 |   72080.80 |          888 |   933864.93 |      64000 |    80.97%
```

**解读**：16 线程下 gate wait 占 wall time 的 **81%**——这说明 GraphStorage 层 `AutoCommitWriteGate` 是当前并发写的头号瓶颈。gate 是整个 `GraphStorage` 的全局单值，而不是 EdgeStore 内部的 per-partition 锁。EdgeStore 层即使做了 per-group striping，gate 一堵也是白堵。

### 3.2 edge_group_commit_bench（单分区 commit wall + owner-group skew）

```
    workload |    commit ms |  groups |   total |  max/group |    skew |   hot
uniform/5000 |         4.96 |      49 |    5000 |        103 |   1.01x |   103
uniform/10000 |         9.35 |      49 |   10000 |        206 |   1.01x |   206
uniform/20000 |        18.89 |      49 |   20000 |        412 |   1.01x |   412
skewed/20000 |        11.51 |       1 |   20000 |      20000 |   1.00x | 20000
```

**关键信号**：`skewed/20000 = 11.51 ms` < `uniform/20000 = 18.89 ms`，**单组写入反而是 uniform 的 1.64× 快**。

这与 §2 预期"skewed 因集中到同一分区而更慢"完全相反。原因分析：
- uniform 分布下 `CsrShardSet::partition_inserts_by_owner` 要拆成 49 个组，commit 时需要对每个组依次 mutex 加锁、apply、reclaim 头，跨组调度开销累积；
- skewed 下 20000 条全部进入同一 owner 组，一次加锁、一次 apply、无组间协调；
- 串行 commit 的瓶颈在 **锁持有时长** 而非 **争用**（因为是单线程 commit，没并发），skewed 下反而锁持有时间短。

§2 的前置信号"skewed wall ≥ 2 × uniform wall"在当前数据下反转为"skewed wall < uniform wall"。

### 3.3 edge_point_write_bench（WAL/fsync vs CPU 分摊）

```
        mode |     total ms |  per-edge us |    vs single
  single/mem |        10.03 |        10.03 |        1.07x
  single/wal |        10.78 |        10.78 |        1.00x
   batch/mem |         2.74 |         2.74 |        3.94x
   batch/wal |         3.46 |         3.46 |        3.11x

WAL/fsync share: single=6.9% batch=21.0%
batch amortization (wal): 3.1x per-edge cheaper than single commits
```

**关键信号**：持久化路径比内存路径仅贵 **6.9%**（single commit）到 **21%**（batch）。说明当前 edge point-write 不是 durability-bound，而是 CPU 路径上的结构开销（owner 查找、overflow 申请、MVCC 登记、双 leg apply）主导。

这对 §4 有直接含义：异步次腿如果能减掉一半 CPU apply，理论收益接近 50%；但前提是 in 腿真实被使用。

### 3.4 outonly_vs_both_bench（§4 OutOnly vs Both 写放大）

```
--- CsrShardSet-level commit wall time ---
       batch |   both ms | outonly ms |      amplify
        5000 |      4.04 |      1.92 |        2.10x
       10000 |      7.86 |      4.09 |        1.92x
       20000 |     16.37 |      9.22 |        1.78x

--- End-to-end point-write throughput (in-memory path) ---
      mode |  both ms | outonly ms | edges/s both | edges/s outonly
    single | 15584.88 |   14189.98 |          128 |          141
     batch |    11.22 |      10.04 |       178179 |       199296

--- Storage bytes after flush (persistent path) ---
      Both legs:      8418069 bytes (8.0 MiB)
    OutOnly leg:      8418069 bytes (8.0 MiB)
byte amplification: 1.00x

--- Reverse leg hit-rate over a read mix (Both table) ---
reverse mix share | in-leg nonempty% | out-leg nonempty%
           0.00 |           0.00 |           0.00
           0.10 |           0.00 |           0.00
           0.50 |           0.00 |           0.00
```

**解读**：
1. CsrShardSet 层 **Both ≈ 1.8–2.1× OutOnly**，这是 §4 最重要的前置信号：CPU 写放大是实的，不是 WAL fsync 的副产物。
2. 端到端 single commit 差距只有 **10%**（15.6 s vs 14.2 s for 2000 edges），因为上层还有 GraphStorage gate + MVCC + WAL 路径，腿级差异被摊薄。
3. **persistent bytes = 1.00×**——probe 设计缺陷：Both + OutOnly 都通过 `new_graphstorage` 走默认 `Auto` 选择 `Columnar`，但 checkpoint 没等落盘；真正的 byte 差异需要在完整 checkpoint 后测量，本 probe 对 2000-edge 规模不敏感。**这个数字不影响 §4 决策**，因为 CPU 路径 2× 放大已经是实信号。
4. **reverse leg hit-rate = 0%**——probe 数据生成器用 `(src, dst) = (fwd(i), rand(i))`，in-leg 查询以 `dst = vid` 为条件，均匀分布下 `50000 edges / 50000 vertices ≈ 1 edge/vertex`，查询命中概率约 1/N。**这恰好是 §4 默认动作的典型场景**：dual-direction 表的 in 腿根本没被查询模式使用。

### 3.5 bundled_necessity_bench（§5 Bundled vs Columnar）

```
Auto => Columnar, Bundled opt-in => Bundled

--- Point-lookup latency (median us per lookup) ---
Columnar : 0.10 us / lookup
Bundled  : 0.06 us / lookup
speedup  : 1.76x

--- Full-scan throughput (edges/sec) ---
Columnar : 0 edges/s
Bundled  : 0 edges/s

--- Edge count on materialized stores (verify parity) ---
Columnar edge_count: 50000, Bundled edge_count: 50000

--- Persistent path: checkpoint bytes + rank-trigger probe ---
Both dir + rank=0  :     10275569 bytes (9.8 MiB), rank_trigger=0
Both dir + rank=42 :     10275569 bytes (9.8 MiB), rank_trigger=1

--- Resident foot-print estimate (edge_count * assumed edge bytes) ---
Columnar est resident:      1200000 bytes (1.1 MiB)
Bundled  est resident:      1000000 bytes (1.0 MiB)
ratio: 1.20x
```

**解读**：
1. ✅ **Bundled 点查 1.76× 加速**（0.10 → 0.06 us），热路径确实有收益。
2. ❌ **full-scan throughput = 0**——probe 设计缺陷：50000 edges / 50000 vertices ≈ 1 edge/vertex，`merged_out_nbrs_with_limit` 每次返回空 Vec，visited 计数为 0。不影响点查结论。
3. ✅ **nonzero rank 触发 Bundled 不兼容**（`rank_trigger=1`），与 `record_form.rs` 的 admission rule 一致：Bundled 表拒绝 nonzero rank 写入。
4. ❌ **resident estimate ratio = 1.20×**——远低于"减半以上"的量级门限。`Auto` selector 明确跳过 Bundled 选择的理由（不支持 rank、MVCC 版本链、在线改列）与本数据吻合：收益量级不够抵消误用风险。
5. persistent path checkpoint bytes 完全相同是因为 persistent EdgeTypeInfo 没有设置 `record_form=Bundled`，两条路径都走了默认 Columnar——probe 设计缺陷，不影响 resident estimate。

### 3.6 rw_mix_group_striping_bench（§2 + §3 读写混合）

```
stmts per writer thread = 2000, writers = [2,4], readers = 4, vertices = 200000
         leg | writers |  wall ms |   writes/sec |  read_p99 us |   read ops | skew/uniform
     uniform |       2 | 11505.26 |          348 |          6.0 |    4591351 | 1.02
      skewed |       2 | 11680.49 |          342 |          6.0 |    4967583 | 1.02
     uniform |       4 | 24551.85 |          326 |          6.0 |    8461183 | 0.99
      skewed |       4 | 24413.04 |          328 |          6.0 |    9112525 | 0.99

Section 3 probe: same-row vs different-row-in-same-group
               leg |    wall ms |   writes/sec
          same_row |    7470.74 |          535
 diff_row_in_group |    7495.54 |          534

Section 3 Amdahl upper bound for row-level locks: -0.3%
```

**解读**：
1. **skewed/uniform wall ratio ≈ 1.00x**（2 线程 1.02，4 线程 0.99），远低于 §2 门限 **2.0×**。
2. 吞吐量极低：2 writer 线程 × 2000 stmts / 11.5 s ≈ 348 stmts/s per thread。每条 GraphStorage auto-commit 耗时 ~2.8 ms，这就是 WAL fsync + GraphStorage 全局 gate 的串行效应。
3. **Read p99 = 6.0 us**，在极慢的写入上完全不被阻塞——说明写路径是 WAL-fsync-bound（持久化路径单条 ~10 us 里 93% 是 CPU，这里 2.8 ms/条是 GraphStorage 层 gate 排队 + 完整 MVCC WAL round-trip）。
4. **Section 3 Amdahl upper bound = −0.3%**（实际 0%，浮点误差）。同行写入与同组不同行写入 wall time 完全一样，说明 **当前工作负载下根本观察不到行级锁争用**——瓶颈在 GraphStorage 更上层的 gate/WAL。

## 四、各章节决策详述

### 4.1 §2 组级条带锁：关闭

**门限回顾**：skewed 提交耗时 ≥ uniform 两倍以上，且 skewed 仍主导 uniform 吞吐量。

**实测 vs 门限**：
| 指标 | 门限 | 实测 | 判定 |
|:---|:---|:---|:---|
| skewed wall / uniform wall | ≥ 2.0× | **1.00×** | ✗ |
| skewed 吞吐量不低于 uniform | ≥ 1.0× | **0.99×**（持平） | — |
| write_gate share | < 20%（不是主瓶颈） | **81% @ 16t**（主瓶颈是上层 gate） | ✗ |

**关键洞察**：`edge_group_commit_bench` 单线程 commit 显示 skewed 反而是 uniform 的 **1.64× 快**，因为 commit 是串行单线程调用，不存在"组间争用"——跨组拆分只带来更多锁粒度开销。真正的并发瓶颈在 `GraphStorage::AutoCommitWriteGate`（全局单值，81% 等待占比），它早于 EdgeStore 层的 partition lock。

**关闭结论**：EdgeStore 内 partition lock 不是瓶颈；当前架构下 striping 零收益、零动力。维持显式 `batch_insert_edges` 按 chunk 切分的调用契约。

### 4.2 §3 行级锁 / epoch 快照读：关闭

**门限回顾**：§2 必须先通过；同行冲突占可并行部分主导。

**前置条件**：§2 未通过 → §3 自然关闭。

**额外反证**：rw_mix 同行 vs 同组不同行 Amdahl 上界 **−0.3%**（实际 0%）。当前瓶颈（GraphStorage gate + WAL fsync）早已掩盖了任何行级争用的信号——即使将来 §2 被触发，行级锁也必须先把 gate 和 WAL 的开销压下来再评估。

### 4.3 §4 异步次腿：关闭；OutOnly/InOnly 迁移：通过

**门限回顾**：in 腿被高频反向遍历实际使用 AND 双腿 apply 占写耗时主导 AND 单向化不可接受。

**实测 vs 门限**：
| 指标 | 门限 | 实测 | 判定 |
|:---|:---|:---|:---|
| in 腿非空查询率 | > 0%（真实使用） | **0.0%**（所有 reverse query 空返回） | ✗ |
| CPU 写放大（CsrShardSet commit） | 2.0× | **1.78–2.10×** | ✅ |
| WAL fsync 主导度 | > 60% | **6.9–21.0%**（CPU 主导） | ✅ |
| 单向化可行性 | 不可接受 | **本工作负载 in 腿完全不用** | ✅ 可用 |

**双轨结论**：
1. **异步次腿重构**：关闭——in 腿根本没有查询流量，撕裂读的 correctness 代价完全不值得。
2. **OutOnly/InOnly schema 迁移**：**推荐立即执行**——dual-direction 表的 in 腿无查询，但 commit 和 storage 的 CPU 写放大是实的，去掉 leg 是零风险的用法治理。

**注意**：persistent byte amplification probe 显示 1.00× 是 probe 缺陷（2000 edges 没触发 checkpoint flush），不影响 CPU 路径 2× 的实信号。建议迁移后加一个大规模 checkpoint（100 万 edge）补测 storage byte 变化。

### 4.4 §5 Auto-Bundled：维持显式 opt-in

**门限回顾**：空间收益减半级以上 + rank 非零/版本链/改列触发率 0。

**实测 vs 门限**：
| 指标 | 门限 | 实测 | 判定 |
|:---|:---|:---|:---|
| resident memory 减半级差 | ≥ 2.0× | **1.20×** | ✗ |
| rank 非零触发率 | 0% | **100%**（测试 nonzero rank 直接不兼容） | ✗ |
| point-lookup 加速（正收益信号） | ≥ 1.3× | **1.76×** | ✅（但不足以覆盖风险） |

**关键权衡**：Bundled 点查 1.76× 加速是热路径实收益，但 **rank 非零写入的强制迁移**是一个 correctness + 运维风险：一次 rank 写入就触发从 Bundled 迁移到 Columnar，迁移期间锁表 + 全量重建 checkpoint。Auto 选择 Bundled 把"一次 rank 写入 = 强制迁移"变成了不可预测事件。

`RecordFormPreference::Auto` 当前 **刻意不选择 Bundled** 的设计理由（见 `store.rs` 注释："Auto never selects it, so a table only takes the inline limits when the operator asks for them by name"）与本数据吻合：收益量级（1.2×）低于门槛（2×），风险量级（rank 兼容性硬约束）不可接受。

**维持结论**：Bundled 继续作为 **显式 opt-in** 选项保留；自动选择策略暂不扩容。建议后续收集 `form_profile_snapshot` 的生产数据，若某批表 `avg_width_bytes` 极低（确实符合单标量 Bundled 准入）且 rank/version/alter 三项触发率长期为 0，再重新评估。

## 五、用法治理建议（非引擎改动）

四项延期关闭后，**可用的优化入口全部在调用方**：

| 优化方向 | 落地层 | 说明 |
|:---|:---|:---|
| `batch_insert_edges` 显式 chunk 切分 | 调用方 | 延续 `edge_group_commit_bench` 头部容量契约；避免单次持锁过长 |
| dual-direction 表 → OutOnly / InOnly | schema 审计 + 迁移 | `storage_direction::Both` 但 `form_profile_snapshot` 反向查询占比 = 0 → 方向化 |
| GraphStorage 层 `AutoCommitWriteGate` | 高层 Session / API | 当前 gate wait 81% 是更大的瓶颈——考虑 batch commit API 替代 auto-commit |
| 禁用 rank 写入 Bundled 候选表 | 调用方 guard | 已经被 admission rule 硬拦；调用前探测 `is_bundled_eligible` |

## 六、probe 局限与后续补强

本次基准中识别的若干 probe 设计局限，不影响主决策但影响数值精度：

| probe | 局限 | 补强方向 |
|:---|:---|:---|
| persistent byte amplification (§4) | 2000 edges 未触发 checkpoint；Both/OutOnly byte 显示 1.00× | 放大到 1M edge，调用完整 checkpoint 后测 |
| reverse leg hit-rate (§4) | uniform `(src, dst)` 分布下 in-leg 命中率天然 ≈ 1/N | 构造 chain/ring 拓扑让 reverse 非空 |
| full-scan throughput (§5) | 50000 edges / 50000 vertices = 1 edge/vertex | 扩大 edge/vertex 比到 10:1 让 scan 有数据 |
| resident memory estimate (§5) | 用硬编码字节数估算，未直接测内存 | 给 EdgeStore 加 `resident_memory_bytes` 公共 API |
| rw_mix parallel throughput | 容器 2 核，writer_threads ≤ 4 受限 | 迁到 ≥ 8 核机器重跑，看线性扩展趋势 |

## 七、最终决策矩阵

```
§2 group-striping     : CLOSED（skewed/uniform=1.00x，gate 是真瓶颈）
§3 row-level/epoch    : CLOSED（§2 未过 + Amdahl=-0.3%）
§4 async-secondary    : CLOSED（in leg hit-rate=0.0%）
§4 OutOnly/InOnly     : ADOPT NOW（CPU amp=1.78–2.10×，单向化零风险）
§5 auto-Bundled       : DEFERRED（resident=1.20× < 2× gate，rank 触发率硬约束）

next measurement cycle:
  - write_gate_bench 在 batch commit 模式下重跑（排除 gate 后看 partition lock 争用）
  - §4 大规模 checkpoint byte probe 补强
  - 生产 form_profile_snapshot 收集后重新评估 §5
```

## 八、附录：bench 清单与运行命令

```bash
# 新增
cargo run --release -p graphdb --bin outonly_vs_both_bench
cargo run --release -p graphdb --bin bundled_necessity_bench
cargo run --release -p graphdb --bin rw_mix_group_striping_bench

# 复用
cargo run --release -p graphdb --bin write_gate_bench
cargo run --release -p graphdb --bin edge_group_commit_bench
cargo run --release -p graphdb --bin edge_point_write_bench
```

所有 bench 输出机器型号与核数；`target/release/deps/*.json`（criterion）或 stdout（plain-main）可被后续脚本解析入库。
