# CSR 溢出块碎片问题分析与解决方案

**Status**: Implemented (Phase 1 - Lightweight Monitoring)  
**Date**: 2026-06-19  
**Related Code**: `crates/graphdb-storage/src/storage/edge/mutable_csr.rs`, `mutable_csr_variant.rs`

---

## 1. 问题概述

### 问题现象

`MutableCsr` 采用"两级 CSR"存储设计：
- **主块**：每个顶点的邻接列表存储在 `nbr_list` 中的连续区间
- **溢出块**：当主块满后，新边存入附加的溢出块，位于 `nbr_list` 尾部

当顶点反复插入新边时，会多次触发溢出块扩容，导致**内部碎片**：

```
初始状态:
nbr_list: [v0_primary | v1_primary | ... | v0_overflow_v1 ]
           ^-------主块区域-------^       ^---溢出块---^

扩容一次后:
nbr_list: [v0_primary | v1_primary | ... | v0_overflow_v1 | v0_overflow_v2 ]
           ^-------主块区域-------^       ^废弃^         ^新块^

扩容两次后:
nbr_list: [v0_primary | v1_primary | ... | v0_overflow_v1 | v0_overflow_v2 | v0_overflow_v3 ]
           ^-------主块区域-------^       ^废弃^         ^废弃^             ^新块^
```

旧溢出块 `v0_overflow_v1` 和 `v0_overflow_v2` 仍占用 `nbr_list` 空间，但已没有任何索引引用，成为不可达的"僵尸数据"。

### 影响范围

| 维度 | 影响 | 严重度 |
|------|------|--------|
| **查询正确性** | ❌ 无影响（通过 `overflow_starts` 指针访问当前有效块） | 无 |
| **内存浪费** | ✅ `nbr_list.len() >> 有效边数` | 中等 |
| **序列化大小** | ✅ `dump()` 序列化整个 `nbr_list`，包含碎片 | 中等 |
| **加载性能** | ✅ 反序列化时完整还原 `nbr_list`，持续携带垃圾 | 低 |
| **迭代性能** | ❌ 无影响（迭代器通过指针访问有效数据） | 无 |

### 根本原因

**设计权衡**：`expand_vertex_capacity()` 选择了以下策略：

```rust
// 在 nbr_list 尾部追加新块，避免 O(n) 重排
let append_pos = self.nbr_list.len();
self.nbr_list.resize(append_pos + additional, ...);

// 复制旧块数据到新位置
if self.overflow_starts[src_idx] != NO_OVERFLOW {
    let old_start = self.overflow_starts[src_idx] as usize;
    for i in 0..old_count {
        self.nbr_list[append_pos + i] = self.nbr_list[old_start + i];
    }
}

// 更新指针，旧块被"遗忘"
self.overflow_starts[src_idx] = append_pos as u32;
```

**优点**: O(1) 分摊扩容成本  
**缺点**: 积累不可达空间

---

## 2. 解决方案对比

### 方案 A：定期紧凑（Periodic Compaction）

**实现原理**：利用已有的 `compact_with_ts()` 定期重建整个 `nbr_list`，将有效边紧凑排列，清除僵尸块。

**优点**
- 实现已完整，仅需添加自动触发逻辑
- 一次彻底清理：同时回收溢出碎片和逻辑删除边
- 恢复标准 CSR 布局，保持最优的缓存局部性
- 对现有结构零侵入

**缺点**
- 需主动调用，间隔过长仍会累积碎片
- O(V+E) 全局重排成本高，可能造成延迟峰值
- 需要冻结写入，不适合高并发在线服务
- 依赖时间戳参数 `ts`，自动化时难以确定可见性版本

**适用场景**
- 读多写少、批量加载的场景
- 可接受离线维护窗口的图分析系统
- 碎片容忍度低，需序列化前释放所有浪费空间

**复杂度**: ⭐

---

### 方案 B：空闲块重用（Free‑List / Allocation Cache）

**实现原理**：维护被遗弃溢出块的起始地址与容量列表，扩容时优先从空闲块中分配匹配的块。

**优点**
- 延缓 `nbr_list` 膨胀，回收近期释放的较大块
- 分配成本低（首次适配/最佳适配），远小于全局紧凑
- 对插入性能影响较小，适合无法频繁紧凑的在线系统

**缺点**
- 实现复杂度高：需管理链表/位图、处理分割合并、类似内存分配器
- 可能产生外部碎片：小块无法满足大请求，最终仍需紧凑
- 搜索开销可能影响高频插入的尾延迟
- 元数据开销增加内存占用，需保证序列化/反序列化一致

**适用场景**
- 写入密集、碎片生成快但无法忍受频繁紧凑的 OLTP 系统
- 作为紧凑机制的补充，降低紧凑频率
- 溢出块大小分布集中的场景，匹配成功率高

**复杂度**: ⭐⭐⭐

---

### 方案 C：剥离溢出存储（Per‑Vertex Independent Containers）

**实现原理**：将溢出边从中央 `nbr_list` 中剥离，改为每个顶点独立维护的动态数组（如 `SmallVec<[Nbr; INLINE]>`）。

