# 三方差距收敛设计（Linkrs / Ladybug / Neug）

**日期**：2026-11-24  
**依据**：`docs/plan/next-stage-parity-gap-design.md`、`docs/plan/stage1-stage2-current-gap-remediation.md`、`docs/issue/linkrs-remaining-parity-issues.md` 第 45/46 节，以及 `perf-data/parity/results/2026-11-18/`、`perf-data/parity/results/2026-11-24/` 的三方结果与微基准。本文只描述剩余差距、实施顺序和验证门槛；不新增版本、不新增并行配置默认值、不引入近似或截断。

## 1. 当前结论

Ladybug/Neug 为各自内部计时，Neug AP 为单线程 wall-clock；Linkrs 为固定门两轮加生产默认单轮。倍率只作量级判断。

| 用例 | Linkrs 当前 | Ladybug | Neug | 状态 |
|---|---:|---:|---:|---|
| `group_by_count` | 26–32 ms | 7.10 ms | 13.76 ms | 已从百毫秒级收敛，距 ≤20 ms / ≤3× Ladybug 仍差一步 |
| `distinct_limit` | 25–31 ms | 3.76 ms | 28.70 ms | 对 Neug 同量级，对 Ladybug 约 7–8× |
| `join_two_hop` | 5.5 ms | 2.42 ms | 24.09 ms | 已优于 Neug，距 Ladybug 约 2.3× |
| `variable_path_count` | 5.2–5.6 ms | 457.68 ms | 117.40 ms | 大幅领先，不再优化 |
| 微扫描算子 | 1.2–1.7 ms | 0.88–1.52 ms | 0.044–0.317 ms | 基本对齐 Ladybug，对 Neug 的差距来自执行形态 |
| 库体积 | 约 38 MB | 约 35 MB | 约 43 MB | 早期 197 MB 差距已偿还 |

第 45 节已落地按父直写、两跳度数直写和 schema 证明的身份键直通；第 46 节的阻塞读与非阻塞 `_mm_prefetch` 两轮 A/B 均无收益，已拒绝进入生产。当前不再通过换 DTO、加预取、放宽并行或近似 TopK 求收益。

## 2. 设计取舍

| 维度 | Ladybug | Neug | Linkrs | 判断 |
|---|---|---|---|---|
| 读写模型 | OLAP + buffer pool | mmap 主存 + AP 单线程链 | 写密集 + 点查 + 时间旅行，容量即 RAM | 取舍不同，容量是长期项 |
| 并发粒度 | node group / 列 chunk latch | 逐顶点自旋锁 | 分片 `RwLock` | 偏斜父批下粒度偏粗 |
| 行身份 | CSR 位置即行号 | 条目内 `row_id` | `edge_id → row` + 外部版本权威 | 保留稳定 ID 与单份共享，代价是二次解析 |
| 边属性 | 同序 CSR 行 + zone map | 单属性内联 | 形态适配 + 列存共享 + CRC/WAL 围栏 | 邻域投影随机访问更贵，运维更强 |
| 执行形态 | packed child slices + multiplicity | id-only 扫描 + typed key/TopN | 分区流水线 + typed chunk + 精确局部 TopK | 批式骨架已具备，继续减少算子边界 |

改进原则：只在原模块内收敛，保留 MVCC、边身份、提交清单、全局 Distinct/TopN 权威和 Exchange 契约。

## 3. 实施顺序

每项按“实现 → 同二进制 A/B → 固定门两轮 → 生产默认 → 全量正确性 → 采纳或拒绝”执行。未过门槛的候选不进入生产，并删除无收益复杂度。

### 任务一：LocalTopK 边界剪枝与权重省略

- 在 `LocalTopKDistinctState::insert_key` 中把边界判断前移到 `seen_keys` 查询之前：ASC 下 `key >= worst`、DESC 下 `key <= worst` 直接进入拒绝路径，不做哈希查找或状态修改。
- `PackedExpandPayload.weights` 在全为 1 的批上省略分配，以空 `weights` 表示“每键权重为 1”；聚合侧读取时统一归一。重复边折叠出非 1 权重时仍显式写入。
- 反例覆盖重复键、并列值、NULL 键、空分区、`k` 大于局部行数、重复边权重。

### 任务二：packed 批到 typed 聚合的直接键通道

