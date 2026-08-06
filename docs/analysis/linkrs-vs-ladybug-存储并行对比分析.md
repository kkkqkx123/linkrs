# linkrs vs ladybug：存储模块并行执行对比与 linkrs 设计缺陷分析

> **分析对象**
> - `linkrs` — Rust 单机图数据库，存储模块 `crates/graphdb-storage/`（166 个 `.rs`，78,118 行）
> - `ladybug` — C++ 嵌入式图数据库（Kuzu fork，命名空间 `lbug`），存储模块 `src/storage/`（113 个 `.cpp`，27,070 行）
>
> **方法**：逐文件精读源码 + 全量并发原语普查，所有结论标注 `文件路径:行号`。未修改任何代码。

---

## 一、结论先行

一句话概括两者的根本差异：

> **linkrs 是"用锁包住单线程数据结构"，ladybug 是"为并行而设计的数据结构"。**

linkrs 存储层拥有大量并发**外观**——8~16 路分片、79 个 `RwLock`、121 处 `AtomicU64`、16 处 `ArcSwap`——但**没有任何一行数据并行代码**。它解决的是"多个请求同时到达时不要崩"（并发正确性），而 ladybug 解决的是"一个请求如何吃满 32 核"（并行性能）。这是两个不同维度的问题，linkrs 只做了前者，且前者也做得不彻底。

最刺眼的证据：`graphdb-storage/Cargo.toml:31-32` 同时声明了 `crossbeam-utils` 和 `rayon`，而对整个 crate 的源码 grep `rayon|crossbeam` 的结果是 **0 命中**。

```
crates/graphdb-storage/src $ grep -rn "rayon\|crossbeam" --include="*.rs" . | wc -l
0
```

**并行库被声明、被编译、被链接，但从未被调用。** README 宣称的性能特性在存储层缺少实现支撑。

---

## 二、量化对照总览

| 指标 | linkrs | ladybug |
|---|---|---|
| 存储层代码量 | 78,118 行 / 166 文件 | 27,070 行 / 113 文件 |
| 数据并行框架 | **无**（rayon 声明未用，0 命中） | TaskScheduler 线程池 + morsel-driven |
| 工作线程管理 | 15 处裸 `thread::spawn`，无池化 | 构造期固定线程池，数 = `hardware_concurrency()` |
| 单查询多核 | **否** | 是 |
| 缓冲池同步 | 单把全局 `Mutex` | 8 字节原子状态机 + CAS 队列 + 乐观读 |
| 锁数量 | `RwLock` 79 + `Mutex` 25 | `unique_lock` 40 + `shared_lock` 14 + `mutex` 9 |
| 读读并发 | **互斥**（点表用 `Mutex` 非 `RwLock`） | 完全并行（乐观读零同步） |
| MVCC | 平坦 `(start_ts, end_ts)`，**无版本链** | `VersionInfo` + `UpdateInfo` 真版本链 |
| 事务开启成本 | **O(表数 × 分片数)**，持 catalog 写锁 | O(1)，原子 load |
| WAL 组提交 | 默认**关闭**，仅 Sync 级生效 | 默认开启，N 事务 1 次 fsync |
| Flush | 全库串行，**catalog 写锁内做磁盘 IO** | NodeGroup 粒度，锁外 IO |
| 索引分片 | 点表 ≤16（硬上限） | 256 路 + MPSC 队列 + 三级缓冲 |
| 写事务并发 | 分片级（但受全局串行点制约） | 默认单写（可配置放开） |
| 内存序分布 | Relaxed 141 / Acquire 57 / Release 17 / AcqRel 6 / SeqCst 5 | 分散但审慎 |

---

## 三、并行执行机制逐项对比

### 3.1 任务调度：线程池 vs 裸线程

**ladybug —— morsel-driven 并行流水线**

线程池在数据库构造期一次性创建（`src/common/task_system/task_scheduler.cpp:23-30`）：

```cpp
TaskScheduler::TaskScheduler(uint64_t numWorkerThreads)
    : stopWorkerThreads{false}, nextScheduledTaskID{0} {
    for (auto n = 0u; n < numWorkerThreads; ++n) {
        workerThreads.emplace_back([&] { runWorkerThread(); });
    }
}
```

