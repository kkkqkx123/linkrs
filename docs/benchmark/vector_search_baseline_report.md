# vector-search 基线报告（Phase 1 交付物）

> 状态：基线存档（2026-08-26）。
>
> 对应改进方案 §3 路线图 Phase 1 任务"四个既有 bench 出基线报告
> （build/search/并发/分配统计）"。后续所有优化以本表数字为对照；
> 复测时须在同一硬件、同一命令参数下进行。

## 1. 环境

| 项 | 值 |
|----|-----|
| CPU | x86_64 16 核（`.cargo/config.toml` 启用 `-C target-cpu=x86-64-v3`） |
| 构建 | bench profile（opt-level 3）；criterion 参数 `--warm-up-time 1 --measurement-time 2..3 --sample-size 10..15` |
| 数据集 | 随机单位向量，固定种子（各 bench 自带 SEED） |
| 运行时负载 | 采样期间机器有无关编译负载，绝对值可能偏高 10-30%，相对比值可信 |

复测命令示例：

```shell
cargo bench -p vector-search --bench hnsw_build_bench -- \
  --warm-up-time 1 --measurement-time 3 --sample-size 15
cargo bench -p vector-search --bench alloc_stats_bench   # harness=false 直跑
```

## 2. 影响基线有效性的前置修复

本次建立基线过程中发现并修复了三个问题。**它们不是优化，而是让数字
可信/可用所必需的正确性修复**，任何对照历史行为的讨论都应以此为分界：

### 2.1 HNSW 构建内层微任务扇出（构建慢约三个数量级的根因）

`select_neighbors` 与 `link_neighbor` 的 overflow 路径曾在顺序循环内对
每个候选发起 `par_iter` 扇出——每次扇出只有 ≤32 个距离计算（微秒级），
却要支付 rayon 任务路由 + 全池唤醒的成本；每 insert 触发上百次。
实测 n=500 构建耗时 174s（16 worker），单线程池也要 4s。

修复：两处内层循环改回顺序执行。槽位级并行保留在多 worker 构建路径
（`max_indexing_threads`），那才是正确的并行粒度。

| 场景 | 修复前 | 修复后 |
|------|--------|--------|
| n=500 默认配置构建 | 174 s | **69 ms** |
| n=20000 dim64 构建 | >400 s（未完成） | ~10 s |
| 全量 lib 测试套件 | 10.6 s | 0.32 s |

### 2.2 IVF bench 的召回率"真值"不真

`latency_and_recall` 先发布 IVF 索引再计算 ground truth，而
`ground_truth()` 走的是引擎搜索入口——被路由到 IVF 近似路径
（默认 nprobe=8）。导致 recall 曲线随 nprobe 反常下降（0.675→0.255）。

修复：真值改在索引发步前（精确扫描态）计算。修复后 recall 单调，
nprobe=list 数时严格等于 1.000（自校验通过）：

| nprobe | 1 | 4 | 16 | 64 | 256(=lists) |
|--------|-----|------|-------|------|-------------|
| recall@10 | 0.130 | 0.180 | 0.335 | 0.660 | **1.000** |

### 2.3 搜索快照与 pending 的跨代读取竞态

`search_hnsw`/`search_ivf` 在文件快照之后才加载 `pending` 列表，并发
写入下可能对旧代 key 文件读取新 slot，报出
`CorruptData("slot N has no key")`（新增并发回归测试暴露）。

修复：`pending` 改为与 tombstones/vectors/keys/payloads 同一
读锁临界区内加载，保证 pending ⊆ 当前快照可见范围。

## 3. 构建基线（hnsw_build_bench，dim 128）

| shape | n=2000 | n=10000 | 加速比@10k |
|-------|--------|---------|-----------|
| sequential（全局池顺序） | 575 ms | 5.47 s | 1.0x |
| single_pool（专用单线程池） | 567 ms | 5.52 s | ≈1.0x |
| workers_4 | 164 ms | 1.34 s | **4.1x** |

- 多 worker 子集并发接近线性；推荐 `max_indexing_threads = 4..8`。
- 重构构建签名（IndexBuildParams）后复测 sequential/2000 = 569 ms，无回归。

## 4. 写入吞吐基线（hnsw_ingest_bench，batch 256，dim 128）

