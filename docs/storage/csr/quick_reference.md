# CSR 速查

## 变体选型指南

```
有一对一关系（配偶、现雇主）？
  YES -> Columnar + Single（O(1)，内存省；第二条存活边报 Conflict，不静默覆盖）
  NO  -> 继续

无属性、无 rank、无时间戳的纯拓扑大图？
  YES -> Pure 记录形态（12 字节/边）
  NO  -> 继续

恰好一个内联数值属性、读多写少、模式稳定？
  YES -> Bundled 记录形态（20 字节/边；要冻结须先迁回列式）
  NO  -> 继续

通用多边关系（好友、关注）？
  YES -> Columnar + Multiple（默认，最灵活）
  NO  -> 继续

读为主、长期不变的组？
  YES -> 显式 freeze 到 Frozen；open 时边车有效则走 Mapped（mmap 视图）
  NO  -> 继续

该方向不存边？
  YES -> None（组级占位；注意建表校验拒绝单向表）
```

## 代码示例

### 按策略建 CSR

```rust
use crate::edge::{CsrVariant, EdgeStrategy};

// 通用多边
let csr = CsrVariant::from_strategy_with_overflow(
    EdgeStrategy::Multiple,
    1000,   // 顶点容量
    10000,  // 边容量
    4096,   // 每溢出块边数（须 > 0）
)?;

// 一对一
let csr = CsrVariant::from_strategy_with_overflow(
    EdgeStrategy::Single,
    1000, 0, 4096,
)?;

// 占位
let csr = CsrVariant::from_strategy_with_overflow(
    EdgeStrategy::None,
    1000, 0, 4096,
)?;

// Pure / Bundled / Frozen / Mapped 不走策略工厂：
// 由组容器按 RecordForm 经 fresh_variant() 装配，
// 或由冻结/解冻与 mmap open 路径产生。
```

### 插入与查询

```rust
use crate::edge::{MutableCsrTrait, EdgeId, VertexId};

// 插入（拓扑与属性解耦：CSR 只存拓扑，无 prop_offset 参数）
csr.insert_edge(
    0u32,                      // 源顶点 ID
    VertexId::from_int64(42), // 目的顶点 ID
    EdgeId(100),               // 边 ID
    5,                         // 时间戳
)?; // Ok(()) 成功；存活键重复报 EdgeAlreadyExists；Single 槽被占报 Conflict

// 单边查询（测试原语；生产走版本权威）
let edge = csr.get_edge(0, VertexId::from_int64(42), 5);
match edge {
    Some(nbr) => println!("Found: {:?}", nbr.edge_id),
    None => println!("Not found"),
}

// 全邻居（测试原语；生产用 visit_physical / fill_physical_into）
let neighbors = csr.edges_of(0, 5);
for nbr in neighbors {
    println!("Neighbor: {:?}", nbr.to_vertex_id());
}
```

### 删除与回滚

```rust
// 按边 ID 删除
csr.delete_edge(0u32, EdgeId(100), 5)?; // Ok(true) 删掉；Ok(false) 不存在/不可删

// 删除全部到该目的的边（全匹配，返回计数）
csr.delete_edge_by_dst(0u32, VertexId::from_int64(42), 5);

// 按行内偏移删除（偏移按存活度数索引，非预留容量）
csr.delete_edge_by_offset(0u32, 0, 5)?;  // 删第 1 条

// 撤销删除
csr.revert_delete_by_offset(0u32, 0, 5);
```

### 回收与维护

```rust
// 查碎片（预留容量的浪费占比，0.0–1.0）
let ratio = csr.fragmentation_ratio();
println!("Fragmentation waste share: {:.2}", ratio);

// 生产用按行回收（上报移除以便集中提升删除）
let removed = csr.compact_vertex_with_reporting(0, 5, &mut |edge_id, ts| {
    println!("promoted {:?} at {}", edge_id, ts);
});
println!("Removed {} edges", removed);
```

### 遍历

```rust
// 出借遍历（零分配，热点扫描首选；含墓碑、除空位哨兵）
csr.visit_physical(0, |nbr| {
    println!("Entry: {:?}", nbr.edge_id);
    true // 返回 false 提前结束
});

// 调用方缓冲填充（批量扫描共享一缓冲）
let mut buf = Vec::new();
csr.fill_physical_into(0, &mut buf);

// 时间戳过滤遍历（测试语义；生产可见性由版本权威裁决）
let mut iter = csr.iter(5);
while let Some((vertex_id, nbr)) = iter.next() {
    println!("Vertex {}: neighbor {:?}", vertex_id.as_int64(), nbr.edge_id);
}
```