线程数默认 `std::thread::hardware_concurrency()`（`src/main/database.cpp:75`），会话级可覆盖。

并行的核心在 `src/processor/processor_task.cpp:19-32`——**每线程克隆一份私有算子树，只共享 morsel 游标**：

```cpp
void ProcessorTask::run() {
    lock_t lck{taskMtx};
    if (!sharedStateInitialized) {
        sink->initGlobalState(executionContext);
        sharedStateInitialized = true;
    }
    auto taskRoot = sink->copy();   // 每线程私有算子树
    lck.unlock();
    auto resultSet = sink->getResultSet(...);
    taskRoot->ptrCast<Sink>()->execute(resultSet.get(), executionContext);
}
```

morsel 分发只是一个游标自增（`src/processor/operator/scan/scan_node_table.cpp:89-120`），**一个 NodeGroup = 一个 morsel**，天然动态负载均衡——慢线程少拿几个 morsel，无需 work-stealing。

还有一个容易被忽略的精妙设计（`task_scheduler.cpp:113-130`）：工作线程注销任务必须持全局锁，这使得**全局锁充当了跨 pipeline 的内存屏障**，Task_j 的所有写入对 Task_{j+1} 天然可见。这是 ladybug 敢在算子内部大量使用非原子共享状态的根本原因——**用一处集中同步换取整个算子层的零同步开销**。

**linkrs —— 15 处裸 `thread::spawn`，无调度器**

```rust
// crates/graphdb-storage/src/storage/engine/graph_storage/context/freeze.rs:35-61
pub(crate) fn schedule_background_maintenance(&self) {
    if self.runtime.background_freeze_running
        .swap(true, Ordering::AcqRel) { return; }
    let context = self.clone();
    std::thread::spawn(move || {          // 每次新建 OS 线程
        if let Err(error) = context.trigger_background_maintenance() { ... }
        running.store(false, Ordering::Release);
    });
}
```

问题有三：
1. **每次触发新建 OS 线程**，无池化、无背压；
2. 门闩语义限定全局只允许 **1 个**后台维护线程 → compaction/freeze 吞吐上限 = 单核；
3. 线程句柄被丢弃，**无法 join、无法优雅关闭**，进程退出可能截断维护中状态。

更值得注意的是命名误导：`engine/background_freeze.rs` 中的 `BackgroundFreezeManager` 名为 Manager，实际只维护 `Arc<Mutex<FreezeStats>>` 统计，**根本不启动任何线程**。

**注释与实现不符的典型**（`data_store.rs:638-661`）：

```rust
/// ... partitions from different edge labels can be mutated concurrently
pub(crate) fn for_all_edge_partitions_mut<R>(...) -> StorageResult<Vec<R>> {
    keys.into_iter()
        .map(|(key, arc)| {
            let mut table = arc.write();
            operation(key, &mut table)      // 串行 .map()，非 par_iter
        })
        .collect()
}
```

注释描述的是"锁粒度允许并发"，代码实现的是"单线程串行"。改成 `par_iter()` 是一行的事，却没做——而 `rayon` 就在依赖里。

### 3.2 缓冲/缓存层：无锁乐观读 vs 单把全局锁

**ladybug —— vmcache 式无锁缓冲池**

整个页的并发控制压缩进 **8 字节**（`src/include/storage/buffer_manager/page_state.h:18-27, 108-110`）：

```cpp
static constexpr uint64_t DIRTY_MASK   = 0x0080000000000000;
static constexpr uint64_t STATE_MASK   = 0xFF00000000000000;
static constexpr uint64_t VERSION_MASK = 0x00FFFFFFFFFFFFFF;
...
std::atomic<uint64_t> stateAndVersion;   // state + version + dirty 三合一
```

读路径**完全不加锁**（`src/storage/buffer_manager/buffer_manager.cpp:229-263`）：先读数据，再回头对比版本号，不一致就重试。

```cpp
case PageState::UNLOCKED: {
    if (!try_func(func, getFrame(...), ...)) { continue; }
    if (pageState->getStateAndVersion() == currStateAndVersion) { return; }  // 版本校验
} break;
```

意味着 N 个线程并发扫描同一批页时**彼此零同步开销**，无原子写、无 cacheline 争抢。淘汰队列同样无锁（`buffer_manager.cpp:38-51`，`fetch_add(relaxed)` + `compare_exchange_weak`）。

