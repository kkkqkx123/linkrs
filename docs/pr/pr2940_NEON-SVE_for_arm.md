# PR #2940 文档总结：为 ARM 架构添加 NEON + SVE 向量化支持

## 一、PR 概述

| 项目 | 内容 |
|------|------|
| **PR 编号** | #2940 |
| **标题** | Add sve + neon filter vec implementation as spotted by Adam |
| **作者** | fulmicoton (Paul Masurel) |
| **代码变更** | +610 行 / -73 行，涉及 5 个文件 |
| **核心目的** | 为 `bitpacker` 库的 `filter_vec` 操作添加 ARM 平台的 SIMD 加速（NEON 和 SVE） |

## 二、修改的文件

| 文件 | 变更 | 说明 |
|------|------|------|
| `bitpacker/src/filter_vec/mod.rs` | +136 / -20 | 添加 NEON/SVE 调度逻辑，重构指令集检测 |
| `bitpacker/src/filter_vec/neon.rs` | +113 | **新增** NEON 实现（4路 SIMD） |
| `bitpacker/src/filter_vec/sve.rs` | +258 | **新增** SVE 实现（可扩展向量） |
| `bitpacker/benches/bench.rs` | +98 / -53 | 重构基准测试 |
| `bitpacker/Cargo.toml` | +5 | 添加 `proptest` 依赖 |

## 三、技术背景

### 什么是 `filter_vec`？
在位解包器（bitpacker）中，`filter_vec` 的功能是：**给定一个数值范围 `[start..=end]` 和一组打包存储的值，找出所有值在范围内的位置索引（id）**。

```rust
// 核心函数签名
fn filter_vec_in_place(range: RangeInclusive<u32>, offset: u32, output: &mut Vec<u32>)
```

- `output` 输入时包含原始数值，输出时被替换为符合条件的索引
- `offset` 用于计算全局索引（`index = offset + i`）

### 为什么需要这个功能？
在倒排索引、列式存储等场景中，需要快速过滤出满足条件的文档 ID 或行号，这个操作是性能热点。

## 四、新增实现的技术细节

### 4.1 NEON 实现（`neon.rs`）

**目标平台**：Apple M 系列芯片、ARMv8 设备

**核心策略**：每次处理 4 个 u32（128 位向量）

**关键算法**：

```
1. 加载 4 个值: [v0, v1, v2, v3]
2. 并行比较: v0 >= start && v0 <= end → inside[0] = 0xFFFFFFFF 或 0
3. 构建 4-bit mask: bit k = 1 表示第 k 个值在范围内
4. 根据 mask 查表重排，将匹配的索引紧凑到向量前部
5. 写入输出，更新输出指针
```

**关键代码片段**：

```rust
// 4-bit mask 计算（使用 bit_weights [1,2,4,8] 加权求和）
let inside = vandq_u32(ge_start, le_end);
let inside_bits = vandq_u32(bit_weights, inside);
let mask = vaddvq_u32(inside_bits) as u8;  // mask ∈ [0, 15]

// 查表紧凑化
let filtered_ids = compact(ids, mask);
```

**性能特点**：
- 固定 128 位向量，简单可靠
- 使用预计算的 `BYTE_SHUFFLE_TABLE`（16 种 mask 对应的重排模式）
- Apple M 系列上实测效果显著

### 4.2 SVE 实现（`sve.rs`）

**目标平台**：ARMv9、支持 SVE 的服务器级 ARM 芯片

**核心特点**：向量长度在**运行时**确定（128/256/512/2048 位），代码自适应

**实现难点**：Rust 稳定版没有 SVE intrinsics，必须使用内联汇编

**双向量循环优化**（Double Pump）：

```
每个迭代处理两个 SVE 向量（2 × VL 个 u32）
    ├─ 加载 word_a，word_b
    ├─ 计算 in_range_a，in_range_b
    ├─ compact_a，compact_b
    ├─ cntp_a，cntp_b  ← 两个计数指令独立，可乱序并行
    ├─ 写回 compacted_a
    ├─ 写回 compacted_b
    └─ 指针前进 2 × VL
```

**关键汇编代码**：

```asm
// 查询向量长度
cntw {vl_gpr}           // VL = 能装多少个 u32

// 两个 cntp 并行执行，打破延迟链
cntp {cnt_a}, p0, p1.s  // 计数匹配数量
cntp {cnt_b}, p0, p2.s

// 紧凑化（保留匹配的索引）
compact z5.s, p1, z0.s  // word_a 的匹配索引
compact z6.s, p2, z4.s  // word_b 的匹配索引
```

## 五、调度逻辑（`mod.rs`）

### 指令集优先级

| 平台 | 优先级 | 说明 |
|------|--------|------|
| x86_64 | AVX2 → Scalar | 已有实现 |
| Apple M (aarch64 + apple) | NEON → Scalar | Apple 不支持 SVE |
| 其他 ARM64 | SVE → NEON → Scalar | SVE 优先（如果硬件支持）|
| 其他架构 | Scalar | 仅标量回退 |

### 运行时检测 + 缓存

```rust
static INSTRUCTION_SET_BYTE: AtomicU8 = AtomicU8::new(u8::MAX);

fn get_best_available_instruction_set() -> FilterImplPerInstructionSet {
    // 首次调用时检测并缓存，后续直接返回
}
```

## 六、关于我 Fork 的库的考虑

### 是否需要合并这个 PR？

**需要评估以下几点：**

| 检查项 | 说明 |
|--------|------|
| **你的库是否使用了 `tantivy-bitpacker`？** | 如果依赖了该库，合入后可免费获得 ARM 性能提升 |
| **是否部署在 ARM 平台？** | Apple M 系列（Mac）、ARM 服务器、树莓派等 |
| **`filter_vec` 是否是性能热点？** | 可用 profiling 确认 |
| **你的库是否有独立的 `bitpacker` 模块？** | 如果是从上游复制/modified 的，需要同步修改 |

### 潜在风险

1. **MSRV 影响**：SVE 内联汇编可能需要较新的 Rust 版本
2. **测试覆盖**：Copilot 发现了 proptest 中的 bug（每个实现应该用原始数据副本），虽然不影响正确性但需要留意
3. **SVE 运行时检测**：`is_aarch64_feature_detected!("sve")` 在部分平台可能不可用

### 建议行动

```bash
# 1. 检查你的项目是否依赖 bitpacker
grep -r "bitpacker" Cargo.toml

# 2. 如果依赖，可以 cherry-pick 这个 PR
git remote add upstream https://github.com/quickwit-oss/tantivy.git
git fetch upstream pull/2940/head:pr-2940
git cherry-pick <commit-hash>  # 或合并整个分支

# 3. 在 ARM 设备上运行测试
cargo test --package tantivy-bitpacker

# 4. 运行 benchmark 对比性能
cargo bench --bench bench
```

### 如果不需要立即合并

可以关注上游合并后的稳定版本，等待 tantivy 发布新版本后升级依赖即可。

---

**总结**：这是一个成熟的性能优化 PR，为 ARM 平台补齐了 SIMD 加速。如果你的库在 ARM 设备上运行且 `bitpacker` 是热点，强烈建议合入或升级依赖。
