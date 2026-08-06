# 问题：BufferPool 单把全局锁 + 锁内 IO + O(n·m) 淘汰 + insert TOCTOU

- 状态：新建（已验证，待修复）
- 类型：可扩展性缺陷（缓存层 / 并发热点）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 E
- 关联：`docs/issue/defect-I-cache-key-with-timestamp.md`（同属缓存层）、`docs/issue/defect-C-flush-under-catalog-write-lock.md`（同病：锁内 IO）

## 问题描述

位于所有点查最前端的缓存是一把大锁。`crates/graphdb-storage/src/storage/cache/buffer_pool.rs`：

```rust
struct BufferPoolInner<K, T> {
    capacity: AtomicU64,
    items: Mutex<HashMap<K, CachedItem<T>>>,   // 唯一一把全局锁
    clock_hand: Mutex<usize>,
    cached_ids: Mutex<Vec<K>>,
    ...
}
```

点表尚且分了 8 片，缓存却未分片。

## 根因分析（三重问题，均已代码级确认）

1. **锁内磁盘 IO**（`buffer_pool.rs:216-220`）：`evict` 中脏页写回在 `items` + `cached_ids` + `writer` 三把锁内执行，写回期间所有缓存读全部阻塞；
2. **O(n·m) 淘汰**（`buffer_pool.rs:228`）：`ids.retain(|i| *i != id)` 在淘汰循环内部，淘汰 m 项需 O(n·m)；内存压力下 `shrink_cache` 把容量砍半（一次淘汰 ~n/2 项）→ 退化为 **O(n²) 且全程持锁**；
3. **insert TOCTOU 竞态**（`buffer_pool.rs:121-129`）：容量检查在锁外（`current_usage` + `evict`），N 个线程可同时通过检查并各自插入 → 实际用量可超 capacity N 倍，误导 `Spiller` 溢写决策。

**另**：`get` 在锁内深拷贝（`buffer_pool.rs:88-91`，`items.get(key).cloned()`），每次缓存命中都在锁内做完整堆分配 + 字符串复制，抵消缓存收益；`writer` 是用户注入闭包，在持两把锁时调用，`parking_lot::Mutex` 非可重入，若该闭包回调任何 `BufferPool` 方法即自锁死，类型系统无约束。

## 影响

- 点查最高频路径（缓存访问）全局串行；
- 写密集时脏页写回长阻塞读路径；
- 内存压力下 O(n²) 淘汰放大停顿。

## 修复方向

1. **按 key hash 分片**：`items` 拆为 N 片 Mutex，命中/插入只锁单片；
2. **`get` 返回 `Arc<T>`**（或 `Arc<CachedItem<T>>`）避免锁内深拷贝；
3. **写回移出锁**：脏页回收时先摘出待写条目、释放锁、再调 `writer`；`writer` 闭包改用可重入通道（队列 + 后台落盘）或文档化禁止回调；
4. **insert 容量检查移入锁内**：消除 TOCTOU，容量以锁内实际用量为准；
5. **淘汰去 O(n·m)**：`cached_ids` 改链表或 `clock_hand` 直接遍历 items（HashMap 迭代），`retain` 移出循环。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P2-E。

## 验收

- 并发点查吞吐随分片数线性提升（基准：N 线程读缓存，单锁 vs 分片对比）；
- 写回期间读路径不再阻塞（并发写 + 读时延无长尾）；
- 容量限制在任意并发下严格成立（TOCTOU 竞态测试）；
- 全量 `cargo test --test '*'` + clippy 全绿。