更关键的一步：用 `mmap` + `MADV_DONTNEED`（`src/storage/buffer_manager/vm_region.cpp:62, 121`）使 **frame 虚拟地址永远固定**，`getFrame()` 退化为纯地址算术——**传统缓冲池中最大的争用点 pageID→frame 哈希表被彻底消灭**。

**linkrs —— 单把 `Mutex` 管全部缓存**

```rust
// crates/graphdb-storage/src/storage/cache/buffer_pool.rs:54-66
struct BufferPoolInner<K, T> {
    capacity: AtomicU64,
    items: Mutex<HashMap<K, CachedItem<T>>>,   // 唯一一把全局锁
    clock_hand: Mutex<usize>,
    cached_ids: Mutex<Vec<K>>,
    ...
}
```

点表尚且分了 8 片，**位于所有点查最前端的缓存却是一把大锁**。且读操作也要独占锁 + 深拷贝（`buffer_pool.rs:88-91`）：

```rust
pub(crate) fn get(&self, key: &K) -> Option<CachedItem<T>> {
    let items = self.inner.items.lock();
    items.get(key).cloned()          // 深拷贝 String + Vec<(String, Value)>
}
```

**每次缓存命中都在锁内做完整堆分配 + 字符串复制**，抵消了缓存的大部分收益。

### 3.3 WAL 与提交：组提交 vs 每事务独立 fsync

**ladybug —— 教科书级 group commit**（`src/storage/wal/wal.cpp:137-173`）

```cpp
syncInProgress = true;
while (durableCommitSequence < appendedCommitSequence) {
    const auto targetSequence = appendedCommitSequence;
    serializer->getWriter()->flush();
    auto* fileToSync = fileInfo.get();
    lck.unlock();                    // fsync 期间释放锁！
    fileToSync->syncFile();
    lck.lock();
    durableCommitSequence = targetSequence;
    groupCommitCV.notify_all();
}
```

三个要点：① 一个 leader 线程为所有已 append 事务统一 fsync，N 事务 1 次 fsync；② **最慢的 I/O 不持锁**，其他事务继续追加；③ 内层循环重读 `appendedCommitSequence`，fsync 期间新到的事务被下一轮自动带上，形成流水线。

**linkrs —— 组提交默认关闭**

- `WalConfig.group_commit_enabled` 默认 `false`（`graphdb-core/src/core/wal/types.rs:796`）
- 即便开启，`append_and_wait_timeout` **只在 `DurabilityLevel::Sync` 分支被调用**
- WAL 用 `RwLock<LocalWalWriter>` 包一个只写对象（`engine/wal_manager.rs:25-31`）——单写者场景用读写锁本身就是设计错误

**默认配置下每个事务一次独立 fsync，无批量摊销。** 写事务 TPS 在 NVMe 上被钉在 ~10k 量级。

讽刺的是，`GroupCommitCoordinator` 本身实现质量不差（`Mutex<File>` + `Condvar` + 双 `AtomicU64`，leader/follower 模式正确）——**问题是它默认走不到。**

### 3.4 MVCC：真版本链 vs 平坦时间戳

**ladybug** 的 `UpdateInfo`/`VectorUpdateInfo` 通过 `prev`/`next` + `version` 构成真正的版本链（`src/include/storage/table/update_info.h:51, 127`），读走 `shared_lock`、写走 `unique_lock`，`VersionInfo` 按 vector 粒度记录插入/删除时间戳，扫描时按事务 start timestamp 过滤——**读事务永远看快照，与写者不互斥**。

**linkrs** 每个 slot 只有**一对**时间戳（`vertex/vertex_timestamp.rs:9-12`）：

```rust
pub struct VertexTimestamp {
    start_ts: Vec<Timestamp>,
    end_ts: Vec<Timestamp>,
}
```

这只能表达"存在/删除"两态。属性更新是**破坏性原地覆盖**（`vertex/vertex_table/core.rs:362-363`）：

```rust
self.columns.set_property(internal_id as usize, col_name, Some(&converted_value))
// 不保留旧值，不追加新版本
```

后果见 §4.3——**声明的 RepeatableRead 隔离级别不成立**。

---

## 四、linkrs 设计缺陷清单

按严重程度排序，每条均附可复现的代码证据。

