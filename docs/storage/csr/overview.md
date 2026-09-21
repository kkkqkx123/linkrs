# CSR 架构总览

## 什么是 CSR？

**CSR**（Compressed Sparse Row，压缩稀疏行）是本项目边存储的邻接格式：
每个顶点一行，用偏移数组定位行、用扁平邻居数组存放边、用度数数组记录
实际边数，空间复杂度为 O(V + E)，而非邻接矩阵的 O(V²)。

一个边标签对应一张分片边表（`EdgeStore`）：按方向分片的拓扑
（`CsrShardSet`）、集中式版本权威（`MVCCManager` 的 `edge_timestamps`）、
列式属性存储（`CsrWithProperties`）、边归属映射，以及按组统计信息。
分片按端点区间划分顶点组，每组持有一个 `CsrVariant`，
用叶子区域脏标记驱动增量检查点。

## 记录形态与变体：两级选择

选型分两级，先定记录形态（`RecordForm`），再定列式形态下的策略
（`EdgeStrategy`）：

| 记录形态 `RecordForm` | 每边字节 | 适用场景 |
|---|---|---|
| `Pure` | 12（端点 + 边 ID） | 无 rank、无时间戳、无属性的纯拓扑 |
| `Bundled` | 20（拓扑 + 一个内联标量） | 单数值属性、读多写少、模式稳定的边类型 |
| `Columnar`（默认） | 32（热端 24 + 冷端 8）+ 列式属性 | 通用多属性边 |

`Columnar` 形态下再按 `EdgeStrategy` 选择 CSR 变体：

| 变体 | 底层类型 | 适用场景 |
|---|---|---|
| `Multiple` | `MutableCsr` | 通用多边行，主块 + 分级溢出块 |
| `Single` | `SingleMutableCsr` | 一对一关系，直接槽数组，空槽用永不存活哨兵 |
| `Frozen` | `ImmutableCsr` | 冻结的只读紧凑组，显式解冻前拒绝写入 |
| `Mapped` | `MappedFrozen` | 同 `Frozen` 内容的内存映射服务视图 |
| `Pure` | `PureTopologyCsr` | `Pure` 记录形态的拓扑 |
| `Bundled` | `BundledCsr` | `Bundled` 记录形态的拓扑 + 内联值列 |
| `None` | 占位 | 该方向不存边 |

`EdgeStrategy` 只有 `Multiple` / `Single` / `None` 三种取值
（定义在 `graphdb-core` 的 `types/edge.rs`）。
`EdgeSchema::validate` 要求出入两个方向均为启用的非 `None` 策略，
单向表在构造时即被拒绝。`csr_variant.rs` 中的 `dispatch!` 宏
把 trait 调用路由到具体形态，无 `dyn` 开销。详见
[variants.md](variants.md) 与 [dispatch.md](dispatch.md)。

## Trait 层级

### CsrBase

`vertex_capacity()`、`edge_count()`、`dump()`、`load()`，
以及零拷贝的 `dump_into()`（检查点序列化直接借用存活拓扑，
避免先克隆再编码）。`CsrShardSet` 对整方向 dump/load  fail-closed：
持久化只走按组增量协议。

### MutableCsrTrait

插入、按 ID / 按端点 / 按偏移删除、回滚、物理读与可见读、
按顶点回收探测（`reclaimable_count`、`vertex_reclaim_probe`、
`vertex_census`）、带移除上报的行级回收
（`compact_vertex_with_reporting`），以及内存核算。

带时间戳的行读（`get_edge`、`edges_of`、`get_edge_physical`）
是仅供测试的原语；生产环境可见性走版本权威
（`MVCCManager`），在表层做归并查询。行遍历统一为三种形态：
零分配出借遍历 `visit_physical`（内联形态与热点扫描首选）、
调用方缓冲填充 `fill_physical_into`（批量扫描首选）、
分配式 `physical_edges_of`（仅测试与离线使用）。
禁止按变体新增遍历方言。

