# LinkRS vs Ladybug 存储模块对比分析

## 项目概述

| 维度 | LinkRS (graphdb-storage) | Ladybug (原 Kuzu) |
|------|--------------------------|-------------------|
| 项目类型 | 轻量级单机图数据库 | 嵌入式图数据库 |
| 实现语言 | Rust | C++20 |
| 构建系统 | Cargo (workspace) | CMake |
| 存储 crate/module | `graphdb-storage` (122 个源文件) | `src/storage/` (100+ 个源文件) |
| 存储定位 | 图原生 CSR 存储引擎 | 通用列式图存储引擎 |

---

## 一、整体架构对比

### 1.1 LinkRS：内存优先 + 分层持久化

```
写入操作 → WAL(预写日志) → 内存(可变CSR) → Flush(定时刷盘)
                                            → Checkpoint(定时快照)
                                            → Snapshot(全量备份)
```

LinkRS 采用**内存优先 (in-memory-first)** 架构。所有写操作先写入内存中的可变 CSR 结构，同时追加到 WAL 保证持久性。通过定时 flush/checkpoint 将数据序列化到磁盘。数据在内存中以原生 CSR 格式存在，磁盘格式与内存格式一致。

### 1.2 Ladybug：磁盘优先 + 影子分页

```
写入操作 → LocalStorage(事务本地) → WAL → BufferManager(Pin/Unpin)
                                           → ShadowFile(写时复制)
                                           → Checkpoint(原子Apply)
```

Ladybug 采用**磁盘优先 (disk-first)** 架构。所有数据以 4KB 页面为单位存储在磁盘上，通过 BufferManager 缓存热页面。更新操作通过 Shadow File（写时复制）创建影子副本，检查点时原子地 apply 回数据文件。WAL 作为补充保障崩溃恢复。

### 1.3 架构差异总结

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 存储模式 | 内存优先 | 磁盘优先 |
| 数据粒度 | 行/边级别 | 页面级别 (4KB) |
| 写路径 | 直接写入内存 CSR | Shadow Paging (COW) |
| 读路径 | 直接读内存 CSR/列 | BufferManager Pin/Unpin |
| 持久化 | WAL + Flush + Checkpoint | WAL + Shadow File + Checkpoint |
| 内存管理 | moka 缓存 + Arena 分配器 | VM Region + 驱逐队列 |

---

## 二、CSR (压缩稀疏行) 实现对比

### 2.1 LinkRS：多态可变 CSR

LinkRS 实现了 **6 种可变 CSR 类型** + 1 种不可变 CSR：

| CSR 类型 | 适用场景 | 复杂度 |
|----------|---------|--------|
| `MutableCsr` | 通用多边/多顶点 | O(边数) |
| `SingleMutableCsr` | 一对一边关系 | O(1) |
| `MultiSingleMutableCsr` | 多个一对一边关系 | O(1) |
| `LabeledMutableCsr` | 带标签边 | O(标签数) |
| `Csr` (不可变) | 冻结段/快照 | O(1) 读取 |
| `CsrVariant` (枚举) | 运行时选择 | 动态分发 |

**双层存储设计**（MutableCsr）：
```
主块 (Primary)：每顶点固定容量槽位 (默认 4 条/顶点)
    ↓ 主块满时触发
溢出块 (Overflow)：在 nbr_list 末尾分配新空间，复制旧溢出数据
```

**碎片回收**：通过 `compact_with_ts()` 将主块和溢出块合并为扁平 CSR 布局，物理删除墓碑边。

**冻结机制**：后台异步将可变 CSR 段转换为不可变 `Csr`，冻结后 O(1) 随机读取，内存占用更紧凑。

### 2.2 Ladybug：列式 CSR

Ladybug 将 CSR 实现为**列式存储上的两列**：

```
NodeGroup (128K 节点/组)
  ├── csrOffsetColumn：每条边的起始位置
  ├── csrLengthColumn：该节点的边数
  └── 数据列：边的属性数据
```

**CSR 段管理**：
- 每个 `NodeGroup` 管理一个 CSR 段
- 段分为 `COMMITTED` / `UNCOMMITTED` / `NONE` 三种来源
- 扫描时按 `nodeGroupIdx` 定位到对应 NodeGroup，读取 CSR offset/length 列

