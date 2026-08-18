# SIMD 距离核派发改造方案：tantivy 借鉴评估 + 现状修正

> 状态：实施方案（2026-08-18）
>
> 上游：
> - [simd-optimization-implementation.md](../../archive/simd-optimization-implementation.md)（Phase 0 全局 SIMD 落地）
> - `docs/plan/vector/phase_a_implementation.md`（vector-search W4 距离核）
>
> 参照实现：`crates/tantivy/bitpacker/src/filter_vec/mod.rs`（quickwit 的指令集派发结构）
>
> 说明：本文是**改动级**实施方案，解决上一轮 SIMD 分析发现的 5 个问题点，
> 并给出「是否借鉴 tantivy 派发设计以扩大优化适用范围」的评估与决策。
> 涉及现有代码位置均标注 `路径:行号`。

## 0. 结论摘要

| 项 | 决策 |
|----|------|
| 全局 `-C target-cpu=x86-64-v3` | **保留**；修正文档表述：「v3 构建的二进制**要求** AVX2 硬件，老 CPU 必须用 fallback 构建（`RUSTFLAGS="-C target-cpu=x86_64"`），运行时检查不是 v3 构建下的保命符」 |
| `horizontal_sum` 缺 `#[target_feature]`（`avx2.rs:16`） | **修复**：补 `#[target_feature(enable = "avx2")]`，消除仅靠内联才能编译的隐式假设 |
| 差分测试零范数盲区（`mod.rs:115`） | **修复**：补零向量 cosine 用例 |
| 派发结构 | **借鉴 tantivy 的部分设计**：新增 `Kernel` 枚举（cfg 门控变体）+ 首选序 `IMPLS` + `is_available()` + `OnceLock` 单点缓存 + `distance()` 单次原子读派发（约 50 行） |
| 调试/诊断能力 | 新增 `selected()` 公开接口（可日志输出当前内核）+ `#[cfg(test)] force()` 覆写，为差分调试提供外部手段 |
| 多架构（NEON/SVE） | **不落地**；枚举结构已预留扩展点，aarch64 成为目标平台时每个新架构 = 1 模块 + 1 变体 + 1 IMPLS 项 + 1 match 臂 |
| AVX-512 | **不落地**；与 v3 全局基线兼容（`#[target_feature]` + 运行时检测），待实测确认 kernel 是瓶颈后再立项 |
| filter/payload 求值 SIMD 化 | Backlog；与本次派发改造正交，搜索主循环的实际瓶颈在此而非 kernel |

## 1. 现状回顾与问题点

现状是「两层」SIMD 策略：

1. **全局编译级**（`.cargo/config.toml:14-15`）：`[target.x86_64-unknown-linux-gnu] rustflags = ["-C", "target-cpu=x86-64-v3"]`，整个 workspace（含 tantivy）自动向量化，Phase 0 验证 3.46x。
2. **显式 intrinsic 级**（`crates/vector-search/src/distance/`）：`avx2.rs` 三个核（L2/Dot/Cosine，`#[target_feature(enable = "avx2,fma")]`），`mod.rs:17` cfg 门控模块，`mod.rs:24-30` 运行时 `is_x86_feature_detected!` 双检查派发，naive 兜底；差分测试保证一致性。

分析发现的 5 个问题点：

| # | 问题 | 位置 | 严重度 |
|---|------|------|--------|
| P1 | **虚假安全**：v3 全局编译下整个二进制都以 AVX2 为前提，运行时回落 naive 实际上从不触发（老 CPU 会在别处先 SIGILL），文档却暗示 fallback 可用 | `.cargo/config.toml:5-8`、`AGENTS.md:48-50`、`README.md:59-62`、`avx2.rs:3-5` | 高（误导） |
| P2 | `horizontal_sum` 未标 `#[target_feature]`，却含 AVX intrinsic（`_mm256_extractf128_ps` 等），仅靠 `#[inline]` 内联进 target_feature 函数才合法 | `avx2.rs:16-23` | 中（脆点） |
| P3 | Manhattan 分支只是把标量循环搬进 avx2 模块，未向量化 | `avx2.rs:121-127` | 低（接受，该 metric 创建时被拒） |
| P4 | `distance()` 派发无 `#[inline]`，naive 循环无法内联进 rayon 热循环 | `mod.rs:24` | 低 |
| P5 | 差分测试只覆盖随机非零向量，零范数 cosine 分支（naive 与 avx2 都有 `denom==0 → 1.0`）未在 avx2 路径验证 | `mod.rs:115-150` | 中 |

