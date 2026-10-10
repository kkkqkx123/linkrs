# 问题：avx512 距离核 Manhattan 用单累加器，吞吐仅为 Euclid/Dot 的 1/4

- 状态：新建（待优化）
- 类型：性能缺陷（SIMD 内核实现不一致）
- 复现：`cargo bench -p simvec --bench distance_bench`（2026-10-09，AVX-512 硬件，
  运行时 dispatch 选择 Avx512 内核）

## 实测数据（distance_kernel，Gelem/s）

| 维度 | Euclid | Dot | Cosine | Manhattan |
|---:|---:|---:|---:|---:|
| 128 | 11.39 | 10.63 | 6.80 | **4.29** |
| 256 | 15.51 | 15.32 | 10.45 | **4.58** |
| 512 | 18.19 | 18.53 | 12.72 | **4.61** |

Manhattan 在 512 维仅 4.6 Gelem/s，且**不随维度提升**（Euclid/Dot 提升 62%）——
呈延迟受限（latency-bound）特征。

## 代码依据

- `crates/simvec/src/distance/avx512.rs` `distance_l1`（约 208-249 行）：
  主循环只有**一个**累加器 `acc`，`_mm512_add_ps(acc, abs)` 形成串行依赖链，
  每次 add 需等待上一次结果（vaddps 延迟约 4 cycle），吞吐被链长卡死。
- 对照 `avx2.rs` `distance_l1`（约 177 行起）：使用 `acc0/acc1` **双累加器**交织，
  同为手写内核却模式更优——首选内核（Avx512 排在 IMPLS 首位，kernel.rs:132）反而是较差实现。

## 影响

- 使用 Manhattan 度量的向量扫描吞吐被压到 1/4（vector_scan 基准确认 exact scan
  各度量吞吐与内核一致）；HNSW/IVF 使用 Euclid/Cosine 时不受影响。

## 修复方向

- avx512 `distance_l1` 改双累加器（照抄 avx2 模式，寄存器充裕）；
  可进一步用 `_mm512_abs_ps` 替代 andnot 掩码；
- 在 bench 中加各内核（Naive/Avx2/Avx512）× 各度量的差分矩阵，防止再出现单点退化
  （kernel.rs 已有 test-only `force_for_test` 可直接复用）。