### 2.3 CSR 实现差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| CSR 实现方式 | 原生内存结构体 | 列式存储上的列 |
| 可变性管理 | 多态可变 CSR → 冻结为不可变 | Shadow Paging (COW) |
| 溢出处理 | 溢出块 + 压缩 | 页内空间 + 新分配页面 |
| 碎片管理 | fragmentation_ratio + compact | 页面回收 |
| 边插入性能 | O(1) (主块) / O(n) (溢出) | O(1) (新页面) + BufMgr 开销 |
| 边读取性能 | O(1) 直接内存切片 | O(1) 通过 offset/length + BufMgr Pin |

---

## 三、MVCC (多版本并发控制) 对比

### 3.1 LinkRS：快照隔离 + 分级墓碑 GC

**快照机制**：
```
SnapshotHandle { ts: Timestamp, id: u64 }
  → 每个查询获取一个快照句柄
  → 读操作只读 ts 之前的已提交版本
  → 写操作分配新 ts
```

**分级墓碑管理器** (`TieredTombstoneManager`)：

| 层 | 数据结构 | 复杂度 | 容量 |
|----|---------|--------|------|
| 热层 (Hot) | `HashMap<K, Timestamp>` | O(1) | ~1000 |
| 冷层 (Cold) | 排序 `Vec<TombstoneEntry>` | O(log n) | 无上限 |

- **降级策略**：热层超过 150% 容量时，30% 条目迁移到冷层
- **GC 时机**：手动触发或 AutoGC 模式
- **GC 逻辑**：移除所有 `delete_ts < min_active_snapshot_ts` 的墓碑

### 3.2 Ladybug：版本记录 + 本地存储隔离

**版本机制**：
```
VersionRecordHandler (version_info)
  → 每条记录带版本号
  → 读操作根据事务版本过滤
  → 写操作创建新版本记录
```

**三层隔离**：
| 层 | 说明 |
|----|------|
| LocalStorage | 事务内未提交数据，其他事务不可见 |
| ShadowFile | 已提交但未检查点的数据（COW 副本） |
| DataFile | 已检查点的持久化数据 |

**扫描时的 MVCC 处理**：
```
scanNext():
  1. 扫描 COMMITTED 数据 (DataFile + ShadowFile)
  2. 切换到 UNCOMMITTED 数据 (LocalStorage)
  3. 根据事务 ts 过滤版本
```

### 3.3 MVCC 差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 隔离级别 | 快照隔离 (SI) | 快照隔离 (SI) |
| 版本存储 | 墓碑标记 + GC | 版本链 + 可见性过滤 |
| 未提交数据存储 | 事务内联 | LocalStorage 独立存储 |
| 垃圾回收 | 分级墓碑 GC | 检查点后回收旧版本 |
| 内存开销 | 墓碑 HashMap + Vec | 版本号 + Shadow Page |

---

## 四、WAL (预写日志) 对比

### 4.1 LinkRS：简明 WAL

```
WalManager
  └── LocalWalWriter (共享)
       ├── append_redo(op_type, ts, redo): 写入单条操作日志
       ├── append_transaction(tx_id, entries, intents): 批量提交事务日志
       ├── set_checkpoint_seq(seq): 标记检查点序列号
       ├── truncate(lsn): 截断旧日志
       └── sync(): 强制刷盘
```

- **LSN 管理**：由底层 `LocalWalWriter` 统一管理
- **序列化**：使用 `postcard` 格式序列化操作记录
- **配置**：`max_wal_size = 100MB`, `sync_policy = EveryWrite`
- **恢复**：重放 WAL 条目到指定 LSN

### 4.2 Ladybug：组提交 WAL

```
WAL (全局共享)
  ├── LocalWAL (事务本地)
  │    └── 事务操作先写入本地 WAL
  ├── logCommittedWAL(): 批量 flush 到 WAL 文件
  └── waitForDurabilityNoLock(): 组提交核心
       ├── appendedCommitSequence / durableCommitSequence
       ├── syncInProgress 标志
       └── groupCommitCV 条件变量 (notify_all)
```

**组提交流程**：
```
1. 线程 A: 检测 durableCommitSequence < commitSequence && syncInProgress == false
   → 承担 sync 责任，设置 syncInProgress = true
2. 线程 B, C, ...: 检测 syncInProgress == true
   → 等待在 groupCommitCV 上
3. 线程 A: 完成 fsync，更新 durableCommitSequence
   → notify_all 唤醒所有等待线程
```