### 【致命】缺陷 A：每个事务对全库每表每分片注册 MVCC 快照

`engine/graph_storage/context/accessors.rs:88-107`：

```rust
self.persistent.data_store.with_vertex_tables_mut(|vertex_tables| {
    for (label_id, vertex_table) in vertex_tables.iter() {   // 遍历全部点标签
        if let Ok(handle) = vertex_table.register_snapshot(timestamp) { ... }
    }
    Ok(())
})?;
self.persistent.data_store.for_all_edge_partitions_mut(|_key, table| {
    table.register_snapshot(timestamp);                       // 遍历全部边分区
    Ok(())
})?;
```

成本层层放大：

1. `with_vertex_tables_mut` 拿的是 **catalog 写锁**（`data_store.rs:551` 的 `write_vertex_tables()`）→ 所有并发事务在开启阶段完全串行化；
2. 每个 `register_snapshot` 遍历并锁住**全部分片**（`vertex/vertex_table/sharded.rs:323-329`）：

```rust
pub fn register_snapshot(&self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
    let handle = self.shards[0].table.lock().register_snapshot(ts)?;
    for shard in &self.shards[1..] {
        shard.table.lock().register_snapshot(ts)?;      // 锁遍所有分片
    }
    Ok(handle)
}
```

3. 每个分片内部再做 **O(活跃快照数) 的 min 扫描**（`vertex/vertex_table/core.rs:720-731`）：

```rust
self.mvcc.min_active_snapshot_ts = self.mvcc.active_snapshots
    .keys().min().copied()          // HashMap 无序，O(n) 全扫描
    .unwrap_or(Timestamp::MAX);
```

**总复杂度：每事务 O(点标签数 × 分片数 + 边分区数) 次加锁 + 同量级的 O(活跃快照数) 扫描，全程持 catalog 写锁。**

20 点标签 / 50 边分区 / 100 并发的场景下，单事务开启需 `20×8 + 50 = 210` 次互斥加锁 + 210 次 O(100) 扫描 ≈ 2 万次比较。

> **这是整个存储层最严重的可扩展性缺陷：事务开销与库的 schema 规模成正比，而与它实际访问的数据量无关。** 一个只读一个顶点的事务，也要为全库所有表的所有分片付费。ladybug 的对应操作是 `lastTimestamp.load(std::memory_order_acquire)`，O(1)。

### 【致命】缺陷 B：读前沿防卡死逻辑是死代码

`graphdb-transaction/src/transaction/mvcc.rs:383-406`：

```rust
let max_stalled: u64 = self.config.max_frontier_stall;   // 默认 10_000 (mvcc.rs:76)
let mut frontier = self.read_ts.load(Ordering::SeqCst);
let mut stalled: u64 = 0;                                 // ← 函数局部变量
loop {
    let next = frontier.saturating_add(1);
    match states.get(&next).copied() {
        Some(Committed | Aborted) => { frontier = next; states.remove(&next); stalled = 0; }
        Some(Pending) => {
            stalled += 1;                                 // 变成 1
            if stalled >= max_stalled {                   // 1 >= 10000 恒为 false
                frontier = next; states.remove(&next); stalled = 0;
            } else {
                break;                                    // ← 永远走这里
            }
        }
        _ => break,
    }
}
```

`stalled` 是**函数局部变量，每次调用从 0 开始**。命中 `Pending` 后 `stalled = 1`，`1 >= 10_000` 恒为假 → 立即 `break`。**`stalled` 在单次调用内永远不可能超过 1，force-advance 分支永远不会执行。**

注释声称此逻辑 "Prevents a single long-lived write transaction from blocking version GC indefinitely"（`mvcc.rs:63-66`）——**该保护完全失效**。后果：

- 一个长写事务把 `read_ts` **永久钉死**在其 ts−1，所有新读事务拿到陈旧快照；
- `write_states: Mutex<BTreeMap>` 中卡点之后的条目永不清理 → **内存无界增长**；
- `get_safe_gc_timestamp` 停滞 → 墓碑与旧版本永不回收。

**更糟的是这段代码没有正确的配置区间**：若运维为"修复卡顿"把 `max_frontier_stall` 调成 1，force-advance 就会把 `read_ts` 推过一个**仍处于 Pending 的写事务**，新读事务将看到该事务的部分写入 → **脏读**。