## 时间戳与可见性

```rust
// 热端：遍历所需全部拓扑（24 字节）
HotNbr { endpoint: u32, rank: i64, edge_id: EdgeId }
// 冷端：物理 MVCC 副本，仅删除戳（8 字节）
ColdStamps { delete_ts: Timestamp }
// API 边界组装形态（32 字节），无 prop_offset，create_ts 不内联
Nbr { endpoint: u32, rank: i64, edge_id: EdgeId, delete_ts: Timestamp }
```

行上的时间戳是供回收使用的物理投影。可见性权威是
`edge_timestamps`：`create_ts` 只存放在该权威中，
行内仅保留 `delete_ts` 副本；边在 `create_ts <= ts < delete_ts`
时可见。空位填充用 `Nbr::dead_gap()`（不可分配边 ID +
空删除窗口），越界扫描报缺席而非幽灵存活边。
`Pure` 形态无时间戳（物理删除直接覆写边 ID 为哨兵），
`Bundled` 形态值列用有效位表达删除。
列式属性保留各自的按列版本链用于检查点周期内的时间旅行读，
检查点时坍缩为当前值。

## 碎片管理

主行按密度目标保留写入空位，删除项等回收截止线。
因此浪费 = 空位 + 墓碑槽：`fragmentation_ratio()` 报告
预留容量的浪费占比（0.0–1.0），仅作观测。
回收触发看按顶点可回收计数；组合并受调用方碎片门控。
恢复路径回收带按边移除上报，且必传水位截止线；
截止线之上的墓碑予以保留。详见
[fragmentation.md](fragmentation.md)。

## 并发契约

行存储无内部锁：`&mut self` 在编译期拒绝别名写入。
无写入时并发读安全；并发写由调用方（表层或事务层）串行化，
多核扩展来自组分片级并行，而非行锁。`Send + Sync`
表达单写者跨线程转移，而非无锁并发写。

## 文件组织

```
crates/graphdb-storage/src/edge/
├── edge.rs                     # Nbr/HotNbr/ColdStamps、EdgeSchema、RecordForm
├── csr_shared.rs               # 删除状态机、顶点容量增长、溢出表、VertexBookkeeping
├── csr_trait.rs                # CsrBase、MutableCsrTrait
├── csr_variant.rs              # Multiple/Single/Pure/Bundled/Frozen/Mapped/None 枚举与 dispatch 宏
├── csr_variant/                # core、persistence、trait_impl、read、maintenance、values、iter
├── csr_with_properties.rs      # 列式属性存储（EdgeId 索引）
├── csr_with_properties/        # encoding、mapping、read、write、persistence 等
├── mutable_csr.rs + mutable_csr/     # Multiple 变体（core、row、write、read、overflow、compaction…）
├── single_mutable_csr.rs       # Single 变体（热冷双槽数组）
├── pure_csr.rs + pure_csr/           # Pure 变体（端点 + 边 ID）
├── bundled_csr.rs + bundled_csr/     # Bundled 变体（拓扑 + 内联值列）
├── immutable_csr.rs + immutable_csr/ # Frozen 变体（紧凑只读组）
├── frozen_serving.rs + frozen_serving/ # Mapped 变体（mmap 服务视图）
├── node_group.rs + node_group/ # 组分片容器、脏标记、追加日志、冻结/解冻、回收
├── edge_table.rs + edge_table/ # EdgeStore：分片表、提交、检查点、MVCC、WAL、模式机
├── fragmentation_stats.rs      # 浪费占比口径、组合并门控、按顶点碎片视图
└── property_schema.rs          # 属性模式项
```

## 相关文档

- [变体详解](variants.md)——七种变体的结构与取舍
- [选择与分发](dispatch.md)——RecordForm/EdgeStrategy 选择与分发实现
- [碎片与回收](fragmentation.md)——度量与回收细节
- [速查](quick_reference.md)——选型指南与代码示例