**其他特性**：
- **Poison 机制**：I/O 错误后标记 WAL 为 poisoned，拒绝后续写操作
- **Checkpoint 轮转**：检查点时重命名 WAL 文件，清空当前 WAL
- **校验和**：`ChecksumReader` / `ChecksumWriter` 保证数据完整性

### 4.3 WAL 差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 提交模式 | 逐条或批量 append | 组提交 (Group Commit) |
| 并发控制 | Arc + RwLock | 条件变量 + syncInProgress |
| 事务隔离 | 事务级批量写入 | 事务本地 WAL → 全局 WAL 两级 |
| 错误处理 | 标准 Result 返回 | Poison 机制 (标记不可用) |
| 检查点联动 | set_checkpoint_seq + truncate | rotateForCheckpoint (文件重命名) |
| 校验和 | CRC32 (压缩层) | ChecksumReader/Writer (WAL 层) |

---

## 五、索引实现对比

### 5.1 LinkRS：双向 BTreeMap 索引

```
GenericIndexManager<K>
  ├── forward_index: Arc<RwLock<BTreeMap<Key, IndexRecord>>>
  ├── reverse_index: Arc<RwLock<BTreeMap<Key, IndexRecord>>>
  └── version_counter: Arc<AtomicU64>
```

**关键特性**：
- **物理键生成**：`logical_key + version_counter` 保证唯一性
- **双向索引**：正向 + 反向独立维护
- **并发**：`Arc<RwLock<BTreeMap>>` 多读单写
- **GC**：`gc_tombstones()` 批量 / `gc_tombstones_incremental()` 增量
- **持久化**：BTreeMap 序列化到磁盘

### 5.2 Ladybug：ART + 线性哈希

**ART (Adaptive Radix Tree)** — 主键索引：
```
ArtIndex
  ├── 自适应基数树（节点大小随负载变化）
  ├── 磁盘持久化 (art_index_disk.cpp)
  └── 支持范围查询
```

**线性哈希索引** — 二级索引：
```
HashIndex
  ├── pSlots: DiskArray<Slot> (主槽位)
  ├── oSlots: DiskArray<Slot> (溢出槽位)
  ├── SlotHeader { fingerprints[], invalid_flag }
  ├── 8192 条目/槽位
  └── 线性哈希扩展 (splitSlots)
```

**查找流程**：
```
1. getPrimarySlotIdForHash(key) → 定位主槽位
2. 用 fingerprint 快速过滤 (Bloom-like)
3. 逐个比较 key
4. 沿 nextOvfSlotId 溢出链继续查找
```

### 5.3 索引差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 索引类型 | BTreeMap (通用) | ART (主键) + 线性哈希 (二级) |
| 索引层级 | 内存 | 磁盘持久化 |
| 写入性能 | O(log n) | O(1) (哈希) / O(k) (ART) |
| 范围查询 | 支持 (BTree 有序) | ART 支持，哈希不支持 |
| 并发控制 | RwLock | 本地存储 + checkpoint 合并 |
| 指纹过滤 | 无 | fingerprint 快速过滤 |
| 扩展性 | BTree 自动平衡 | 线性哈希 split |
| 持久化 | 序列化整个 BTreeMap | DiskArray 按需读写 |

---

## 六、压缩/编码对比

### 6.1 LinkRS：分层编码选择器

```
CompressionSelector (selector.rs)
  ├── 输入: ColumnStats + CompressionConfig
  ├── 分析: 基基数、值范围、重复模式
  └── 输出: 最优 ColumnEncoding
         ├── Dictionary (低基数字符串)
         ├── RLE (连续重复)
         ├── BitPacking (小范围整数)
         ├── FSST (长字符串)
         └── ALP (浮点数)
```

物理负载使用 `zstd` 压缩 + CRC32 校验。

### 6.2 Ladybug：类型特化压缩树

```
CompressionMetadata Tree:
  ├── CONSTANT (min == max, 无需存储)
  ├── BOOLEAN_BITPACKING (位打包)
  ├── INTEGER_BITPACKING (差值 + FastPFOR)
  │    └── min/max 差值编码 → 位压缩
  ├── ALP (浮点 → 整数 + 子压缩)
  │    ├── 子节点: INTEGER_BITPACKING 或 CONSTANT
  │    └── ALPMetadata: exponent, factor, exceptions
  └── UNCOMPRESSED (原始)
```

通用块压缩支持：`zstd`, `lz4`, `snappy`, `brotli`。

