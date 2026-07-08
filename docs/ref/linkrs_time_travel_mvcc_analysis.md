# Linkrs Time-Travel 与 MVCC 设计分析

## 0. 核心结论（先读这段）

| 问题 | 结论 |
|------|------|
| **Time-Travel 是否需要改进？** | **需要**。当前设计对"当前时间查询"不友好——即使只查最新状态，也要遍历所有 segment。 |
| **是否需要过滤无关 segment？** | **需要**。当前只有粗糙的时间范围剪枝（`create_ts_min > ts`），缺少 **每个顶点的稀疏索引**，无法快速定位某个顶点出现在哪些 segment 中。 |
| **MVCC 是否必须通过多段实现？** | **不是**。LadybugDB 证明，MVCC 可以用**单段 + 行级版本元数据**高效实现。 |
| **是否需要取消多段？** | **不建议全盘取消**。多段的核心价值是 **Time-Travel（时间旅行查询）**，这是一个有意义的差异化特性。但需要**优化架构**将"当前查询"和"历史查询"的代码路径分离。 |

---

## 1. LadybugDB 的事务 / MVCC 模型（对比基准）

### 1.1 整体架构

```
┌─────────────────────────────────────────────────────┐
│                   Transaction                        │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────┐  │
│  │ LocalStorage │  │  UndoBuffer  │  │  LocalWAL │  │
│  │ (写缓冲区)    │  │ (撤销记录)    │  │ (本地WAL)  │  │
│  └──────┬──────┘  └──────┬───────┘  └───────────┘  │
└─────────┼─────────────────┼─────────────────────────┘
          │                 │
          ▼                 ▼
┌─────────────────────────────────────────────────────┐
│                   Global Storage                     │
│  ┌──────────────────────────────────────────────┐   │
│  │           CSRNodeGroup (单段 CSR)              │   │
│  │  ┌────────────────────────────────────────┐   │   │
│  │  │     ChunkedNodeGroup（行存储的列簇）      │   │   │
│  │  │  ┌──────────┐ ┌──────────┐ ┌────────┐  │   │   │
│  │  │  │ 列chunk 0│ │ 列chunk 1│ │  ...   │  │   │   │
│  │  │  └──────────┘ └──────────┘ └────────┘  │   │   │
│  │  │  + VersionInfo (行级事务版本元数据)      │   │   │
│  │  └────────────────────────────────────────┘   │   │
│  └──────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

### 1.2 关键机制

#### 事务本地写入（LocalStorage）

- 每个事务写入先进入 `LocalStorage`（事务本地内存），**不直接修改全局存储**
- 插入的边在 `LocalRelTable` 中以 **CSRIndex** 暂存（`HashMap<offset_t, row_idx_vec_t>`）
- 更新/删除在 **UndoBuffer** 中记录

#### VersionInfo（行级版本追踪）

每个 `ChunkedNodeGroup`（2048 行 = `DEFAULT_VECTOR_CAPACITY`）附带一个 `VersionInfo`，其中每个 vector（2048 行）有一个 `VectorVersionInfo`：

```cpp
struct VectorVersionInfo {
    // 每行的插入事务ID（优化：如果所有行同一事务插入，用 sameInsertionVersion 代替数组）
    std::unique_ptr<std::array<transaction_t, 2048>> insertedVersions;
    transaction_t sameInsertionVersion;
    InsertionStatus insertionStatus;  // NO_INSERTED | CHECK_VERSION | ALWAYS_INSERTED

    // 每行的删除事务ID（同理）
    std::unique_ptr<std::array<transaction_t, 2048>> deletedVersions;
    transaction_t sameDeletionVersion;
    DeletionStatus deletionStatus;    // NO_DELETED | CHECK_VERSION
};
```

**可见性判断**——扫描时通过以下逻辑决定某行是否对当前事务可见：

```cpp
bool isInserted(startTS, transactionID, rowIdx):
    insertion = insertedVersions[rowIdx]  // 或 sameInsertionVersion
    return insertion == transactionID     // 同一事务插入（自己可见）
        || insertion <= startTS;          // 已提交且在 startTS 之前

