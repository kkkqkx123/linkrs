# MultiSingleMutableCsr 设计框架

## 概述

**目的**：支持"单边但多值"关系，即每顶点可对同一目标保留多个时序版本的边。

**适用场景**：
- 需要历史版本追踪（时间旅行查询）但关系本质为"单出边"（如配偶变更历史）
- 分布式系统中可能出现时间戳乱序的并发写
- 不想因此升级到通用 `MutableCsr`（后者内存开销更大）

**性能目标**：
- 绝大多数场景保持 O(1)（使用 SmallVec 内联）
- 多值时 O(k)，其中 k 通常 ≤ 2-3

---

## 1. 核心数据结构

### 定义

```rust
use smallvec::SmallVec;

pub struct MultiSingleMutableCsr {
    // 每顶点存储一个 SmallVec，可容纳 2 条边（内联，无堆分配）
    // 超过 2 条自动升级为 Vec
    nbr_lists: Vec<SmallVec<[Nbr; 2]>>,
    edge_count: AtomicU64,
    vertex_capacity: usize,
}
```

### 关键约束

1. **到同一目标的唯一性**：
   - 每顶点对同一 `dst` 最多保有 1 条有效边
   - 不同 `dst` 的多条边允许共存（违反则违反"单边"语义）

2. **时间戳语义**：
   - 同 dst、不同 ts：允许（支持多值）
   - 同 dst、相同 ts：拒绝（避免歧义）

3. **查询返回**：
   - `get_edge(src, dst, ts)`：返回最新的、满足 `ts' <= ts` 的边
   - `edges_of(src, ts)`：返回所有有效边的最新版本（按 dst 分组）

---

## 2. 核心操作

### insert_edge - 版本管理

```rust
pub fn insert_edge(
    &mut self,
    src: u32,
    dst: VertexId,
    edge_id: EdgeId,
    prop_offset: u32,
    ts: Timestamp,
) -> bool {
    let src_idx = src as usize;
    if src_idx >= self.vertex_capacity {
        self.ensure_vertex_capacity(src_idx + 1);
    }

    let nbr_list = &mut self.nbr_lists[src_idx];

    // 检查是否已存在到 dst 的边
    for nbr in nbr_list.iter_mut() {
        if nbr.neighbor == dst {
            // 已存在：按时间戳覆盖规则
            if ts <= nbr.timestamp {
                return false;  // 新边不够新，拒绝
            }
            // 更新为新版本（覆盖）
            nbr.edge_id = edge_id;
            nbr.prop_offset = prop_offset;
            nbr.timestamp = ts;
            return true;
        }
    }

    // 不存在：添加新边
    nbr_list.push(Nbr::new(dst, edge_id, prop_offset, ts));
    self.edge_count.fetch_add(1, Ordering::Relaxed);
    true
}
```

**关键点**：
- 到同 dst 的新更新会直接覆盖旧边（而非插入新边）
- 这保留了"单出边"的语义：同 dst 只有 1 条有效边
- SmallVec 避免多数情况的堆分配

### get_edge - 返回最新版本

```rust
pub fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
    let src_idx = src as usize;
    if src_idx >= self.vertex_capacity {
        return None;
    }

    self.nbr_lists[src_idx]
        .iter()
        .find(|nbr| {
            nbr.neighbor == dst 
            && nbr.timestamp != INVALID_TIMESTAMP 
            && nbr.timestamp <= ts
        })
        .copied()
}
```

### delete_edge - 标记失效

```rust
pub fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
    let src_idx = src as usize;
    if src_idx >= self.vertex_capacity {
        return false;
    }

    let nbr_list = &mut self.nbr_lists[src_idx];
    for nbr in nbr_list.iter_mut() {
        if nbr.edge_id == edge_id 
            && nbr.timestamp != INVALID_TIMESTAMP 
            && nbr.timestamp <= ts 
        {
            nbr.timestamp = INVALID_TIMESTAMP;
            self.edge_count.fetch_sub(1, Ordering::Relaxed);
            return true;
        }
    }
    false
}
```

### edges_of - 返回所有有效边

```rust
pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
    let src_idx = src as usize;
    if src_idx >= self.vertex_capacity {
        return Vec::new();
    }

    self.nbr_lists[src_idx]
        .iter()
        .filter(|nbr| nbr.timestamp != INVALID_TIMESTAMP && nbr.timestamp <= ts)
        .copied()
        .collect()
}
```

---

## 3. 序列化

### 格式

```
[Header]
- version: u8 = 1  // 新格式版本
- vertex_capacity: u64
- edge_count: u64

[Per-vertex Data]
for each vertex {
    - edge_count_for_vertex: u32
    for each edge {
        - neighbor: VertexId (variable length)
        - edge_id: u64
        - prop_offset: u32
        - timestamp: u32
    }
}
```

### 关键点

- `edge_count_for_vertex` 用来支持变长顶点数据
- 可拆分为多个段（segment）存储不同时间范围的数据

---

## 4. Compaction

```rust
pub fn compact_with_ts(&mut self, _ts: Timestamp, _reserve_ratio: f32) -> usize {
    let mut removed = 0;
    for nbr_list in &mut self.nbr_lists {
        // 清理 INVALID_TIMESTAMP 标记的边
        let original_len = nbr_list.len();
        nbr_list.retain(|nbr| nbr.timestamp != INVALID_TIMESTAMP);
        removed += original_len - nbr_list.len();
    }
    removed
}
```

---

## 5. CsrVariant 集成

### Enum 扩展

```rust
pub enum CsrVariant {
    Multiple(MutableCsr),                // 通用多边（原有）
    Single(SingleMutableCsr),            // 严格单边（原有）
    MultiSingle(MultiSingleMutableCsr),  // ← 新增
    None { vertex_capacity: usize },     // 无边（原有）
}
```

