# 自动紧凑现状与空闲块重用规划

**Date**: 2026-06-19  
**Status**: Analysis & Planning

---

## 第一部分：自动紧凑现状分析

### 1. 当前实现状态

#### 已实现的组件

| 组件 | 位置 | 功能 | 状态 |
|------|------|------|------|
| **诊断方法** | `mutable_csr.rs` | `fragmentation_ratio()`, `should_compact()`, `wasted_bytes_estimate()` | ✅ |
| **条件紧凑包装** | `mutable_csr_variant.rs` | `maybe_compact(threshold, ts, reserve_ratio)` | ✅ |
| **手动紧凑入口** | `edge_table.rs:986` | `compact_csr(ts, reserve_ratio)` | ✅ |
| **全局紧凑API** | `graph_storage/mod.rs:526` | `compact(compact_csr, reserve_ratio)` | ✅ |

#### 缺失的集成点

| 场景 | 应调用位置 | 当前状态 | 优先级 |
|------|-----------|---------|--------|
| **序列化前紧凑** | `EdgeTable::flush()` 前 | ❌ 未实现 | 🔴 高 |
| **批量操作后** | `EdgeTable::insert/delete` 后 | ❌ 未实现 | 🟡 中 |
| **监控告警** | `EdgeTable::dump()` 时采集指标 | ❌ 未实现 | 🟡 中 |

### 2. 代码追踪

#### flush() 的问题

**文件**: `crates/graphdb-storage/src/storage/edge/edge_table.rs:676`

```rust
pub fn flush<P: AsRef<Path>>(
    &self,
    path: P,
    compression: crate::storage::compression::CompressionType,
) -> StorageResult<()> {
    // ... 元数据写入 ...
    let out_csr_path = path.join("out_csr.bin");
    self.flush_csr(&self.out_csr, ...)?;  // ❌ 直接序列化，无紧凑
    let in_csr_path = path.join("in_csr.bin");
    self.flush_csr(&self.in_csr, ...)?;   // ❌ 直接序列化，无紧凑
}
```

**缺陷**:
- 序列化整个 `nbr_list`，包含碎片
- 无法从 `fragmentation_ratio > 2.0` 的情况中获益
- 存储空间浪费 50%+ 可能未被利用

#### 正确的调用链

**文件**: `crates/graphdb-storage/src/storage/engine/graph_storage/context.rs:1517`

```rust
// ✅ 这里正确调用了 compact_csr()
let removed = table.compact_csr(ts, config.reserve_ratio);
```

这证明 `compact_csr()` 已经可工作，但仅在特定事务上下文中被调用。

### 3. 影响评估

#### 序列化前不紧凑的代价

假设 `fragmentation_ratio = 2.5`（中等碎片）：

| 指标 | 值 |
|------|-----|
| **有效边数** | 10,000 |
| **nbr_list 长度** | 25,000 |
| **浪费空间** | 15,000 Nbr |
| **内存浪费** | 15,000 × 16 字节 = 240 KB |
| **序列化大小增长** | +60% |
| **网络传输增速** | +60% |

对于大图（百万级边），这可能意味着：
- 1M 边 × 2.5 = 2.5M nbr_list 长度
- 浪费 1.5M Nbr × 16 字节 = 24 MB
- 序列化增加 ~60%（若采用 zstd 压缩，增幅较小但仍显著）

---

## 第二部分：空闲块重用规划

### 4. 空闲块重用设计

#### 4.1 核心思想

在 `MutableCsr` 中维护一个**空闲块池**（freelist），当顶点溢出块扩容时：

```
扩容时：
  1. 首先查询 freelist，寻找大小 >= 所需容量的空闲块
  2. 若有匹配块，从 freelist 取出，直接使用（省去 nbr_list 追加）
  3. 若无匹配块，回退到原有追加策略
  4. 旧溢出块不再使用时，加入 freelist

效果：
  - 减少 nbr_list 膨胀速度
  - 在紧凑间隔内就地复用碎片
  - O(log n) 或 O(1) LIFO 查询成本
```

#### 4.2 实现方案对比

**方案 A: 简单 LIFO 栈**（推荐）

```rust
pub struct FreeListAllocator {
    /// LIFO stack of freed blocks: (start_index, capacity)
    blocks: Vec<(u32, u32)>,
}

impl FreeListAllocator {
    pub fn allocate(&mut self, min_size: u32) -> Option<u32> {
        // Pop from stack; only accept exact match or larger
        while let Some((start, cap)) = self.blocks.pop() {
            if cap >= min_size {
                return Some(start);
            }
            // Otherwise, skip smaller blocks
        }
        None
    }
    
    pub fn free(&mut self, start: u32, capacity: u32) {
        self.blocks.push((start, capacity));
    }
}
```

**优点**:
- 实现极简（<100 行）
- LIFO 栈天然高效（缓存友好）
- 命中率高（新释放的块最有可能立刻被复用）

