# 存储层并行改造剩余任务：基准验证与决策归档

> 归档时间：2026-08
> 相关文档：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md`（原分析，仍在 analysis 目录）
> 本归档承接该分析文档中的剩余并行化改造任务（R1/R2/R3），记录性能基准验证结果与最终决策，
> 供后续回顾。原执行方案文档 `docs/analysis/存储并行改造剩余任务执行方案.md` 已删除，内容并入本归档。

---

## 一、问题背景

### 1.1 来源

`linkrs-vs-ladybug-存储并行对比分析` 文档对比了 linkrs 与 ladybug 的存储层实现，列出
缺陷（A–K）与 P0/P1/P2 各优先级改造任务。其中：

- **已完成**（2026-08，见 §二）：缺陷 A–K 与全部 P0 项、P1 group commit、P1-1 边表批量扫描并行化、
  P2 D/E/H/I/J/K；
- **剩余未做**（本文档主题）：三项建议级改造任务 R1/R2/R3，均需性能基准验证后才能判定投入产出比。

### 1.2 剩余任务

| 编号 | 任务 | 必要性 | 说明 |
|---|---|---|---|
| R1 | **游标扫描并行化**（`GraphEdgeCursor`、`scan_batch`） | 可选 | 边表批量扫描已并行化（§二），但查询主路径若走游标则吃不到并行收益 |
| R2 | **批量导入三级缓冲**（thread-local → MPSC → 分片） | 不建议 | 与现行单写者架构矛盾；批量导入最终会在 `AutoCommitWriteGate` 处串行 |
| R3 | **回滚分片化**（放开 `AutoCommitWriteGate` + undo 按分片路由） | 暂不建议 | InnoDB/Oracle 式 undo 路由；收益上限受 fsync 主导的写路径约束 |

三项的共同前提是**性能基准验证**：没有实测数据前，R1/R3 的投入产出比无法判定，
R2 在单写者架构下收益约等于零。

### 1.3 决策框架

四个验证目标及其决策出口：

| 目标 | 回答的问题 | 决策出口 |
|---|---|---|
| G1 单查询大扫描并行加速比 | 单条全表边扫描能否吃到多核？R1 是否值得做 | E(8) ≥ 3 才执行 R1 |
| G2 写路径 gate 争用占比 | 放开写并发（R3）能换来多少吞吐 | gate 占用 < 5% 写耗时则 R3 放弃 |
| G3 导入吞吐瓶颈归因 | 批量导入是 CPU 瓶颈还是 fsync/验证瓶颈 | CPU 占比 < 40% 则 R2 放弃 |
| G4 回滚耗时占比 | 大事务 abort 的耗时量级 | abort < 2× 等价写入耗时则回滚无需优化 |

---

## 二、已完成工作（背景铺垫，2026-08）

### 2.1 边表批量扫描并行化（P1-1 剩余部分）

| 文件 | 函数 | 改动 |
|---|---|---|
| `crates/graphdb-storage/src/storage/engine/graph_storage/reader.rs` | `scan_edges_by_type`（无约束分支，src/dst label 均为 0） | 串行遍历全部分区 → **scatter-gather + rayon `par_iter`** |
| `crates/graphdb-storage/src/storage/engine/graph_storage/context/query.rs` | `collect_all_edge_records` | 同上 |

实现要点：

1. **Scatter-gather**：catalog 短读锁内一次性收集匹配分区的 `Arc<RwLock<EdgeStore>>` 句柄，锁外扫描；
2. **并行扫描**：`par_iter().map()` 每个分区在自身 `read()` 锁下独立扫描，`flatten()` 合并；
3. **顺序保持**：rayon 有序 collect 保证分区结果顺序与改动前一致；
4. **快照注册前置**：`ensure_edge_snapshot_registered` 在并行扫描开始前完成，防止 GC 回收版本。

### 2.2 既有完成项（原文档附录）

| 条目 | 落地位置 |
|---|---|
| P0 正确性 B（前沿卡死） | `mvcc.rs` `reap_expired_write_timestamps` + `write_reap_timeout` |
| P0 正确性 F（无版本链） | `column_store.rs` 版本链 + undo before-image |
| P0 正确性 G（无冲突检测） | `AutoCommitMutationRecorder` 记录 WriteSet + 显式事务 certify |
| P0 性能 A（全库快照注册） | `ensure_vertex/edge_snapshot_registered` 惰性注册 |
| P0 性能 C（锁内 flush） | `persistence.rs` scatter-gather + rayon 并行落盘 |
| P1 group commit 默认开启 | `wal/types.rs` 默认 `true` |
| P2 D（并行库接入/线程池） | rayon 3 处 + `StorageThreadPool`，生产代码无裸 spawn |
| P2 E（BufferPool） | 分片 + `Arc` 返回 + 锁外写回 + 锁内容量检查 |
| P2 H（分片上限/读读并发） | `MAX_SHARDS=256`、自适应、`Mutex`→`RwLock` |
| P2 I（缓存键去时间戳） | generation 版本号 + O(1) 失效 |
| P2 J（segment_allocator） | 已删除 |
| P2 K（内存序） | mvcc.rs 无 SeqCst；`record_allocation` 纯算术 |

---

## 三、基准测试结果（2026-08）

> 机器：AMD Ryzen 7 8845HS（15 可见核），NVMe；release profile（`cargo bench`），
> 各基准为中位数采样。

### 3.1 新增基准资产

| 基准 | 文件 | 用途 |
|---|---|---|
| B1 | `benches/edge_scan_speedup_bench.rs` | 边表扫描加速比 E(n) × 分区数 N（rayon 池 1/2/4/8） |
| B2 | `benches/write_gate_bench.rs` | 并发自动提交写（1/4/8/16 线程）下 gate 等待占比 |
| B3 | `benches/import_bench.rs` | 批量导入 CPU 侧 / WAL+fsync 侧拆分 |
| B4 | `benches/rollback_bench.rs` | 显式事务写 100k~1M 边后 commit vs abort 耗时 |

配套代码改动：

| 文件 | 改动 |
|---|---|
| `context.rs` | `AutoCommitWriteGate` 增加 `acquisitions` / `wait_nanos` 计数器与 `WriteGateStats`，`GraphStorageContext::write_gate_stats()` |
| `graph_storage.rs` / `storage.rs` | `GraphStorage::write_gate_stats()` 并 re-export `WriteGateStats` |
| `Cargo.toml` | 注册 4 个 `[[bench]]`（harness=false），dev-dependencies 增加 rayon |

### 3.2 B1：边表扫描并行加速比（G1）—— 未通过

```
总边数 1M，单边类型，N ∈ {1,4,16} 个 (src,dst) 分区，iterations=11（中位数）
（数据加载后同步触发 background freeze，消除异步冻结积压造成的状态差异）
        |  N=1  |  N=4  |  N=16