bool isDeleted(startTS, transactionID, rowIdx):
    deletion = deletedVersions[rowIdx]    // 或 sameDeletionVersion
    return deletion == transactionID      // 同一事务删除
        || deletion <= startTS;           // 已提交删除
```

#### Commit 流程

```
1. updateRelOffsets()      → 为本地事务中的边分配全局 edge_id
2. localStorage->commit()  → 将本地数据刷入全局 CSRNodeGroup
3. undoBuffer->commit()    → 将 UndoBuffer 中的版本信息持久化到 VersionInfo
4. WAL flush               → 写 WAL
```

**Commit 后的数据状态**：
- 新插入的行已追加到 `CSRNodeGroup` 的列簇末尾，`VersionInfo` 中标记了插入事务 ID
- 删除的行在 `VersionInfo.deletedVersions` 中标记了删除事务 ID
- **整个过程中，CSR 始终是单段的**。没有 segment split，没有分段合并。

#### Rollback 流程

```
undoBuffer->rollback() → reverseIterate:
  - INSERT_INFO → 从 ChunkedNodeGroup 截断
  - DELETE_INFO → 从 VersionInfo 清除删除标记
  - UPDATE_INFO → 恢复旧值
```

#### 扫描时的 MVCC 过滤

```cpp
void ChunkedNodeGroup::scan(transaction, scanState, ...):
    if (versionInfo):
        versionInfo->getSelVectorToScan(startTS, transactionID,
                                         selVector, rowIdxInGroup, numRows);
        // 筛选出当前事务可见的行
    // 然后只扫描 selVector 中选中的行
```

### 1.3 LadybugDB 的 Time-Travel 能力

**LadybugDB 不支持任意时间点的历史查询**。其 MVCC 仅用于：
1. **事务隔离**（SI - Snapshot Isolation）
2. **并发控制**（写写冲突检测）

查询只能看到 `startTS` 时刻的提交状态 + 自身修改。**不支持**查询 t=100 时的图快照。

---

## 2. Linkrs 的 Time-Travel / MVCC 模型

### 2.1 整体架构

```
┌──────────────────────────────────────────────────────────┐
│                     EdgeTableCore                         │
│  ┌──────────────────────┐   ┌───────────────────────┐    │
│  │    out_csr (Mutable)  │   │    in_csr (Mutable)   │    │
│  │  可变 CSR（当前写入）  │   │  可变 CSR（当前写入）  │    │
│  └──────────┬───────────┘   └───────────┬───────────┘    │
│             │                           │                │
│             ▼                           ▼                │
│  ┌──────────────────────┐   ┌───────────────────────┐    │
│  │  out_segments[0..N]  │   │  in_segments[0..N]    │    │
│  │  不可变 Segment 列表  │   │  不可变 Segment 列表   │    │
│  │  (create_ts_min/max) │   │  (create_ts_min/max)  │    │
│  └──────────────────────┘   └───────────────────────┘    │
│                                                          │
│  ┌──────────────────────────────────────────────────┐    │
│  │              MVCCManager                         │    │
│  │  pending_segment_deletions(tombstones HashMap)   │    │
│  │  segment_tombstones (已冻结段的删除标记)          │    │
│  │  cold_tombstones (冷层, 排序Vec + BloomFilter)   │    │
│  │  active_snapshots (活跃快照引用计数)               │    │
│  └──────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────┘
```

### 2.2 核心数据流

```
写入流程:
  insert_edge() → 写入 out_csr / in_csr（Mutable CSR）
                → 检查写回压，触发 freeze_csr_only()
                → Mutable CSR → 转为不可变 CsrSegment
                → 追加到 out_segments / in_segments

