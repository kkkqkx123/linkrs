# vector-search benchmark baseline（Phase A W8）

记录 `cargo bench -p vector-search --bench vector_scan_bench` 首次基线结果，
供 Phase B（Tier 1 / 量化 / 并行）决策使用。运行于 2026-08-18。

## 运行环境

- CPU：AMD Ryzen 7 8845HS（6 核 12 线程）
- rustc 1.97.1，`x86-64-v3`（AVX2 + FMA，`.cargo/config.toml`）
- 参数：`--warm-up-time 1 --measurement-time 3`，100 samples，release
- 数据：随机单位向量，dim=128，固定种子；Cosine / Euclid(平方距离) / Dot

## 1. scan_latency（逐向量距离，保留最近）

| 数据量 | Cosine | Euclid | Dot |
|--------|--------|--------|-----|
| 10k | 157.40 µs (63.5 Melem/s) | 92.95 µs (107.6) | 92.26 µs (108.4) |
| 100k | 3.028 ms (33.0) | 3.274 ms (30.5) | 3.294 ms (30.4) |
| 1M | 26.13 ms (38.3) | 22.96 ms (43.6) | 23.44 ms (42.7) |

> 注意：10k 时函数调用/循环开销占比高，100k+ 更贴近稳态吞吐（≈30–44 Melem/s）。

## 2. simd_vs_naive（100k，Cosine）

| 内核 | 延迟 |
|------|------|
| naive | 9.615 ms |
| avx2 | 3.059 ms |

AVX2 ≈ **3.1×** naive。不设硬阈值，仅作基线。

## 3. filter_selectivity（100k 端到端 search，limit=10）

| 命中率 | 延迟 |
|--------|------|
| 100%（无 filter） | 2.789 ms |
| 50% | 28.94 ms |
| 10% | 21.86 ms |
| 1% | 20.43 ms |

> **观察**：加入 payload filter 后端到端延迟显著上升（100%→50% 达 ~10×）。
> 当前 Tier 0 过滤逐点扫描 payload，且 `match_any` 在候选筛选前整体求值；
> 这是 Phase B 最值得优化的方向之一（过滤先行 / 位图预筛）。

## 4. upsert_wal（WAL append + 应用）

| 批次 | 每批延迟 | 单点吞吐 |
|------|----------|----------|
| single | 19.68 µs | ~50k ops/s |
| batch_100 | 18.93 ms | ~189 µs/点 |

> 批量（fsync 摊薄）带来 ~10× 单点吞吐提升；图事务批量提交路径受益于此。

## 复现

```shell
cargo bench -p vector-search --bench vector_scan_bench
```