# Phase 2：空闲块重用（Free-List Allocator）设计文档

**Date**: 2026-06-19  
**Status**: Design (Ready for Implementation)  
**Target Timeline**: 2-3 weeks (optional, data-driven)

---

## 一、问题回顾与机会

### 1.1 当前 Phase 1 的限制

即使有 Phase 1 自动紧凑，仍存在覆盖缺口：

**场景：频繁写入间隔**

```
T0: 初始，无碎片
T1: 插入 100 条边 → fragmentation_ratio = 1.5
T2-T9: 8 次查询密集操作（无序列化）
T10: 再插入 100 条边 → fragmentation_ratio = 2.2
     （旧溢出块仍在 nbr_list 中，无法回收）
T11: 序列化，触发紧凑
     成本：O(V+E)，~20ms（取决于图大小）
```

在 T2-T9 这段期间：
- ❌ 无法回收已释放的溢出块
- ❌ nbr_list 继续膨胀
- ❌ 即使频繁序列化，也要等到 fragmentation_ratio > 2.0 才触发紧凑

### 1.2 空闲块重用的机会

若维护**空闲块池**（freelist）：

```
T1: 插入 100 条边，产生溢出块
    nbr_list: [主块] [overflow_v1] ...
    
T2-T4: 某些顶点被删除（逻辑删除，边失效）
    旧溢出块成为"可复用候选"
    
T5: 新顶点需要溢出块扩容
    ✅ freelist 中优先分配旧块，避免 nbr_list 追加
    
效果：
  - 减少 nbr_list 膨胀
  - 减少紧凑频率
  - 在线消化碎片，无需全局重排
```

---

## 二、设计方案：LIFO 栈 + 定期合并

### 2.1 核心数据结构

```rust
// 新文件：mutable_csr_freelist.rs

pub struct FreeListAllocator {
    /// LIFO 栈：(start_index, capacity)
    /// 最近释放的块在栈顶，优先复用
    blocks: Vec<(u32, u32)>,
    
    /// 统计信息（可选）
    total_freed: u64,     // 总释放大小
    total_reused: u64,    // 总复用大小
    num_merges: usize,    // 合并操作次数
}

impl FreeListAllocator {
    pub fn new() -> Self { ... }
    
    /// 分配一个 >= min_size 的块
    /// 优先使用 LIFO 栈顶（缓存友好）
    pub fn allocate(&mut self, min_size: u32) -> Option<u32> { ... }
    
    /// 释放一个不再使用的块到 freelist
    pub fn free(&mut self, start: u32, capacity: u32) { ... }
    
    /// 合并相邻的空闲块，防止外部碎片
    /// 需要 nbr_list 信息来判断相邻性
    pub fn merge_adjacent_blocks(
        &mut self,
        nbr_list: &[Nbr],
        overflow_starts: &[u32],
    ) { ... }
    
    /// 统计信息
    pub fn stats(&self) -> FreeListStats { ... }
}

pub struct FreeListStats {
    pub num_blocks: usize,
    pub total_freed_capacity: u32,
    pub reuse_rate: f32,      // reused / freed
}
```

### 2.2 分配算法：LIFO 栈

**为什么选择 LIFO？**

1. **时间复杂度**：O(1) 分摊
2. **空间效率**：栈是最简单的数据结构
3. **缓存友好**：LIFO 栈中的块最新释放，数据可能仍在 CPU 缓存
4. **实现简单**：Vec + pop() 即可

**算法细节**：

```rust
pub fn allocate(&mut self, min_size: u32) -> Option<u32> {
    // 从栈顶向下查找
    // 若块大小 >= min_size，直接弹出并返回起始地址
    // 若块大小 < min_size，跳过（可累积成外部碎片）
    
    while let Some((start, cap)) = self.blocks.pop() {
        if cap >= min_size {
            return Some(start);
        }
        // 被跳过的小块无法恢复，会成为外部碎片
        // 定期合并时处理
    }
    None  // 无合适的块，需要追加
}
```

**缺陷**：小块无法被复用，积累成外部碎片

**应对方案**：定期调用 `merge_adjacent_blocks()` 合并相邻块

### 2.3 释放与收集

**何时释放块？**

