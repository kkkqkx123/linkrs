# 列式化系列优化必要性验证设计（Columnar Necessity Verification）

- 状态：验证中（验证结果见「执行记录」章节）
- 关联分析文档：`docs/analysis/query/columnar_vectorization_analysis.md`
- 关联已落地工作：T4（惰性列物化/短路）、T5（ColumnarStats 运行时统计）、P8（Morsel 并行执行器）

## 背景

针对以下六项优化，需要在投入正式改造前用测量确认必要性，避免猜测性优化：

1. 列式 DataChunk（typed `Vec<i64>` 等定长列）
2. SIMD（宽寄存器向量化）
3. Validity Bitmap（null 有效性位图）
4. 惰性选择向量跨算子传播（filter → project → aggregate）
5. Morsel 并行扩展现有能力
6. 存储层架构改造（列式属性块等）

## 现状盘点（调研结论）

| 项 | 现状 | 说明 |
|----|------|------|
| DataChunk | 行式 `Vec<Vec<Value>>` 为主，惰性 `columns` 物化（T4.2），`take_indices` 延迟（T4.3），Filter 短路（T4.1） | `Value` 为 19+ 变体枚举（含 `Box`/`CompactString`），单值 16~40 字节 |
| ColumnarStats | 已落地（T5），`evaluate_expression` 每次记录 hit/miss，有 `hit_rate()` 访问器 | 真实负载证据的采集通道 |
| SIMD | 无依赖，纯标量 | — |
| Validity | `Value::Null`/`Empty` 枚举变体，无独立位图 | storage 已有 `column_stats.rs` 可统计 null 率 |
| 选择向量 | chunk 内延迟（T4.3）；**跨算子传播未做** | — |
| Morsel 并行 | **已在生产路径落地（P8）**：`MorselWorkerPool`、分区执行器、Gather 根、`ProfileBoard.parallel_*` 字段、M6 共享调度器 | 本项验证目标改为：量化现有加速比与扩展空间，而非从零引入 |
| 存储层 | 已有 CSR、mmap 容器、cold 路径、`compression.rs`、`encoding/`、`mvcc.rs`、`cache/` | 改造须为增量 |
| 基准设施 | criterion 齐全：`operator_bench`（expr/filter/column_materialize）、`storage_bench`、`query_bench`、`end_to_end_bench` | — |

## 统一验证方法论

每项优化统一走四步：

1. **基线**：现有 criterion bench + `perf stat` 定位真实热点与缓存表现
2. **理论上界**：计算乐观收益上限（Amdahl / 内存带宽），排除"做了也不够格"的项
3. **最小 PoC**：限定范围的实验原型量化实际收益，不全面改造
4. **门槛验收**：设定量化指标，达标才正式立项；不达标记入 backlog

## 逐项验证方案

### 1. 列式 DataChunk（typed `Vec<i64>`）

**必要性信号**：
- ColumnarStats 显示高频列访问；等宽列（i64/f64/i32）上单列过滤频繁
- 宽表（≥8 列）单列谓词过滤时缓存未命中率高（行式遍历全行）

**PoC 实验**：
- 微型列式容器（`Vec<i64>` + 行索引）vs 行式 `Vec<Value>` 过滤吞吐对比
- 8 列宽表单列过滤：缓存表现（`perf stat -e cache-misses`）
- 列式收益上界计算：`Value` 枚举 16~40B vs 定长 8B，理论带宽比 2~5x

**验收门槛**：单列谓词过滤场景 ≥1.5~2x 吞吐，且多列投影场景无回归

### 2. SIMD

**前置问题**：
- 瓶颈是 CPU 还是内存带宽？（`perf stat` 看 `stalled-cycles`/`cache-misses`）
- 热点是否为"大块等宽数值列上的简单运算"？（图遍历为随机访问型，SIMD 无收益）

**PoC 实验**：
- 零成本试水：`-C target-cpu=native` 编译对比
- 定长列上手动 4-lane 解包循环 vs 标量循环对比
- 验收：SIMD 路径 ≥2x 且该路径端到端耗时占比 >10%，否则不值得

