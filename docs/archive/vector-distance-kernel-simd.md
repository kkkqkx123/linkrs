# 向量距离核 SIMD 实现与设计理念归档

> **状态**：归档文档（2026-08-26）。
>
> 关联代码：`crates/vector-search/src/distance/`
>
> 关联设计：`docs/plan/vector_search_future_improvement_plan.md`（向量距离内核 SIMD 部分）

---

## 1. 概述

本文档归档 `vector-search` crate 中距离计算核的 SIMD 实现细节与设计理念，涵盖多架构运行时分发、条件编译策略、NEON vs AVX2 对等优化，以及 `std::simd` 评估分支的定位。旨在为后续维护者提供完整的决策上下文。

---

## 2. 架构总览

```
crates/vector-search/src/distance/
├── mod.rs          # 公共 API：distance() + to_score()
├── kernel.rs       # 运行时分发：Kernel 枚举 + best_available() + OnceLock 缓存
├── naive.rs        # 标量基线（始终可用，正确性参考）
├── avx2.rs         # AVX2+FMA 内核（x86_64，16-wide 主循环 + 8-wide tail）
├── avx512.rs       # AVX-512F 内核（x86_64，16-wide ZMM + 8-wide YMM tail）
├── neon.rs         # NEON 内核（aarch64，8-wide 双累加器主循环 + ≤7 标量 tail）
└── portable.rs     # `std::simd` 评估分支（feature-gated，当前委托 naive）
```

### 2.1 分发模型

```
kernel::selected()  ──►  OnceLock<Kernel> 缓存
        │
        └── best_available()  ──►  遍历 IMPLS[架构×feature 组合]
                                    找到第一个 is_available() 的内核
```

**关键设计决策**：

- **运行时分发而非编译期单档**：`.cargo/config.toml` 基线设为 `x86-64`（非 `x86-64-v3`），二进制可在任意 CPU 上运行，启动时自动选优。
- **OnceLock 缓存**：`selected()` 首次调用后缓存结果，后续调用零开销。
- **测试覆盖**：`force_for_test()` 可 pin 到任意内核做差分验证。

---

## 3. 条件编译策略

### 3.1 `cfg` 门控矩阵

| 模块 | `cfg` 条件 | 非宿主行为 |
|---|---|---|
| `avx2.rs` | `#[cfg(target_arch = "x86_64")]` | 编译排除，`kernel.rs` 中 `use` 同样门控 |
| `avx512.rs` | `#[cfg(target_arch = "x86_64")]` | 同上 |
| `neon.rs` | 内部 `#[cfg(target_arch = "aarch64")]` 分 `imp` / `imp_fallback` | **可编译**：`imp_fallback` 委托 `naive`，跨平台编译不报错 |
| `portable.rs` | `#[cfg(feature = "simd_portable")]` 分 `imp` / `fallback` | 两个分支均委托 `naive`，当前为空壳 |

### 3.2 `Kernel` 枚举 `cfg` 展开

```rust
pub enum Kernel {
    Naive,                              // 始终存在
    #[cfg(target_arch = "x86_64")] Avx2,
    #[cfg(target_arch = "x86_64")] Avx512,
    #[cfg(target_arch = "aarch64")] Neon,
    #[cfg(feature = "simd_portable")] Portable,
}
```

`IMPLS` 常量通过 `cfg(all(target_arch, feature))` 组合展开为 6 路：

```rust
// x86_64 + simd_portable
const IMPLS: [Kernel; 4] = [Avx512, Avx2, Portable, Naive];
// x86_64 无 simd_portable
const IMPLS: [Kernel; 3] = [Avx512, Avx2, Naive];
// aarch64 + simd_portable
const IMPLS: [Kernel; 3] = [Neon, Portable, Naive];
// aarch64 无 simd_portable
const IMPLS: [Kernel; 2] = [Neon, Naive];
// 其他架构 + simd_portable
const IMPLS: [Kernel; 2] = [Portable, Naive];
// 其他架构无 simd_portable
const IMPLS: [Kernel; 1] = [Naive];
```

### 3.3 设计原则

1. **新增内核 = 1 枚举 + 1 `is_available` + 1 `distance` + 1 `IMPLS` 条目**：扩展成本固定。
2. **`neon.rs` 跨平台可编译**：非 aarch64 上编译为 `naive` 委托，使 `test_naive_vs_neon_consistency` 在任意架构执行。
3. **`portable.rs` 评估占位**：`simd_portable` feature 当前无实际 SIMD 代码，仅为 bench 接线预留。

---

## 4. NEON vs AVX2 对等实现

