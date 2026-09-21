# 碎片与回收

## 问题：碎片从何而来？

### 两级存储设计

`MutableCsr` 用两级结构避免 O(n) 重排：

```
初始态（主行按密度目标预留日常写入空位）：
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  主行
+---------------------------------+

V0 写满后（溢出块按顶点追加，从不拷贝旧块）：
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  主块（不动）
+---------------------------------+
     +---> [E3, ...] chunk 1
     +---> [E7, ...] chunk 2        分级定长溢出块

行超块数上限且持有死项时在写路径重排；移除使块置空即立即摘除。
```

### 根因

浪费来自两处，不存在不可达块：

1. 主行按紧凑密度目标预留空位；未填满的空位在日常写入或合并消费前
   一直是浪费。
2. 删除项在回收截止线通过前物理保留，其槽位计为浪费。

高度数顶点 spill 到分级溢出块。空块在移除路径上立即摘除，
无不可达块账目残留。

### 累积效应

长期运行后：

- 墓碑槽与未填空位主导 `wasted_capacity`；
- 整表比例仅作观测；回收触发用按顶点可回收计数；
- 组级合并收紧存活项，恢复密度目标预留。

---

## 度量碎片

### 碎片率

**定义**（触发与面板共用的单一浪费占比口径）：

```
fragmentation_ratio = wasted_capacity / total_capacity
                    = (total_capacity - live_edges) / total_capacity
```

**示例**：

- `0.0`：无浪费（完全紧凑）
- `0.5`：一半浪费（组级合并门限 `GROUP_FRAGMENTATION_THRESHOLD`）
- 趋近 `1.0`：预留容量几乎全是浪费（恒小于 1，有存活边即如此）

取值恒在 0.0–1.0 之间；凡写“大于 1.5/2.0/3.0 才回收”者皆为过时笔误，
应以 `>= 0.5` 为组门限、以按顶点可回收计数为行触发。

**位置**：`MutableCsr::fragmentation_ratio()`

```rust
pub fn fragmentation_ratio(&self) -> f32 {
    if self.total_edge_capacity == 0 {
        return 0.0;
    }
    let active_edges = self.edge_count as usize;
    self.total_edge_capacity.saturating_sub(active_edges) as f32
        / self.total_edge_capacity as f32
}
```

明细口径见 `FragmentationStats`：

```rust
pub struct FragmentationStats {
    pub total_capacity: usize,   // 主行 + 溢出块预留总量
    pub reachable_edges: usize,  // 存活边
    pub dead_entries: usize,     // 物理存活项 - 存活边（主墓碑 + 溢出死项）
    pub wasted_capacity: usize,  // 预留 - 存活边（空位 + 墓碑槽）
}
```

按顶点视图 `VertexFragmentation { vertex, live_edges, dead_entries,
capacity, reclaimable }`：`reclaimable > 0` 即该行是回收候选
（持有当前截止线已覆盖的墓碑）。

### 诊断

```rust
let ratio = csr.fragmentation_ratio();
if ratio >= GROUP_FRAGMENTATION_THRESHOLD {
    println!("High fragmentation: {:.2}", ratio);
}
```

---

## 回收：恢复手段

### 目的

把主块 + 溢出块合并为扁平主块布局：

- 消除溢出链（全部存活项落回各自主块）；
- 丢弃截止线下的墓碑并逐边上报；
- `fragmentation_ratio()` 回到预留水平；
- 减小序列化体积。

### 方法签名

整表重建（恢复与离线重建用，生产不用）：

```rust
// MutableCsr
pub fn compact_with_ts_reporting(
    &mut self,
    cutoff: Timestamp,
    reserve_ratio: f32,
    on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
) -> usize
```

行级回收（生产路径，按顶点调用）：

```rust
fn compact_vertex_with_reporting(
    &mut self,
    vid: u32,
    cutoff: Timestamp,
    on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
) -> usize
```

单槽形态签名无 `reserve_ratio`：

```rust
// SingleMutableCsr
pub fn compact_with_ts_reporting(
    &mut self,
    cutoff: Timestamp,
    on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
) -> usize
```

旧 `compact_with_ts(cutoff, reserve)`（无上报、返回计数语义不明）
已删除，一律用带 `_reporting` 的上报入口。

**参数**：

- `cutoff`：GC 水位。`delete_ts` 在水位下的墓碑被丢弃并上报；
  水位及之上的墓碑保留，快照历史不丢。
  `Timestamp::MAX` 表示“只搬迁、不丢弃”。
- `reserve_ratio`（仅整表入口）：未来增长预留比例，
  `0.25` 即多留 25% 容量，减少刚收完即扩；
  `>= 1.0` 按无预留处理（防除零爆炸）。
- `on_edge_removed`：每丢弃一个墓碑回调一次 `(edge_id, delete_ts)`，
  调用方集中提升删除。

**返回**：移除的墓碑数。

### 算法

```
compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed):
  1. 只读第一遍：逐行数保留项，定各行新容量（含预留）
  2. 分配一座新主块；第二遍把保留项直拷到位
     （存活项 + 水位及上墓碑保留；水位下墓碑丢弃并上报）
  3. 空位填 dead_gap() 哨兵；重建偏移/度数/容量
  4. 清空溢出链，重建存活集，重置复用提示与行序缓存
  5. 返回移除数
```