**优点**
- 从根本上杜绝中央数组碎片
- 溢出块只归属顶点，扩容不影响全局布局
- 局部性更好，顶点溢出与主块可共同缓存
- 天然支持并发，每个顶点可独立加锁

**缺点**
- 重构成本极高：需修改存储结构、序列化、迭代器、所有 CRUD 逻辑
- 内存分配分散，小分配可能增加堆管理开销
- 迭代顺序不连续，不利于全图扫描的缓存预取
- 序列化/反序列化复杂化

**适用场景**
- 高度动态的图，顶点度数方差极大（幂律分布）
- 并发写入要求极高，需要缩小冲突域
- 新设计或重大重构项目

**复杂度**: ⭐⭐⭐⭐⭐

---

### 方案 D：优化扩容策略（Expand Strategy Optimization）

**实现原理**：
- **选项 D1**：增大默认容量 `DEFAULT_VERTEX_DEGREE` 或根据历史数据动态调整
- **选项 D2**：尝试在扩容时整体移动主块+溢出块到新位置

**优点**
- D1 改动最小，可立即缓解碎片生成速率
- D1 适用于有先验知识的场景

**缺点**
- D1 过度预分配浪费内存，对低度数顶点不经济
- D2 与 CSR 布局根本冲突：移动一顶点主块会破坏所有后续顶点的 `adj_offsets`，导致全局重构，已等价于 compact
- D2 不可行

**适用场景**
- D1：顶点平均度数分布稳定，有用户预估
- D2：无可行场景，应直接使用紧凑方案

**复杂度**: ⭐

---

## 3. 推荐方案：轻量级监控 + 主动紧凑

### 设计理念

综合考虑：
1. **项目阶段**：开发阶段，重点是架构清晰而非完美优化
2. **改动成本**：最小化（~50 行代码）
3. **数据驱动**：先收集实际使用中的碎片情况，再决策更复杂方案
4. **可维护性**：简单透明，易于理解和演进

### 实施内容（已完成）

#### 3.1 添加诊断接口到 `MutableCsr`

```rust
/// 计算碎片率：nbr_list.len() / active_edges
/// 返回 0.0 若无活跃边
pub fn fragmentation_ratio(&self) -> f32

/// 判断是否应该执行紧凑
pub fn should_compact(&self, threshold: f32) -> bool

/// 估算浪费内存（字节数）
pub fn wasted_bytes_estimate(&self) -> usize
```

**解释**
- `fragmentation_ratio() > 1.5` 表示中等碎片
- `fragmentation_ratio() > 2.0` 表示严重碎片，建议紧凑
- `wasted_bytes_estimate()` 用于监控和决策

#### 3.2 添加包装方法到 `CsrVariant`

```rust
/// 条件紧凑：若碎片率超过阈值则执行
pub fn maybe_compact(&mut self, threshold: f32, ts: Timestamp, reserve_ratio: f32)

/// 获取碎片率（仅 Multiple 变体有效）
pub fn fragmentation_ratio(&self) -> f32
```

#### 3.3 增强文档

在模块头添加"溢出块碎片"部分，明确说明：
- 碎片产生的原因
- 何时需要紧凑
- 紧凑的成本和收益

### 使用指南

#### 场景 1：序列化前（最重要）

```rust
// 在持久化或网络传输前
if csr.should_compact(2.0) {
    csr.compact_with_ts(current_timestamp, 0.25);
}
let serialized = csr.dump();
```

**收益**: 减少序列化体积，可能大幅节省存储空间

#### 场景 2：批量操作后（可选）

```rust
// 在 CRUD 密集操作末尾
if csr.should_compact(2.5) {
    csr.compact_with_ts(current_timestamp, 0.25);
}
```

**收益**: 控制碎片累积，改善后续操作的空间效率

#### 场景 3：监控和诊断

```rust
// 定期检查碎片情况
let ratio = csr.fragmentation_ratio();
let wasted = csr.wasted_bytes_estimate();
metrics::gauge!("csr_fragmentation_ratio", ratio);
metrics::gauge!("csr_wasted_bytes", wasted as f64);
```

**收益**: 数据驱动的决策，决定是否需要升级方案

---

## 4. 后续演进路径

### 第 2 阶段（根据实际反馈）

#### 路径 A：保持现状
**触发条件**：实际使用中 `fragmentation_ratio < 1.5`，碎片不成问题

**行动**：保持轻量级监控，定期在序列化前调用 `maybe_compact()`

---

#### 路径 B：升级到空闲块重用
**触发条件**：
- 碎片率常见 > 2.0，且紧凑频率不可接受
- 在线系统不能忍受 `compact_with_ts()` 的 O(V+E) 延迟

**实施步骤**：
1. 在 `MutableCsr` 中添加 `free_list: Vec<(u32, u32)>` 字段（起始位置、容量）
2. 修改 `expand_vertex_capacity()` 优先从 `free_list` 分配
3. 在溢出块不再使用时将其加入 `free_list`
4. 定期（或当 `free_list` 太长时）合并相邻的空闲块

