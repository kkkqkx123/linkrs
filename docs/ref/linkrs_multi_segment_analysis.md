# Linkrs 多段 Segment 设计分析 — 目的、成本与取舍

> 分析日期：2026-06-25
> 基于 `EdgeTableCore` / `segment.rs` / `freeze.rs` / `merge.rs` 的代码分析
> 对比对象：LadybugDB 的单段 CSR 设计

---

## 1. 多段 Segment 的架构全景

```
┌─────────────────────────────────────────────────────────┐
│                   EdgeTableCore                         │
├──────────────────────┬──────────────────────────────────┤
│    Mutable CSR       │       Immutable Segments         │
│  (out_csr / in_csr)  │   (out_segments / in_segments)  │
├──────────────────────┼──────────────────────────────────┤
│  写入直接落在此处     │   Freeze 后变为不可变 segment    │
│  支持 Point Query     │  Background Merge 合并小段      │
│  有 size 阈值上限     │  时间戳 + 删除元信息            │
│  (max_mutable_csr_    │  用 SegmentIndex 加速时间剪枝   │
│   bytes: 100MB)      │                                  │
└──────────────────────┴──────────────────────────────────┘
         │                        │
         │     merged_edges_of():  │
         │   遍历 Mutable + 所有   │
         │   Segment, HashSet 去重 │
         ▼                        ▼
       ┌──────────────────────────────────┐
       │       查询结果 (Vec<Nbr>)        │
       └──────────────────────────────────┘
```

**数据流**：写入 → Mutable CSR → **Freeze**（转为 Immutable Segment）→ **Merge**（合并小段，物理删除）

---

## 2. 多段 Segment 的设计目的

### 2.1 目的一：Time-Travel 查询（核心目的）

这是多段设计的**根本原因**。每个 `CsrSegment` 记录了完整的版本信息：

```rust
pub struct CsrSegment {
    pub csr: Csr,
    pub create_ts_min: Timestamp,  // 本段边的最小创建时间
    pub create_ts_max: Timestamp,  // 本段边的最大创建时间
    pub deletion_info: DeletionInfo, // 删除时间范围 + 删除数量
    pub created_at_ts: Timestamp,     // 段本身的创建时间
    // ...
}
```

Segment 索引按时间戳排序，查询时**跳过不相关的段**：

```rust
// freeze.rs - out_segment_index 结构
pub out_segment_index: Vec<(Timestamp, usize)> // 按 create_ts_min 降序排列
pub in_segment_index: Vec<(Timestamp, usize)>

// core.rs - 查询时的段跳跃逻辑
fn base_edges_of(&self, segments: &[CsrSegment], src: u32, ts: Timestamp) -> Vec<Nbr> {
    for segment in segments.iter().rev() {
        if segment.create_ts_min > ts { continue; }  // ★ 时间剪枝
        if segment.deletion_info.all_deleted_before(ts) { continue; } // ★ 删除剪枝
        // ... 读取邻接
    }
}
```

**Time-Travel 的使用示例**（测试代码验证）：

```rust
// segment.rs 测试
table.freeze_csr_only(100); // 冻结 t=100 时的边
table.delete_edge(0, i, 0, 200); // t=200 时删除
table.freeze_csr_only(200); // 冻结 t=200 的删除

let edges_at_150 = table.out_edges(0, 150); // query at ts=150
assert_eq!(edges_at_150.len(), 10); // 看到 10 条边（删除不可见）

let edges_at_250 = table.out_edges(0, 250); // query at ts=250
assert_eq!(edges_at_250.len(), 0); // 看到 0 条边（删除可见）
```

**结论**：**时间旅行查询必须保留不同时间片的边状态**。单段 CSR 无法实现这一点——删除后数据就丢失了。

### 2.2 目的二：写优化 — LSM 风格的 Delta → Segment 流水线

写操作全部进入 Mutable CSR（delta），达到阈值后 freeze 为 immutable segment：

