# P2 [simvec] HNSW 构建分配抖动大；并行构建有 3× 收益

## 现象（实测）

`alloc_stats_bench`（10000 pts, dim 64, 200 queries）：

| phase | allocs | alloc bytes |
|---|---:|---:|
| ingest (batched upsert) | 130437 | 35,268,323 |
| hnsw build | 207919 | **167,964,626 (~168 MB)** |
| search xN unfiltered | 5444 | 2,699,029 |
| search xN filtered-miss | 4493 | 1,232,762 |

数据来源：`bench_results/linkrs-simvec.log:501-506`

- HNSW 构建为 10k 点分配 **~168 MB（约 16 KB/点）**——构建期分配抖动大。
- `hnsw_build` 并行扩展（workers 验证）：

| 规模 | sequential | single_pool | workers_4 | workers_4 提速 |
|---|---:|---:|---:|---:|
| 2000 | 483.62 ms | 480.22 ms | 188.68 ms | 2.56× |
| 10000 | 3.9938 s | 4.1381 s | 1.3124 s | **3.04×** |

数据来源：`bench_results/linkrs-simvec.log:377-430`

## 影响 / 结论

- **负向**：构建热路径分配抖动大（168MB/10k 点），可能在大数据集上触发 GC/页压力。
- **正向**：并行构建收益明确（workers_4 比顺序快 3.0×），说明构建并行化方向正确，应默认开启。

## 疑似根因

构建期每次 `search_layer` 新建 `HashSet`/`BinaryHeap`（既有 `docs/issue/hnsw-alloc-churn.md` 记录），热路径分配未复用；`single_pool` 与 `sequential` 几乎无差，说明线程池本身不是瓶颈，瓶颈在单 worker 内分配。

## 建议

1. 构建热路径复用分配器 / 线程局部 buffer，降低 168MB churn（预分配邻居列表容量）；
2. **默认启用 `workers_4` 并行构建**（已验证 3× 收益）；
3. 监控大数据集（≥1M 点）构建期峰值内存，确认抖动不影响稳定性。

## 严重级
**P2（性能，偏正向——并行构建收益明确）**

## 数据来源
`bench_results/linkrs-simvec.log:377-430`、`:501-506`