## 2. tantivy 设计借鉴评估

### 2.1 tantivy 的设计概要

`crates/tantivy/bitpacker/src/filter_vec/mod.rs`：

```rust
enum FilterImplPerInstructionSet {   // cfg 门控变体：AVX2 / SVE / Neon / Scalar
    AVX2 = 0, SVE = 3, Neon = 2, Scalar = 1,
}
impl FilterImplPerInstructionSet {
    fn is_available(&self) -> bool;              // 每个实现自己的运行时检测
    fn filter_vec_in_place(self, ...);           // match 派发（cfg 门控 match 臂）
}
// 每架构一个「首选序」常量数组
const IMPLS: [FilterImplPerInstructionSet; 2] = [AVX2, Scalar];          // x86_64
const IMPLS: [FilterImplPerInstructionSet; 3] = [SVE, Neon, Scalar];     // aarch64(非 Apple)
const IMPLS: [FilterImplPerInstructionSet; 2] = [Neon, Scalar];          // aarch64(Apple)
// 单点缓存：AtomicU8 首次取最佳可用实现后缓存，后续仅一次 relaxed load
fn get_best_available_instruction_set() -> FilterImplPerInstructionSet;
```

核心价值：**派发与实现解耦**——调用方永远只面对一个函数，实现集（架构 × 指令集）通过
「1 个 enum 变体 + 1 个 is_available + 1 个 IMPLS 项 + 1 个 match 臂」增量扩展；
检测只做一次并缓存；每架构有独立的优选顺序。

### 2.2 借鉴价值对照

| 维度 | 现状（vector-search） | tantivy 设计 | 借鉴收益 |
|------|----------------------|--------------|----------|
| 运行时检测 | 每点每调用 `is_x86_feature_detected!` 两次（rustc 内部已静态缓存，实际是两次 relaxed load，开销≈0） | 一次初始化 + 单次 relaxed load | 边际（主要收益在结构而非性能） |
| 扩展新指令集（AVX-512） | 需改 `mod.rs` 派发 + 新增模块，派发点逻辑膨胀 | 新增模块 + 1 变体 + 1 IMPLS 项 + 1 match 臂，调用点零改动 | **显著** |
| 扩展新架构（NEON/SVE） | 无结构支撑 | 天然支持（多 IMPLS 常量按 cfg 选择） | **显著** |
| 测试性 | 差分测试直接调 avx2，无法从外部强制走某路径 | 可用 `is_available()` 查询、枚举值驱动 | 中等 |
| 诊断性 | 无法得知运行中实际用了哪个内核 | `selected()` 可打印 | 中等 |
| 持久化 code（tantivy 的 `from(code)` 用于读磁盘列的 impl 标记） | 无持久化需求 | — | 不借鉴 |

### 2.3 决策

**借鉴**（约 50 行，价值/成本比高）：
- `Kernel` 枚举 + cfg 门控变体；
- 每架构首选序 `IMPLS` 常量 + `is_available()`；
- `OnceLock`（等价 tantivy 的 AtomicU8）单点缓存最佳可用实现；
- `distance()` = 单次读 + match 派发 + `#[inline]`（一并解决 P4）；
- 公开 `selected()` + `#[cfg(test)] force()`。

**不借鉴**：
- 多架构实现矩阵（SVE/Neon 实现体）：当前唯一构建目标为 x86_64 Linux（`.cargo/config.toml` 仅配置该 target），写未经验证的架构代码违反项目「低维护面」原则；
- `from(code)`/序列化标记：vector-search 无磁盘持久化指令集选择的需求。

## 3. 具体修改方案

### 3.1 新增 `distance/kernel.rs`（新文件，~50 行）