两种时机：

#### 方案 A：紧凑时重建 freelist（推荐）

```rust
impl MutableCsr {
    pub fn compact_with_ts(&mut self, ts: Timestamp, ...) -> usize {
        // ... 原有紧凑逻辑 ...
        
        #[cfg(feature = "csr-freelist")]
        {
            // 紧凑后，清空 freelist，等待下次扩容时重新积累
            // 或者：扫描 overflow_starts，找出未被引用的块
            self.freelist.clear();
            // 此时 nbr_list 是干净的平坦 CSR
            // 新的溢出块从 nbr_list 末尾追加
        }
        
        removed
    }
}
```

**优点**：紧凑后 freelist 自动清空，简洁

**缺点**：紧凑间隔内积累的释放块无法回收

#### 方案 B：实时跟踪（未来优化）

```rust
// 在 delete_edge 时，若溢出块完全失效，立即加入 freelist
// 更复杂，但在线碎片复用更高效
```

目前推荐方案 A（紧凑时重建），因为：
1. 简化实现
2. freelist 状态与 nbr_list 一致
3. 紧凑本身不常见，重建成本低

### 2.4 合并相邻块

**目标**：防止小块碎片堆积

**时机**：
- 紧凑后调用一次
- 或在 freelist 块数超过阈值时触发

**算法**：

```rust
pub fn merge_adjacent_blocks(
    &mut self,
    nbr_list: &[Nbr],
    overflow_starts: &[u32],
) {
    if self.blocks.is_empty() {
        return;
    }
    
    // 1. 排序 freelist 块（按起始地址）
    self.blocks.sort_by_key(|&(start, _)| start);
    
    // 2. 线性扫描，合并相邻块
    let mut merged = Vec::new();
    let (mut curr_start, mut curr_cap) = self.blocks[0];
    
    for &(start, cap) in &self.blocks[1..] {
        if curr_start + curr_cap == start {
            // 相邻，合并
            curr_cap += cap;
        } else {
            // 不相邻，推入
            merged.push((curr_start, curr_cap));
            curr_start = start;
            curr_cap = cap;
        }
    }
    merged.push((curr_start, curr_cap));
    
    self.blocks = merged;
    self.num_merges += 1;
}
```

---

## 三、集成策略

### 3.1 编译期 Feature Flag

**Cargo.toml 中**（建议）：

```toml
[features]
default = ["server", "fulltext-search", "c-api"]
csr-freelist = []  # ✨ 新增：可选的空闲块重用
```

**优点**：
- 线上可用不同编译版本对比测试
- 不使用时零成本
- 易于 A/B 测试

### 3.2 在 MutableCsr 中集成

**mutable_csr.rs 修改** (~50 行)：

```rust
#[cfg(feature = "csr-freelist")]
use super::mutable_csr_freelist::FreeListAllocator;

pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    // ... 既有字段 ...
    
    #[cfg(feature = "csr-freelist")]
    freelist: FreeListAllocator,
}

impl MutableCsr {
    pub fn with_capacity(vertex_capacity: usize, edge_capacity: usize) -> Self {
        Self {
            nbr_list: Vec::with_capacity(edge_capacity),
            // ...
            #[cfg(feature = "csr-freelist")]
            freelist: FreeListAllocator::new(),
        }
    }
    
    fn expand_vertex_capacity(&mut self, src_idx: usize, ...) {
        let needed_capacity = ...;
        
        #[cfg(feature = "csr-freelist")]
        {
            // 优先从 freelist 分配
            if let Some(reused_start) = self.freelist.allocate(needed_capacity as u32) {
                // ✨ 复用旧块位置
                return self.reuse_block(reused_start, needed_capacity);
            }
        }
        
        // 回退到追加（无 freelist 或无合适块）
        self.append_block(needed_capacity)
    }
    
    pub fn compact_with_ts(&mut self, ts: Timestamp, ...) {
        // ... 紧凑逻辑 ...
        
        #[cfg(feature = "csr-freelist")]
        {
            self.freelist.clear();  // 紧凑后清空 freelist
        }
    }
}
```

### 3.3 新增辅助方法