查询流程（当前时间 ts = MAX）:
  out_edges(src, ts) =
    merged_edges_of(out_csr, out_segments, src, ts):
      1. 遍历 Mutable CSR 所有邻边
      2. base_edges_of(): 反转序遍历所有 segment
         - 检查 create_ts_min > ts? → skip
         - 检查 all_deleted_before(ts)? → skip
         - 遍历 segment.csr.edges_of_with_position(src)
         - 检查 edge.timestamp <= ts
         - 检查 mvcc.is_tombstoned(edge_id, ts)
      3. HashSet<EdgeId> 去重（多个 segment 可能含相同 edge_id）
      4. 合并结果
```

### 2.3 Time-Travel 的实现本质

Time-Travel 的"时间旅行"能力源自 **segment 的不可变性和时间范围**：

```
ts=100: insert(0→1, 0→2)  → freeze → Segment A: create_ts=[100,100]
ts=200: insert(0→3, 0→4)  → freeze → Segment B: create_ts=[200,200]
ts=300: delete(0→1)       → freeze → Segment C: create_ts=[300,300] + tombstone(0→1@300)

查询 ts=150: 访问 Segment A（create_ts_min=100 ≤ 150）
           跳过 Segment B（create_ts_min=200 > 150）
           跳过 Segment C（create_ts_min=300 > 150）
           → 结果: {0→1, 0→2}

查询 ts=250: 访问 Segment A（100 ≤ 250）
           访问 Segment B（200 ≤ 250）
           跳过 Segment C（300 > 250）
           → 结果: {0→1, 0→2, 0→3, 0→4}
           （注意：ts=250 时 0→1 还没被删除，因为 deletion_ts=300 > 250）

查询 ts=MAX: 访问所有 Segment + tombstone 检查
           → 结果: {0→2, 0→3, 0→4}
           （0→1 被 tombstone 过滤掉，因为 delete_ts=300 ≤ MAX）
```

---

## 3. 核心问题分析：Time-Travel 是否需要过滤无关 segment？

### 3.1 当前存在的问题

**问题 1：当前时间查询无法避免全量 segment 遍历**

当前最频繁的查询是 `ts = MAX`（获取最新状态），但代码路径仍然是：

```rust
fn base_edges_of(&self, segments: &[CsrSegment], src: u32, ts: Timestamp) -> Vec<Nbr> {
    for segment in segments.iter().rev() {           // ← 遍历所有 segment
        if segment.create_ts_min > ts { continue; }   // ← 对 ts=MAX，永不为 true
        if segment.deletion_info.all_deleted_before(ts) { continue; } // ← 对 ts=MAX，永不为 true
        for (position, edge) in segment.csr.edges_of_with_position(src) {  // ← 遍历每个 segment 中 src 的邻接列表
            // ... 检查 tombstone
        }
    }
}
```

当 `ts = MAX` 时，`create_ts_min > ts` 和 `all_deleted_before(ts)` 两个剪枝条件**永远不生效**。所有 segment 都被无条件遍历。

**问题 2：缺少顶点级索引来跳过 segment**

当前只能跳过整个 segment（基于时间范围），但**不能针对单个顶点跳过无关 segment**。例如：
- Segment A 包含顶点 0, 1, 2 的边
- Segment B 只包含顶点 3, 4, 5 的边
- 查询 `out_edges(0, MAX)` 时，仍然要遍历 Segment B，虽然 B 根本不包含顶点 0 的边

Segment 内部有一个 CSR 可以快速找到 `src` 在 segment 中的邻接列表（如果该 segment 有 `src` 的数据）或返回空（如果该 segment 没有 `src` 对应的邻接列表），但如果 `src` 的度是 0 那么该 segment 内的 CSR 仍然会被访问，只是 CSR 内部扫描该顶点时会发现邻接列表为空。

其实 CSR 本身已经提供了按顶点索引能力：`edges_of_with_position(src)` 在 CSR 内部会通过 `offset[src]` 和 `length[src]` 快速定位。如果一个 segment 不包含 `src` 的任何边，CSR 返回空迭代器，但**遍历这个 segment 的元数据 + CSR 的 offset 表访问本身仍有开销**。

主要开销来源：
1. **segment 元数据遍历**：遍历 N 个 segment 对象，读 `create_ts_min` 等字段
2. **CSR 内部查找**：在每个 segment 中调用 `edges_of_with_position(src)` 到 CSR 的 offset/length 数组中执行二分/直接查找
3. **结果合并**：`merged_edges_of` 中的 `HashSet` 去重

### 3.2 数量级估计

假设：
- 50 个 segment（触发 merge 前的典型数量）
- 平均每个 segment 有 2000 条边
- 查询典型顶点的度 = 50

| 操作 | LadybugDB（单段） | Linkrs（50 段） |
|------|-------------------|-----------------|
| CSR offset 查找 | 1 次 | 50 次（每个段各 1 次） |
| 邻接边遍历 | 遍历 50 条邻边 | 遍历 50 条邻边 × 50 段 = 2500 次迭代 |
| 去重 HashSet | 不需要 | 50 次 insert |
| tombstone 检查 | 不需要（行级版本） | 50 次 HashMap 查询 |

### 3.3 改进方向：稀疏顶点索引

可以为每个 segment 维护一个**稀疏位图或 Bloom Filter**，记录哪些顶点在该 segment 中有邻边：

```
Segment A: vertex_bitmap = [1, 1, 1, 0, 0, 0, ...]  // 顶点0, 1, 2有边
Segment B: vertex_bitmap = [0, 0, 0, 1, 1, 1, ...]  // 顶点3, 4, 5有边