E(2)    |  0.97 |  1.35 |  1.52
E(4)    |  0.96 |  1.76 |  1.89
E(8)    |  0.77 |  1.69 |  1.64
T(1)    | 1.83ms | 1.83ms | 1.55ms
```

- 验收条件 `N ≥ 4 且边数 ≥ 1M 时 E(8) ≥ 3`：实测 **1.69 / 1.64，不满足**；
- 加速比在 4 worker 处封顶（E(4)≈1.8-1.9），8 worker 因 catalog 短锁（`get_external_id`
  每边取 `with_vertex_tables` 读锁）与分配竞争反而回落；
- 全表 1M 边扫描本身仅 ~1.8ms，即使线性扩展也仅省 ~1.5ms/次；
- **结论：R1（游标并行化）放弃**。

### 3.3 B2：写路径 gate 争用占比（G2）—— gate 争用显著

```
N 线程并发自动提交 insert vertex（内存态，无 WAL/fsync），每线程 4000 语句
threads |  stmts/sec | gate 等待占比
      1 |   208411 |   0.6%
      4 |    50650 |  64.7%
      8 |    45805 |  79.4%
     16 |    43254 |  88.6%
```

- 出口条件：`gate 等待占比 < 5% 写耗时 → R3 放弃`；实测 N=16 时 **88.6%**；
- gate 是并发单语句写入的绝对串行点；内存态（无 fsync）已是下限，生产含 fsync 时占比更高；
- 注意：这是 gate 的**设计行为**（串行化 WAL 提交点），并非缺陷。

### 3.4 B3：批量导入瓶颈归因（G3）—— 通过（CPU 为瓶颈）

```
同批次分别跑内存态（CPU 侧）与持久化（+WAL append + commit fsync），median of 3
batch | vertices CPU 占比 | edges CPU 占比
 10k  |       55.3%     |     40.2%
100k  |       56.5%     |     47.0%
  1M  |       65.4%     |     52.3%
```

- 出口条件：`CPU 侧占比 < 40% → R2 放弃`；实测全部 ≥ 40%，**CPU 是导入瓶颈**；
- 当前单线程批量导入已达约 **107k 顶点/s、116k 边/s**（1M 批次持久化态 9.3s / 8.6s）；
- **结论：R2（三级缓冲导入）放弃**（详见 §四决策）。

### 3.5 B4：大事务回滚耗时（G4）—— 通过（回滚无需优化）

```
显式事务路径（不受 gate 影响），持久化态，写入 N 边后 commit / abort，median of 3
 edges | T_write ms | T_commit ms | T_abort ms | abort/write
