# 索引数据结构选型分析：BTreeMap vs ART

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
  [..]     version             (8 bytes, LE, 单调递增)

反向 Vertex Index Key:
  [0..7]   space_id            (8 bytes, LE)
  [8]      key_type            (1 byte,  0x01 = VERTEX_REVERSE)
  [9..]    OrderedCodec(entity_id)    (variable)
  [..]     index_name          (variable)
  [..]     version             (8 bytes, LE, 单调递增)
```

## 操作逐路径对比

所有数值为单次操作在单索引 100 万条目场景下的估计值。

### 读路径

#### 1. 索引范围扫描（cursor.next_batch）— 最高频读路径

```rust
// vertex_index_manager.rs:214-224
index.range((
    Bound::Included(range_start.clone()),
    Bound::Excluded(range_end.clone()),
))
```

| 步骤 | BTreeMap | ART |
|------|----------|-----|
| 定位起始 | O(log₂ N) ≈ 20 次 memcmp，每次 8-30B | O(k) ≈ 50-250 字节跳转 |
| 推进到下条 | 叶节点内顺序扫描 + sibling 指针 O(1) 跳到下一叶 | 递归回溯到最近分叉内部节点再下降 |
| 批量 10K 行 | ~100μs | ~150μs |

**BTreeMap 快约 50%**。核心原因：BTreeMap 叶节点之间通过 sibling 指针跳转，局部性好；ART 的 `next()` 需回溯到 parent 再下降，均摊 3-8 次指针跳转 vs BTreeMap 的 1-2 次。

#### 2. 等值查找（lookup_tag_index_mvcc）— 修复后

```rust
// vertex_ops.rs:162-186
let prefix = KeyBuilder::build_vertex_index_value_prefix(space_id, &index.name, value)?;
let end = KeyBuilder::build_range_end(&prefix);
shard.forward().read().range(prefix.0.clone()..end.0.clone())
```

| 指标 | BTreeMap | ART |
|------|----------|-----|
| 定位 | log₂(1M) ≈ 20 次 memcmp | k ≈ 50B 字节跳转 |
| 结果数 K | 一次 mvcc 可见性检查 | 一次 mvcc 可见性检查 |
| 总计 | ~2-5μs | ~3-6μs |

差距 <2μs，可忽略。

#### 3. 反向索引扫描（entity 更新时的清除阶段）

```
模式：reverse_prefix = [space_id][0x01][entity_id][index_name]
```
通常只返回 1-5 条记录，两者无实质差异。

### 写路径

#### 4. 插入（physical_key + insert）

```rust
// shard_runtime.rs:57-63 — 生成唯一 key
let version = self.version_counter.fetch_add(1, Ordering::Relaxed);
key.extend_from_slice(&version.to_le_bytes());

// vertex_ops.rs:124-131 — 插入双索引
target.forward().write().insert(target.physical_key(&forward.0), entry);
target.reverse().write().insert(target.physical_key(&reverse.0), entry);
```

| 步骤 | BTreeMap | ART |
|------|----------|-----|
| key 生成 | 8B 追加 + atomic fetch_add | 同左 |
| 查找插入位 | O(log N) ≈ 20 次 memcmp | O(k) ≈ 50 字节跳转 |
| 结构变更 | 偶发节点分裂：B/2 个键 memcpy 到新节点 | 偶发节点升级：4→16→48→256 全量指针复制 |
| 均摊成本 | ~2μs | ~2μs |

差异 <5%，可忽略。

#### 5. 墓碑标记（get_mut + mark_deleted）

```rust
// vertex_ops.rs:91-96
let mut data = shard.forward().write();
for key in keys {
    if let Some(entry) = data.get_mut(&key) {
        entry.mark_deleted(write_ts);
    }
}
```

两者均为 O(log N) / O(k) 查找 + O(1) 原地 bool 翻转。等价。

### 维护路径

#### 6. GC 全遍历

```rust
// gc.rs:26-34
map.read().iter()
    .filter(|(_, entry)| entry.deleted_ts.is_some_and(|d| d < safe_ts))
    .take(remaining)
    .map(|(key, _)| key.clone())