```rust
//! SIMD kernel selection, mirroring tantivy's per-instruction-set dispatch
//! (crates/tantivy/bitpacker/src/filter_vec/mod.rs): an enum of kernel
//! variants (cfg-gated), a preferred-order per-arch list, a single cached
//! best-available selection, and a tiny dispatch.

use std::sync::OnceLock;

use crate::types::DistanceMetric;

use super::{avx2, naive};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kernel {
    Naive = 0,
    #[cfg(target_arch = "x86_64")]
    Avx2 = 1,
}

impl Kernel {
    pub fn is_available(self) -> bool {
        match self {
            Kernel::Naive => true,
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => {
                std::arch::is_x86_feature_detected!("avx2")
                    && std::arch::is_x86_feature_detected!("fma")
            }
        }
    }

    #[inline]
    pub fn distance(self, metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Kernel::Naive => naive::distance(metric, a, b),
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => unsafe { avx2::distance(metric, a, b) },
        }
    }
}

/// Preferred order per architecture (mirrors tantivy's IMPLS).
#[cfg(target_arch = "x86_64")]
const IMPLS: [Kernel; 2] = [Kernel::Avx2, Kernel::Naive];
#[cfg(not(target_arch = "x86_64"))]
const IMPLS: [Kernel; 1] = [Kernel::Naive];

fn best_available() -> Kernel {
    IMPLS.into_iter().find(|k| k.is_available()).unwrap_or(Kernel::Naive)
}

static SELECTED: OnceLock<Kernel> = OnceLock::new();

/// The kernel active for this process (first call detects and caches).
pub fn selected() -> Kernel {
    *SELECTED.get_or_init(best_available)
}

/// Override for differential debugging; tests only.
#[cfg(test)]
pub fn force_for_test(kernel: Kernel) {
    let _ = SELECTED.set(kernel);
}
```

要点：
- `OnceLock` 语义等价 tantivy 的 `AtomicU8` 缓存（更简单，无需处理 `u8::MAX` 哨兵）；
- 每个未来新增内核 = 1 变体 + 1 `is_available` 臂 + 1 `distance` 臂 + IMPLS 插序，调用点零改动；
- `force_for_test` 弥补现状「无法从外部强制 naive 路径」的调试缺口（解决差分验证的对称性问题）。

### 3.2 改写 `distance/mod.rs` 派发（解决 P4）

`mod.rs:24-30` 的 `distance()` 改为：

```rust
#[inline]
pub fn distance(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    kernel::selected().distance(metric, a, b)
}
```

并 `pub mod kernel;`（或 `mod kernel;` + 重导出）。同时保留 naive/avx2 模块的
`pub` 导出不变（差分测试直接调 naive/avx2 的现有结构不动）。

### 3.3 `avx2.rs` 修正（解决 P2）

`horizontal_sum` 补 `#[target_feature(enable = "avx2")]`（其使用的
`_mm256_extractf128_ps`/`_mm256_castps256_ps128` 属 AVX），并保留 `#[inline]`，
消除「仅靠内联才合法」的隐式假设；`# Safety` 文档注明由调用方保证特性启用。

### 3.4 差分测试补零范数用例（解决 P5）

`mod.rs` 的 `test_naive_vs_avx2_consistency` 在随机用例之外追加固定用例：
`a=全零 / b=全零 / a=b=全零` × 三种 metric，naive vs avx2 断言一致
（cosine 零范数分支 `denom==0 → 1.0` 双路径都要走到）。
另新增 `test_selected_kernel`：断言 `selected()` 在 AVX2+FMA 机器上为 `Avx2`、
否则为 `Naive`；`force_for_test(Kernel::Naive)` 后 `distance()` 结果与 avx2 一致。

### 3.5 文档修正（解决 P1）

| 文件 | 现状表述 | 修正为 |
|------|----------|--------|
| `.cargo/config.toml:5-8` | fallback 表述含糊（「Fallback to the baseline x86-64 target on older CPUs」） | 明确：**v3 构建的二进制要求 AVX2 硬件**；老 CPU 必须用 fallback 命令完整重建；运行时内核检查只对 fallback 构建有意义 |
| `AGENTS.md:48-50` | 同左 | 同上，补一句「x86-64-v3 不是运行时自适应」 |
| `README.md:59-62` / `README_zh.md:59-62` | 同左 | 同上 |
| `avx2.rs:1-9` 模块头注释 | 「v3 target makes this the always-hit path」 | 补：always-hit 仅对 v3 构建成立；fallback 构建下由运行时检查保护 |

### 3.6 启动诊断（可选，建议做）

engine 初始化时 `info!("vector distance kernel: {:?}", distance::kernel::selected())`，
便于线上确认实际内核（与 P1 的「v3 必中」陈述互相印证）。