100k   |     415.7  |     240.9   |     ~0.00  |   0.00
500k   |    2168.7  |    1272.2   |     ~0.00  |   0.00
  1M   |    4034.4  |    2369.9   |     ~0.00  |   0.00
```

- 出口条件：`abort 耗时 < 2× 等价写入耗时 → 回滚无需分片化`；实测 abort ≈ 0（µs 级），**满足**；
- 插入型事务的 undo 由 MVCC 版本链可见性承担（aborted 时间戳），无 before-image 执行，
  abort 只做 staged WAL 丢弃 + 时间戳释放。

---

## 四、最终决策

```
B1 未通过（E(8)=1.69/1.64 < 3）           → R1 放弃
B2 通过（gate 88.6%）但 B4 回滚可忽略     → R3 放弃（触发条件要求 B2 且 B4 同时显著）
B3 通过（CPU≥40%）但无 >100k/s 产品需求    → R2 放弃（默认不执行 + 前置障碍未解除）
结论：R1/R2/R3 全部放弃，剩余任务关闭。
```

逐项说明：

- **R1 放弃**：扫描已并行化（§2.1）但加速比上限 ~1.9，E(8) < 3；如需进一步收益应先消除
  `get_external_id` 的 catalog 短锁串行化，而非做游标并行。且对 `limit ≤ 1000` 的分页查询
  并行化收益可忽略，游标状态机复杂度却会显著上升。
- **R3 放弃**：gate 争用真实存在（B2），但 undo 执行可忽略（B4），回滚分片化无收益；
  gate 本身是设计内的提交点串行化，放开需另立"并发写 + 冲突检测"专项（依赖
  `WriteSetAnalyzer::analyze_conflict` 接线，10–15 人日），独立于本任务评估。
- **R2 放弃**：CPU 是导入瓶颈且单线程吞吐已 >100k 条/s，无启动需求；且 R2 前置障碍
  （gate 架构级绕行专用通道、边表哈希分片、WAL/回滚 per-partition 语义）在 R3 放弃后无解；
  批量导入每批次仅 1 次 gate 获取（~30ns），「消除逐语句 gate 获取」的收益对批量路径不成立。

### 明确不做

PostgreSQL 式"无 undo"（CLOG + 可见性过滤）：linkrs 的版本链（`column_store.rs` 的
`set_versioned`/`get_at_ts`）已让 abort 后读不可见，属性更新有真版本链，删除有墓碑；
做 CLOG 改造等于重写 MVCC 读路径，收益为零。

---

## 五、验证中发现并修复的缺陷

在 B1 数据加载（单分区 > 80 万边）期间触发背景冻结时发现 **OOM 崩溃**：

| 位置 | 缺陷 | 修复 |
|---|---|---|
| `crates/graphdb-storage/src/storage/engine/graph_storage/context/freeze.rs` `trigger_background_freeze` | `CompactConfig::with_fixed_ratio(true, 2.0)` 的 2.0 被 clamp 到 1.0 | 改为 `0.5`（等价于 2× 容量意图） |
| `crates/graphdb-storage/src/storage/edge/mutable_csr.rs` `compact_with_ts` | `valid / (1.0 - reserve_ratio)` 在 ratio=1.0 时除零 → `inf as u32` 饱和到 `u32::MAX`，逐顶点容量爆炸（~205TB 分配，进程 OOM） | 对 `reserve_ratio ≥ 1.0` 守卫为"无预留"（`new_cap = valid`）；新增 2 个回归测试 |

验证：`cargo test -p graphdb-storage --lib`（717 个）全绿；`cargo clippy -p graphdb-storage --all-targets`、
`cargo check -p graphdb --features server,fulltext,c_api,grpc,qdrant` 通过。

---

## 六、复现方式

```shell
cargo bench --bench edge_scan_speedup_bench   # B1：E(n) 曲线
cargo bench --bench write_gate_bench          # B2：gate 等待占比
cargo bench --bench import_bench              # B3：CPU/WAL 拆分
cargo bench --bench rollback_bench            # B4：commit vs abort
```

机器要求：≥ 8 核（建议 16 核）、NVMe；记录 CPU 型号与核数以便复现。

---

## 七、后续关注点（非本任务）

- 若未来出现明确的并发写需求，可启动"并发写 + 冲突检测"专项（R3 的 gate 放开部分，
  需先接 `WriteSetAnalyzer` 冲突检测）；
- 若批量导入成为产品重点（>100k 条/s 不够用），需先解决批量导入专用通道（绕过 gate）
  再做 R2 阶段 B/C；
- 边表扫描若需更高加速比，优先消除 `get_external_id` 的 catalog 短锁串行化（如批次解析
  外部 ID 或缓存），而非游标并行化。