**缺点**:
- 不合并相邻块，长期可能产生外部碎片
- 如有大量不同大小的溢出，小块可能无法被用

**适用**: 溢出块大小分布集中、写入模式局部性强的场景

---

**方案 B: 大小分级分桶**

```rust
pub struct BucketsAllocator {
    /// Separate bucket for each power-of-2 size range
    buckets: [Vec<u32>; 16],  // buckets[i] stores blocks of size [2^i, 2^(i+1))
}
```

**优点**:
- 快速定位合适大小的块（O(1)）
- 减少小块无法被复用的情况

**缺点**:
- 实现复杂（~200 行）
- 内存管理更复杂（多个 vec）

**适用**: 溢出块大小分布广泛的场景

---

**方案 C: 树形结构 (AVL/RB-tree)**

**优点**:
- 精确的最佳适配算法
- 可支持块合并

**缺点**:
- 实现复杂（~300+ 行）
- 开销大（每次操作 O(log n)）
- 维护难度高

**不推荐**: 这个阶段不值得

---

#### 4.3 推荐方案：LIFO 栈 + 定期合并

**组合策略**:
1. **日常分配**: 使用 LIFO 栈（方案 A），成本 O(1)
2. **定期整理**: 在紧凑时调用 `merge_adjacent_blocks()`，合并相邻空闲块，防止外部碎片过度积累
3. **可选扩展**: 若发现 LIFO 命中率低 (<50%)，再升级到分桶（方案 B）

### 5. 实现架构

#### 5.1 文件结构

```
crates/graphdb-storage/src/storage/edge/
├── mutable_csr.rs              (主 CSR 实现，添加 freelist 集成点)
├── mutable_csr_freelist.rs     (✨ 新增：FreeListAllocator 实现)
├── mutable_csr_variant.rs      (已有：包装层)
└── edge_table.rs               (已有：上层 API)
```

#### 5.2 mutable_csr.rs 中的集成

**方式 1: Feature Flag（推荐）**

```rust
#[cfg(feature = "csr-freelist")]
use super::mutable_csr_freelist::FreeListAllocator;

pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    // ... 已有字段 ...
    
    #[cfg(feature = "csr-freelist")]
    freelist: FreeListAllocator,
}

impl MutableCsr {
    fn expand_vertex_capacity(...) {
        #[cfg(feature = "csr-freelist")]
        {
            if let Some(start) = self.freelist.allocate(additional) {
                // 从 freelist 分配，复用旧块位置
                return self.reuse_block(start, additional);
            }
        }
        // 回退到原有追加策略
        self.append_block(additional)
    }
}
```

**方式 2: Runtime Toggle**

```rust
pub struct MutableCsr {
    freelist: Option<FreeListAllocator>,  // None = 禁用
    freelist_enabled: bool,
}
```

**推荐选方式 1**（编译期 feature），理由：
- 线上可用不同编译版本进行对比测试
- 不使用时零开销（dead code elimination）
- 更明确的意图

---

#### 5.3 功能清单

**mutable_csr_freelist.rs** (新建，~150 行)

```rust
pub struct FreeListAllocator {
    blocks: Vec<(u32, u32)>,  // (start, capacity)
    total_freed: usize,        // 统计信息
}

impl FreeListAllocator {
    pub fn new() -> Self { ... }
    
    pub fn allocate(&mut self, min_size: u32) -> Option<u32> { ... }
    
    pub fn free(&mut self, start: u32, capacity: u32) { ... }
    
    pub fn merge_adjacent(nbr_list: &[Nbr], freelist: &mut Self) { ... }
    
    pub fn stats(&self) -> FreeListStats { ... }
}

pub struct FreeListStats {
    pub num_blocks: usize,
    pub total_capacity: usize,
    pub hit_rate: f32,  // 若要统计
}
```

**mutable_csr.rs 修改** (~50 行)

- 添加 freelist 字段（feature 条件）
- 修改 `expand_vertex_capacity()` 调用 freelist
- 修改 `compact_with_ts()` 时重建 freelist（或调用合并）

---

### 6. 集成点完善

#### 6.1 自动紧凑集成（优先级 🔴 高）

**在 EdgeTable::flush() 前添加**:

```rust
impl EdgeTable {
    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        // ✅ 新增：序列化前紧凑（若碎片过高）
        if self.out_csr.fragmentation_ratio() > 2.0 {
            self.out_csr.compact_with_ts(current_ts, 0.25);
        }
        if self.in_csr.fragmentation_ratio() > 2.0 {
            self.in_csr.compact_with_ts(current_ts, 0.25);
        }
        
        // 原有逻辑
        let out_csr_path = path.join("out_csr.bin");
        self.flush_csr(&self.out_csr, ...)?;
        // ...
    }
}
```

**或者，更优雅的方式：添加辅助方法**