> 执行状态：`Kernel` 已实现 `Display`（`kernel.rs`），日志调用随 W5 引擎
> 接口落地时补上（当前 vector-search 无 engine 初始化点）。

## 4. 适用范围扩展分析（借鉴后能获得什么）

「增加优化的适用范围」有三个维度，本方案的结构性收益与决策如下：

### 4.1 架构维度（aarch64 / Apple Silicon）

- 现状：`#[cfg(target_arch = "x86_64")]` 门控下，aarch64 全程 naive——本地开发若换 Mac/ARM 服务器，距离核零加速。
- 借鉴后：新增 `neon.rs`（L2/Dot/Cosine 三核）+ 1 变体 + 1 `is_available`（aarch64 恒真）+ 1 match 臂 + IMPLS 项，调用点零改动；每个架构约 1 个模块的增量。
- **决策**：不落地。理由：当前唯一构建 target 是 x86_64 Linux（`.cargo/config.toml` 未配置 aarch64），
  全局自动向量化同样不覆盖 aarch64，先有部署需求再补。列为条件任务。

### 4.2 指令集维度（AVX-512）

- 现状：v3 全局基线**不含** AVX-512（v3 = AVX2+BMI1/2+F16C+FMA+LZCNT+...），
  因此 LLVM 不会发射 zmm 指令；补 `avx512.rs` 用 `#[target_feature(enable = "avx512f")]` +
  运行时检测，与 v3 构建完全兼容（与当前 avx2 模式同构）。
- 收益预期：kernel 提速 ~1.5-2x，但 Tier 0 搜索主循环是**内存带宽瓶颈**（mmap 全扫 + payload 装载），
  kernel 提速对端到端 top-K 影响有限；且老 Skylake-X 有降频顾虑。
- **决策**：不落地；枚举结构已就位，若 kernel 实测成为瓶颈（bench 数据支撑）再立项。

### 4.3 代码路径维度（filter / payload 求值）

- 真正的端到端瓶颈在每点 `serde_json` payload 装载与 filter 求值（`filter.rs`），
  SIMD 化空间大于 kernel 本身；但这是独立的优化域，与派发结构正交。
- **决策**：Backlog 立项，不在本方案内。

### 4.4 结论

tantivy 设计的价值**不在立即扩大指令集覆盖**，而在把派发从「固定双路径 if」
变成「可插拔实现集」：优化适用范围（架构 × 指令集 × 可测试性 × 可诊断性）
的每次扩张都收敛为 4 处小增量，且不回归调用点。本方案先行铺设该结构，
架构/指令集扩张按条件任务触发。

## 5. 验收标准

1. `cargo test -p vector-search` 全过（含新增零范数差分用例与 `test_selected_kernel`）；
2. `cargo clippy -p vector-search --all-targets` 0 警告；
3. 手工验证：AVX2 机器上 `selected() == Avx2`；`force_for_test(Naive)` 后搜索结果与默认路径逐点一致；
4. （可选）`objdump` 确认 avx2 路径仍含 `vfmadd*`/`vsubps` 指令；
5. 文档修正落地（§3.5 四份文件）。

## 6. 文件清单与执行顺序

```
crates/vector-search/src/distance/kernel.rs   # 新增：Kernel 枚举 + IMPLS + 缓存 + 派发
crates/vector-search/src/distance/mod.rs      # distance() 改为 kernel::selected() 派发；pub mod kernel
crates/vector-search/src/distance/avx2.rs     # horizontal_sum 补 target_feature；头注释修正
crates/vector-search/src/distance/naive.rs    # 不动
crates/vector-search/src/distance/mod.rs      # 差分测试补零范数用例 + test_selected_kernel
crates/vector-search/src/engine.rs(或等价初始化点) # 启动日志打印 selected()（§3.6）
.cargo/config.toml                            # fallback 表述修正
AGENTS.md / README.md / README_zh.md          # SIMD 说明修正
```

执行顺序：3.3 → 3.4 → 3.1/3.2 → 3.6 → 3.5 → 验收。

## 7. Backlog（条件任务）

| 任务 | 触发条件 | 预估增量 |
|------|----------|----------|
| aarch64 NEON 三核 | 出现 mac/ARM 部署需求 | 1 模块 + 4 处小增量 |
| AVX-512 核 | kernel 实测为搜索瓶颈（bench 证据） | 同上 |
| filter/payload 求值 SIMD 化 | 独立立项（§4.3） | 与本文正交 |