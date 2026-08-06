# 问题：缓存键含时间戳，跨事务命中率趋近 0，失效需全表扫描

- 状态：新建（已验证，待修复）
- 类型：性能缺陷（缓存命中率 / 失效放大）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 I
- 关联：`docs/issue/defect-E-bufferpool-single-lock.md`（缓存层锁）、`docs/issue/defect-A-snapshot-registration-o-schema.md`（每事务新时间戳的来源）

## 问题描述

`crates/graphdb-storage/src/storage/cache/record_cache.rs` 的缓存键是 `(CacheKey, Timestamp)`：

```rust
// record_cache.rs:150 / :165
self.vertex_pool.get(&(*key, query_ts))
self.vertex_pool.insert((key, ts), vertex, size);
```

由于每个事务都分配新时间戳，**不同 ts 的同一顶点是不同缓存条目** → 跨事务缓存命中率接近 0，而缓存空间被同一顶点的 N 个时间戳副本占满。

## 根因分析（已代码级确认）

- 键含 ts：`record_cache.rs:149-165`（vertex）、`:132`（id_index 同样 `(key, ts)`）；
- 失效只能全表扫描（持全局 items 锁）：
  - `remove_vertex`（`record_cache.rs:169-174`）：O(n) `retain` 扫全池；
  - `invalidate_vertices_by_label`（`:180-184`）：注释自承 "Scans all entries, O(n) complexity"；
  - `clear`：注释 "BufferPool doesn't support clear, use retain with false predicate"；
- 与缺陷 E 叠加：单点更新触发全缓存扫描 + 全局锁，写放大严重。

## 影响

- 缓存形同虚设：同一顶点每事务一条，命中率 ≈ 0，内存被 N 个时间戳副本占满；
- 写路径每次更新都做全缓存 O(n) 扫描 + 全局锁。

## 修复方向

1. **键去掉 `Timestamp`**：改用"版本号 + 失效列表"——缓存条目记录其可见的数据版本（如 `min_visible_ts` 或全局版本号），新事务读取时校验版本号而非用 ts 作键；
2. **失效列表**：按 label/entity 维护失效列表（dirty 标记），失效 O(1) 标记而非 O(n) 扫描，惰性清理；
3. 若保留 ts 键：至少在池满时按 ts 淘汰旧版本（当前无此策略，导致占满）。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P2-I。

## 验收

- 同顶点跨事务重复读：缓存命中率显著提升（插桩对比 hit/miss）；
- 单点更新不再触发全缓存 O(n) 扫描（失效路径复杂度测试）；
- 全量 `cargo test --test '*'` + clippy 全绿。
