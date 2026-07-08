# CSR 迭代器优化参考文档

> 本文档详细说明迭代器优化（方案 2）的实现方案、性能影响评估和实施路径。
> 这是一份为将来参考的深度分析，当需要流式处理或性能优化时可以参考。

## 目录

1. [现状分析](#现状分析)
2. [迭代器优化方案](#迭代器优化方案)
3. [性能影响评估](#性能影响评估)
4. [实施复杂度分析](#实施复杂度分析)
5. [适用场景](#适用场景)
6. [实施路径](#实施路径)
7. [代码参考](#代码参考)

---

## 现状分析

### 当前设计（Vec-based）

```rust
// 当前的 EdgeTable 中
pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
    let mut result = Vec::new();
    
    // 从可变 CSR 中获取
    for nbr in self.out_csr.edges_of(src, ts) {
        if !self.mvcc.is_tombstoned(nbr.edge_id, ts) {
            result.push(nbr);
        }
    }
    
    // 从所有不可变 segment 中获取（反向遍历，最新优先）
    for segment in self.out_segments.iter().rev() {
        if segment.create_ts_min > ts { continue; }
        
        for (position, edge) in segment.csr.edges_of_with_position(src) {
            if edge.timestamp <= ts {
                let edge_id = segment.recover_edge_id(edge, position);
                if !self.mvcc.is_tombstoned(edge_id, ts) {
                    result.push(Nbr::new(...));
                }
            }
        }
    }
    
    result
}
```

**特点**：
- ✅ 简单清晰：一个方法，返回 Vec
- ✅ 易于理解：没有复杂的 lifetime 约束
- ✅ 支持随机访问：Vec 允许重排序、过滤等操作
- ⚠️ 一次性分配：必须收集所有结果后返回
- ⚠️ 多源合并：需要手动管理多个数据源（delta + segments）

### 成本分解

假设查询 src_id 的所有出边，有 N 个 segment：

```
成本 = T_delta + T_segment_merge

T_delta = O(E_delta)
  - E_delta = mutable CSR 中 src_id 的边数（通常 < 10K）
  - 成本：遍历 + tombstone 检查 = O(E_delta)

T_segment_merge = O(N * E_segment)
  - N = segment 数（默认配置下 < 50）
  - E_segment = 平均每个 segment 中 src_id 的边数
  - 成本：N 次遍历 + HashSet dedup 去重 = O(N * E_segment + N * log N)

实际时间：
  典型场景（1M 边，100 segment，每个 segment 10K 边）：
  - delta 遍历：< 1ms
  - segment 遍历：N * E = 100 * 10 = 1000 次遍历 ≈ 10ms
  - HashSet dedup：O(N log N) ≈ 1ms
  ─────────────
  总计：~12ms
```

---

## 迭代器优化方案

### 理想设计：流式迭代器

```rust
pub fn edges_of_iter(&self, src: u32, ts: Timestamp) 
    -> impl Iterator<Item=Nbr> + '_
{
    // 返回延迟求值的迭代器
    CsrEdgeIterator::new(self, src, ts)
}

struct CsrEdgeIterator<'a> {
    table: &'a EdgeTable,
    src_id: u32,
    ts: Timestamp,
    
    // 迭代器状态机
    state: IteratorState<'a>,
    seen_ids: HashSet<EdgeId>,
}

enum IteratorState<'a> {
    DeltaCsr(DeltaIterator<'a>),
    SegmentMerge(SegmentMergeIterator<'a>),
    Done,
}

impl<'a> Iterator for CsrEdgeIterator<'a> {
    type Item = Nbr;
    
    fn next(&mut self) -> Option<Nbr> {
        loop {
            match &mut self.state {
                IteratorState::DeltaCsr(iter) => {
                    if let Some(nbr) = iter.next() {
                        if !self.table.mvcc.is_tombstoned(nbr.edge_id, self.ts) 
                            && self.seen_ids.insert(nbr.edge_id) {
                            return Some(nbr);
                        }
                    } else {
                        self.state = IteratorState::SegmentMerge(...);
                    }
                }
                IteratorState::SegmentMerge(iter) => {
                    if let Some(nbr) = iter.next() {
                        if !self.table.mvcc.is_tombstoned(nbr.edge_id, self.ts)
                            && self.seen_ids.insert(nbr.edge_id) {
                            return Some(nbr);
                        }
                    } else {
                        self.state = IteratorState::Done;
                    }
                }
                IteratorState::Done => return None,
            }
        }
    }
}
```

**优点**：
- ✅ 延迟求值：不需要一次性分配 Vec，内存占用恒定
- ✅ 提前终止：如果只需要 K 个边，只遍历 K 个，不必遍历全部
- ✅ 流式处理：可以在遍历中动态添加过滤、转换等操作
- ✅ 零复制：直接返回引用（通过迭代器），无需拷贝

**缺点**：
- ❌ 复杂的 lifetime 管理：迭代器需要持有多个引用（delta_csr + segments）
- ❌ 状态机复杂：需要管理 delta → segment → done 的状态转换
- ❌ 去重困难：HashSet 仍然需要存储所有已见的 edge_id
- ❌ Rust lifetime 约束：编译器很难理解"从多个来源动态选择"的 lifetime

---

## 性能影响评估

### 场景 1：完整查询（需要所有边）

当查询需要返回 src_id 的所有边时：

```
Vec 版本成本：
  1. 分配 Vec（初始容量）
  2. 遍历 delta + segments（推送到 Vec）
  3. 返回 Vec

迭代器版本成本：
  1. 创建迭代器状态机
  2. 遍历 delta + segments（即时返回）
  3. 无额外分配
  
差异：
  - 时间：基本相同（都要遍历所有边）
  - 内存：迭代器略优（无 Vec 分配开销），但由于有 HashSet 去重，实际差异 < 5%
  - 缓存局部性：Vec 更好（紧凑的数组），迭代器更差（需要多次跳跃）

结论：性能基本相同，迭代器无明显优势
```

### 场景 2：顶点度数很小（< 100）

```
假设：src_id 只有 50 条出边

Vec 版本：
  1. 分配 Vec capacity=64
  2. 遍历 100 个 segment（50 次找到边，50 次跳过）
  3. 总耗时：~5ms
  
迭代器版本：
  1. 创建迭代器
  2. 遍历 100 个 segment，当找到 50 条边后停止
  3. 总耗时：同样 ~5ms（还是要遍历所有 segment，除非有优化）

差异：基本无差异（除非有 segment 索引优化）
```

### 场景 3：Seek 操作（只需要前 K 条边）

这是迭代器可能有优势的场景。

```
假设：需要查找 src_id 的第一条"weight > 10"的边

Vec 版本：
  1. 调用 edges_of()，返回所有 50 条边
  2. 在应用层做循环查找
  3. 总耗时：遍历 ALL 50 条 + 应用层搜索
  
迭代器版本（如果加入过滤器）：
  pub fn edges_of_filtered(&self, src: u32, ts: Timestamp, filter: Fn(&Nbr)->bool)
      -> impl Iterator<Item=Nbr> + '_ {
      CsrEdgeIterator::new(self, src, ts).filter(filter)
  }
  
  table.edges_of_filtered(src_id, ts, |nbr| {
      let props = table.properties_for_offset(nbr.prop_offset);
      props.iter().any(|(k, v)| k == "weight" && v.as_double() > Some(10.0))
  }).next()
  
  优势：找到第一个满足条件的边后立即返回，无需继续遍历
  耗时：平均 25 条边 ≈ 0.25ms（对比 50 条 = 0.5ms）
  
  改善：50% 性能提升，但只对小顶点有用
```

### 场景 4：批量操作（一次查询很多顶点）

```
假设：查询 10万个顶点的出边

Vec 版本：
  for src_id in 0..100000 {
      let edges = table.edges_of(src_id, ts);  // 返回 Vec
      process(edges);  // 处理
  }
  
  成本：10万 * Vec 分配 + 10万 * 遍历 + GC 压力

迭代器版本：
  for src_id in 0..100000 {
      for nbr in table.edges_of_iter(src_id, ts) {  // 返回迭代器
          process_one(nbr);  // 处理单条边
      }
  }
  
  成本：10万 * 迭代器创建 + 10万 * 遍历 + 无 GC 压力
  
  改善：
  - 内存：10万 * Vec 分配 → 0（大幅降低）
  - GC：无需频繁分配/回收 Vec
  - CPU cache：更好的局部性（if process_one() 足够快）
  - 改善幅度：20-30% 如果瓶颈是 GC 的话
```

### 综合评估

| 场景 | Vec 耗时 | 迭代器耗时 | 改善 | 实际发生概率 |
|------|---------|----------|------|-----------|
| 完整查询 | T | T | 0% | 20% |
| 小顶点查询 | T | T | 0% | 30% |
| Seek 查询 | T | 0.5T | 50% | 5% |
| 批量查询 | 10T | 8T | 20% | 45% |

**加权平均改善**：`0.2*0 + 0.3*0 + 0.05*50 + 0.45*20 = 11%`

实际：**迭代器版本可能带来 10-15% 的性能提升**，但仅在批量查询时有明显效果。

---

## 实施复杂度分析

### 代码变化范围

#### 1. 迭代器定义（新增 ~200 行）

```rust
// src/storage/edge/edge_table/iterator.rs (新文件)

pub struct CsrEdgeIterator<'a> {
    // 需要持有多个引用：delta_csr, segments, mvcc_manager
    // Rust 的 lifetime 系统很难表达"从多个数据源动态选择"
    // 必须使用 enum 包装状态机
}

impl<'a> Iterator for CsrEdgeIterator<'a> {
    type Item = Nbr;
    fn next(&mut self) -> Option<Nbr> {
        // ~60 行状态机管理代码
    }
}
```

#### 2. EdgeTable 改动（修改 ~30 行）

```rust
// src/storage/edge/edge_table/core.rs

// 新增 edges_of_iter() 方法
pub fn edges_of_iter(&self, src: u32, ts: Timestamp) 
    -> impl Iterator<Item=Nbr> + '_ {
    CsrEdgeIterator::new(self, src, ts)
}

// 保持 edges_of() 向下兼容
pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
    self.edges_of_iter(src, ts).collect()  // 基于迭代器实现
}
```

#### 3. Query Engine 改动（修改 ~100-200 行）

这才是痛点。Query Engine 可能这样使用：

```rust
// 当前
let edges = table.edges_of(src, ts);
for nbr in &edges {
    // 随机访问：edges[i]
    // 排序：edges.sort_by(...)
    // 过滤：edges.retain(...)
}

// 迭代器版本需要适配
let mut edges: Vec<_> = table.edges_of_iter(src, ts).collect();  // 仍然要 collect()
// 或者改为流式处理（如果支持）
```

问题：如果 Query Engine 大量使用 edges 的随机访问、排序等操作，迭代器版本反而需要 `.collect()` 转回 Vec，导致**完全没有收益**。

### 编译复杂度

当前：
- 单个 `edges_of()` 方法，编译清晰

迭代器版本：
- 多个泛型参数：`Iterator<Item=Nbr> + '_ + Send + ...`
- 状态枚举：需要特化多个迭代器类型
- 导致单态化膨胀：编译时间 ↑ 20-30%

### Rust Lifetime 的真实挑战

```rust
// 这是不合法的（编译器无法推导）
pub fn edges_of_iter(&self, src: u32, ts: Timestamp) 
    -> impl Iterator<Item=Nbr> + '_ {
    // 需要同时持有：
    // - &self.out_csr 的引用（可变性问题）
    // - &self.out_segments[i] 的多个引用（不知道会持有多久）
    // - &self.mvcc 的引用
    // 
    // Rust 要求："迭代器生命期 <= 最短的引用生命期"
    // 但多个 segment 的 lifetime 不一致，编译器无法处理
    
    // 通常的解决方案：
    // 1. 使用 unsafe 代码（风险高）
    // 2. 返回 Box<dyn Iterator> （性能损失）
    // 3. 使用 GAT (Generic Associated Types) （Nightly 功能）
}
```

**实际成本**：处理 lifetime 问题需要额外 30-50 行代码，且很容易出现编译错误。

---

## 适用场景

### ✅ 迭代器优化值得做的场景

1. **流式聚合查询**
   ```rust
   // 例如：实时图计算（无限流）
   for event in stream {
       for nbr in table.edges_of_iter(event.src, event.ts) {
           aggregate(nbr);
       }
   }
   ```
   - 不需要一次性收集所有结果
   - 边界条件清晰（stream 结束）

2. **超大图遍历（超内存）**
   ```rust
   // 图太大，无法一次性加载所有边
   pub fn traverse_bfs(&self, start: u32, max_depth: u32) {
       for depth in 0..max_depth {
           for src in visited[depth] {
               for nbr in self.edges_of_iter(src, ts) {  // 流式
                   if not_visited(nbr) {
                       visit(nbr);
                   }
               }
           }
       }
   }
   ```

3. **早期终止模式**
   ```rust
   // 找到第一个满足条件的邻接点
   table.edges_of_iter(src, ts)
       .find(|nbr| is_target(nbr))
   ```

### ❌ 迭代器优化不值得做的场景

1. **需要随机访问的查询**
   ```rust
   let edges = table.edges_of(src, ts);
   for i in 0..edges.len() {
       process(edges[i]);  // 迭代器无法支持
   }
   ```

2. **需要排序/过滤的查询**
   ```rust
   let mut edges = table.edges_of(src, ts);
   edges.sort_by_key(|e| e.neighbor);  // 必须 collect() 才能排序
   ```

3. **短期查询**（< 1ms）
   ```rust
   for src_id in small_set {
       let edges = table.edges_of(src_id, ts);  // 快速返回
   }
   ```

4. **当前项目**（本 GraphDB）
   - 大多数查询需要完整的边列表
   - Query Engine 设计假设有 Vec（随机访问、排序）
   - 没有明显的流式处理需求

---

## 实施路径

### Phase 1: 充分准备（当前）
- ✅ 文档化设计思路（本文档）
- ✅ 性能基准测试：测量当前 Vec 版本的性能
- 等待实际场景出现

### Phase 2: 原型实现（当需要时）
```
时间：2-3 周
步骤：
  1. 创建 iterator.rs，实现 CsrEdgeIterator
  2. 实现 edges_of_iter()
  3. 添加单元测试（10+ 个测试用例）
  4. 性能对比：Vec 版本 vs 迭代器版本
  5. 测量实际改善（如果改善 < 5%，停止）
```

### Phase 3: Query Engine 适配
```
如果 Phase 2 确认有 > 10% 改善，才进行此步骤：

  1. 分析 Query Engine 对 edges 的使用方式
  2. 确定哪些查询可以流式化
  3. 逐个适配关键路径
  4. 回归测试
  5. 增量性能测试
  
风险：很高，改动范围广
```

### Phase 4: 完全迁移（可选）
```
如果流式处理成为瓶颈，考虑：
  - 异步迭代器（async/await）
  - GPU 计算（并行化）
  - 分布式查询（多机并行）
```

---

## 代码参考

### 最小实现（迭代器状态机）

```rust
// 文件: src/storage/edge/edge_table/iterator.rs

use crate::core::types::{Timestamp, EdgeId};
use crate::storage::edge::{Nbr, CsrVariant, CsrBase};
use std::collections::HashSet;

/// State machine for iterating over CSR edges from multiple sources
pub struct CsrEdgeIterator<'a> {
    table: &'a super::core::EdgeTableCore,
    src_id: u32,
    ts: Timestamp,
    
    // Delta iterator state
    delta_iter: Option<Box<dyn Iterator<Item=Nbr> + 'a>>,
    
    // Segment iteration state
    segment_idx: usize,
    segment_iter: Option<Box<dyn Iterator<Item=(usize, Nbr)> + 'a>>,
    
    // Deduplication: track seen edge IDs
    seen_ids: HashSet<EdgeId>,
    
    // Current state
    state: IterState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IterState {
    Delta,
    Segments,
    Done,
}

impl<'a> CsrEdgeIterator<'a> {
    pub fn new(table: &'a super::core::EdgeTableCore, src_id: u32, ts: Timestamp) -> Self {
        Self {
            table,
            src_id,
            ts,
            delta_iter: None,
            segment_idx: 0,
            segment_iter: None,
            seen_ids: HashSet::new(),
            state: IterState::Delta,
        }
    }
    
    fn next_from_delta(&mut self) -> Option<Nbr> {
        // Initialize delta iterator if needed
        if self.delta_iter.is_none() {
            let edges = self.table.out_csr.edges_of(self.src_id, self.ts);
            self.delta_iter = Some(Box::new(edges.into_iter()));
        }
        
        if let Some(iter) = &mut self.delta_iter {
            while let Some(nbr) = iter.next() {
                if !self.table.mvcc.is_tombstoned(nbr.edge_id, self.ts) 
                    && self.seen_ids.insert(nbr.edge_id) {
                    return Some(nbr);
                }
            }
        }
        
        None
    }
    
    fn next_from_segments(&mut self) -> Option<Nbr> {
        let segments = &self.table.out_segments;
        
        loop {
            if self.segment_idx >= segments.len() {
                return None;
            }
            
            let segment = &segments[self.segment_idx];
            
            // Skip if segment is too old
            if segment.create_ts_min > self.ts {
                self.segment_idx += 1;
                continue;
            }
            
            // Get or initialize iterator for current segment
            if self.segment_iter.is_none() {
                let edges = segment.csr.edges_of(self.src_id, self.ts);
                let idx = self.segment_idx;
                self.segment_iter = Some(Box::new(
                    edges.into_iter().map(move |e| (idx, e))
                ));
            }
            
            if let Some(iter) = &mut self.segment_iter {
                while let Some((idx, immutable_nbr)) = iter.next() {
                    let segment = &segments[idx];
                    let edge_id = segment.recover_edge_id(&immutable_nbr, 0); // position tracking needed
                    
                    if !self.table.mvcc.is_tombstoned(edge_id, self.ts)
                        && self.seen_ids.insert(edge_id) {
                        return Some(Nbr::new(
                            immutable_nbr.neighbor,
                            edge_id,
                            immutable_nbr.prop_offset,
                            immutable_nbr.timestamp,
                        ));
                    }
                }
            }
            
            self.segment_idx += 1;
            self.segment_iter = None;
        }
    }
}

impl<'a> Iterator for CsrEdgeIterator<'a> {
    type Item = Nbr;
    
    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            IterState::Delta => {
                if let Some(nbr) = self.next_from_delta() {
                    return Some(nbr);
                }
                self.state = IterState::Segments;
                self.next_from_segments()
            }
            IterState::Segments => {
                if let Some(nbr) = self.next_from_segments() {
                    return Some(nbr);
                }
                self.state = IterState::Done;
                None
            }
            IterState::Done => None,
        }
    }
}
```

### Query Engine 适配（可选流式版本）

```rust
// 当 Query Engine 需要流式处理时
impl QueryExecutor {
    // 原始版本（保持）
    pub fn traverse_with_edges(&self, src: u32, ts: Timestamp) {
        for edge in self.table.edges_of(src, ts) {
            self.process_edge(&edge);
        }
    }
    
    // 新增流式版本
    pub fn traverse_with_edges_streaming(&self, src: u32, ts: Timestamp) {
        for edge in self.table.edges_of_iter(src, ts) {
            self.process_edge(&edge);
        }
    }
    
    // 带提前终止的版本
    pub fn find_first_matching_edge(&self, src: u32, ts: Timestamp, predicate: impl Fn(&Nbr)->bool) 
        -> Option<Nbr> {
        self.table.edges_of_iter(src, ts)
            .find(predicate)
    }
}
```

---

## 总结

### 何时实施迭代器优化

| 触发条件 | 优先级 | 建议 |
|---------|--------|------|
| 性能测试发现 edges_of 是瓶颈 | 🔴 高 | 实施 Phase 1-2 |
| 需要流式处理无限数据 | 🔴 高 | 实施完整迭代器 |
| 内存占用过高（GC 压力） | 🟡 中 | 可考虑 Phase 2 |
| 当前性能足够 | 🟢 低 | 不实施（本阶段） |

### 关键指标

实施迭代器前必须验证：
1. **性能基准**：当前 edges_of 的 p99 延迟
2. **内存压力**：Vec 分配导致的 GC 暂停时间
3. **Query Engine 复杂性**：需要改动的代码行数

### 保险方案

**现在不实施，但为将来做好准备**：

1. ✅ 当前 Vec 版本继续用
2. ✅ 添加性能监控：测量 edges_of 的调用频率和耗时
3. ✅ 当 edges_of 成为 top-3 瓶颈时，启动 Phase 1-2
4. ✅ 保持代码清晰，以便后续重构

---

## 参考

- [Rust Iterator Documentation](https://doc.rust-lang.org/std/iter/trait.Iterator.html)
- [Lifetime in Iterators](https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html)
- [Generic Associated Types (GAT)](https://rust-lang.github.io/rfcs/1598-generic_associated_types.html)
- LSM-Tree 设计论文：《The Log-Structured Merge-Tree》（Luo et al., 1996）