```
写入 → Mutable CSR (delta) ──达到 100MB 或 freeze 触发──→ Immutable Segment
                                                              ↓
                                              Background Merge 将小段合并成大段
```

这种设计**将随机写转换为顺序写**：

- 写入只需要追加到 Mutable CSR
- Freeze 时一次性地将 Mutable CSR 转换为紧凑的 CSR 结构
- Merge 在后台做，不影响写入和查询

测试验证了多种 merge 策略：

| 策略 | 代码函数 | 触发条件 |
|------|---------|----------|
| LSM-tiered | `merge_lsm_tiered()` | 按段大小分 L0/L1/L2/L3+ 级别，每级超过阈值触发 |
| Adaptive | `merge_adaptive()` | 优先合并老段和高删除率段 |
| In-place | `merge_in_place()` | 按时间间隔 + 大小阈值合并 |
| Aggressive | `auto_merge_segments()` | 段数量超过 `max_segments_per_direction` |

### 2.3 目的三：物理删除 / 垃圾回收

没有 segment，就无法做物理删除。Edge 删除分两层：

```
逻辑删除（标记 tombstone） ← 即时完成
     ↓
物理删除（在 merge 中跳过已删除边） ← 后台完成
```

在 `merge_selected_segments_with_deletion_filter()` 中，当 `min_active_snapshot_ts` 存在时，会**物理跳过**在该时间戳之前已删除的边，实现真正的空间回收。

### 2.4 目的四：数据校验与恢复

每个 `CsrSegment` 有 `SegmentVersion`，包含 CRC32 checksum：

```rust
pub struct SegmentVersion {
    pub checksum: u32,
}
```

支持分段级别的完整性校验 vs 全文件级别的校验——段级别可以**局部恢复**。

---

## 3. 多段 Segment 的成本分析

### 3.1 遍历成本：最严重的性能损失