### 6.3 压缩差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 压缩策略 | 自动选择 (CompressionSelector) | 类型硬编码 |
| 整数压缩 | BitPacking | FastPFOR (更高效) |
| 浮点压缩 | ALP | ALP + 子压缩链 |
| 字典压缩 | 有 (Dictionary) | 有 (dictionary_column) |
| 适配度 | 基于统计的运行时选择 | 编译时类型决定 |
| 原位更新 | 取决于编码 | BOOLEAN_BITPACKING/UNCOMPRESSED 支持 |

---

## 七、缓存/缓冲区管理对比

### 7.1 LinkRS：moka 缓存 + Bumpalo Arena

```
SharedRecordCache (基于 moka)
  ├── 顶点记录缓存
  ├── 可配置过期时间和内存上限
  └── 自动驱逐

Bumpalo Arena 分配器
  ├── 批量分配，批量释放
  └── 减少分配开销
```

### 7.2 Ladybug：自定义 BufferManager + VM Region

```
BufferManager
  ├── VMRegion (虚拟内存区域)
  │    ├── REGULAR_PAGE: 按 maxDBSize 预分配
  │    └── TEMP_PAGE: 按 bufferPoolSize 分配
  ├── PageState { EVICTED, UNLOCKED, MARKED, LOCKED }
  ├── EvictionQueue (无锁环形缓冲区)
  │    └── CAS 竞争空槽位
  └── Pin/Unpin API
       ├── pin(): CAS 读状态 → 返回裸指针
       └── 调用方负责写后 setFrameDirty
```

### 7.3 缓存差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 缓存策略 | moka (LRU-like) | 自定义 LRU 驱逐队列 |
| 内存模型 | 托管 (安全指针) | 非托管 (裸指针, 调用方负责) |
| 内存分配 | moka + Bumpalo Arena | VM Region 预分配 |
| 页面粒度 | 无页面概念 | 4KB 页面 |
| 并发安全 | moka 内置线程安全 | CAS + 无锁队列 |
| 适用场景 | 缓存顶点记录 | 缓存磁盘页面 |

---

## 八、事务处理对比

### 8.1 LinkRS：TransactionOps + MVCC

```
事务流程:
  1. begin_transaction() → 获取 SnapshotHandle
  2. 写操作 → 分配新 ts → append WAL
  3. 读操作 → 用快照 ts 过滤
  4. commit() → 提交 WAL → 释放快照
  5. abort() → undo 日志回滚

Undo 机制:
  UndoTarget { AddVertex, DeleteVertex, AddEdge, DeleteEdge, ... }
  → 提交前写 undo 记录
  → abort 时回放 undo
```

### 8.2 Ladybug：LocalStorage + UndoBuffer

```
事务流程:
  1. begin_transaction() → 创建 LocalStorage
  2. 写操作 → LocalStorage (事务本地) + WAL
  3. 读操作 → scanNext() 合并 COMMITTED + UNCOMMITTED
  4. commit() → flush LocalStorage → WAL group commit
  5. abort() → 丢弃 LocalStorage

UndoBuffer:
  → 仅存储 catalog 变更的回滚数据
  → 非数据行级别的 undo
```

### 8.3 事务差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 未提交数据存储 | 与已提交数据同结构 (ts 标记) | LocalStorage 独立存储 |
| Undo 机制 | 行级别 undo 日志 | Catalog 级别 undo |
| 提交冲突 | MVCC 快照隔离 | 乐观并发 |
| 读已提交数据 | O(1) 直接读 CSR | BufMgr Pin + scanNext 合并 |
| 事务大小 | 内存限制 | LocalStorage 限制 |

---

## 九、持久化对比

### 9.1 LinkRS：五层持久化链

| 层 | 默认间隔 | 说明 |
|----|---------|------|
| WAL | 实时 | 每次写操作追写日志 |
| 内存 | 实时 | 数据驻留在可变 CSR |
| Flush | 60s | 序列化到磁盘 |
| Checkpoint | 300s (阈值 10000 条) | 创建一致快照 |
| Snapshot | 3600s | 全量备份 |

**持久化格式**：魔法字节 `GRDB` + 版本号 + 标准化文件头。

### 9.2 Ladybug：影子分页 + 检查点

```
Shadow Page 生命周期:
  1. getOrCreateShadowPage() → 在 shadow file 分配新页面
  2. 修改数据 → 写入 shadow page
  3. shadowPagesMap[file][page] = shadowPageIdx
  4. Checkpoint:
     a. Flush shadow pages
     b. Apply: 将 shadow page 内容写回 data file
     c. updateFrameIfPageIsInFrame: 更新 BufferManager
     d. clear(): 重置 shadow file 容量
```