查询 out_edges(0, MAX):
  Segment A: bitmap[0] = 1 → 查询
  Segment B: bitmap[0] = 0 → 跳过！
```

这个索引的额外开销：
- 每个 segment 一个位图（如果是固定大小，每个 segment 增加 顶点数/8 字节）
- 或者使用 RoaringBitmap（稀疏场景更省空间）

### 3.4 更激进的改进：当前时间快照

核心思想是：**对当前时间（ts=MAX）的查询，不需要遍历任何 segment**。

```rust
struct EdgeTableCore {
    out_csr: CsrVariant,        // Mutable CSR（当前写入）
    out_segments: Vec<CsrSegment>,  // 历史 segment（只用于 Time-Travel）
    out_current_snapshot: Option<Csr>, // ← 新增：当前状态的合并快照
    // ...
}
```

维护策略：
1. 每次 merge 后，更新 `out_current_snapshot` = 所有 segment 合并后的 CSR
2. 查询 `ts == MAX` 时，直接查 `out_current_snapshot + out_csr`
3. 查询 `ts < MAX` 时，走原有 segment 遍历路径

代价：需要额外维护一份合并后的 CSR，增加写放大（每次 merge 后重建）。

---

## 4. 核心问题分析：MVCC 是否必须通过多段实现？

### 4.1 LadybugDB 的答案：不需要

LadybugDB 的 MVCC 采用了**完全不同的技术路线**：

| 维度 | LadybugDB | Linkrs |
|------|-----------|--------|
| **版本追踪** | 行级版本元数据（`VectorVersionInfo`） | 段级时间范围 + 全局 tombstone |
| **存储模型** | 单段 CSR（每个 NodeGroup 一个 CSR） | 多段 CSR（多个 segment 拼接时间线） |
| **扫描方式** | 单段连续扫描，按 VersionInfo 过滤 | 多段分散扫描 + HashSet 去重 |
| **读放大** | 极小（只读需要的行） | 较大（遍历所有 segment） |
| **写放大** | 极小（原地追加） | 中等（freeze + merge 写放大） |
| **Time-Travel** | 不支持 | 支持（核心优势） |
| **实现复杂度** | ~700 行（VersionInfo + UndoBuffer） | ~2500 行（segment + freeze + merge） |

**关键差异**：LadybugDB 的 MVCC 是**行级版本元数据**模式——每行只存储一个 `transaction_t` 作为插入/删除版本，扫描时通过 `startTS` 比较即可确定可见性。这完全不需要将数据分到多个 segment。

### 4.2 LadybugDB 模式的局限性

如果 Linkrs 完全采用 LadybugDB 的行级 MVCC 模式，会**损失 Time-Travel 能力**。因为 LadybugDB 的模式只跟踪"当前数据 vs 当前可见性"，不保留数据的历史版本。

要同时支持 MVCC + Time-Travel，有两种策略：

**策略 A：LadybugDB 模式 + 额外的历史版本链**
```
当前版本（CSR 主数据）+ VersionInfo → 可见性判断
历史版本（UndoBuffer / 版本链）→ Time-Travel
```
代价：UndoBuffer 需要保留大量历史记录，占用内存。

**策略 B：Linkrs 当前模式（多段 + tombstone）**
```
Segment[0..N]（每段是一个时间窗口的快照）→ 时间旅行
tombstone（标记删除）→ 可见性
```
代价：当前查询需要扫描多段。

### 4.3 策略选择的分水岭

| 场景 | 更好的策略 |
|------|-----------|
| 只需要事务隔离，不需要历史查询 | LadybugDB 行级 MVCC（策略 A） |
| 核心需求是 Time-Travel 分析 | Linkrs 多段（策略 B） |
| 两者都重要 | 混合模式（见第 6 节） |

---

## 5. 设计调整建议

### 5.1 改进项汇总

| # | 改进项 | 影响 | 优先级 |
|---|--------|------|--------|
| 1 | 当前时间查询不走 segment（维护合并快照） | P0：当前查询性能提升 10-50x | **高** |
| 2 | 稀疏顶点索引（跳过无关 segment） | P1：历史查询也能受益 | **高** |
| 3 | Segment 内嵌 min/max vertex ID 快速剪枝 | P1：简单实现，低成本 | **中** |
| 4 | 去掉 merged_edges_of 中的 HashSet 去重 | P2：如果 edge_id 不跨段重复，可以去掉 | **中** |
| 5 | 将 tombstones 从 HashMap 改为段内位图 | P2：减少 tombstone 内存和查找开销 | **低** |
| 6 | MVCC snapshot 关联 segment GC | P3：当无 snapshot 引用时提前淘汰旧 segment | **低** |

### 5.2 改进项 1（P0）：当前时间合并快照

这是最关键的改进。对于 `ts = MAX` 或 `ts >= latest_segment_ts` 的查询，应该维护一个**合并后的单一 CSR**。

```rust
impl EdgeTableCore {
    /// 获取当前时间的合并边列表（不遍历 segment）
    fn current_edges_of(&self, src: u32) -> Vec<Nbr> {
        let mut result = Vec::new();

        // 1. 从合并快照中读取（快速路径）
        if let Some(snapshot) = &self.current_snapshot {
            for edge in snapshot.edges_of(src) {
                if !self.mvcc.is_tombstoned(edge.edge_id, u32::MAX) {
                    result.push(edge);
                }
            }
        }

        // 2. 合并 Mutable CSR 中的最新边
        for edge in self.out_csr.edges_of(src, u32::MAX) {
            if !self.mvcc.is_tombstoned(edge.edge_id, u32::MAX) {
                result.push(edge);
            }
        }

        result
    }