### 序列化

```rust
// 零拷贝追加转储（与 dump 字节一致，检查点首选）
let mut out = Vec::new();
csr.dump_into(&mut out);

// 分配式转储
let data = csr.dump();

// 加载（首字节标签分发；CRC 与结构重算校验失败即拒）
let mut csr2 = CsrVariant::from_strategy_with_overflow(
    EdgeStrategy::Multiple, 1000, 10000, 4096)?;
csr2.load(&data)?;

// 注意：碎片态原样持久化；ratio >= 0.5 的组建议先按行回收
```

## 数据结构速查

### Nbr（组装邻居，32 字节）

```rust
pub struct Nbr {
    pub endpoint: u32,        // 邻居内部顶点 ID
    pub rank: i64,            // 边多重 key（Pure 形态恒 0）
    pub edge_id: EdgeId,      // 边标识
    pub delete_ts: Timestamp, // 删除戳（Timestamp::MAX = 存活）；create_ts 在版本权威中
}

impl Nbr {
    pub fn dead_gap() -> Self;               // 永不存活的空位哨兵
    pub fn hot(&self) -> HotNbr;             // 拆热端（24 字节）
    pub fn cold(&self) -> ColdStamps;        // 拆冷端（8 字节）
    pub fn from_parts(hot: HotNbr, cold: ColdStamps) -> Self;
    pub fn to_vertex_id(&self) -> VertexId;  // (endpoint, rank) 还原完整 VertexId
}
```

行内无 `prop_offset`（拓扑与属性解耦，属性按 `EdgeId` 存列式存储），
无 `create_ts` 内联字段，无 `ImmutableNbr` 类型（冻结形态同样组装 `Nbr`）。

### EdgeSchema

```rust
pub struct EdgeSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,  // 出方向 CSR
    pub ie_strategy: EdgeStrategy,  // 入方向 CSR
    pub schema_version: u64,
    pub record_form: RecordForm,    // Pure / Bundled / Columnar（默认）
}
```

`validate`：两方向须同为启用（非 `None`），单向表构造即拒。

### RecordForm

```rust
pub enum RecordForm {
    Pure,      // 12 字节/边，无 rank、无时间戳
    Bundled,   // 20 字节/边，内联单个标量
    #[default]
    Columnar,  // 标准列式（默认/回退）
}
```

## Trait 速查

### CsrBase（全变体）

```rust
pub trait CsrBase: Debug + Send + Sync {
    fn vertex_capacity(&self) -> usize;
    fn edge_count(&self) -> u64;
    fn dump(&self) -> Vec<u8>;
    fn load(&mut self, data: &[u8]) -> StorageResult<()>;
    fn dump_into(&self, out: &mut Vec<u8>); // 零拷贝追加，默认经 dump 实现
}
```

### MutableCsrTrait（可变变体；Frozen/Mapped/None 为拒绝或空实现）

```rust
pub trait MutableCsrTrait: CsrBase {
    fn insert_edge(&mut self, src_vid: u32, dst: VertexId,
                   edge_id: EdgeId, ts: Timestamp) -> StorageResult<()>;
    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId,
                   ts: Timestamp) -> StorageResult<bool>;
    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId,
                          ts: Timestamp) -> usize; // 全匹配，返回计数
    fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32,
                             ts: Timestamp) -> StorageResult<bool>;
    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32,
                               ts: Timestamp) -> bool;
    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr>;
    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr>;
    fn compact_vertex_with_reporting(&mut self, vid: u32, cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp)) -> usize;
    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize;
    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize);
    fn used_memory_size(&self) -> usize;
}
```

旧 `compact_with_ts`（无上报）已删除；`delete_edge_by_dst` 全匹配语义，
无首个匹配变体。

## 文件与位置

| 文件 | 内容 |
|---|---|
| `edge.rs` | `Nbr`/`HotNbr`/`ColdStamps`、`EdgeSchema`、`RecordForm` |
| `csr_variant.rs` | `CsrVariant` 七变体枚举、`dispatch!` 宏、序列化标签 |
| `csr_variant/` | core（工厂/清理/统计）、persistence、trait_impl、read、maintenance、values、iter |
| `csr_trait.rs` | `CsrBase`、`MutableCsrTrait` 定义 |
| `mutable_csr.rs` + `mutable_csr/` | Multiple 变体（core、row、write、read、overflow、compaction…） |
| `single_mutable_csr.rs` | Single 变体（O(1) 直接槽） |
| `pure_csr.rs` + `pure_csr/` | Pure 变体（端点 + 边 ID） |
| `bundled_csr.rs` + `bundled_csr/` | Bundled 变体（拓扑 + 内联值列） |
| `immutable_csr.rs` + `immutable_csr/` | Frozen 变体（紧凑只读组） |
| `frozen_serving.rs` + `frozen_serving/` | Mapped 变体（mmap 服务视图） |
| `node_group.rs` + `node_group/` | 组分片容器、脏标记、追加日志、冻结/解冻、回收 |
| `edge_table.rs` + `edge_table/` | `EdgeStore`（分片表、提交、检查点、MVCC、WAL、模式机） |
| `csr_with_properties.rs` + 同名目录 | 列式属性存储（`EdgeId` 索引） |
| `fragmentation_stats.rs` | 口径统计、组门限、按顶点视图 |
| `csr_shared.rs` | 删除状态机、顶点增长、溢出表、`VertexBookkeeping` |