这是多段设计中最显著的性能开销。核心问题在 `merged_edges_of()`（[core.rs](file:///workspace/linkrs/crates/graphdb-storage/src/storage/edge/edge_table/core.rs)）：

```rust
fn merged_edges_of(&self, delta: &CsrVariant, segments: &[CsrSegment],
                   src: u32, ts: Timestamp) -> Vec<Nbr> {
    let mut seen = HashSet::new();     // ← 需要去重
    let mut result = Vec::new();

    // 1. 查 Mutable CSR
    for nbr in delta.iter_edges_of(src, ts) {
        if !self.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
            result.push(*nbr);
        }
    }

    // 2. 遍历所有 Segment
    for nbr in self.base_edges_of(segments, src, ts) {
        if seen.insert(nbr.edge_id) {  // ← 每条边都要 HashSet 插入
            result.push(nbr);
        }
    }

    result
}
```

**成本量化**（假设默认配置 `segment_merge_threshold=50`）：

| 场景 | 遍历的段数 | 去重方式 | O 复杂度 |
|------|-----------|---------|----------|
| LadybugDB (单段 CSR) | 1 | 无去重 | O(degree) |
| Linkrs (50 segments, 密集顶点) | ~50 | HashSet | O(50 × degree_per_seg) |
| Linkrs (50 segments, 稀疏顶点) | ~50 | HashSet | O(50 × 1) 但 50 次 CSR 查询 |

**典型场景的具体数字**（10K 边、1K 出边的源顶点）：

```
LadybugDB: 1 次 CSR 偏移量定位 + 顺序读取 1000 个邻接条目
Linkrs:    50 次 CSR 偏移量定位 + 读取 50 个 mini 数组 + 1000 次 HashSet 插入
```

### 3.2 内存成本

每个 `CsrSegment` 独立存储 CSR 结构，带来额外开销：

- CSR 的 offset 数组：每个源顶点至少 1 个 entry
- CSR 的 length 数组（部分策略）：每个源顶点至少 1 个 entry
- Segment 元数据：~40 bytes/segment（时间戳、deletion_info、checksum、created_at_ts）
- Segment index 条目：~8 bytes/segment

对于一个被 `merge_keep_newest=5` 控制后的系统（6 个 segment/direction）：

- 额外元数据开销 ~240 bytes/table
- 但如果 merge 速度跟不上 freeze 速度，段数可能达到阈值 50 甚至更多

### 3.3 CPU 成本 (Background Merge)

Merge 操作需要：

1. 遍历所有被合并 segment 的所有边（O(total_edges)）
2. 重建 CSR 结构（O(vertices + edges)）
3. 重新计算 checksum

虽然 merge 在后台执行，但写密集场景下 merge 可能跟不上，造成段堆积。

### 3.4 代码复杂度

EdgeTable 相关的文件数量和行数（仅就存储层关键模块）：

| 文件 | 用途 | 代码行 |
|------|------|--------|
| `edge_table/core.rs` | 核心 CRUD / 遍历 | ~1160 |
| `edge_table/freeze.rs` | Freeze 逻辑 | ~370 |
| `edge_table/merge.rs` | Merge 策略（4 种） | ~660 |
| `edge_table/segment.rs` | Segment 结构体 | ~280 |
| `edge_table/compaction.rs` | Compaction | ~80 |
| `edge_table/stats.rs` | Merge 统计 | ~60 |
| **合计** | **边存储额外复杂度** | **~2610 行** |

对比 LadybugDB 的 CSR 实现（`csr_node_group.cpp` 约 540 行，且无多段概念），Linkrs 的多段设计增加了约 **4-5 倍的代码量**。

---

## 4. 是否值得？— 场景化评估

### 4.1 需要 Time-Travel = 必须保留多段

如果 Linkrs 的目标场景需要**查询历史状态**（如"这张图在昨天是什么样？"），那么多段设计是**不可替代**的。单段 CSR 无法做到：

- 时间旅行需要保留多个时间片的数据
- 每个时间片的数据独立不可变，segment 是天然的实现方式
- 替代方案（snapshot 全量复制）的空间成本远高于 delta-based segment

### 4.2 不需要 Time-Travel = 多段是过度设计

如果 Linkrs 的目标是**实时 OLTP 型图查询**（如社交推荐、知识图谱查询），多段的成本超过收益。更适合的做法：

1. **单段 CSR**（LadybugDB 方式）— 最大遍历性能
2. **WAL 只做崩溃恢复**，不拿来做快照管理
3. **逻辑删除用标记位**，compact 时批量清理

### 4.3 值得与否的定量判断

| 判定维度 | 得分 (1-10) | 说明 |
|----------|------------|------|
| Time-travel 的必要性 | ? | 取决于用户需求，Linkrs 显然在设计上投入很大 |
| 写吞吐收益 | 7 | LSM 风格的 delta-freeze-merge 流水线确实有利于批量写入 |
| 空间回收收益 | 6 | merge 时物理删除，但单段 CSR+标记位+compact 也能做到 |
| 遍历性能损失 | -8 | 这是对日常查询影响最大的扣分项 |
| 内存额外开销 | -4 | 段数在默认阈值下可控（~6 段），但段数多时很严重 |
| 代码复杂度 | -5 | 4-5 倍的实现和维护成本 |

**如果 Time-Travel 是核心特性，多段是"值得"的**（收益 13 vs 成本 -17，但 Time-Travel 的独特性补足差值）。  
**如果 Time-Travel 不是核心特性，多段"不值得"**（收益 13 vs 成本 -17）。

---

## 5. 三种可选的技术路线

### 路线 A：保留多段 + 优化性能（推荐）

**前提**：Time-Travel 是核心特性，需要保留。

**核心思路**：不改变架构，用之前分析中的优化手段弥补性能损失：

1. **稀疏顶点索引**（P0）：避免对不含目标顶点的 segment 做 CSR 查询
2. **合并去重优化**（P1）：利用 segment 的时间顺序替代 HashSet
3. **更积极的 merge 策略**：降低活跃段数（默认 50 调低到 10-20）
4. **全场景快照模式**：为"当前时间"查询提供缓存合并视图

其中**最有效的方案是"全场景快照"**：当查询不需要时间旅行时（ts = MAX_TIMESTAMP），直接在内存维护一个**合并后的当前快照**（merged_csr），只在 segment 变更时增量更新：

```rust
// 核心思路
pub struct EdgeTableCore {
    // 现有字段...
    
    /// 当前时间快照（合并所有 segment + mutable CSR 的结果）
    /// 用于非时间旅行查询（ts = MAX_TIMESTAMP）
    current_snapshot: Option<Csr>,
    
    /// 上次构建快照时的 segment 版本
    snapshot_segment_checksums: Vec<u32>,
}

impl EdgeTableCore {
    fn get_current_snapshot(&mut self) -> &Csr {
        let current_checksums: Vec<u32> = self.out_segments
            .iter().map(|s| s.version.checksum).collect();
        
        if self.snapshot_segment_checksums != current_checksums {
            // 重新构建快照
            self.current_snapshot = Some(self.merge_all_segments());
            self.snapshot_segment_checksums = current_checksums;
        }
        
        self.current_snapshot.as_ref().unwrap()
    }
    
    // 对 ts = MAX_TIMESTAMP 走快照，否则走原逻辑
    fn edges_of_opt(&mut self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        if ts == u32::MAX {
            self.get_current_snapshot().edges_of(src, ts)
        } else {
            self.merged_edges_of(&self.out_csr, &self.out_segments, src, ts)
        }
    }
}
```

**效果**：非时间旅行查询退化为 LadybugDB 级别的单段 CSR 遍历，时间旅行查询保留多段能力。

### 路线 B：去除多段，退化为单段 CSR

**前提**：Time-Travel 可以被舍弃，或降级为全量快照方式。

**核心改动**：

1. 移除 `out_segments` / `in_segments` / `out_segment_index` / `in_segment_index`
2. 边直接写入并驻留在 Mutable CSR（增大 `max_mutable_csr_bytes` 或移除上限）
3. 删除 freeze / merge / compaction 全套机制
4. 删除 `DeletionInfo`、`CsrSegment`、`SegmentVersion` 等结构
5. WAL 仅做崩溃恢复，不再用于时间旅行
6. 删除处理改为：在 CSR 中标记删除 + bitmask，checkpoint 时物理清理

**影响**：

| 维度 | 变化 | 效果 |
|------|------|------|
| 邻接遍历 | 从多段合并 → 单段 CSR | O(degree)，去重开销降为 0 |
| 写入 | 保持 Mutable CSR | 写性能不变 |
| 时间旅行 | 彻底移除 | 功能降级 |
| MVCC 删除 | 改为标记位 + batch compact | 实现简化 |
| 代码行数 | 删除 ~2000 行 | 维护成本大幅降低 |

**实现量估计**：
- 移除代码（安全）：删除 freeze/merge/segment，~2 天
- 替换删除机制：CSR 内部支持标记删除，~3 天
- 改写查询接口：去掉 merged_edges_of / base_edges_of 的区分，~1 天
- 改写持久化 / WAL：~3 天
- 测试：~3 天  
**合计**：~12 个工作日

### 路线 C：混合模式（折中方案 — 推荐作为过渡）

**前提**：保留 Time-Travel 能力，但默认走无段快照路径。

**核心思路**：

```
默认查询模式: 走 merged snapshot (单段 CSR) ← 路线 A 的 snapshot 方案
时间旅行模式: 按需通过 export_snapshot(ts) 创建指定时间点的只读快照
后台机制:    freeze/merge 继续运行，但仅用于 WAL 恢复和时间旅行
```

**实现方式**：

1. 将 `EdgeTableCore` 拆分为两层：

```rust
pub struct EdgeTable {
    // 当前状态层：单段 CSR（供日常查询使用）
    current: CsrSnapshot,
    
    // 历史层：segment 链（仅供时间旅行查询和崩溃恢复）
    history: SegmentChain,
    
    // 后台同步
    reconciler: BackgroundReconciler,
}
```

2. `current` 是一个**持续更新的合并 CSR**：
   - Mutable CSR 直接合并到 `current`
   - Freeze 不产生新的 segment 结构，而是将 Mutable CSR 合并到 `current`
   - Merge 只是对 `current` 的 compact

3. `history` 是一个**时间戳排序的只读段链**：
   - 定期从 `current` 中导出快照（snapshot），附加时间戳
   - 用户查询 `edges_of(v, ts)` 时，若 `ts != MAX`，从 `history` 中查找

**优势**：
- 日常查询走单段 CSR，零合并零去重
- Time-travel 仍在，但查询成本按需承担
- 渐进式迁移：可以先在 EdgeTableCore 上加一个 snapshot 层

**劣势**：
- 实现复杂度中等（需要两层 CSR 的同步逻辑）
- 写路径增加一次额外更新（current 和 mutable 两面写）
- 需要 `BackgroundReconciler` 做最终一致性同步

---

## 6. 最终建议

### 推荐：路线 A → 逐步过渡到路线 C

不应简单"取消多段 Segment"，而应该根据查询模式做区分处理，分阶段演进：

#### 阶段 1（短期，2-4 周）— 路线 A 的优化措施

| 措施 | 预期收益 |
|------|----------|
| 稀疏顶点索引（`HashMap<u32, Vec<usize>>`） | 稀疏顶点 10-50x 遍历加速 |
| 降低默认 segment_merge_threshold（50→10） | 减少活跃段数，降低合并开销 |
| 代码量预计：~200 行新增 + 配置参数调整 |

#### 阶段 2（中期，4-8 周）— 当前时间快照（路线 C 的第一步）

为 `ts = MAX_TIMESTAMP` 的查询提供缓存合并的 CSR 快照，使**非时间旅行查询退化为单段 CSR**。

```rust
// 已有 snapshot 机制可以复用
let snapshot = table.export_snapshot(u32::MAX);
snapshot.get_out_edges(src);  // O(degree)，无合并无去重
```

日常查询走此路径，时间旅行（`ts < MAX`）走原路径。

#### 阶段 3（长期，8-12 周）— 完整混合模式

将 EdgeTable 重构为 `Current` + `History` 两层，彻底分离时间旅行与非时间旅行查询路径。

### 不做"简单删除"

**不建议全盘删除多段 Segment**，原因有三：

1. **Time-Travel 是有价值的差异化能力**：业界多数图数据库（Neo4j、ArangoDB）不提供行级时间旅行，Linkrs 在这方面有独特定位
2. **LSM 风格的写流水线对批量写友好**：LadybugDB 的 page 管理在随机写和小批量写时开销不低
3. **已有代码和测试投入大**：freeze/merge/segment 共 ~2500 行代码，配套测试也很多，直接删除浪费大

### 与 LadybugDB 的定位差异

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| 设计哲学 | 分析型查询优先 | 写入 + 时间旅行优先 |
| 遍历性能 | 极致（单段 CSR） | 当前有损失（多段合并） |
| 时间旅行 | 不支持 | 原生支持 |
| 写优化 | page 管理 + WAL | LSM 风格流水线 |
| 合理的改进方向 | 增加时间旅行 | 减少多段合并开销 |

Linkrs 的核心优势（Time-travel + MVCC + 属性索引）和多段设计是**一体的**。去掉多段意味着去掉时间旅行的能力，这会**破坏 Linkrs 的核心定位**。改进的方向不是**取消多段**，而是**为不同场景提供不同的查询路径**——让"当前时间"查询走最优路径，让"历史时间"查询走功能完整路径。