    /// 获取历史时间的合并边列表（遍历 segment）
    fn historical_edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        // 走原有的 merged_edges_of 路径
        self.merged_edges_of(&self.out_csr, &self.out_segments, src, ts)
    }
}
```

**触发更新**：在每次 `freeze_csr_only()` 或 `merge_segments()` 成功后，异步重建 `current_snapshot`。

**代价**：
- 内存：额外一份合并 CSR ≈ 当前总边数 × 每条边的字节数
- 写放大：每次 merge 后需要重建，但 merge 本身就是批量操作

### 5.3 改进项 2（P1）：稀疏顶点索引

```rust
struct CsrSegment {
    pub csr: Csr,
    // ... 现有字段
    
    /// 新增：稀疏顶点索引
    /// true = 该顶点在此 segment 中可能有邻边
    /// false = 该顶点在此 segment 中一定没有邻边
    pub vertex_presence: RoaringBitmap,  // 或 fixed_bitmap
}
```

查询时：

```rust
fn base_edges_of(&self, segments: &[CsrSegment], src: u32, ts: Timestamp) -> Vec<Nbr> {
    for segment in segments.iter().rev() {
        if segment.create_ts_min > ts { continue; }
        if segment.deletion_info.all_deleted_before(ts) { continue; }
        // ★ 新增：顶点存在性检查
        if !segment.vertex_presence.contains(src) { continue; }
        
        for (position, edge) in segment.csr.edges_of_with_position(src) {
            // ...
        }
    }
}
```

**实现成本**：
- freeze 时：遍历 segment 的 CSR，收集所有 src vertex ID 到 RoaringBitmap
- 额外内存：如果使用 RoaringBitmap，每个 segment 约 顶点数 × 0.1-0.5 字节

**收益**：
- 对于度稀疏的顶点，可以跳过大量 segment
- 对于社交网络等幂律分布图，大多数顶点只出现在少数 segment 中

### 5.4 改进项 3（P1）：Segment 级 min/max vertex 剪枝

比稀疏索引更轻量的优化：

```rust
struct CsrSegment {
    // ... 现有字段
    pub vertex_id_min: u32,  // 新增
    pub vertex_id_max: u32,  // 新增
}
```

```rust
fn base_edges_of(...) {
    for segment in segments.iter().rev() {
        // ★ 新增：顶点 ID 范围剪枝
        if src < segment.vertex_id_min || src > segment.vertex_id_max {
            continue;
        }
        // ...
    }
}
```

这个优化在顶点 ID 连续分配时非常有效（例如，顶点 ID 按时间顺序递增分配，早期 segment 只有低 ID 顶点）。

### 5.5 改进项 4（P2）：消除 HashSet 去重

当前 `merged_edges_of` 使用 `HashSet<EdgeId>` 进行去重：

```rust
fn merged_edges_of(&self, delta, segments, src, ts) -> Vec<Nbr> {
    let mut seen = HashSet::new();  // ← 去重数据结构
    let mut result = Vec::new();

    for nbr in delta.iter_edges_of(src, ts) {
        if seen.insert(nbr.edge_id) { result.push(*nbr); }
    }
    for nbr in self.base_edges_of(segments, src, ts) {
        if seen.insert(nbr.edge_id) { result.push(nbr); }
    }
    result
}
```

如果每个 entry 的 edge_id 在 segment 中是**严格唯一的**（不跨段重复），则去重可以去掉。但这需要保证 freeze 时 entry 确实是从 Mutable CSR 移入 segment 而非复制，并且不会因 merge 产生重复。

实际上 merge 操作是物理合并（合并多个 segment 为一个新的 segment，旧 segment 被丢弃），所以理论上 edge_id 在 segment 之间不重复。但需要仔细验证 `delete_edge` 的行为——删除操作是否产生了跨段的 edge_id 重复。

### 5.6 改进项 5（P2）：段内删除位图

当前删除用全局 `HashMap<EdgeId, Timestamp>` 存储，查询时每个 edge 都要查一次 HashMap。

替代方案：在 segment 内部维护删除位图：

```rust
struct CsrSegment {
    pub csr: Csr,
    // ... 现有字段
    
