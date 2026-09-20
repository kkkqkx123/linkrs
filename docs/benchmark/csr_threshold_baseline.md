# CSR 阈值基线文档

本文档记录当前 CSR 存储引擎的关键阈值参数及其设计依据，供后续阶段调优时引用。

## 阈值清单

### 1. TOMBSTONE_REUSE_SCAN_BOUND = 64

- **位置**: `mutable_csr/write.rs`
- **含义**: 热路径墓碑复用扫描的回退上界。当复用提示失效或未知时，最多扫描前 64 个主块槽位寻找可回收墓碑。
- **设计依据**: 窄行（<=64 条边）占绝大多数顶点，扫描 64 个冷半（16 字节/条）共 1024 字节，远小于一次缓存行未命中代价。宽行复用提示命中率为 O(1)，扫描上界仅作为兜底。需通过 `bench_tombstone_dense_insert` 场景验证该上界对吞吐的影响。
- **备选值**: 32（更保守）、128（更宽松）
- **验证场景**: `bench_tombstone_dense_insert`，输出 `tombstone_reuse` 计数器

### 2. LIVE_SET_WIDTH_BOUND = 8

- **位置**: `mutable_csr/live_set.rs`
- **含义**: 行宽超过此阈值时创建 `LiveKeySet` 索引（`HashMap<(u32, i64), EdgePosition>`），否则通过行扫描回答判重和点查。
- **设计依据**: HashMap 的固定开销（约 200+ 字节/entry）在行宽 <=8 时相对于行扫描无优势。超过 8 条边后，O(1) 查找显著优于 O(n) 扫描。需通过 `bench_routing_hit_fallback` 场景对比索引命中和回退扫描的纳秒级延迟。
- **备选值**: 4（更激进索引）、16（更保守索引）
- **验证场景**: `bench_routing_hit_fallback`

### 3. OVERFLOW_REPACK_CHUNKS_PER_VERTEX = 8

- **位置**: `mutable_csr/overflow.rs`
- **含义**: 每顶点溢出块数超过此值时触发行内重排（repack），将多块合并为单个连续块。
- **设计依据**: 多块遍历需逐块跳转，缓存局部性随块数线性下降。8 块上限将最坏情况控制在 8 次跳转以内。合并为单块后，读路径走 `single_chunk` 快径。需通过 `bench_overflow_multi_block_traversal` 场景验证块数对扫描带宽的影响。
- **备选值**: 4（更激进合并）、16（更宽松合并）
- **验证场景**: `bench_overflow_multi_block_traversal`，输出 `repack` 计数器

### 4. SEGMENT_SHIFT = 10 (SEGMENT_SIZE = 1024)

- **位置**: `csr_shared.rs`
- **含义**: 稀疏行表的分段地址位数。每段覆盖 1024 个顶点，通过移位+掩码路由，无除法。
- **设计依据**: 1024 行/段 × 8 字节/槽 = 8KB/段，刚好放入 L1 缓存。段内偏移用 10 位掩码，路由开销为一次移位+一次 AND。对于百万级顶点场景，段指针表约 8KB（1000 段 × 8 字节），内存开销可忽略。需通过 `bench_overflow_multi_block_traversal` 间接验证路由开销。
- **备选值**: 8（256 行/段，更小缓存占用）、12（4096 行/段，更少段指针）

### 5. OVERFLOW_CHUNK_MIN = 8 / OVERFLOW_CHUNK_MAX = 4096

- **位置**: `mutable_csr/row.rs`
- **含义**: 溢出块大小的几何分级区间。小行分配 8 槽，大行分配至 4096 槽。
- **设计依据**: `graded_overflow_chunk_edges(live)` 取 `next_power_of_two(live).clamp(8, 4096)`。5 条边的行分配 8 槽（1 个块），512 条边的行分配 512 槽，5000 条边的行分配 4096 槽。分级避免了小行过度分配和大行碎片化。需通过 `bench_overflow_multi_block_traversal` 和 `bench_tombstone_dense_insert` 验证分级合理性。
- **备选值**: 下限 4/16，上限 2048/8192

### 6. PACKED_CSR_DENSITY = 0.8

- **位置**: `mutable_csr/row.rs`
- **含义**: 重建时行容量目标密度。`ceil(live / 0.8)` 为行容量，预留 20% 空位供日常写入。
- **设计依据**: 20% 空位平衡了写入放行率和内存浪费。密度过高（>0.9）导致频繁溢出，过低（<0.6）浪费内存。需通过 `bench_reserve_vs_rebuild` 验证重建后行间隙对写入吞吐的影响。
- **备选值**: 0.7（更多间隙）、0.9（更少间隙）

### 7. DEFAULT_VERTEX_DEGREE = 4

- **位置**: `mutable_csr/core.rs`
- **含义**: 顶点首次写入时分配的主块槽数。
- **设计依据**: 大多数顶点度数在 4 以内，初始分配 4 槽覆盖常见场景，避免首次写入即溢出。零度顶点不分配主块，通过懒分配节省内存。
- **备选值**: 2（更保守）、8（更宽松）

### 8. DEFAULT_VERTEX_CAPACITY = 1024 / VERTEX_GROWTH_FACTOR = 1.25

- **位置**: `csr_shared.rs`
- **含义**: 顶点数组初始容量和扩容系数。
- **设计依据**: 1024 初始容量覆盖小数据集，1.25 增长因子避免过度分配（对比 2x 翻倍）。
- **备选值**: 增长因子 1.5 或 2.0

## 运行基准

```shell
cargo bench --bench csr_perf_bench
```

基准输出包含以下计数器，供后续阶段引用：
- `overflow_chunk_allocs` — 溢出块分配次数
- `primary_block_allocs` — 主块分配次数
- `repack` — 行内重排次数
- `tombstone_reuse` — 墓碑复用次数
- `live_set_rebuilds` — 活跃集重建次数

## 阶段引用规则

后续阶段调整任何阈值时，必须引用本文档对应条目的基准场景编号，并在修改后重新运行基准确认影响方向与预期一致。