**改动规模**：~300-500 行代码  
**维护成本**：中等（需处理链表操作和边界情况）  
**收益**：减少 `nbr_list` 膨胀速率，缓解紧凑频率

---

#### 路径 C：重构溢出存储
**触发条件**：
- 碎片成为严重瓶颈，碎片率常见 > 3.0
- 或高并发写入场景需要顶点级别的独立锁

**实施步骤**：
1. 将主块保持在 `nbr_list` 中
2. 将溢出数据改为 `Vec<Vec<Nbr>>` 或为每个顶点分配 `SmallVec<[Nbr; K]>`
3. 重构序列化/反序列化逻辑
4. 重构迭代器和所有访问路径
5. 更新 `CsrBase` 和 `MutableCsrTrait` 实现

**改动规模**：~1500-2000 行代码  
**维护成本**：高（结构性改变）  
**收益**：完全消除中央数组碎片，天然支持并发

---

### 升级决策树

```
监控 fragmentation_ratio
  │
  ├─ < 1.5 (低碎片)
  │   └─ 保持轻量级监控 ──→ 定期序列化前紧凑
  │
  ├─ 1.5 ~ 2.5 (中等碎片)
  │   └─ 评估紧凑频率
  │       ├─ 低频可接受 ──→ 保持路径 A
  │       └─ 高频不可接受 ──→ 升级到路径 B（空闲块重用）
  │
  └─ > 2.5 (高碎片)
      └─ 评估场景
          ├─ 写少读多 ──→ 路径 A + 定期离线紧凑
          ├─ 写多且可忍受峰值 ──→ 路径 A + 主动触发
          ├─ 高并发实时 ──→ 路径 B（空闲块重用）
          └─ 极端场景 ──→ 路径 C（重构存储）
```

---

## 5. 实施指南

### 5.1 当前状态（Phase 1）

**已实现**：
- ✅ `fragmentation_ratio()` - 监控碎片
- ✅ `should_compact(threshold)` - 判断是否需要紧凑
- ✅ `wasted_bytes_estimate()` - 估算浪费空间
- ✅ `maybe_compact()` - 条件紧凑包装器
- ✅ 文档和注释

**何时升级**：
1. 收集一个月的生产数据
2. 查看碎片率分布：P50, P95, P99
3. 查看序列化大小变化
4. 若 P99 碎片率 > 2.0，考虑升级到 Phase 2

### 5.2 建议的集成点

#### EdgeTable（高层）

```rust
// 在 EdgeTable::flush() 或持久化前调用
pub fn maybe_compact(&mut self) {
    if let EdgeStrategy::Multiple = self.out_strategy {
        self.out_csr.maybe_compact(2.5, ts, 0.25);
    }
    if let EdgeStrategy::Multiple = self.in_strategy {
        self.in_csr.maybe_compact(2.5, ts, 0.25);
    }
}
```

#### Transaction（事务级）

```rust
// 在大批量 CRUD 操作后调用
impl Transaction {
    pub fn maybe_compact_edges(&mut self) {
        for table in &mut self.edge_tables {
            table.maybe_compact();
        }
    }
}
```

#### 监控与告警

```rust
// 定期采集指标
pub fn collect_csr_metrics(&csr: &MutableCsr) {
    metrics::gauge!("csr_fragmentation_ratio", csr.fragmentation_ratio());
    metrics::gauge!("csr_wasted_bytes", csr.wasted_bytes_estimate() as f64);
}
```

---

## 6. 风险评估

### 现状风险

| 风险 | 描述 | 概率 | 影响 |
|------|------|------|------|
| **序列化膨胀** | 无必要的碎片被序列化 | 高 | 中等（存储和网络）|
| **内存浪费** | `nbr_list` 占用空间虚高 | 高 | 低（仍可用）|
| **分析性能** | 高度数顶点的扩容累积 | 中等 | 低（查询不受影响）|

### 方案风险

| 风险 | 轻量级监控 | 空闲块重用 | 存储剥离 |
|------|-----------|-----------|---------|
| **实施错误** | 极低 | 中等 | 高 |
| **性能回归** | 无 | 低（搜索开销） | 中等（局部性变差）|
| **维护复杂度** | 低 | 中等 | 高 |

---

## 7. 参考

### 相关代码

- `MutableCsr::compact_with_ts()` - 紧凑实现
- `MutableCsr::expand_vertex_capacity()` - 溢出块扩容
- `MutableCsr::dump()` / `load()` - 序列化

### 相关讨论

- [多层次 CSR 设计](./multi_single_csr_design.md)
- [存储架构重构](../storage_architecture_refactoring.md)

---

## 8. 总结

当前采用**轻量级监控 + 主动紧凑**的方案，优点是：

1. **改动最小**（~50 行代码）
2. **零侵入**现有逻辑
3. **数据驱动**后续决策
4. **易于演进**到更复杂方案

该方案足以应对开发阶段的需求。根据实际使用情况收集数据后，可灵活升级到空闲块重用或存储剥离方案。