```rust
impl MutableCsr {
    /// 复用已有的 freelist 块位置（内部方法）
    fn reuse_block(&mut self, block_start: u32, needed_capacity: usize) {
        let block_start = block_start as usize;
        let needed_capacity = needed_capacity as usize;
        
        // 检查块是否足够大
        let actual_capacity = ...;  // 从 nbr_list 已有空间推导
        
        if actual_capacity < needed_capacity {
            // 块不足，需扩展（复杂，暂时回退追加）
            self.append_block(needed_capacity);
            return;
        }
        
        // 复用成功，更新 overflow_starts 指针
        self.overflow_starts[src_idx] = block_start as u32;
        // 跳过重新调整 nbr_list（已有数据）
    }
}
```

---

## 四、性能与复杂度分析

### 4.1 时间复杂度

| 操作 | 无 freelist | 有 freelist LIFO | 有 freelist 分桶 |
|------|-------------|-----------------|-----------------|
| 分配 | O(1) 追加 | O(1) pop | O(log n) 查找 |
| 释放 | 无 | O(1) push | O(log n) 插入 |
| 合并 | 无 | O(n log n) 定期 | O(n log n) 定期 |

### 4.2 空间复杂度

**freelist 额外开销**：

| 场景 | 块数 | 额外内存 |
|------|------|----------|
| 轻度（1-5 块）| 5 | ~200 字节 |
| 中度（5-20 块）| 20 | ~800 字节 |
| 重度（20+ 块）| 50+ | ~2 KB |

**结论**：freelist 本身的内存开销极小（相对于 nbr_list）

### 4.3 预期收益

**假设**：
- 图大小：100K 顶点，1M 边
- 平均 fragmentation_ratio：2.5（若无 freelist）
- 写入模式：高并发，频繁扩容

**对比**：

| 指标 | 无 freelist | 有 freelist |
|------|-------------|------------|
| nbr_list 膨胀 | 100% (~2M Nbr) | 60-70% (~1.2M Nbr) |
| 序列化大小 | 32 MB | 19 MB |
| 紧凑频率 | 每天 2-3 次 | 每周 1-2 次 |
| 总紧凑成本 | ~60ms/天 | ~20ms/周 |

**估算**：减少 nbr_list 膨胀 30-40%，减少紧凑成本 80%+

---

## 五、实施路线图

### 5.1 第一阶段：基础 LIFO 实现（1 周）

- [ ] 新建 `mutable_csr_freelist.rs` (~200 行)
- [ ] 实现 `FreeListAllocator` 结构
- [ ] 实现 `allocate()` / `free()` / `clear()`
- [ ] 编写单元测试 (~100 行)
- [ ] 集成到 `MutableCsr` (~50 行修改)

**验收标准**：
- ✅ 所有单元测试通过
- ✅ 编译通过（feature flag 启用和禁用）
- ✅ 无性能回归

### 5.2 第二阶段：块合并与统计（1 周）

- [ ] 实现 `merge_adjacent_blocks()`
- [ ] 实现 `stats()` 诊断方法
- [ ] 在紧凑后自动调用合并
- [ ] 编写测试 (~50 行)

**验收标准**：
- ✅ freelist 块数稳定在较低水平
- ✅ 复用率可观 (>60%)
- ✅ 无内存泄漏

### 5.3 第三阶段：灰度与监控（1 周）

- [ ] 添加编译时 feature flag 检查
- [ ] 集成到编译脚本（可生成两个版本）
- [ ] 添加监控指标：freelist 块数、复用率、合并次数
- [ ] 编写部署与对比测试指南

**验收标准**：
- ✅ 线上可同时部署两个版本
- ✅ 监控数据清晰
- ✅ A/B 测试可执行

---

## 六、风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| **freelist 块地址错误** | 中 | 数据损坏 | 详尽的单元测试、address sanitizer |
| **内存访问越界** | 低 | Crash | 边界检查、release build 测试 |
| **性能回归** | 低 | 用户体验下降 | benchmark、对比测试 |
| **外部碎片积累** | 中 | freelist 失效 | 定期合并、监控 |

---

## 七、决策点与后续

### 7.1 何时启动 Phase 2

**触发条件（满足任一）**：