### 3. Validity Bitmap

**前置问题**：
- 真实 null 率：复用 storage `column_stats` 统计各 schema 属性 null 密度
- 位图收益区间在稀疏 null（<5%）；图数据库属性多可选，null 率可能 30%+，需数据说话

**PoC 实验**：
- `Option<Vec<i64>>` vs `(Vec<i64>, u64 位图)` 过滤对比，扫 null 率 0%/1%/30%/80% 四档
- 验收：1% null 档 ≥1.3x 且 80% null 档无回归

### 4. 惰性选择向量跨算子传播

**必要性信号**：
- 链式查询（match→where→return）中中间算子实际行数 vs 输入行数差距大
- 高选择率输入的链式过滤存在

**PoC 实验**：
- filter→project→agg 链：物化中间行 vs 传播 indices，扫选择率 1%/10%/50%
- 存储层 scan 下沉谓词是更大机会点（CSR 遍历直接跳过），单独评估
- 注意：selection vector 对行式 chunk 收益有限（省拷贝非省计算），最强收益待列式后

### 5. Morsel 并行（P8 已落地，验证扩展空间）

**现状修正**：非"从零引入"，而是量化现有实现（`MorselWorkerPool`/Gather 根/ProfileBoard 并行字段）的实际加速比。

**验证**：
- `ProfileBoard.parallel_wall_time_us` vs `parallel_work_time_us` 之比 = 并行效率
- 端到端查询在 1/2/4/8 核下的加速曲线；图遍历型查询（随机访问）vs 表扫描型查询的加速差异
- 验收：≥4 核时 ≥2x 加速、无事务语义破坏、复杂度增量可控

### 6. 存储层架构改造

**必要性信号**：
- 端到端 profiling 中"存储读"占执行时间比例
- mmap 随机访问 page fault 开销（`perf stat -e minor-faults`）

**PoC 实验**：
- 负载对：宽表全扫描 vs 窄表随机取属性，对比吞吐/延迟
- 在现有 encoding/compression 之上做列式属性块，复用 cold 路径基础设施
- 与查询侧列式联动验收，共用一套 Q-set

## 决策顺序（依赖约束）

```
① ColumnarStats 采集真实负载证据（零成本，纯观测）
   ↓ 数据决定是否触发
② 列式 DataChunk（typed 定长列 + 行式兜底）  ← 其余项的地基
   ↓
③ 按热点评估：SIMD（依赖②）/ 选择向量下沉到 scan（依赖②）
   ↓
④ Morsel 并行扩展（收益在②③之后最大化，复杂度最高，放最后）
```

存储层改造与查询侧②并行评估，共用 Q-set 验收。

## 执行记录

验证执行于 2026-08-03，环境：release 编译、单核基准（criterion）。`perf`/`valgrind` 不可用，缓存论据由吞吐数据（已含缓存行为）替代。

### 实测数据

**0. 类型开销（理论上界）**
- `size_of::<Value>()` = 56 字节（含 `Box<Vertex>`/`Map` 等大变体）；`Option<i64>` = 16 字节；`i64` = 8 字节
- 列式定长列理论内存带宽上界：7x

**1. 列式 DataChunk**（`row_vs_column_filter`，单列 i64 过滤）

| 行数 | 行式 `Vec<Value>` | 列式 `Vec<i64>` | 加速比 |
|------|-------------------|-----------------|--------|
| 64 | 34.9 ns | 19.6 ns | 1.8x |
| 1024 | 536 ns | 303 ns | 1.8x |
| 16384 | 11.2 µs | 4.9 µs | 2.3x |
| 262144 | 394.5 µs | 84.0 µs | **4.7x** |

宽表（5 列）单列谓词：`DataChunk::evaluate_expression` 全流程 119.3 ms vs 列剪枝 82.5 µs @262144 行（**1446x**，含物化/求值基础设施成本）。
真实路径基线：`expr_eval/simple_predicate/4096` = 599 µs ≈ 6.8M rows/s。
**结论：必要，立项。** 超出 1.5~2x 门槛（4.7x）。

**2. SIMD**（`autovectorization`，262144 元素 i64 过滤）