> 也就是说：**要么无效，要么不安全。**

### 【致命】缺陷 C：全库 flush 在 catalog 写锁内做磁盘 IO

`engine/graph_storage/context/persistence.rs:56-66`：

```rust
self.persistent.data_store.with_vertex_tables_mut(|vertex_tables| {
    for (label_id, table) in vertex_tables.iter() {
        let table_dir = vertex_dir.join(format!("label_{}", label_id));
        table.flush(&table_dir, compression)?;      // 磁盘写 + 压缩，全在 catalog 写锁内
    }
    Ok(())
})?;
```

`with_vertex_tables_mut` 内部即 `write_vertex_tables()`（catalog 写锁）。因此：**整个数据库的点表落盘期间（含压缩、序列化、文件 IO），catalog 写锁被独占持有——所有事务开启（缺陷 A 也要这把锁）、所有 DDL、所有 `with_vertex_table_mut` 操作全部阻塞。**

紧随其后的边表 flush（`persistence.rs:71-84`）虽用 per-table 锁，但仍是串行遍历，且 `maybe_compact_for_flush` 的**压缩也在写锁内**。

**在锁内执行 `write()`/`fsync` 是本项目最普遍的反模式**，缓存淘汰路径同样如此（缺陷 E）。

### 【严重】缺陷 D：并行库声明但零使用

```toml
# crates/graphdb-storage/Cargo.toml:31-32
crossbeam-utils.workspace = true
rayon.workspace = true
```

对整个 crate（src + tests）grep：**0 命中**。不存在并行扫描、并行压缩、并行 flush、并行 compaction。

这不只是"没优化"，而是**依赖清单与实现能力不符**——它会误导代码审查者和新贡献者，也让 README 的性能宣称失去支撑。

### 【严重】缺陷 E：BufferPool 单锁 + 锁内 IO + O(n²) 淘汰

`cache/buffer_pool.rs:174-249`：

```rust
let mut items = self.inner.items.lock();
let mut ids = self.inner.cached_ids.lock();
...
    if item.dirty.load(Ordering::Acquire) {
        if let Some(writer) = self.inner.writer.lock().as_ref() {
            if let Err(e) = writer(id.clone(), &item.item) {   // 磁盘写回，在两把锁内
```

三重问题：
1. **锁内磁盘 IO**：脏页写回期间所有缓存读全部阻塞；
2. **O(n·m) 淘汰**：`ids.retain(|i| *i != id)` 在淘汰循环内部，淘汰 m 项需 O(n·m)。内存压力下 `shrink_cache` 把容量砍半（一次淘汰 ~n/2 项）→ 退化为 **O(n²) 且全程持锁**；
3. **不可重入死锁隐患**：`writer` 是用户注入闭包，在持两把锁时调用。`parking_lot::Mutex` 非可重入，若该闭包回调任何 `BufferPool` 方法即**自锁死**，类型系统对此无任何约束。

此外 `insert`（`buffer_pool.rs:121-132`）存在 **TOCTOU 竞态**：容量检查在锁外，N 个线程可同时通过检查并各自插入 → 实际用量可超 capacity N 倍，进而误导 `Spiller` 的溢写决策。

### 【严重】缺陷 F：无版本链，属性更新破坏快照隔离

见 §3.4。`VertexTimestamp` 只有平坦的 `(start_ts, end_ts)`，`update_property` 原地覆盖（`vertex/vertex_table/core.rs:362-363`）。

**并发正确性风险**：事务 T1 在 `ts=100` 开启并读取顶点 V 的属性 P；事务 T2 在 `ts=105` 更新 P；T1 再次读取 P 时会看到**新值**——旧值已被物理覆盖。

这**违反 Snapshot Isolation / Repeatable Read**，而默认隔离级别恰恰声明为 `IsolationLevel::RepeatableRead`（`graphdb-transaction/src/transaction/transaction.rs:109`）。`storage/mvcc.rs` 的 `TieredTombstoneManager` 只管**删除墓碑**，对更新无能为力。

### 【严重】缺陷 G：存储层无写写冲突检测

`engine/transaction/ops.rs:116-154`（`add_edge`）、`:246-271`（`update_vertex_property_by_vid`）都是直接 `arc.write()` 后修改。`have_write_conflict` / `WriteSetAnalyzer` 只存在于 `graphdb-transaction` crate，**`graphdb-storage` 内无任何调用点**。