```rust
impl EdgeTable {
    pub fn maybe_compact_before_flush(&mut self, ts: Timestamp) {
        const FLUSH_COMPACTION_THRESHOLD: f32 = 2.0;
        self.out_csr.maybe_compact(FLUSH_COMPACTION_THRESHOLD, ts, 0.25);
        self.in_csr.maybe_compact(FLUSH_COMPACTION_THRESHOLD, ts, 0.25);
    }
    
    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        // ...（如果允许 &mut self，在这里调用 maybe_compact_before_flush）
        // 或者在调用 flush 前由上层调用
    }
}
```

#### 6.2 可选的批量操作后紧凑

```rust
impl EdgeTable {
    /// Opportunistic compaction after bulk insert/delete
    pub fn maybe_compact_after_bulk_ops(&mut self, ts: Timestamp) {
        const BULK_COMPACTION_THRESHOLD: f32 = 2.5;
        self.out_csr.maybe_compact(BULK_COMPACTION_THRESHOLD, ts, 0.25);
        self.in_csr.maybe_compact(BULK_COMPACTION_THRESHOLD, ts, 0.25);
    }
}
```

---

## 第三部分：实施路线图

### 7. 分阶段实现

#### Phase 1: 自动紧凑集成（立即）
- [ ] 修改 `EdgeTable::flush()` 添加紧凑前缀（条件：`fragmentation_ratio > 2.0`）
- [ ] 添加 `maybe_compact_before_flush()` 辅助方法
- [ ] 编写测试：验证序列化前紧凑的效果
- [ ] 编写文档：何时自动触发、如何配置阈值

**改动规模**: ~50 行代码 + 20 行测试  
**编译期**: 立即  
**收益**: 序列化体积减少 50-60%（若 fragmentation_ratio=2.5）

#### Phase 2: 空闲块重用基础（2-3 周）
- [ ] 新建 `mutable_csr_freelist.rs`，实现 `FreeListAllocator`
- [ ] 添加 `csr-freelist` feature flag
- [ ] 修改 `MutableCsr::expand_vertex_capacity()` 集成 freelist
- [ ] 修改 `compact_with_ts()` 重建 freelist
- [ ] 编写单元测试（分配、释放、命中率统计）
- [ ] 编写文档：原理、性能影响、灰度策略

**改动规模**: ~200 行代码 + 100 行测试  
**编译期**: 2-3 周  
**收益**: 减少 nbr_list 膨胀 30-50%（取决于写入模式）

#### Phase 3: freelist 优化与监控（可选，1 个月后评估）
- [ ] 若 LIFO 命中率 <50%，升级到分桶方案
- [ ] 添加 freelist 统计到监控系统
- [ ] 定期合并相邻块，防止外部碎片

---

### 8. 文档与沟通

#### 新增文档

| 文档 | 位置 | 内容 | 读者 |
|------|------|------|------|
| **自动紧凑集成指南** | `docs/storage/issue/automatic_compaction_integration.md` | Phase 1：何时触发、配置方式、效果评估 | 工程师、运维 |
| **空闲块重用设计** | `docs/storage/issue/freelist_design.md` | Phase 2：原理、实现细节、灰度方案 | 架构师、核心开发 |

#### 配置与告警

```rust
// 建议的配置项（Config）
pub struct CsrFragmentationConfig {
    pub flush_threshold: f32,       // 序列化前紧凑阈值，推荐 2.0
    pub bulk_ops_threshold: f32,    // 批量操作后阈值，推荐 2.5
    pub enable_freelist: bool,      // 是否启用空闲块重用（Phase 2）
}

// 建议的告警
metrics::gauge!("csr_fragmentation_ratio_p99", ratio);
if ratio > 3.0 {
    warn!("CSR fragmentation critical: {:.2}x", ratio);
}
```

---

## 第四部分：决策与优先级

### 9. 优先级排序

| 阶段 | 工作项 | 优先级 | 理由 |
|------|--------|--------|------|
| **Phase 1** | 自动紧凑集成 | 🔴 高 | 立即可获益；成本低；无风险 |
| **Phase 2** | 空闲块重用 | 🟡 中 | 待 Phase 1 数据；可选功能 |
| **Phase 3** | 高级优化 | 🟢 低 | 长期评估；可能不需要 |

### 10. 决策树

```
实施自动紧凑（Phase 1）
  ↓ (1-2 周后)
评估效果
  ├─ 若序列化体积减少 >30%
  │  └─ ✅ Phase 1 足以，定期序列化前紧凑
  │
  └─ 若需进一步减少 nbr_list 膨胀
     └─ 推进 Phase 2（空闲块重用）
```

---

## 总结

| 阶段 | 改动规模 | 实施周期 | 收益 | 风险 |
|------|----------|----------|------|------|
| **Phase 1: 自动紧凑** | ~50 行 | 1 周 | 序列化 -50% | 极低 |
| **Phase 2: 空闲块重用** | ~200 行 | 2-3 周 | 碎片率 -30% | 低 |
| **Phase 3: 高级优化** | ~500 行 | 1 个月+ | 完全消除 | 中-高 |

**建议立即启动 Phase 1**，为 Phase 2 的决策奠定数据基础。