### 复杂度

- **时间**：O(V + E)，全顶点全边走一遍
- **空间**：O(E)，一座新边表（无中间 staging 全拷贝）
- **锁**：独占写（单写者纪律，调用方串行化）

### 示例

```rust
// 回收前
let ratio = csr.fragmentation_ratio();  // 0.8

// 回收：丢水位下墓碑并上报，留 25% 预留
let removed = csr.compact_with_ts_reporting(500, 0.25, &mut |edge_id, ts| {
    println!("promoted tombstone {:?} at {}", edge_id, ts);
});

// 回收后
let ratio = csr.fragmentation_ratio();  // 回到预留水平约 0.2
```

---

## 何时回收

### 用 fragmentation_ratio()

```rust
// 组级门限判断（观测口径）
if csr.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD {
    // 生产走按行 compact_vertex_with_reporting；
    // 整表 compact_with_ts_reporting 仅恢复/离线用
}

// 持久快照前
if csr.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD {
    // 按行回收后再做组级合并，保持载荷紧凑
}
```

### 场景

| 场景 | 时机 | 动作 |
|---|---|---|
| 高吞吐写入 | 极少 | 观测比例，闲时按行回收 |
| 批量删除后 | 大删之后 | 回收墓碑密集行 |
| 序列化前 | 快照时 | 脏组先合并，保持载荷紧凑 |
| 定期维护 | 定时任务 | 如每小时查 `ratio >= 0.5` 的组 |
| 内存压力 | 接近预算 | 紧急回收墓碑行 |

---

## 代价权衡

### 不回收的代价

| 影响 | 效果 |
|---|---|
| 磁盘占用 | 快照被空位与墓碑撑大 |
| 网络 | 脏组载荷传输变大 |
| 缓存效率 | 死项浪费缓存行（冷端扫描除外：纯拓扑扫描不碰时间戳行） |
| 查询延迟 | 扫描死块的轻微开销 |

### 回收的代价

| 影响 | 效果 |
|---|---|
| CPU 时间 | O(V + E) 全量扫描重写（整表入口） |
| 锁时长 | 独占写（阻塞同组其他写） |
| 内存峰值 | 重写期约双倍 |
| 延迟毛刺 | 回收期同组查询被挡 |

### 建议

- **高并发 OLTP**：极少整表回收，用按行回收摊 cost；
- **OLAP / 分析**：导出快照前回收（体积优先）；
- **批量导入**：大批量插入后回收（消除初期溢出）；
- **重保留负载**：墓碑占比超 50% 时按水位回收。

---

## 各变体的回收

### MutableCsr（Multiple）

- 完整回收：合并溢出、丢水位下墓碑并逐边上报、
  按密度目标恢复预留；
- 额外有热路径墓碑复用（`tombstone_reuse_cutoff` 水位提示，
  内存态）：水位下墓碑槽可被新写直接复用，减少溢出分配。

### SingleMutableCsr

- 水位下单槽墓碑清槽并上报；O(1) 直接槽，无溢出、无整表预留参数。

### Pure / Bundled

- 行级回收；Bundled 拓扑与值双列同步移动，有效位同步清理。

### Frozen / Mapped / None

- 无操作：只读或零边。

---

## 软删除语义

### 时间戳字段

```rust
// 行内仅存删除戳副本；create_ts 在版本权威中
pub struct Nbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
    pub delete_ts: Timestamp,   // MAX = 存活
}
```

行戳是物理投影，仅供回收；查询可见性由上层版本权威裁决，
`Nbr::is_alive_at` 探针不得用于查询（仅维护路径）。

### 可见窗口

版本权威判定边在 `T` 可见当且仅当：

```
create_ts <= T && T < delete_ts
```

### 软删除流程

1. **删除**：盖 `delete_ts = current_ts` 戳；
2. **查询**：版本权威过滤 `delete_ts <= query_ts` 的边；
3. **回收**：丢 `delete_ts` 在 GC 水位下的墓碑，
   逐边上报以便墓碑层同步。

**收益**：

- 删除快（无重分配）；
- MVCC（多快照各见其态）；
- 时间旅行查询（查历史态）；
- 可撤销（回滚重置 `delete_ts`）。

---

## 序列化与碎片

### dump() 持久化按组列载荷

```rust
fn dump_into(&self, out: &mut Vec<u8>) {
    // 借用存活拓扑逐列编码，峰值 = 一列物化 + 输出缓冲。
}
```

**影响**：

- 脏组持久化其空位直至被合并；
- 检查点前按组合并保持载荷紧凑。

**缓解**：

- `ratio >= GROUP_FRAGMENTATION_THRESHOLD` 时先按行回收再序列化。

### load() 重建碎片态

从快照反序列化精确碎片态（含空位与墓碑分布，行序缓存与
复用水位等内存态重建为默认值）。加载后可查
`fragmentation_ratio()`，超组门限则回收。

---

## 未来优化

### 懒回收

- 标记待回收块，延迟到闲时批量执行。

### 增量回收

- 一次一顶点，O(V + E) 摊到多次操作。

### 自适应阈值

- 观测负载模式，自调回收门限，写速下降时提前触发。