自动提交路径正是绕过 transaction manager 直接走 storage API 的——因此是 **Last-Writer-Wins，静默丢失更新**。

### 【中】缺陷 H：分片数硬上限 16，且运行期不可变更

```rust
// vertex/vertex_table/sharded.rs:15-16
const DEFAULT_NUM_SHARDS: usize = 8;
const MAX_SHARDS: usize = 16;
```

三个问题：

1. **不自适应**：不随 CPU 核数、数据量、冲突率变化。64 核机器上单个点标签的写并发上限被钉死在 16。
2. **不可扩容**：分片数同时决定 **ID 编码布局**（`sharded.rs:34-47`，`shard = (id >> 12) % num_shards`），改分片数 = 改 ID 语义，是**破坏性变更**，运行期无法 rehash。
3. **读读互斥**：分片用的是 `Mutex` 而非 `RwLock`（`sharded.rs:49-57`），**读路径也要拿独占锁**：

```rust
pub fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
    let table = self.shards[idx].table.lock();   // 读操作，独占锁
    table.get_by_internal_id(local_id, ts)
}
```

点查是图数据库最高频操作，这里读读互斥，8 分片意味着理论上只有 8 路并发点查。

**边表完全不参与哈希分片**——分区维度是 `EdgeTableKey{src_label, dst_label, edge_label}` 的 schema 划分。单一 `(Person)-[Follows]->(Person)` 超级标签的所有边共用**一把** `RwLock<EdgeStore>`，社交图这类典型负载下是单点瓶颈。

跨分片操作还会退化为串行全锁且**结果撕裂**（`sharded.rs:230-236`）：

```rust
pub fn total_count(&self) -> usize {
    let mut total = 0;
    for shard in &self.shards {
        total += shard.table.lock().total_count();   // 逐个上锁，不同分片不同时刻取样
    }
    total
}
```

`total_count` 返回的是一个**永不对应任何真实时刻的近似值**。

### 【中】缺陷 I：缓存键含时间戳，跨事务命中率趋近 0

```rust
// cache/record_cache.rs:150 / :165
self.vertex_pool.get(&(*key, query_ts))
self.vertex_pool.insert((key, ts), vertex, size);
```

键是 `(VertexCacheKey, Timestamp)`。由于每个事务都分配新时间戳（缺陷 A），**不同 ts 的同一顶点是不同缓存条目** → 跨事务缓存命中率接近 0，而缓存空间被同一顶点的 N 个时间戳副本占满。

失效操作因此只能全表扫描（`record_cache.rs:169-174`）：

```rust
pub fn remove_vertex(&self, key: &VertexCacheKey) {
    self.vertex_pool.retain(|(vk, _ts), _| {          // O(n) 扫全池，持 items 锁
        vk.label_id != key.label_id || vk.internal_id != key.internal_id
    });
```

`invalidate_vertices_by_label`（注释自承 "Scans all entries, O(n) complexity"）、`clear`（注释 "BufferPool doesn't support clear, use retain with false predicate"）同理。**单点更新触发全缓存扫描 + 全局锁**，写放大严重。

### 【低】缺陷 J：`segment_allocator` 是纯负收益的死计数器

```rust
// sharded.rs:70   segment_allocator: AtomicU32,
// sharded.rs:128-131
fn claim_segment(&self) {
    self.segment_allocator.fetch_add(1, Ordering::Relaxed);
}
```

全库仅 4 处引用：声明、初始化、`fetch_add`、加载时 `store`。**没有任何一处 `load` 它来做分配决策**——段归属完全由 `segment_of(idx, local_counter)` 计算。

因此 `claim_segment` 是**纯粹的无效副作用**：所有 8~16 个分片的插入都在同一条 cache line 上做 RMW（典型 false sharing），带来跨核竞争却不产生任何语义。**纯净损失。**

### 【低】缺陷 K：内存序使用不一致

