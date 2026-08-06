# 问题：全库 flush 在 catalog 写锁内做磁盘 IO（锁内 write/fsync 反模式）

- 状态：新建（已验证，待修复）
- 类型：可扩展性缺陷（持久化路径 / 锁粒度）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 C
- 关联：`docs/issue/defect-A-snapshot-registration-o-schema.md`（同把 catalog 写锁）、`docs/issue/defect-D-parallel-deps-unused.md`（并行落盘前提）

## 问题描述

`crates/graphdb-storage/src/storage/engine/graph_storage/context/persistence.rs:56-66`：顶点表 flush（磁盘写 + 压缩 + 序列化）整体在 `with_vertex_tables_mut` 内执行，而该函数拿的是 **catalog 写锁**（`data_store.rs:560 → write_vertex_tables → vertex_tables.write()`）。

```rust
self.persistent.data_store.with_vertex_tables_mut(|vertex_tables| {
    for (label_id, table) in vertex_tables.iter() {
        let table_dir = vertex_dir.join(format!("label_{}", label_id));
        table.flush(&table_dir, compression)?;      // 磁盘写 + 压缩，全在 catalog 写锁内
    }
    Ok(())
})?;
```

## 根因分析

- `with_vertex_tables_mut` 即 catalog 写锁：**整个数据库点表落盘期间，catalog 写锁被独占持有**——所有事务开启（缺陷 A 也要这把锁）、所有 DDL、所有 `with_vertex_table_mut` 操作全部阻塞；
- 边表 flush（`persistence.rs:71-84`）虽走 `for_all_edge_partitions_mut`（scatter-gather、逐表锁，`data_store.rs:649-670`），但仍是串行遍历，且 `maybe_compact_for_flush` 的压缩（`persistence.rs:80`）也在逐表写锁内；
- **在锁内执行 `write()`/`fsync` 是本项目最普遍的反模式**，缓存淘汰路径同病（缺陷 E，`buffer_pool.rs:216-220`）。

## 影响

- 落盘期间全库写路径（含事务开启）全局停顿；
- flush 数据量大时（压缩 + 序列化 + 文件 IO）停顿可达秒级；
- 无并行落盘：多表 flush 本可并行（rayon 在依赖中，见缺陷 D）。

## 修复方向

1. **scatter-gather 收集 Arc**：flush 前在短 catalog 读锁内收集表 `Arc` → 释放锁 → 锁外逐表加锁做 IO（对标 `for_each_edge_partition_mut` 已有模式，`data_store.rs:623-641`）；
2. **并行落盘**：表间用 rayon `par_iter` 并行 flush/压缩（缺陷 D 落地后），表内顺序保持；
3. **压缩移出锁**：`maybe_compact_for_flush` 的压缩在收集引用后、持表锁前或锁外执行；
4. flush 只写新数据（增量），避免全量重写。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P0-C。

## 验收

- flush 期间其他事务开启/读取不被阻塞（插桩或并发压测验证 catalog 锁持有时间）；
- 多表 flush 并行（`EXPLAIN`/计时对比，flus h时延随表数不再线性叠加）；
- 全量 `cargo test --test '*'` + clippy 全绿。