| 编译 | scalar | unrolled4（手工展开） |
|------|--------|----------------------|
| 默认 target | 80.97 µs | 69.8 µs |
| `-C target-cpu=native` | **23.39 µs（3.46x）** | 32.5 µs |

编译器自动向量化（AVX2）带来 3.46x 收益，**零代码成本**；手工 4-lane 展开反而妨碍自动向量化。
**结论：必要，但通过编译选项获取（`RUSTFLAGS="-C target-cpu=native"` 或发布 profile 设 `x86-64-v3`），不做手工 SIMD 代码。**

**3. Validity Bitmap**（`null_bitmap`，262144 元素，null 感知过滤）

| null 率 | `Option<i64>`（16B/元素） | 位图双数组 | 胜者 |
|---------|---------------------------|-----------|------|
| 0% | 112.9 µs | 145.0 µs | Option |
| 1% | 125.4 µs | 134.5 µs | Option |
| 30% | 100.7 µs | 131.5 µs | Option |
| 80% | 81.1 µs | 139.3 µs | Option |

位图固定位提取开销恒定，Option 编码的跳过分支有预测优势；无空值压缩的纯位图实现全档位落后。
**结论：不必要（当前编码/访问模式）。** 若未来采用"HasNull 标志 + fast/slow 双路径"（0% null 直接走纯列循环）需重新评估，记入 backlog。

**4. 惰性选择向量跨算子传播**（`selectivity_propagation`，16384 行 5 列）

| 选择率 | take_indices 物化 | index 直传 | 加速比 |
|--------|-------------------|-----------|--------|
| 1% | 430 µs | 130 ns | **3300x** |
| 10% | 828 µs | 3.3 µs | 250x |
| 50% | 910 µs | 8.9 µs | 102x |

物化成本与选择率弱相关（拷贝主导）；index 直传与选择率线性，低选择率优势达 3 个数量级。
**结论：必要，立项。** T4.3 已做 chunk 内延迟，此为其跨算子（filter→project→join→agg）延伸；存储层 scan 谓词下沉为更大机会点，一并规划。

**5. Morsel 并行（P8）**：已在生产路径落地（`MorselWorkerPool`、Gather 根、分区执行器、M6 共享调度器），`ProfileBoard` 记录 `parallel_work_time_us`/`parallel_wall_time_us` 可算并行效率。本轮未构造端到端多核加速曲线（需并行端到端基准），**必要性待加速比数据确认，记入 backlog**；预期收益集中在表扫描/哈希聚合，图遍历型查询加速有限。

**6. 存储层架构改造**：现有 `storage_bench` 仅覆盖写入路径（单条 1.8ms、批量 10000 条 270ms），**读取路径基线缺失**。评估顺序：先补读取基准（宽表全扫 vs 窄表随机取属性）→ profiling → 再判定。**待评估，记入 backlog**。

### 决策结论

| 项 | 结论 | 依据 |
|----|------|------|
| 列式 DataChunk | **立项** | 单列过滤 4.7x，理论上界 7x，宽表 1446x |
| SIMD | **立项（编译选项）** | native 3.46x 零代码；拒绝手工 SIMD |
| Validity Bitmap | **不立项** | 四档 null 率全落后；backlog 备注 HasNull 双路径 |
| 选择向量跨算子 | **立项** | 低选择率 3300x；与列式联动 |
| Morsel 并行扩展 | 待确认 | P8 已落地，需端到端加速曲线 |
| 存储层改造 | 待评估 | 读取基线缺失，先补基准 |

后续立项工作按依赖顺序执行：列式 DataChunk（typed 定长列 + 行式兜底）→ 选择向量跨算子 + scan 谓词下沉 → SIMD 编译选项落地（独立、可先做）→ 并行扩展评估。

| 项 | 基线 | PoC 结果 | 结论 | 立项 |
|----|------|----------|------|------|
| 列式 DataChunk | | | | |
| SIMD | | | | |
| Validity Bitmap | | | | |
| 选择向量跨算子 | | | | |
| Morsel 并行扩展 | | | | |
| 存储层改造 | | | | |
