# 索引数据结构选型分析：BTreeMap vs ART（归档）

> **本文档为历史归档，反映 2026-07 的旧架构。当前实现已全面重写，见下文「架构演进」。**

## 背景

LinkRS 的二级索引当前使用 `std::collections::BTreeMap<Vec<u8>, IndexRecord>` 作为存储结构，每个分片包含独立的 BTreeMap 实例（正向索引和反向索引各一张）。

本文件对比 BTreeMap 与 Adaptive Radix Tree (ART) 在实际代码中的性能差异，作为后续重新评估该问题时的参考依据。

## 键结构

```
SecondaryIndexKey = Vec<u8>

正向 Vertex Index Key:
  [0..7]   space_id            (8 bytes, LE)
  [8]      key_type            (1 byte,  0x03 = VERTEX_FORWARD)
  [9..12]  index_name_len      (4 bytes, LE)
  [13..N]  index_name          (variable, UTF-8)
  [N..]    OrderedCodec(prop_value)   (variable, 保序编码)
  [..]     OrderedCodec(entity_id)    (variable, entity tie-breaker)

反向 Vertex Index Key:
  [0..7]   space_id            (8 bytes, LE)
  [8]      key_type            (1 byte,  0x01 = VERTEX_REVERSE)
  [9..]    OrderedCodec(entity_id)    (variable)
  [..]     index_name          (variable)
```

## 性能差异汇总

| 操作 | 频度 | BTreeMap | ART | 差异方向 | 差异幅度 |
|------|------|----------|-----|----------|----------|
| 范围扫描 | **最高** | ~100μs/10K | ~150μs/10K | BTree 领先 | +50% |
| 等值查找 | 高 | ~3μs | ~3.5μs | 持平 | <2μs |
| 插入 | 高 | ~2μs | ~2μs | 持平 | <5% |
| 墓碑标记 | 中 | ~1μs | ~1μs | 持平 | — |
| GC 遍历 | 低 | O(N) cache-friendly | O(N) 递归跳转 | BTree 领先 | +20% |
| 快照克隆 | 中 | O(N) 批量分配 | O(N) 独立分配 | BTree 领先 | +50% |
| 分片拆分 | **低但关键** | O(N) partition | O(N log N) 重建 | **BTree 大幅领先** | **10-100x** |
| 序列化 | 低 | O(N) | O(N) | 持平 | — |
| 内存 | 持续 | 每 key 冗余 13-50B | 路径级压缩 | ART 领先 | -30~65% |

## 结论（归档）

ART 在性能维度无一领先。其唯一优势是**内存节省**（长前缀场景下 30-65%）。BTreeMap 在范围扫描、GC、快照、拆分上全面优于 ART，其中**分片拆分**的差距最大（10-100x）且直接影响在线能力。

---

## 架构演进

此后索引子系统经过以下重大演进：

### ChunkedIndex + BufferPool + WAL

- `ChunkedIndex`：将 `BTreeMap` 分包（64KB/chunk），支持 chunk 粒度的惰性加载与驱逐
- `BufferPool`：CLOCK 淘汰算法，支持冷 chunk 自动写回磁盘
- `WAL`：增量写前日志，避免整表重写

### 代际链（Generation Chain）

- `physical_key()` 版本号追加方案被废弃，改为**增量代际发布**（`publish_delta_generation`）
- 每次写入创建新的 delta generation，含仅变更 entry 的快照
- 读路径逐代链式 fallback（最新代优先），无需扫描墓碑
- `compact_native_index` 合并多代并清除墓碑

### 写路径优化

- `reverse_range_suffix_visible`：避免 prefix 重建开销，直接返回 suffix key
- `extract_value_from_reverse_suffix`：suffix 键解析器，省去跳过头部 9 字节的浪费
- `HashSet<Vec<u8>>` 去重：替代 `Vec::contains` O(n²) 扫描，存在值去重降为 O(1)
- `clone_from` + 提前跳出：covering columns 只取第一个有效记录，避免循环内重复分配

### 当前架构优势

| 维度 | 改进 |
|------|------|
| 等值查找 | 淘汰 BTreeMap 裸扫描，支持 Bloom Filter 预过滤跳过 shard |
| 范围扫描 | `ChainForwardIterator` 惰性逐代遍历，不克隆整表 |
| 写路径 | 增量代际发布，仅一次 reverse suffix 扫描即可收集存在值 |
| 持久化 | chunk 增量 checkpoint + WAL，非整表重写 |
| 内存 | chunk 级 CLOCK 淘汰，支持超出内存限制自动驱逐 |
| 并发 | `ArcSwap` 无锁读，delta generation 避免读写互斥 |
| 崩溃恢复 | WAL 回放 + CRC32 校验，无静默丢字段 |
| MVCC | 代际链，墓碑仅存在于 delta generation，compaction 时消除 |