**崩溃恢复**：
1. 检测 shadow file 中有未 apply 的页面
2. `replayShadowPageRecords`: 验证 databaseID，按序写回

### 9.3 持久化差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 写时复制 | 无 (内存原地更新 + WAL) | ShadowFile (COW) |
| 检查点 | 内存数据全量序列化 | Shadow Page → Data File apply |
| 恢复 | WAL 重放 | WAL 重放 + ShadowFile replay |
| 数据一致性 | WAL + 检查点清单 | ShadowFile + WAL + Checksum |
| 检查点开销 | 全量序列化 (高) | 增量 apply (低) |

---

## 十、组件性能定性比较

### 10.1 写入性能

| 操作 | LinkRS | Ladybug | 优势 |
|------|--------|---------|------|
| 单条边插入 | O(1) 主块写入 | O(1) + BufMgr Pin | LinkRS (无 BufMgr 开销) |
| 批量边插入 | O(n) 批量 append + freeze | O(n) + 页面分配 | LinkRS (内存顺序写) |
| 顶点插入 | O(1) ColumnStore 写入 | O(1) + Shadow Page COW | LinkRS (无 COW 开销) |
| 事务提交 | WAL append + sync | WAL + SyncInProgress | Ladybug (组提交, 高并发更优) |

**结论**：LinkRS 在单条/批量写入上有优势（内存直写）。Ladybug 的组提交通过高并发场景下吞吐量更高。

### 10.2 读取性能

| 操作 | LinkRS | Ladybug | 优势 |
|------|--------|---------|------|
| 边扫描 | O(1) 内存切片 | O(n) + BufMgr Pin | LinkRS (无 I/O) |
| 顶点随机读 | O(1) 列直接读 | O(1) + BufMgr Pin | LinkRS (无缓存未命中) |
| 范围扫描 | 内存 CSR 顺序读 | 页面级顺序读 | 相近 |
| 冷数据读取 | 需要从磁盘加载 | BufMgr 缓存命中 | Ladybug (页面缓存, 更好的局部性) |

**结论**：LinkRS 内存优先架构在热数据读取上有压倒性优势（零 I/O 开销）。Ladybug 的 BufferManager 在数据量超过内存时，对冷数据有更好的渐进式访问特性，LRU 驱逐策略能更好地利用有限内存。

### 10.3 CSR 边存储

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 邻接表扫描 | 极高 (连续内存) | 中 (列存 offset 跳跃) |
| 边过滤 | 需扫描邻接表 | 列存谓词下推 |
| 写时碎片 | 有 (溢出块) → compact 解决 | 页面级碎片 → 页面回收 |
| 空间效率 | 约 24 bytes/边 | 取决于列存压缩 |
| 冻结/合并开销 | compact (可后台) | Shadow Page apply (检查点时) |

**结论**：LinkRS 在纯图遍历场景（邻接表扫描）有优势，统一内存布局消除了 I/O 开销。Ladybug 的列式 CSR 在图遍历与属性过滤混合查询中可能更优（列存谓词下推）。

### 10.4 MVCC 读开销

| 场景 | LinkRS | Ladybug |
|------|--------|---------|
| 无冲突读 | O(1) 快照过滤 (几乎零开销) | O(n) 版本过滤 + 双源合并 |
| 多版本读 | 墓碑查找 (O(1) 热层 / O(log n) 冷层) | 版本链遍历 |
| 垃圾回收 | 分级 GC (热→冷降级) | 检查点后自然回收 |
| 内存占用 | 墓碑管理有额外内存 | 版本号存储轻量 |

**结论**：LinkRS 的 MVCC 读开销更低（快照 ts + 墓碑过滤，无版本链遍历）。Ladybug 的 scanNext 需合并两个数据源（COMMITTED + UNCOMMITTED），略微增加了每次扫描的控制流复杂度。

### 10.5 索引性能

| 操作 | LinkRS (BTreeMap) | Ladybug (ART + 哈希) |
|------|-------------------|----------------------|
| 等值查找 | O(log n) | O(1) (哈希) |
| 范围查询 | O(log n + k) | O(k) (ART), 哈希不支持 |
| 前缀查询 | 不支持 | O(k) (ART) |
| 插入 | O(log n) | O(1) (哈希) |
| 并发读 | RwLock (多读单写) | 本地存储隔离 |
| 磁盘 I/O | 批量序列化 | 按需读写 DiskArray |