### 切换逻辑

```rust
impl CsrVariant {
    pub fn from_strategy(
        strategy: EdgeStrategy,
        vertex_capacity: usize,
        edge_capacity: usize,
    ) -> StorageResult<Self> {
        match strategy {
            EdgeStrategy::None => {
                Ok(CsrVariant::None { vertex_capacity })
            }
            EdgeStrategy::Single => {
                Ok(CsrVariant::Single(
                    SingleMutableCsr::with_capacity(vertex_capacity)
                ))
            }
            EdgeStrategy::MultiSingle => {  // ← 新增
                Ok(CsrVariant::MultiSingle(
                    MultiSingleMutableCsr::with_capacity(vertex_capacity)
                ))
            }
            EdgeStrategy::Multiple => {
                Ok(CsrVariant::Multiple(
                    MutableCsr::with_capacity(vertex_capacity, edge_capacity)
                ))
            }
        }
    }
}
```

### 扩展 EdgeStrategy 枚举

```rust
pub enum EdgeStrategy {
    None,
    Single,        // 严格单边，O(1)，不支持并发
    MultiSingle,   // 单边但多值，O(k)，支持时序版本
    #[default]
    Multiple,      // 通用多边
}
```

---

## 6. 测试用例

### 基础操作

```rust
#[test]
fn test_basic_operations() {
    let mut csr = MultiSingleMutableCsr::with_capacity(10);

    // 插入到 dst=1，ts=100
    assert!(csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 100));
    
    // 相同 dst，更新时间戳
    assert!(csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(101), 0, 150));
    
    // 不同 dst，新边
    assert!(csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(102), 0, 120));
    
    assert_eq!(csr.edge_count(), 2);  // 2 条有效边（不同 dst）
}
```

### 时间戳覆盖

```rust
#[test]
fn test_timestamp_overwrite() {
    let mut csr = MultiSingleMutableCsr::with_capacity(10);

    // 初始边
    assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 100));
    
    // 更新为更新的时间戳（覆盖）
    assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(101), 0, 150));
    
    // 尝试用更老的时间戳（拒绝）
    assert!(!csr.insert_edge(0, VertexId::from_int64(1), EdgeId(102), 0, 120));
    
    // 获取时应返回最新版本
    let edge = csr.get_edge(0, VertexId::from_int64(1), 200).unwrap();
    assert_eq!(edge.timestamp, 150);
    assert_eq!(edge.edge_id.0, 101);  // 最新的 edge_id
}
```

### 删除与时间戳

```rust
#[test]
fn test_delete_with_timestamp() {
    let mut csr = MultiSingleMutableCsr::with_capacity(10);

    // 插入三个版本
    csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 100);
    csr.insert_edge(0, VertexId::from_int64(1), EdgeId(101), 0, 150);
    csr.insert_edge(0, VertexId::from_int64(1), EdgeId(102), 0, 200);

    // 删除（标记为失效）
    assert!(csr.delete_edge(0, EdgeId(102), 200));

    // 查询应返回次新版本
    let edge = csr.get_edge(0, VertexId::from_int64(1), 250).unwrap();
    assert_eq!(edge.timestamp, 150);  // 退到上一个版本
}
```

---

## 7. 与 EdgeTable 的集成

**最小改动**：

```rust
// edge_table.rs
pub fn insert_edge(&mut self, src: VertexId, dst: VertexId, ...) {
    // 现有逻辑：match self.out_csr { ... }
    // 自动适配所有 CsrVariant，无需特殊处理
    self.out_csr.insert_edge(src_idx, dst, edge_id, prop_offset, ts);
}
```

因为 `MutableCsrTrait` 已包含所有必要方法，新 variant 只需实现同样的 trait。

---

## 8. 实施路线

### Phase 1：核心实现（~4h）
- [ ] 实现 `MultiSingleMutableCsr` 结构和操作
- [ ] 实现 `CsrBase` 和 `MutableCsrTrait`
- [ ] 基础 dump/load 序列化

### Phase 2：集成（~2h）
- [ ] 扩展 `EdgeStrategy` 枚举
- [ ] 更新 `CsrVariant` 
- [ ] 集成到 `EdgeTable`

### Phase 3：测试（~3h）
- [ ] 单元测试（操作、时间戳、删除）
- [ ] 序列化测试（dump/load）
- [ ] 集成测试（与 EdgeTable 交互）

### Phase 4：优化与文档（~1h）
- [ ] 性能基准
- [ ] 使用文档

---

## 9. 与现有设计的关系

| 特性 | SingleMutableCsr | MultiSingleMutableCsr | MutableCsr |
|------|------------------|----------------------|------------|
| 每顶点最多边数 | 1 | 1 per dst | 无限 |
| 支持多值 | ❌ | ✓ (per dst) | ✓ |
| 时间戳覆盖 | ✓ | ✓ | ✓ |
| 读取 O(1) | ✓ | ✓ (绝大多数) | ✓ |
| 内存开销 | 低 | 低-中 | 中高 |
| 适用场景 | 严格单边 | 单边+历史 | 通用多边 |

---

## 10. 决策流程

```
需要单边关系吗？
├─ 否 → 用 MutableCsr（多边）
│
└─ 是
   ├─ 需要支持并发/时序多值？
   │  ├─ 否 → 用 SingleMutableCsr（严格单边）
   │  │
   │  └─ 是 → 用 MultiSingleMutableCsr（单边+多值）
   │
```

---

## 注意事项

⚠️  **此设计仅在业务确实需要时实施**。大多数场景下：
1. 严格单边 → `SingleMutableCsr`
2. 支持多值 → `MutableCsr`

`MultiSingleMutableCsr` 是一个中间层，适用于性能和功能之间的平衡需求。