> 边 CSR 读路径无布隆咨询；如未来需要删除定位加速，
> 重新立项并附命中率度量。

## 常见坑

### 1. 拿测试原语当生产读

```rust
// 错：行戳过滤不是可见性裁决
let edges = csr.edges_of(0, current_ts);

// 对：生产遍历走版本权威归并；行层只用 visit_physical / fill_physical_into 取物理项
csr.visit_physical(0, |nbr| { /* 按 edge_id 到权威查可见性 */ true });
```

### 2. 误判 Single 语义

```rust
// 错：以为第二条静默覆盖或按时间戳排序
csr.insert_edge(v, dst1, id1, 100)?;
csr.insert_edge(v, dst2, id2, 99);   // 报 Conflict，不是静默拒绝也不是覆盖

// 对：先删后建；墓碑槽重建接受任意时间戳
csr.delete_edge(v, id1, 150)?;
csr.insert_edge(v, dst2, id2, 140)?; // 合法
```

### 3. 高碎片直接序列化

```rust
// 错：脏组载荷被空位与墓碑撑大
let data = csr.dump();

// 对：超组门限先按行回收
if csr.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD {
    // 逐行 compact_vertex_with_reporting(...)
}
let data = csr.dump();
```

### 4. 偏移按容量索引

```rust
// 错：偏移按存活度数索引，不是预留容量
csr.delete_edge_by_offset(0, 5, ts);  // 度数不足 6 即越界失败

// 对：偏移 0 = 第 1 条存活边
csr.delete_edge_by_offset(0, 0, ts);
```

### 5. 跨变体复用行位置

```rust
// 错：从 Multiple 拿的 EdgePosition 拿到解冻后的 Frozen 解释
// 对：任何层间交接先按 edge_id 重解；定位写对无位置形态 fail-closed，不回退扫描
```

## 性能特征

### 查询复杂度

| 操作 | Multiple | Single | Pure | Bundled | Frozen/Mapped |
|---|---|---|---|---|---|
| `get_edge` | O(degree) | O(1) | O(degree) | O(degree) | O(log degree) 二分后取首个可见 |
| `edges_of` | O(degree) | O(1) | O(degree) | O(degree) | O(degree) |
| `insert_edge` | O(1) 摊销* | O(1) | O(1) 摊销* | O(1) 摊销* | 拒绝 |
| `delete_edge` | O(degree) | O(1) | O(degree) | O(degree) | 拒绝 |
| 行回收 | O(行宽) | O(1) | O(行宽) | O(行宽) | 无操作 |

*Multiple/Pure/Bundled：O(1) 摊销；宽行集合判重 O(1)，
窄行扫描 O(degree)；溢出满时分配定长新块（不拷贝旧块）。

### 内存特征

| 变体 | 空间 | 碎片 |
|---|---|---|
| Multiple | O(E + V) + 宽行存活集 | 有（空位 + 墓碑；溢出空块即摘） |
| Single | O(V) | 仅单槽墓碑 |
| Pure | O(E + V)，12 字节/边 | 同 Multiple（无时间戳列） |
| Bundled | O(E + V)，20 字节/边 | 同 Multiple（双列同步） |
| Frozen | O(E + V)，无容量/溢出/索引 | 无（紧凑有序） |
| Mapped | 文件页按需换入 + 行偏移表 | 无 |
| None | O(1) | 无 |

## 测试

```bash
# CSR 变体分发
cargo test --lib edge::csr_variant -- --nocapture

# 全部边模块
cargo test --lib edge -- --nocapture
```

## 文档

- [总览](overview.md)——架构全貌
- [变体](variants.md)——七种变体实现细节
- [分发](dispatch.md)——选择与多态分发
- [碎片](fragmentation.md)——内存管理与回收
- **[速查](quick_reference.md)**——本文件