```

| 指标 | BTreeMap | ART |
|------|----------|-----|
| 遍历 | O(N) 叶节点数组顺序 | O(N) 递归中序遍历 |
| 缓存局部性 | 连续分配，cache-friendly | 随机节点跳转 |
| 均摊 | 快 ~20% | — |

BTreeMap 的 `iter()` 按序访问叶节点数组，现代 CPU 的硬件预取器可提前加载；ART 的中序遍历需要维护显式或隐式栈，每次跳转地址不连续。

#### 7. 快照克隆

```rust
// shard_runtime.rs:73-75
pub fn snapshot(&self) -> IndexMaps {
    (self.forward.read().clone(), self.reverse.read().clone())
}
```

| 指标 | BTreeMap | ART |
|------|----------|-----|
| 遍历 | 叶节点数组迭代，同 alloc 分配 | 递归遍历树节点 |
| 分配次数 | N/B 次（B≈8） | N 次 |
| O(N) 常数 | 低 | 高约 50% |

**BTreeMap 快约 50%**。Rust std 的 BTreeMap clone 对父子节点在同 allocator 下的批量分配做了优化；ART 的每个叶子是独立分配，递归克隆需要更多小分配。

#### 8. 分片拆分（partition）

```rust
// index_data_manager.rs:560-562 — 按排序边界切分
let (forward_a, forward_b): (BTreeMap<_, _>, BTreeMap<_, _>)
    = forward.into_iter().partition(|(key, _)| key.as_slice() < boundary.as_slice());
```

**这是 BTreeMap 的关键架构优势。** BTreeMap 的有序迭代使得 `partition` 一次遍历即可产出两半有序数据：

| 步骤 | BTreeMap | ART |
|------|----------|-----|
| 关键能力 | `into_iter()` 天然有序 | 无序 |
| 拆分 | O(N) 一次遍历 + partition | O(N) 遍历收集 + O(N log N) 重建两棵树 |
| 锁持有时间 | 低（只写锁一次 swap） | 高（整个重建期间持写锁或造成不一致窗口） |

**BTreeMap 快 10-100x**，且差距随数据增长扩大。拆分虽低频，但它是索引层在线能力的关键路径——拆分期间其他操作等待写锁或 fence，ART 的 O(N log N) 重建会显著延长锁定时间。

#### 9. 持久化序列化

```rust
// generic_index_manager.rs:90-132
for (key, entry) in index.iter() {
    writer.write_all(&(key.len() as u32).to_le_bytes())?;
    writer.write_all(key)?;
    // ...serialize entry fields...
}
```

| 指标 | BTreeMap | ART |
|------|----------|-----|
| 遍历方式 | `iter()` 有序迭代 | 需递归遍历或 BFS |
| 压缩友好 | 有序输出利于 delta 压缩 | 需额外排序才能达到相同压缩率 |
| 复杂度 | O(N) | O(N) 但实现更复杂 |

对于纯全量序列化场景，两者同为 O(N)。ART 的树形序列化（如 Ladybug 的 varint 编码）在实现复杂度上高出 5-10 倍。

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

## 结论

ART 在性能维度无一领先。其唯一优势是**内存节省**（长前缀场景下 30-65%）。BTreeMap 在范围扫描、GC、快照、拆分上全面优于 ART，其中**分片拆分**的差距最大（10-100x）且直接影响在线能力。

如果未来内存成为瓶颈（监控指标参考 `shard_runtime.rs:94-111` 的 `memory_usage_bytes()`），ART 的可选替换方案是：

1. **不替换 BTreeMap**，改为在 BTreeMap 内实现前缀压缩（如将 `space_id + key_type + index_name` 抽象为隐式上下文，每个 key 只存差值）
2. **分层替换**：只对内存占比最大的索引（通常是大字符串索引）替换为 ART，其余保持 BTreeMap
3. **完整替换**：为分片拆分、原子重建增加 `ArcSwap<ArtTree>` 包装层，预计 +300 行额外代码

当前（2026-07）不做迁移。
