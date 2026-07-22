# GraphDB Storage 包 Clippy 分析报告

## 概述

对 `graphdb-storage` 包执行 clippy 检查发现了 60 个警告，主要集中在死代码和未使用字段上。这些发现揭示了存储子系统存在显著的实现不完整性和冗余问题。

## 主要问题分类

### 1. 边缘存储系统 (Edge Storage)

- **删除功能缺失**：`delete_edge_by_offset` 方法在多个组件中定义但从未使用，包括 `MutableCsrTrait`、`EdgeStore`、`TimeTravelEdgeStore` 和 `SingleMutableCsr`
- **分段管理未实现**：`evict_to_spill`、`reload_from_spill` 等方法表明设计了溢出到磁盘的功能，但这些功能未被调用
- **访问时钟机制废弃**：`AccessClock` 结构体及其 `tick`、`now` 方法完全未使用，表明基于访问频率的缓存淘汰策略未实现

### 2. 编码与压缩模块 (Encoding & Compression)

- **编码器功能不完整**：ALP 编码器的 `compress` 和 `exceptions` 方法、FSST 编码器的 `decode_to_string` 方法、RLE 编码器的 `decode` 方法均未使用
- **动态编码选择未启用**：`EncodingSelector` 的反馈机制（`should_reencode`、`average_ratio`）和阈值配置（`fsst_rebuild_threshold`、`reencode_threshold`）未被读取或使用

### 3. 索引系统 (Indexing System)

- **查询接口未实现**：边缘索引和顶点索引的 `lookup_*_mvcc` 系列方法未使用，MVCC 支持可能不完整
- **键编解码器冗余**：`KeyBuilder` 的 `build_vertex_reverse_prefix_v2` 和 `build_edge_reverse_prefix` 方法未使用
- **序列化功能废弃**：`serialize_value` 和 `deserialize_value` 函数未使用，可能有其他序列化方案

### 4. MVCC 与快照系统 (MVCC & Snapshot)

- **快照机制未完成**：`SnapshotHandle` 结构体从未构造，`MVCCTable` trait 从未使用，表明 MVCC 实现不完整
- **墓碑管理未启用**：`TieredTombstoneManager` 的 `is_tombstoned`、`gc_batch` 等方法未使用

### 5. 顶点存储系统 (Vertex Storage)

- **批量操作未实现**：`batch_insert`、`batch_delete` 等方法未使用
- **ID 管理功能不全**：`IdIndexer` 的 `is_empty`、`memory_usage` 方法和 `enable_free_list` 配置字段未被读取
- **时间戳压缩未使用**：`VertexTimestamp` 的 `compact` 和 `compact_without_mapping` 方法未使用

### 6. 存储引擎层 (Storage Engine)

- **资源管理未启用**：`Spiller` 的 `try_reserve_with_spill`、`spill_cold_data` 等方法未使用，内存溢出到磁盘的功能可能未实现
- **持久化协调器功能不全**：`inject_failure`、`latest_safe_lsn`、`load_latest_manifest` 方法未使用
- **WAL 管理未完成**：`append_transaction` 方法未使用，事务日志功能可能不完整

## 总结

`graphdb-storage` 包显示出典型的增量开发特征：许多功能模块被设计并部分实现，但未完全集成到主流程中。这导致了大量的死代码和未使用字段。建议优先完善核心数据路径（读写操作），然后逐步激活这些辅助功能模块。