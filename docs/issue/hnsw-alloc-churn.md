# 问题：HNSW 构建/过滤搜索的分配churn——10k 点构建分配 1.85GB

- 状态：新建（待优化）
- 类型：性能缺陷（分配热点）
- 复现：`cargo bench -p simvec --bench alloc_stats_bench`（2026-10-09，10000 点 / 64 维）

## 实测数据（allocation stats）

| 阶段 | allocs | alloc bytes | 备注 |
|---|---:|---:|---|
| ingest (batched upsert) | 130,437 | 35.3 MB | 正常 |
| **hnsw build** | **4,399,632** | **1,854,548,134 (1.85 GB)** | 每点 ~185 KB 分配 |
| search xN unfiltered | 17,553 | 16.7 MB | 正常 |
| **search xN filtered-miss** | **4,011,691** | **645,314,426 (645 MB)** | 过滤路径每次搜索大量临时分配 |

对应耗时（同一 bench 的 phase 输出）：

- search unfiltered：27.4ms；filtered 100/200：1.10s；filtered 200/200：2.17s
  （过滤搜索比无过滤慢 40-80 倍，其中很大一部分是分配/释放开销）。

## 代码依据

HNSW 热路径每次 `search_layer` 调用都新建容器（crates/simvec/src/index/hnsw.rs）：

- hnsw.rs:763 `let mut visited: HashSet<u32> = HashSet::with_capacity(initial_cap);`
- hnsw.rs:769 `let mut candidates: std::collections::BinaryHeap<...> = ...`

构建 = 每点 × 每层各一次 search_layer；10k 点 × 多层 × (HashSet + BinaryHeap + Cand 内 Vec)
的反复分配即 1.85GB churn。过滤搜索路径同理（每次查询重建全部容器）。

## 影响

- 构建吞吐受限（hnsw_build/sequential/10000 每次迭代 ~12s）；
- 过滤搜索 P99 受分配器锁影响（并发场景放大）；
- 内存峰值无谓抬高 ~2GB。

## 修复方向

- 引入 per-thread / per-query scratch（可复用的 visited 位图代替 HashSet——
  访问模式是递增 u32 slot，`BitSet`（tantivy_common 已有）或世代标记数组更合适）；
- BinaryHeap/Cand 缓冲复用（`clear()` 后复用 Vec 底层存储）；
- 重测 alloc_stats_bench，目标：build 阶段 alloc bytes 降一个数量级。