| 路径 | 每批耗时 | 吞吐 |
|------|----------|------|
| exact_scan（无已发布索引） | 2.65 ms | 96.8 K elem/s |
| published_hnsw（pending 路由） | ~20 ms（波动大） | ~12.8 K elem/s |

已发布 HNSW 后写入仍走 WAL fsync + store 锁，pending 路由本身 O(1)；
波动来自批次间维护线程 drain 的竞争，属预期行为。

## 5. 搜索延迟与并发扩展基线

### 5.1 concurrent_search_bench（HNSW 20K 点 dim128，ef=40，256 条查询/迭代）

| 线程数 | 每迭代耗时 | 加速比 |
|--------|------------|--------|
| 1 | 88.9 ms（347 µs/查询） | 1.0x |
| 4 | 16.1 ms | **5.5x** |
| 8 | 8.87 ms | **10.0x** |

搜索路径只持短邻接读锁 + entry 原子加载，8 线程超线性（缓存驻留效应）
——当前无锁竞争瓶颈信号；§2.1.1 的 lock-metrics 采集保持默认关闭，
待真实负载出现争用证据再开启对照。

### 5.2 vector_scan_bench（精确扫描，dim 128）

| 数据集 | Cosine | Euclid | Dot |
|--------|--------|--------|-----|
| 10k | 172 µs | 118 µs | 122 µs |
| 100k | ~2.9 ms | ~2.9 ms | ~2.87 ms |
| 1M | 28.4 ms | 25.7 ms | 25.6 ms |

SIMD vs naive ≈ 3.4x；WAL upsert 单条 13.6 µs，百条批量 21.6 ms。

### 5.3 ivf_bench（100K 点 dim128，lists=256）

| 项 | 值 |
|----|-----|
| IVF 构建 | 3.27 s |
| probe 延迟 nprobe=1/4/16/64 | 1.10 / 1.12 / 1.79 / 2.75 ms |
| upsert 开销（50K 点） | 无索引 2.09 ms/批 vs 有索引 2.74 ms/批 |
| 交叉点 @100K | exact 3.95 ms vs ivf(nprobe=8) 1.62 ms |

已知观察：随机查询下 recall@10 在 nprobe=64 时仅 0.66（lists=256 过碎，
nprobe 需逼近 lists 才高召回）——IVF 调优（lists/nprobe 缺省策略）留待
Phase 2 召回测试与后续调优议题，不影响本轮正确性结论。

## 6. 分配统计基线（alloc_stats_bench，10K 点 dim64）

自定义计数分配器包裹系统分配器（仅该报告二进制内生效，库零影响）：

| 阶段 | 耗时 | 分配次数 | 分配字节 |
|------|------|----------|----------|
| ingest（千条批量 ×10） | 191 ms | 160 K | 35 MB |
| hnsw build | 4.06 s | **4.40 M** | **1.85 GB** |
| search ×200 未过滤 | 37 ms | 17.6 K（88/查询） | 16.7 MB（84 KB/查询） |
| search ×200 过滤未命中 | 3.68 s | **4.01 M（20 K/查询）** | 645 MB |

结论（对应改进方案 §2.1.2 测量驱动路线）：

1. **查询期正常路径分配极少**（88 次/查询），无需池化；
2. **过滤未命中的重试链是分配热点**（20K 次/查询）：每轮迭代扩张重新
   物化候选 Vec 并全图遍历。若未来立项优化，方向是重试轮间复用候选
   缓冲与 visited 位图，而非通用内存池；
3. **构建期 4.4M 次分配**主要来自 search_layer/select_neighbors 的临时
   容器——预容量按 ef_construct/m 估计可显著削减，作为 Phase 5 条件项
   的量化依据留存于此。

## 7. 锁竞争观测（lock-metrics feature）

`MetricsSnapshot` 新增 `adjacency_write_locks` /
`adjacency_lock_wait_nanos` / `search_version_reloads`，经 server 采样链路
透出为 `VectorLockOps` / `VectorLockLatencyUs` / `VectorVersionReloads`。
默认关闭（feature `lock-metrics`），开启方式：

```shell
cargo build --release -p graphdb-server --features vector-search/lock-metrics
```

验收口径（改进方案 §2.1.1）：开启后并发吞吐回退 < 5%。本机
concurrent_search_bench 显示当前无争用热点，该 feature 保持默认关闭。