1. **生产数据驱动**：
   - Phase 1 部署 1 个月后
   - P99 fragmentation_ratio > 2.0
   - 紧凑频率 > 5 次/天

2. **客户反馈**：
   - 存储空间成为瓶颈
   - 序列化延迟不可接受

3. **项目规划**：
   - 架构稳定性改进的一部分
   - 性能优化的关键项

### 7.2 如何评估效果

**对比指标**（启用 vs 禁用 freelist）：

```
nbr_list 平均长度增长率：
  - 禁用：200%/周（快速膨胀）
  - 启用：100%/周（增长缓慢）

紧凑频率：
  - 禁用：5 次/天
  - 启用：1 次/天

序列化时间：
  - 禁用：50ms（可能含紧凑）
  - 启用：40ms（更少的紧凑）
```

---

## 八、代码示例

### 8.1 完整的 freelist 模块框架

```rust
// mutable_csr_freelist.rs

#[derive(Debug, Clone)]
pub struct FreeListAllocator {
    blocks: Vec<(u32, u32)>,
    total_freed: u64,
    total_reused: u64,
    num_merges: usize,
}

impl FreeListAllocator {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            total_freed: 0,
            total_reused: 0,
            num_merges: 0,
        }
    }
    
    pub fn allocate(&mut self, min_size: u32) -> Option<u32> {
        // LIFO 实现
        while let Some((start, cap)) = self.blocks.pop() {
            if cap >= min_size {
                self.total_reused += cap as u64;
                return Some(start);
            }
        }
        None
    }
    
    pub fn free(&mut self, start: u32, capacity: u32) {
        self.blocks.push((start, capacity));
        self.total_freed += capacity as u64;
    }
    
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
    
    pub fn merge_adjacent_blocks(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        
        self.blocks.sort_by_key(|&(start, _)| start);
        let mut merged = Vec::new();
        let (mut curr_start, mut curr_cap) = self.blocks[0];
        
        for &(start, cap) in &self.blocks[1..] {
            if curr_start + curr_cap == start {
                curr_cap += cap;
            } else {
                merged.push((curr_start, curr_cap));
                curr_start = start;
                curr_cap = cap;
            }
        }
        merged.push((curr_start, curr_cap));
        
        self.blocks = merged;
        self.num_merges += 1;
    }
    
    pub fn stats(&self) -> FreeListStats {
        FreeListStats {
            num_blocks: self.blocks.len(),
            total_freed_capacity: self.total_freed as u32,
            reuse_rate: if self.total_freed == 0 {
                0.0
            } else {
                (self.total_reused as f64 / self.total_freed as f64) as f32
            },
        }
    }
}

#[derive(Debug)]
pub struct FreeListStats {
    pub num_blocks: usize,
    pub total_freed_capacity: u32,
    pub reuse_rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_allocate_reuses_freed_blocks() { ... }
    
    #[test]
    fn test_lifo_order() { ... }
    
    #[test]
    fn test_merge_adjacent_blocks() { ... }
    
    #[test]
    fn test_reuse_rate() { ... }
}
```

---

## 九、与其他组件的兼容性

### 9.1 与序列化的兼容性

**dump/load 格式**：不改变（freelist 不序列化）

```rust
impl MutableCsr {
    pub fn dump(&self) -> Vec<u8> {
        // 若启用 freelist，只序列化 nbr_list 有效数据
        // freelist 本身不持久化（紧凑后清空）
        // 重新加载后，freelist 从头开始积累
    }
}
```

### 9.2 与迭代的兼容性

**迭代器**：无影响（freelist 只影响分配策略，不影响逻辑）

### 9.3 与并发的兼容性

**当前单线程**：freelist 完全兼容

**未来多线程**（如需）：
- 可添加 Mutex<FreeListAllocator>
- 或分散 freelist 到顶点级别

---

## 总结

Phase 2 提供了一个**成本可控、效果显著、风险低的可选优化**，预期可：

- ✅ 减少 nbr_list 膨胀 30-40%
- ✅ 减少紧凑成本 80%+
- ✅ 保持代码清晰，易于维护
- ✅ 与现有架构无冲突

建议在 Phase 1 部署 1-3 个月后，根据生产数据评估是否启动。