### 4.1 寄存器宽度与循环结构

| 特性 | AVX2 | NEON | 备注 |
|---|---|---|---|
| 寄存器宽度 | 256-bit (8 x f32) | 128-bit (4 x f32) | 架构固有差异 |
| 主循环宽度 | 16 f32/iter (双累加器) | 8 f32/iter (双累加器) | 均为寄存器宽度 x2 |
| 尾部处理 | 8-wide YMM → 标量 | ≤7 标量 | AVX2 多一级 YMM tail |

### 4.2 指令级对比

| 操作 | AVX2 | NEON | 评估 |
|---|---|---|---|
| 加载 | `_mm256_loadu_ps` (unaligned) | `vld1q_f32` (unaligned) | 等价 |
| FMA | `_mm256_fmadd_ps(d,d,acc)` | `vfmaq_f32(acc,d,d)` | 等价 |
| 减法 | `_mm256_sub_ps` | `vsubq_f32` | 等价 |
| 绝对值 | `_mm256_andnot_ps(sign,d)` | `vabsq_f32(d)` | NEON 单指令更简洁 |
| 水平归约 | 5 步: extract128 → add → hadd → hadd → cvtss | `vaddvq_f32` **单指令** | **NEON 更优** |
| 预取 | `_mm_prefetch` (stable) | `prfm pldl1keep` (inline asm) | NEON 需绕过 unstable `_prefetch` |

### 4.3 双累加器展开（共同优化）

两者均采用双累加器展开，核心思路：

```rust
// 伪代码：双累加器主循环
let mut acc0 = zero;  // 处理 [i, i+4)
let mut acc1 = zero;  // 处理 [i+4, i+8)
while i + 8 <= len {
    acc0 = fma(acc0, load(a[i..i+4]), load(b[i..i+4]));
    acc1 = fma(acc1, load(a[i+4..i+8]), load(b[i+4..i+8]));
    i += 8;
}
let sum = hadd(acc0) + hadd(acc1);
```

**收益**：FMA 延迟 4 周期，双累加器使两条独立 FMA 指令可流水线并行，理论吞吐提升 ~2x（受限于加载端口）。

### 4.4 NEON 预取方案