    /// 段内删除位图：bit[i] = 1 表示第 i 条边已被删除
    pub deleted_bitmap: Option<RoaringBitmap>,
}
```

查询时，如果查询时间 `ts >= deletion_info.max_ts`，可以直接用位图过滤，**不需要查全局 tombstone HashMap**。

### 5.7 改进项 6（P3）：Segment GC 与 MVCC snapshot 关联

当前 segment 只在 merge 时被淘汰。可以增加基于 snapshot 引用的提前淘汰：

```rust
fn try_drop_unreferenced_segment(&mut self) -> bool {
    let min_ts = self.mvcc.get_min_active_snapshot_ts();
    // 如果某个 segment 的 create_ts_max < min_ts，且无活跃 snapshot 需要它
    // 则可以提前将它的数据合并到当前快照后丢弃
}
```

（但要注意，segment 本身就承载 Time-Travel 数据，如果 snapshot 不需要但用户可能发起历史查询，不能随便丢弃。）

---

## 6. 推荐方案：混合模式（Hybrid Approach）

综合考虑，不建议取消多段 segment（会损失 Time-Travel 能力），但建议采用**混合模式**：

```
                   ┌────────────────────────────┐
                   │    EdgeTableCore            │
                   │                            │
                   │  ┌──────────────────────┐  │
  当前状态查询 ──────►  │  CurrentSnapshot     │  │  ← 合并后的单段 CSR（新增）
                   │  │  (单段, 只读优化)     │  │     只用于 ts=MAX 或 ts≥latest
                   │  └──────────────────────┘  │
                   │                            │
                   │  ┌──────────────────────┐  │
                   │  │  Mutable CSR (delta)  │  │  ← 当前写入缓冲区
                   │  └──────────────────────┘  │
                   │                            │
  历史查询 ──────────►  │  ┌──────────────────────┐  │
                   │  │  Segments[0..N]       │  │  ← 历史不可变段（保留 Time-Travel）
                   │  │  (稀疏索引加速)        │  │     只用于 ts < 当前快照时间
                   │  └──────────────────────┘  │
                   └────────────────────────────┘
