# CSR（Compressed Sparse Row）边存储文档

`crates/graphdb-storage/src/edge/` 下边存储 CSR 实现的导航。
本目录五份文档均为现状文档，以代码为准；
已删除的历史形态（`MultiSingle`、`Labeled`、旧不可变 `Csr`、
`prop_offset`、`from_strategy`、`compact_with_ts`）不再收录。

## 文档

- **[总览](overview.md)**——当前架构：记录形态 × 策略两级选型、
  七变体、`CsrVariant` 分发、trait 层级、可见性权威、
  浪费占比口径与文件组织
- **[变体](variants.md)**——七种变体实现细节：
  `Multiple` / `Single` / `Pure` / `Bundled` /
  `Frozen` / `Mapped` / `None`
- **[选择与分发](dispatch.md)**——`fresh_variant` 两级选择、
  `from_strategy_with_overflow` 工厂、单个 `dispatch!` 宏、
  序列化标签、迭代器分发、表集成
- **[碎片与回收](fragmentation.md)**——度量、水位门控移除上报、
  按行回收签名、各变体回收行为
- **[速查](quick_reference.md)**——选型指南、当前 API 示例、
  数据结构与 trait 速查、常见坑

## 真实来源

代码注释即第一来源，关键入口：

- `crates/graphdb-storage/src/edge.rs`——`Nbr`/`HotNbr`/`ColdStamps`、
  `EdgeSchema`、`RecordForm`
- `crates/graphdb-storage/src/edge/csr_variant.rs`——七变体枚举与分发宏
- `crates/graphdb-storage/src/edge/csr_trait.rs`——`CsrBase`、`MutableCsrTrait`
- `crates/graphdb-storage/src/edge/node_group.rs`——组分片、脏标记、冻结/解冻
- `crates/graphdb-storage/src/edge/fragmentation_stats.rs`——浪费占比口径与组门限

## 仍然成立的设计决策

- 基于枚举的 `CsrVariant` 分发，无虚表
- 软删除：行上时间戳是物理投影，可见性权威是集中式
  `MVCCManager`（`edge_timestamps`）；`create_ts` 只存权威，
  行内仅保留 `delete_ts` 副本
- `Single` 变体服务一对一关系，哨兵空槽；第二条存活边报冲突，
  从不静默覆盖
- 记录形态按边类型选（`Pure` 12 字节 / `Bundled` 20 字节 /
  `Columnar` 通用），列式形态内再按 `EdgeStrategy` 选变体
- 按端点区间做顶点组分片，脏区域驱动增量检查点；
  整方向转储被 fail-closed 拒绝
- 冻结组只读：显式解冻前拒绝写入；mmap 服务视图是派生缓存，
  检查点为准