`std::arch::aarch64::_prefetch` 需要 unstable feature `stdarch_aarch64_prefetch` (#117217)。替代方案：

```rust
#[inline(always)]
unsafe fn prefetch_l1(ptr: *const f32) {
    std::arch::asm!(
        "prfm pldl1keep, [{ptr}]",
        ptr = in(reg) ptr,
        options(nostack, readonly),
    );
}
```

`prfm pldl1keep` 将缓存行预取到 L1，与 AVX2 的 `_MM_HINT_T0` 语义一致。inline asm 是 stable Rust 下的唯一选择，待 `stdarch_aarch64_prefetch` stable 后可替换为 `_prefetch::<_PREFETCH_READ, _PREFETCH_LOCALITY3>`。

---

## 5. 各内核实现细节

### 5.1 `avx2.rs` — AVX2+FMA

- **target_feature**: `avx2,fma`
- **主循环**: 16 f32/iter（双 `__m256` 累加器）
- **8-wide tail**: `_mm256_loadu_ps` + `_mm256_mul_ps` + `horizontal_sum`
- **预取**: `_mm_prefetch(..., _MM_HINT_T0)` 在 `i+32` 处（4 cache line ahead）
- **L1**: `_mm256_andnot_ps(sign, d)` 实现绝对值
- **Cosine**: 6 个累加器（dot0/na0/nb0 × 2），单 pass

### 5.2 `avx512.rs` — AVX-512F

- **target_feature**: `avx512f,avx2`（需显式声明 avx2 因 tail 用 `_mm256_*`）
- **主循环**: 16 f32/iter（单 `__m512` 累加器，ZMM 宽度已足够）
- **8-wide tail**: 复用 `_mm256_*` intrinsics + `horizontal_sum512`
- **水平归约**: ZMM → YMM → XMM 三级，与 `avx2::horizontal_sum` 保持相同舍入
- **预取**: `_mm_prefetch(..., _MM_HINT_T0)` 在 `i+64` 处（4 ZMM = 256 bytes）

### 5.3 `neon.rs` — ARM NEON

- **target_feature**: `neon`
- **主循环**: 8 f32/iter（双 `float32x4` 累加器）
- **标量 tail**: ≤7 个 f32 逐元素处理
- **预取**: inline asm `prfm pldl1keep`（stable 替代方案）
- **绝对值**: `vabsq_f32(d)` 单指令（比 AVX2 的 `andnot` 更直接）
- **水平归约**: `vaddvq_f32` 单指令（比 AVX2 的 5 步归约更高效）

### 5.4 `portable.rs` — `std::simd` 评估占位

- **feature gate**: `simd_portable`（`Cargo.toml` 声明）
- **当前状态**: 委托 `naive`，无实际 SIMD 代码
- **定位**: 为 `std::simd` stable 或 `wide` crate 采用预留的评估分支
- **约束**: bench 对比 `portable` vs `avx2`/`avx512`，误差需 < 1e-4 才可激活

---

## 6. 正确性验证

### 6.1 测试矩阵

| 测试 | 覆盖范围 | 通过条件 |
|---|---|---|
| `test_naive_vs_avx2_consistency` | dim ∈ {1,2,7,8,15,16,128,1025} × 20 随机 × 4 metric | 误差 < 1e-4 × max(\|expected\|, 1) |
| `test_naive_vs_avx512_consistency` | dim ∈ {1,7,8,15,16,31,32,128,384,768,1025,1536} × 20 随机 × 4 metric | 同上 |
| `test_naive_vs_neon_consistency` | dim ∈ {1,7,8,15,16,128,1025} × 20 随机 × 4 metric | 同上（跨架构执行） |
| `test_naive_vs_portable_consistency` | dim ∈ {1,7,8,15,16,128,384,1025} × 20 随机 × 4 metric | 同上 |
| `test_all_kernels_agree` | 所有可用内核交叉对比，dim ∈ {8,31,128,1536} | 同上 |
| `test_zero_vector_cosine_boundary` | 零向量边界（denom=0 → distance=1.0） | 精确相等 |
| `test_selected_kernel` | `selected()` 返回最高可用内核 | 枚举相等 |

### 6.2 误差容忍

- **FMA 舍入**: 累加顺序差异导致 ~1e-6 级别偏差，阈值设为 `1e-4 × max(|expected|, 1)`。
- **零向量边界**: Cosine 的 `denom=0 → 1.0` 分支在所有内核上精确一致。

---

## 7. 编译配置

### 7.1 基线模式（默认）

```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "target-cpu=x86-64"]
```

基线 `x86-64` 生成 portable 二进制，运行时通过 `is_x86_feature_detected!` 选优。

### 7.2 专用部署模式

```bash
# 极致性能（绑定硬件）
RUSTFLAGS="-C target-cpu=native" cargo build --release

# 兼容 AVX2 硬件
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release
```

这些构建仍受益于运行时分发，额外获得编译器自动向量化。

### 7.3 SIMD feature 门控

```bash
# 启用 portable SIMD 评估分支
cargo bench -p vector-search --features simd_portable --bench vector_scan_bench
```

---

## 8. Bench 准入阈值

| 条件 | 门槛 |
|---|---|
| AVX-512 vs AVX2 延迟提升 | > 15% across dim=384/768/1536 |
| p99 延迟回归 | 不允许 |
| `portable` vs 手写内核误差 | < 1e-4 |
| 合并决策 | `cargo bench --bench vector_scan_bench` 同机复测 |

---

## 9. 已知限制与后续方向

| 项 | 现状 | 后续 |
|---|---|---|
| NEON 预取 | inline asm `prfm`，待 `stdarch_aarch64_prefetch` stable | 替换为 `_prefetch` intrinsic |
| NEON SVE | 未实现 | `kernel.rs` 追加 `Sve` 变体，`distance/sve.rs` |
| AVX-512 VNNI/BF16 | 未区分，仅检测 `avx512f` | 量化落地后按需扩展 |
| `portable` 实际 SIMD | 当前委托 naive | `std::simd` stable 或 `wide` crate 采用后激活 |
| 多累加器 AVX-512 | 单 ZMM 累加器（16-wide 已足够） | 按需展开为双累加器 |

---

## 10. 文件索引

| 文件 | 行数 | 职责 |
|---|---|---|
| `distance/kernel.rs` | ~180 | Kernel 枚举 + 分发 + IMPLS + 测试覆盖 |
| `distance/avx2.rs` | ~180 | AVX2+FMA 四核（L2/Dot/Cosine/L1）双累加器 |
| `distance/avx512.rs` | ~297 | AVX-512F 四核，ZMM 主循环 + YMM tail |
| `distance/neon.rs` | ~185 | NEON 四核双累加器 + inline asm 预取 |
| `distance/portable.rs` | ~84 | `std::simd` 评估占位（委托 naive） |
| `distance/naive.rs` | ~60 | 标量基线，正确性参考 |
| `distance/mod.rs` | ~410 | 公共 API + 12 个单元测试 |