- 对非身份形状，若单跳展开只有一个整数目的端属性、无过滤/path、运行时标签与计划一致，让 `read_destinations` 直接产出 typed 键列，避免把每个唯一目的端重新装配成 `Value` 行再回读。
- 聚合仍消费 `PackedExpandPayload`，权重语义不变；多属性、字符串、nullable、未知标签一律回退原列读取路径。
- EXPLAIN 记录推进标记与回退原因，pushed/unpushed 逐值相等。

### 任务三：两跳存在性折叠

- 在证明单标签、单边类型、无 TTL、无 staged 写入的窄形状上，评估把 `join_two_hop` 的目的端存在性读取折叠进度数累加，避免为每个唯一目的端构造空投影结果。
- 不修改全局路径计数语义；结果必须与 oracle 逐值相等。

### 任务四：微扫描下推与 morsel 自适应

- `filter_id_range` / `order_limit` 借鉴 Neug 的 id-only 扫描与 typed TopN；`aggregate_min_max` 借鉴 zone map 常量谓词短路。
- `order_limit` 必须先有跨 shard 全局有序证明和物理 limit/offset 接线，否则只保留逻辑证明，不开放截断。
- morsel 数量按 worker 数与行数/度数分布自适应，替代固定阈值；不得改变分区归属和结果顺序。

### 任务五：并发粒度与容量

- 在分片内评估列 chunk / CSR 行区间读守卫，缩短父批锁持有时间；保持提交顺序与 `PendingGate` 语义。
- 评估冷数据只读映射或分层，缓解容量即 RAM；必须保留 CRC、提交清单和崩溃恢复。
- 高度数溢出链和冻结单行回收只在有微基准证据时改动。

## 4. 验证门槛

- 微基准：F1/F2/F5 与对应路径要求时间下降且分配不增；候选至少 10% 才进入固定门，任务一/二/三以 15% 为强门槛。
- 固定门：两轮 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`，结果仍为 `group_by_count [14|59]`、`distinct_limit [14]`，整数逐值相等。
- 生产默认：单轮无回归；九条纯计数、两跳、变长路径不退化。
- 正确性：partitioning 49/49、typed group 8/8、local topk 10/10、parallel 27/27、DQL 324/324、storage lib 全量；预取/下推类改动另要求 pushed/unpushed 对拍。
- 资源：parity JSON 记录 `db_size_bytes`、RSS 峰值、spill 与 reclaim 失败数；不通过牺牲持久性换体积。

## 5. 不做项

近似算法、截断采样、动态重分区、跨分片事务、在生产配置中静默打开并行、以放弃 MVCC/边身份/提交原子性换取局部性能，均不在本方案范围内。

## 6. 实施结论

| 任务 | 结论 | 依据 |
|---|---|---|
| 任务一 | 保留 | 边界判断前移到 `seen_keys` 查询之前，权重全为 1 时省略 `PackedExpandPayload.weights` 分配。新增重复键/并列值反例、空权重与显式权重等价单测；release 全门通过 |
| 任务二 | 拒绝 | 非主键整数属性直通已按计划实现，但当前 LSQB 的四个目的端标签都只有主键整数列，无法构造可激活该证明的查询；没有 A/B 证据，代码已撤回 |
| 任务三 | 拒绝 | `frontier_degree_batch` 的可见性过滤依赖目的端存在性解析；当前 schema 不能证明无悬空边，跳过该解析会改变计数语义 |
| 任务四 | 暂缓 | 固定门 `max_partitions=10` 已跑满，`order_limit` 仍缺物理 limit/offset 与跨 shard 全局有序证明；没有可验证收益面 |
| 任务五 | 暂缓 | 分片锁与 RAM 容量属于架构取舍；当前无偏斜/容量微基准支撑改动，按门槛不引入 |

任务一的 devs release 全门结果为 `BUILD_EXIT=0`，配置默认值、partitioning 49/49、typed group 9/9、local topk 11/11、parallel 27/27、DQL 324/324、storage 全量、column batch 全绿；固定门两轮结果逐值稳定为 `[14|59]` / `[14]`，`actual=10`、`result_stable=true`、`spill_rows=0`。两轮耗时在 30–38 ms 区间波动，与本切片前同量级，不作为独立性能收益；采纳理由是它移除了每批一条确定性分配且不改变语义。两跳 5.0–5.6 ms、变长路径 4.8–5.8 ms，未退化。日志、JSON、EXPLAIN 见 `perf-data/parity/results/2026-11-24/parity-convergence/`。