**结论**：Ladybug 的哈希索引在等值查询上有 O(1) 优势，ART 提供前缀查询能力。LinkRS 的 BTreeMap 提供完整的范围查询支持，但写入有 O(log n) 的树重组开销。

### 10.6 压缩效率

| 维值类型 | LinkRS (选择器) | Ladybug (类型树) |
|---------|----------------|------------------|
| 低基数字符串 | Dictionary (~90%) | Dictionary (~90%) |
| 连续整数 | RLE (极高) | FastPFOR (高) |
| 随机整数 | BitPacking (中) | FastPFOR (中高) |
| 浮点数 | ALP (高) | ALP + 子压缩 (更高) |
| 长字符串 | FSST (高) | 无专用方案 |

**结论**：两者在浮点压缩上均有 ALP，Ladybug 的子压缩链（ALP → INTEGER_BITPACKING）压缩率更高。Ladybug 的 FastPFOR 在随机整数上略优于 BitPacking。LinkRS 的 FSST 对长字符串有专属优化。

### 10.7 数据量可扩展性

| 场景 | LinkRS | Ladybug |
|------|--------|---------|
| 内存内数据集 | 极优 (原生 CSR) | 优 (页面缓存 + 内存池) |
| 超内存数据集 | 受限 (需要 OS swap) | 优 (页面级 I/O, LRU 驱逐) |
| 大规模图遍历 | 优 (连续内存) | 中 (随机页面访问) |
| 持久化大小 | 全量序列化 | 增量 Shadow Page |

**结论**：LinkRS 在数据量可完全放入内存时有最佳性能。Ladybug 的页面级管理使其能处理超过内存容量的大数据集，且通过 BufferManager 的 LRU 策略保持良好的缓存命中率。

### 10.8 崩溃恢复

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 恢复速度 | WAL 重放 (全量) | WAL 重放 + ShadowFile replay (增量) |
| 数据完整性 | WAL (postcard) + CRC32 | WAL + Checksum + ShadowFile |
| 故障注入测试 | PersistenceFaultPoint (7 个注入点) | 未观察到专用注入点 |

**结论**：Ladybug 的 ShadowFile 机制减少了检查点崩溃时重放的 WAL 量，恢复更快。LinkRS 依赖完整的 WAL 重放，恢复时需要重放自上次检查点以来的所有日志。

---

## 十一、总结

### 11.1 设计哲学差异

| 维度 | LinkRS | Ladybug |
|------|--------|---------|
| 核心设计 | 内存优先的图原生存储 | 磁盘优先的列式通用存储 |
| 目标场景 | 中小规模、高性能图计算 | 大规模、持久化优先的分析场景 |
| 实现复杂度 | 可变 CSR 类型多、冻结机制精细 | 页面管理、BufferManager 复杂 |
| 代码量 | 约 122 个源文件 | 约 160+ 个源文件 |

### 11.2 各自优势场景

**LinkRS 更优的场景**：
- 数据量可完全放入内存的中小规模图
- 高吞吐写入（内存直写）
- 图遍历密集型查询（连续内存 CSR）
- 需要细粒度行级 undo 的事务
- 对延迟敏感的低延迟读取

**Ladybug 更优的场景**：
- 超内存容量的大数据集
- 高并发事务提交（Group Commit）
- 属性过滤密集型查询（列存谓词下推）
- 需要多种后端格式兼容（ICE、Arrow）
- 前缀/范围查询（ART 索引）

### 11.3 关键启示

1. **CSR 实现**：LinkRS 的 6 种可变 CSR 类型 + 冻结机制提供了磁盘格式与内存格式高度统一的优势，但类型复杂度较高。Ladybug 将 CSR 实现为列式存储上的列，在架构统一性上更优。

2. **MVCC 设计**：LinkRS 的分级墓碑 GC 是新思路，分离热/冷墓碑减少了 GC 扫描开销。Ladybug 的 LocalStorage 隔离未提交数据，避免了内存中的墓碑标记。

3. **持久化策略**：LinkRS 的 WAL + Flush + Checkpoint 分层策略更简洁，但恢复时需要重放更多 WAL。Ladybug 的 ShadowFile 提供了增量检查点能力，恢复更快但实现更复杂。

4. **索引选择**：ART + 线性哈希的组合（Ladybug）在查询类型覆盖上优于纯 BTreeMap（LinkRS），但实现复杂度更高。