- **`SeqCst` 滥用**：`graphdb-transaction/mvcc.rs` 全文清一色 `SeqCst`——`write_ts` CAS 循环、`read_ts` load/store、`read_pending`/`write_pending` 计数器。这些**全在事务开启/提交最热路径上**，绝大多数只需 `Acquire`/`Release`/`Relaxed`。典型的"不确定就用 SeqCst"。
- **`Relaxed` 的脆弱契约**（`sharded.rs:136-146`）：`record_allocation` 是 load-modify-store 三步非原子序列，目前安全**仅因为**调用点仍持有 `shard.table.lock()`。但字段声明为 `AtomicU32` 暗示可无锁访问——任何未来在锁外调用的改动都会引入**静默的 ID 分配竞态（重复 internal_id）**。既然已被锁保护就不该用 Atomic；既然用了 Atomic 就该用 `fetch_max`。
- **无效的同步对**（`buffer_pool.rs:135, 170-172`）：`Acquire` load 配 `Relaxed` store 不构成同步关系，`Acquire` 在此毫无作用。

### 架构层面的观察：重构未收敛

同一文件内并存两套矛盾风格。错误做法（`data_store.rs:586-596`）：

```rust
pub(crate) fn with_vertex_table_mut<R>(&self, label: LabelId, operation: ...) -> StorageResult<R> {
    let tables = self.write_vertex_tables();   // 只为取一个 Arc 就拿 catalog 写锁
    let table = tables.get(&label).ok_or_else(...)?;
    operation(table)                           // 用户回调在写锁内执行
}
```

正确做法就在几十行之外（`data_store.rs:614-632`，scatter-gather：短读锁收集 `Arc` → 释放 → 逐表加锁）。**说明作者已经知道正确模式，但重构没有铺开。**

值得肯定的是 `catalog_write_set`（`data_store.rs:571-584`）建立了**文档化的全局锁序**，这是防死锁的正确做法。审计中未发现明确的锁序倒置。代价是任何 schema/undo/recovery 操作都要同时持有 7 把写锁，**冻结整个存储引擎**。

---

## 五、客观看待 ladybug 的代价

为避免单向吹捧，ladybug 的复杂度成本同样明确：

1. **全局 `taskSchedulerMtx` 是理论瓶颈**。换任务时抢全局锁，且 `cv.wait` 的 predicate 执行 O(队列长度) 线性扫描（`task_scheduler.cpp:179-203`）。其假设是"任务少、任务大"，在**高频小查询的 OLTP 负载下不成立**。

2. **默认单写事务是硬性吞吐上限**（`src/transaction/transaction_manager.cpp:85-89`）：

```cpp
if (!clientContext.getDBConfig()->enableMultiWrites && hasActiveWriteTransactionNoLock()) {
    throw TransactionManagerException(
        "Cannot start a new write transaction in the system. "
        "Only one write transaction at a time is allowed in the system.");
}
```

这换来了"无需写写冲突检测、无需死锁检测、无需锁管理器"。对分析型负载划算，但**多客户端并发写场景需要打开 `enableMultiWrites`，而那条路径的成熟度显然低于默认路径**。

3. **Checkpoint 完全串行**：`grep -c "TaskScheduler" storage/checkpointer.cpp` 结果为 **0**。写密集场景下 checkpoint 是周期性长停顿（尽管读事务不受阻）。

4. **存在已知未修复的并发 bug**——这是最诚实的证据（`page_state.h:55-56`）：

```cpp
// TODO(Keenan / Guodong): Track down this rare bug and re-enable the assert. Ref #2289.
// DASSERT(getState(stateAndVersion.load()) == LOCKED);
```

**处理方式是把断言注释掉。** 乐观读 + CAS 状态机 + `MADV_DONTNEED` 的组合意味着"读者正在读的内存可能被并发 unmap"，Linux 上表现为静默读到零页，Windows 上要靠捕获 SEH 访问违例兜底（`buffer_manager.cpp:182-193`）。**这类代码的调试难度比 `RwLock` 方案高一个数量级。**

5. **Windows SEH 转换有热路径成本**：每次 `optimisticRead` 都要构造 `ScopedTranslator`，且要求整个项目用 `/EHa` 编译。

---

## 六、改进建议（按投入产出比排序）

### P0 — 正确性，必须修