```

### 6.1 数据流变更

```
写流程:
  insert_edge() → out_csr → 写回压 → freeze → segment[new]
                     ↓
              失效化 current_snapshot（标记为 stale）

读流程（ts = MAX）:
  current_snapshot.is_stale()? → 异步重建从 segments
  合并 current_snapshot + out_csr (Mutable CSR)
  → 返回结果

读流程（ts < MAX）:
  merged_edges_of(out_csr, segments, src, ts)
  → 稀疏顶点索引跳过无关 segment
  → 返回结果
```

### 6.2 迁移路径

| 阶段 | 变更 | 预期收益 |
|------|------|---------|
| 1. 当前时间快照 | 新增 `current_snapshot: Option<Csr>`，merge 后重建 | 当前查询 O(1) segment |
| 2. 稀疏顶点索引 | 为每个 segment 添加 `vertex_presence: RoaringBitmap` | 历史查询跳过 >50% segment（按图分布） |
| 3. 段内删除位图 | 删除操作时更新段内位图，减少全局 tombstone 查找 | tombstone 检查 O(1) 段内位图 |
| 4. 移除 HashSet 去重 | 验证 edge_id 唯一性后移除 merged_edges_of 中的 HashSet | 减少 O(degree) 的哈希开销 |

### 6.3 不做（或推迟做）的变更

- **不取消多段**：Time-Travel 是差异化能力，不应牺牲
- **不引入行级版本元数据**：现有 segment 模式已支持 Time-Travel，引入行级版本会增加复杂度，且与 segment 时间范围机制重叠
- **不强制所有查询走合并快照**：避免写放大（每次 freeze 都重建快照）

---

## 7. 总结

**核心结论**：

1. **MVCC 确实不需要多段**——LadybugDB 用单段 + 行级版本元数据实现了更高效的 MVCC。但 Linkrs 的多段设计是为了**Time-Travel**，而非单纯的 MVCC。

2. **Time-Travel 当前实现确实需要改进**——最突出的问题是当前时间查询（ts=MAX）完全无法利用时间剪枝，需要遍历所有 segment。这是 P0 级问题。

3. **改进方向不是取消多段，而是分层**——将"当前状态查询"和"历史查询"的代码路径分离：
   - 当前查询 → 合并快照（单段 CSR，O(1) segment 访问）
   - 历史查询 → segment 遍历（保留原有代码，增加稀疏索引加速）

4. **稀疏顶点索引是第二优先级的优化**——可以在不改变整体架构的情况下，将历史查询的 segment 访问量降低 50% 以上（取决于图的度分布）。

5. **不需要的复杂度可以去掉了**——如果当前时间快照维护正确，`merged_edges_of` 中的 `HashSet` 去重可以验证后移除，因为合并后的快照天然无重复。