## 8. 持久化 CRC32 占比基线（persist_crc_bench，2026-08-27）

> 新增 `crates/vector-search/benches/persist_crc_bench.rs`（`harness=false`，criterion），对应 `index/persist.rs:200-237` 的 `crc32fast::hash` 在 `save`/`save_hnsw` 全路径中的占比，验收 `< 10%`（`vector_search_remaining_and_longterm_design.md §2.1`）。

### 8.1 测量对象

| 文件 | 规模 | 字段 |
|------|------|------|
| `index.bin` (IVF) | lists=256, dim=128, live=100K, slot_list≈100K, centroids 256×128 | `PersistedIvf` 等价负载（`DummyIvf` 复现，postcard 序列化后 299,767 bytes≈293KB，varint 编码后小于理论 500KB 但同数量级） |
| `hnsw.bin` | dim=128, m=16, live=100K, ~2.1M 边（平均 fill cap/2，varint 编码） | `PersistedHnsw` 等价负载（`DummyHnsw` 复现，postcard 后 7,723,220 bytes≈7.4MB，理论满填 12–15MB，编码压缩后同量级） |

生产级 100K 规模与 §3/§4 同硬件复现；`Dummy*` 与真实 `Persisted*` 同结构、同字段、等大小量级，序列化与 CRC 路径一致，仅邻接采样为随机（不影响带宽测量）。

### 8.2 方法

- `ivf_crc` / `hnsw_crc`：对已序列化 `postcard` 字节的 `crc32fast::hash` 纯内存扫描（`criterion::bench_function` 单核迭代，`black_box` 防优化）。
- `ivf_save_total` / `hnsw_save_total`：`postcard::to_stdvec` + `crc32fast::hash` + `File::create`/`write_all`（magic 4 + version u16 LE 1 + crc 4 + payload）+ `sync_all` 的端到端 `write_tagged`（与 `persist.rs:200-215` 同路径；`bench` 侧为共享 `tempdir` 下的独立 `*_tmp.bin`，含 `fsync`，接近真实 `save` 的 tmp+rename 开销）。
- 每组另有 `*_serialize_plus_crc`（序列化+CRC，不含文件 I/O），用于分解占比来源。
- 手动 ratio 循环（`ITERS=20`，`Instant` 计时，打印于 `eprintln!`）：`ratio = crc / save_total`，判 `< 10%`。
- 命令：`cargo bench -p vector-search --bench persist_crc_bench -- --warm-up-time 1 --measurement-time 3`（bench profile，`--sample-size` 保持 criterion 默认 10；示例下用 `--measurement-time 1` 复测，数字相近）。

### 8.3 结果（同机 2026-08-27，x86_64-v3，AVX2，bench profile opt-level 3）

| 项 | payload | crc (mean) | serialize+crc (mean) | save_total (mean) | crc / save_total (criterion) | 手动 Instant 20 轮 ratio |
|----|---------|------------|----------------------|-------------------|------------------------------|--------------------------|
| IVF 100K (256×128) | 299,767 B | 17.86 µs | 659.9 µs | 814.1 µs | **2.19%** (17.86/814.1) | **3.32%** (26.36 µs / 793.97 µs) |
| HNSW 100K (m=16) | 7,723,220 B | 466.89 µs | 15.23 ms | 20.48 ms | **2.28%** (0.467/20.48) | **2.21%** (453.47 µs / 20.56 ms) |

> 采样：`criterion` 各 10 samples，warm-up 1s；手动循环 20 iters。两次口径均 `< 10%`，通过。

- 结论：CRC 为内存带宽线性扫描，占比远低于阈值；`recovery_test.rs:322-376` 的翻转注入拒绝路径无回归；持久化格式版本锁定 1（开发期不升版，损坏即删档降级精确扫描，见 `persist.rs:18-30` 与 `storage/meta.rs:18-20`，`INDEX_VERSION=1`/`HNSW_VERSION=1`/`FORMAT_VERSION=1`/`FILE_VERSION=1`）。

复测后更新示例行保留阈值判定：若 ratio > 10% 则登记为观察项（通常不会，100K 点 payload 数 MB 内容，CRC 仅扫描内存）。

---

> 本报告随 Phase 2-5 推进滚动更新；新基线以同参数复测数字替换对应行，
> 不删除历史行（标注日期即可）。