| 缺陷 | 建议 |
|---|---|
| **B** 前沿卡死 | `stalled` 语义错误必须修复：改为跨调用持久化的水位（基于时间或次数），或彻底移除 force-advance 改用显式长事务超时中止。**绝不能保留"要么无效要么脏读"的现状。** |
| **F** 无版本链 | 为属性更新引入 before-image / undo 记录，否则 `RepeatableRead` 声明不成立——这是对用户的错误承诺。 |
| **G** 无冲突检测 | 在 storage 层写路径接入 `WriteSetAnalyzer`，或明确将自动提交路径的语义降级为 `ReadCommitted` 并在文档中声明。 |

### P0 — 性能，收益最大

| 缺陷 | 建议 |
|---|---|
| **A** 全库快照注册 | 改为**惰性注册**（首次访问某表时才 register），或将引用计数上提到全局 `SnapshotTracker` 单点。**这一项预计带来数量级的并发提升**，是整个清单中 ROI 最高的改动。 |
| **C** 锁内 IO | flush 前在短锁内收集 `Arc` 快照 → 释放 catalog 锁 → 锁外做 IO。配合 rayon 并行落盘。 |

### P1 — 对标 ladybug 的高性价比改造

1. **在扫描层引入 morsel 分发**——只需一个原子游标（对标 `scan_node_table.cpp:89-120`），改动量最小、收益最直接；
2. **批量导入三级缓冲**（thread-local → MPSC → 分片索引，对标 `index_builder.h:85-128`）——**Rust 的所有权模型天然适合表达这种数据流**；
3. **默认开启 group commit** 并扩展到非 Sync 持久化级（对标 `wal.cpp:137-173`），把 fsync 从每事务一次降到每批一次。

### P2 — 清理与调优

- **E**：BufferPool 按 key hash 分片；`get` 返回 `Arc<T>` 避免深拷贝；写回移出锁；`insert` 的容量检查移入锁内消除 TOCTOU。
- **I**：缓存键去掉 `Timestamp`，改用版本号 + 失效列表。
- **H**：分片数按 `available_parallelism()` 自适应；`Mutex` → `RwLock` 让读读并发；解除 16 上限（需先解耦 ID 编码与分片数）。
- **D**：在 `for_all_edge_partitions_mut` / scan / compaction 启用 rayon；引入统一线程池替换 15 处裸 `spawn`。
- **J**：删除 `segment_allocator` 死代码，消除 false sharing。
- **K**：`mvcc.rs` 的 `SeqCst` 降级；`record_allocation` 要么改回普通字段，要么用 `fetch_max`。

### 明确不建议的

**不要在 Rust 里复刻 ladybug 的"乐观读 + mmap 缓冲池"**——除非有明确实测的扫描吞吐瓶颈。它需要裸指针 + 可能被并发 unmap 的内存，会直接抵消 Rust 最大的优势（编译期内存安全），而 ladybug 自己在这条路上都还留着一个未修复的竞态。

---

## 七、总评

linkrs 的存储层代码量是 ladybug 的 **2.9 倍**（78k vs 27k 行），但并行能力**不在同一量级**。规模差异本身就是信号：大量代码花在了"用锁保护单线程结构"的样板上，而非并行数据结构本身。

分片、原子、`ArcSwap` 这些并发原语的存在**营造了并发设计的表象**，但关键路径上仍有 **4 个全局串行点**：

1. catalog 写锁（事务开启 + flush 都要）
2. WAL 写锁（每条记录）
3. `write_states` Mutex（每次提交）
4. BufferPool `items` Mutex（每次缓存访问）

**每一个都在最热路径上。** 其中缺陷 A（事务开启成本与 schema 规模成正比）与缺陷 C（锁内全库 IO）叠加在同一把 catalog 写锁上，构成了事实上的全局串行化。

真正需要警惕的不是性能差距——单机图数据库的定位本就不追求 ladybug 的分析吞吐——而是**三个正确性问题**：

- **缺陷 B**：防护逻辑是死代码，且没有安全的配置区间；
- **缺陷 F**：声明 RepeatableRead 但实现不满足；
- **缺陷 G**：自动提交路径静默丢失更新。

这三条与性能无关，是**对用户的错误承诺**。一个数据库可以慢，但不能在声明了隔离级别之后不兑现。建议优先级：**先修 B/F/G 的正确性，再做 A/C 的性能，最后才谈并行化。**

---

*分析基于两仓库 main 分支快照。全部结论可通过文中标注的 `文件路径:行号` 复核。*
