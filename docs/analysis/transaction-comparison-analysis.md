# LinkRS vs Ladybug 事务模块对比分析

## 项目概述

| 维度 | LinkRS | Ladybug |
|------|--------|---------|
| 项目类型 | 轻量级单机图数据库 | 嵌入式图数据库 |
| 实现语言 | Rust | C++20 |
| 事务核心模块 | `graphdb-transaction` crate | `src/transaction/` + `src/storage/` 中的 MVCC/Undo/WAL |
| 事务定位 | 内存优先、MVCC 快照隔离 | 磁盘优先、LocalStorage + 快照隔离 |

---

## 一、整体架构对比

### 1.1 LinkRS：统一事务管理器 + 三级事务类型

```
TransactionManager
  ├── VersionManager (MVCC 时间戳管理)
  │    ├── acquire_read_timestamp()    ── 读事务（共享快照，无阻塞）
  │    ├── acquire_insert_timestamp()  ── 插入事务（各自独立时间戳，多并发）
  │    └── acquire_update_timestamp()  ── 更新事务（独占，阻塞所有读写）
  ├── UndoLogManager (16 种 Undo 条目，LIFO 回滚)
  ├── SnapshotTracker (O(1) 最小活跃快照查询)
  ├── TransactionCommitSink (两阶段提交支持)
  └── TransactionCleaner (过期事务清理)
```

LinkRS 将事务按操作类型分为**读、插入、更新**三种，分别对应不同的时间戳获取策略和并发度。所有未提交数据与已提交数据共享同一存储结构（通过时间戳标记区分），回滚依赖完整的行级 Undo 日志。

### 1.2 Ladybug：LocalStorage 隔离 + Group Commit

```
TransactionManager
  ├── 单写事务锁 (默认 enableMultiWrites=false)
  ├── WAL + Group Commit (condition_variable 协调)
  │    ├── LocalWAL (事务内 WAL 缓冲)
  │    └── 全局 WAL (logCommittedWAL + fsync)
  ├── LocalStorage (事务内私有写缓冲)
  │    ├── LocalNodeTable (本地节点表)
  │    ├── LocalRelTable (本地关系表)
  │    └── LocalHashIndex (本地 PK 索引)
  ├── UndoBuffer (Catalog/Sequence 级 Undo)
  ├── VersionInfo (行级插入/删除 MVCC)
  └── UpdateInfo (列级更新版本链)
```

Ladybug 将未提交数据隔离在 `LocalStorage` 中（事务私有），提交时合并到持久化存储。行级 MVCC 通过 `VersionInfo`（插入/删除）和 `UpdateInfo`（更新版本链）实现，Undo 主要用于 Catalog 和 Sequence 变更的回滚。

### 1.3 架构差异总结

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 未提交数据存储 | 与已提交数据同结构（ts 标记） | LocalStorage 独立隔离 |
| 事务类型 | Read / Insert / Update 三级 | READ_ONLY / WRITE / CHECKPOINT / DUMMY / RECOVERY |
| 并发策略 | 基于原子变量的请求计数控制 | 基于锁层次结构的互斥控制 |
| Undo 粒度 | 完整的行级 Undo（16 种类型） | Catalog 级 Undo + MVCC 版本标记 |
| WAL 架构 | LocalWalWriter 直接写 | LocalWAL → 全局 WAL 两级 + Group Commit |
| 事务模式 | 单一模式 | AUTO / MANUAL 双模式 |

---

## 二、MVCC 实现对比

### 2.1 LinkRS：原子时间戳 + 环形缓冲区

**时间戳获取协议：**

| 时间戳类型 | 并发度 | 获取策略 |
|-----------|--------|---------|
| 读时间戳 | 多并发（默认1000） | `pending_reqs >= 0` 时获取，共享快照时间戳 |
| 插入时间戳 | 多并发（默认100） | `pending_reqs >= 0` 时获取，各自递增时间戳 |
| 更新时间戳 | 独占（默认1） | CAS 设置 `pending_update_reqs=1`，`pending_reqs` 减为负数阻塞所有读写 |

**环形缓冲区机制：**
- 容量：1M 条目（`RING_BUF_SIZE = 1024 * 1024`）
- 达到 50% 时发出警告，接近容量时阻塞新事务
- 释放时间戳时通过位图跟踪，推进 `read_ts` 清理连续已释放位

**快照跟踪：** `SnapshotTracker` 使用 `DashMap<ts, AtomicU64>` + `BTreeMap<ts, u32>` 维护活跃快照集合，O(log n) 添加/释放，O(1) 查询最小活跃快照。

### 2.2 Ladybug：版本号 + 版本链

**时间戳方案：**
```
事务ID > START_TRANSACTION_ID(1<<63) → 未提交事务
版本号 < START_TRANSACTION_ID → 已提交（commitTS）
可见性: 版本 <= startTS 或 版本 == transactionID
```

**VersionInfo（行级插入/删除追踪）：**
- 每个 2048 行的 Vector 维护 `insertedVersions` 和 `deletedVersions` 数组
- 可见性判定：`isInserted && !isDeleted`，其中 `isInserted = (version == txID || version <= startTS)`
- 优化：同事务插入使用 `sameInsertionVersion` 节省内存

**UpdateInfo（列级更新版本链）：**
- 版本链结构：`VectorUpdateInfo → prev → prev → ...（从新到旧）`
- 写写冲突检测：遍历版本链，发现其他事务更新了同行时抛出异常
- 可见性：取版本链中第一个满足 `version == txID || version <= startTS` 的版本

### 2.3 MVCC 差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 版本存储方式 | 时间戳标记 + 墓碑 GC | 版本数组 + 版本链 |
| 无冲突读开销 | O(1) 快照 ts 过滤（几乎零开销） | O(n) 版本遍历 + 双源合并（COMMITTED+UNCOMMITTED） |
| 写写冲突检测 | WriteSet 集合级比较 | 行级版本链遍历 |
| 内存开销 | 墓碑 HashMap + Vec | 版本号数组 + 版本链节点 |
| GC 策略 | 分级墓碑 GC（热/冷层） | 检查点后自然回收旧版本 |

---

## 三、Undo 机制对比

### 3.1 LinkRS：完整行级 Undo 日志

**16 种 Undo 日志条目类型：**
```
CreateVertexType, CreateEdgeType, InsertVertex, InsertEdge,
UpdateVertexProp, UpdateEdgeProp, RemoveVertex, RemoveEdge,
AddVertexProp, AddEdgeProp, RenameVertexProp, RenameEdgeProp,
DeleteVertexProp, DeleteEdgeProp, DeleteVertexType, DeleteEdgeType
```

**UndoLogManager：**
- 内部封装 `Vec<UndoLogEntry>`
- 回滚时 LIFO 顺序执行（`pop()` 从末尾弹出）
- 支持从指定索引回滚（保存点场景）

**UndoTarget trait：** 15 种 Undo 操作方法，每条 Undo 记录以正向操作不成功为前提设计（如 `InsertVertexUndo` 通过调用 `graph.delete_vertex()` 撤销插入）。

### 3.2 Ladybug：Catalog 级 Undo + MVCC 版本标记

**6 种 Undo 记录类型：**
```
CATALOG_ENTRY, SEQUENCE_ENTRY, UPDATE_INFO, INSERT_INFO, DELETE_INFO
```

**关键区别：**
- 行级数据（INSERT/DELETE）的 MVCC 通过 `VersionInfo` 的版本标记实现回滚，而非 Undo 日志
- Undo 主要用于 Catalog 变更和 Sequence 变更的回滚
- commit 时将 Undo 记录中的未提交版本号替换为 `commitTS`
- rollback 时反向遍历 Undo 记录，恢复旧状态

### 3.3 Undo 机制差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 行级数据 undo | 16 种完整 Undo 条目 | MVCC 版本标记（VersionInfo） |
| 回滚方式 | LIFO 执行 Undo 操作 | FILO 遍历 Undo 记录恢复 |
| 大事务内存压力 | 高（Undo 条目全部在内存） | 低（行级通过版本标记，无需额外 Undo） |
| Catalog undo | 包含在 16 种条目中 | 独立的 CATALOG_ENTRY |
| commit 时 Undo 处理 | clear() 清空 | commit(commitTS) 替换版本号 |

---

## 四、WAL 交互对比

### 4.1 LinkRS：简明 WAL

```
LocalWalWriter (共享)
  ├── append_redo(op_type, ts, redo): 写入单条操作日志
  ├── append_transaction(tx_id, entries, intents): 批量提交事务日志
  ├── set_checkpoint_seq(seq): 标记检查点序列号
  ├── truncate(lsn): 截断旧日志
  └── sync(): 强制刷盘
```

- 插入事务缓冲所有操作到 `wal_buffer`，提交时原子写入
- 支持 `DurabilityLevel::None/Async/Sync` 三种持久化级别
- 序列化格式：`postcard` + CRC32 校验

### 4.2 Ladybug：组提交 WAL

```
全局 WAL
  ├── LocalWAL (事务内缓冲)
  │    └── 操作先写入本地 WAL（内存）
  ├── logCommittedWAL(): Flush 到全局 WAL 文件
  └── waitForDurabilityNoLock(): 组提交核心
       ├── appendedCommitSequence / durableCommitSequence
       ├── syncInProgress 标志
       └── groupCommitCV 条件变量 (notify_all)
```

**组提交流程：**
1. 线程 A 检测到需要 sync → 设置 `syncInProgress = true`
2. 线程 B、C 检测到 `syncInProgress == true` → 等待在 `groupCommitCV`
3. 线程 A 完成 `fsync` → `notify_all` 唤醒所有等待线程

**Poison 机制：** I/O 错误后标记 WAL 为 poisoned，后续所有写操作被拒绝，数据库进入 panic 状态。

### 4.3 WAL 差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 提交模式 | 逐条或事务批量 | 组提交 (Group Commit) |
| WAL 架构 | 单层 LocalWalWriter | 两层：LocalWAL → 全局 WAL |
| 高并发吞吐 | 一般 | 更高（组提交批量 fsync） |
| 错误防护 | 标准 Result 返回 | Poison 机制（标记不可用） |
| 校验 | CRC32 (压缩层) | ChecksumReader/Writer (WAL 层) |
| 检查点联动 | set_checkpoint_seq + truncate | rotateForCheckpoint (文件重命名) |

---

## 五、并发控制对比

### 5.1 LinkRS：原子变量 + 条件变量

```
VersionManager 中的并发控制字段:
  pending_reqs: AtomicI32         ── 控制读/插入并发数
  pending_update_reqs: AtomicI32  ── 更新独占标志（0/1）
  thread_num: AtomicI32           ── 线程数

并发规则:
  - 读事务：pending_reqs >= 0 时获取（默认最多1000并发）
  - 插入事务：pending_reqs >= 0 时获取（默认最多100并发）
  - 更新事务：CAS 将 pending_update_reqs 从 0 设为 1，
    然后将 pending_reqs 减 thread_num 变为负数，阻塞所有读写
```

**写写冲突检测：** 基于 `WriteSet`（顶点 HashSet + 边 HashSet），共享端点的边也被认为冲突。

### 5.2 Ladybug：锁层次结构

```
mtxForSerializingPublicFunctionCalls    (序列化 begin/commit/rollback)
    └── mtxForStartingNewTransactions   (控制新写事务启动)
```

**默认单写事务 (enableMultiWrites=false)：**
- 只允许一个活跃写事务
- 提交过程中允许新写事务排队（`cvForCommittingWriteTransaction` 协调）

**行级冲突检测：** `UpdateInfo::update()` 遍历版本链，如发现其他事务更新了同一行则抛出异常。

### 5.3 并发控制差异

| 特性 | LinkRS | Ladybug |
|------|--------|---------|
| 并发控制原语 | Atomic + Condvar | Mutex + ConditionVariable |
| 写事务并发 | 插入多并发，更新独占 | 默认单写（实验性多写） |
| 冲突检测粒度 | 顶点/边集合级别 | 行级别 |
| 误判风险 | 较高（共享端点被误判为冲突） | 低（精确行级检测） |

---

## 六、事务生命周期能力对比

| 能力 | LinkRS | Ladybug |
|------|--------|---------|
| 隔离级别 | 仅 RepeatableRead（快照隔离） | 仅快照隔离 |
| 保存点 (Savepoint) | 完整支持（创建、释放、回滚到保存点） | 不支持 |
| 两阶段提交 (2PC) | TransactionCommitSink 支持 | 不支持 |
| 嵌套事务 | 不支持 | 不支持 |
| 死锁检测 | 无 | 无 |
| 事务超时 | 多种超时（总时长、查询、语句、空闲） | 仅 Checkpoint 等待超时（5秒） |
| AUTO/MANUAL 模式 | 仅单一模式 | AUTO + MANUAL 双模式 |
| 事务过期清理 | TransactionCleaner 被动清理 | 连接析构时自动回滚 |

---

## 七、LinkRS 事务模块的不足分析

### 7.1 WAL 缺少组提交机制

LinkRS 的 WAL 采用逐条或事务批量追加方式写入，每次 `sync()` 单独进行。在高并发多事务同时提交的场景下，如果多个事务各自独立 fsync，磁盘 I/O 会成为瓶颈。

Ladybug 的 Group Commit 通过 `condition_variable` 协调多个事务共用一次 fsync，大幅提高高并发下的写入吞吐量。`appendedCommitSequence` 和 `durableCommitSequence` 的双序列号机制保证了即使多个事务共用一次同步也不会丢失持久化保证。

### 7.2 更新时间戳获取阻塞所有读写

LinkRS 的更新事务在获取时间戳时需要将 `pending_reqs` 设置为负数，**完全阻塞**所有正在进行的和即将开始的读事务和插入事务。这意味着：

- 即使更新只涉及一个顶点的一条属性，所有不相关的图遍历查询也会被阻塞
- 更新时间戳获取有 10 秒超时（`update_acquire_timeout`），如果存在长时间运行的读事务，更新事务会等待超时后失败

Ladybug 默认情况下也只允许单写事务，但**读写之间不互斥**：写事务通过 LocalStorage 隔离修改，读事务使用快照继续读取已提交数据，互不影响。

### 7.3 写集合冲突检测粒度过粗

LinkRS 的写写冲突检测基于 `WriteSet` 集合级比较：
- 两个事务操作了同一个顶点 → 冲突
- 两个事务操作了同一条边 → 冲突
- 两个事务的边存在共享端点 → **也判为冲突**

第三条规则过于激进。例如：事务 A 插入边 (u1, u2)，事务 B 插入边 (u1, u3)，二者不构成数据冲突，但 LinkRS 会因为共享端点 u1 而判定冲突，导致其中一个事务被拒绝。

Ladybug 的冲突检测在 `UpdateInfo::update()` 中以行级精确判断，不会出现这种过度保守的误判。

### 7.4 Undo 日志内存压力大

LinkRS 的 16 种 Undo 日志条目全部存储在内存中的 `Vec<UndoLogEntry>`。当一个事务包含大量写操作（如批量导入百万条边），Undo 日志将占用大量内存。如果事务最终提交成功，这些 Undo 日志占用的内存实际上是浪费的。

Ladybug 的行级数据回滚通过 `VersionInfo` 的版本标记实现，不需要额外的 Undo 条目存储。仅在 Catalog 和 Sequence 变更时才记录 Undo，内存开销极小。

### 7.5 缺少 WAL Poison 防御机制

Ladybug 在 WAL 的 I/O 操作失败后，会将 WAL 标记为 `poisoned = true`，并记录 `poisonReason`。此后所有写操作都会通过 `throwIfPoisonedNoLock()` 检查并被拒绝，数据库进入受控的 panic 状态，防止数据不一致扩散。

LinkRS 的 WAL 写入错误通过标准的 Rust `Result` 返回，缺少这种"全局污染"的防御性设计。如果 WAL 在中间状态损坏，后续事务可能继续在损坏的日志上追加，导致崩溃恢复时数据不一致。

### 7.6 时间戳环形缓冲区容量瓶颈

LinkRS 使用 1M 条目的环形缓冲区防止时间戳溢出，在达到 50% 时发出警告，接近 100% 时**阻塞新事务**。这意味着：

- 在高并发场景下（如每秒处理数十万事务），如果旧事务持有快照时间过长，缓冲区会快速填满
- 一旦阻塞，整个系统将完全停止接受新事务，影响面极大
- 缺少动态扩容或降级策略

Ladybug 的时间戳方案（事务 ID 从 `1<<63` 递增，commitTS 从 1 递增）通过值域天然分离未提交和已提交时间戳，没有缓冲区容量限制。

### 7.7 提交阶段错误处理不对称

LinkRS 的提交协议在 `sync_manager` 失败时的处理存在不对称：

- **commit** 阶段：`sync_manager.commit_transaction_sync()` 失败时仅记录日志，继续完成本地提交（依赖 outbox 重试）
- **abort** 阶段：`sync_manager.rollback_transaction_sync()` 失败时回退时间戳、移除事务，直接返回错误

这种不对称意味着：如果 sync_manager 在 abort 时不可用，事务将进入无法恢复的状态 —— 时间戳已回退，但 sync_manager 中的回滚未执行，可能导致索引或缓存中的脏数据残留。

### 7.8 提交状态不可重试

LinkRS 的事务状态机从 `Active → Committing` 后，如果提交失败，**无法从 Committing 状态重试**。一旦进入 Committing 状态，唯一的出路是转换为 Committed 或 Aborted。如果 commit_sink 或 sync_manager 临时不可用导致提交失败，事务必须 abort 然后由上层重试整个业务流程，增加了延迟和复杂性。

### 7.9 回滚流程设计缺陷

`abort_transaction_with_undo` 的流程是先执行 Undo 回滚**再进行**状态转换（`Active → Aborting`）。这意味着如果 Undo 回滚过程中发生错误，事务仍处于 Active 状态，状态机的一致性被破坏。

同时，从代码中看，如果 Undo 回滚成功但后续的 `commit_sink.abort_transaction()` 失败，事务可能处于部分回滚的状态 —— 存储层已通过 Undo 恢复，但持久化层（WAL/checkpoint）的丢弃未完成。

### 7.10 缺少专用 Checkpoint 事务类型

Ladybug 支持 `TRANSACTION_TYPE::CHECKPOINT`，允许 checkpoint 操作作为一个独立的事务类型，在检查点期间**停止新写事务并等待现有写事务完成**，确保检查点的一致性。

LinkRS 的 checkpoint 与正常事务共享同一个时间戳管理器，没有专门的隔离机制。在 checkpoint 期间新事务仍然可以开始，可能导致 checkpoint 数据与活跃事务状态之间的不一致。

---

## 八、总结

### 8.1 LinkRS 事务模块的优势

- **读性能极优**：读事务获取共享快照后几乎零 MVCC 开销，O(1) 内存访问
- **保存点支持**：完整的 Savepoint + Rollback to Savepoint，提供了事务内部分回滚的灵活性
- **两阶段提交**：`TransactionCommitSink` 为分布式事务场景提供了扩展点
- **多级超时控制**：事务级、查询级、语句级、空闲级四种超时，粒度精细
- **分级墓碑 GC**：热/冷两层墓碑管理，热层 O(1) 查询，减少 GC 扫描开销

### 8.2 LinkRS 事务模块的主要不足

| 不足项 | 严重程度 | 影响 |
|--------|---------|------|
| 缺少 Group Commit | 高 | 高并发提交吞吐量受限 |
| 更新事务阻塞所有读写 | 高 | 长读事务导致写饥饿 |
| 写冲突检测粒度过粗 | 中 | 不冲突的事务被误判拒绝 |
| Undo 日志内存压力 | 中 | 大事务场景内存膨胀 |
| 缺少 WAL Poison 机制 | 高 | 数据一致性缺乏防线 |
| 环形缓冲区容量瓶颈 | 中 | 极端高并发下系统阻塞 |
| 错误处理不对称 | 中 | abort 失败时状态不一致 |
| Committing 状态不可重试 | 中 | 临时故障导致事务失败 |
| 缺少专用 Checkpoint 事务 | 中 | 检查点期间一致性无法保证 |

### 8.3 改进建议

1. **引入 Group Commit**：参考 Ladybug 的设计，在 WAL 层增加 `appendedCommitSequence` / `durableCommitSequence` 双序列号 + 条件变量协调，实现批量 fsync
2. **细化更新事务的阻塞范围**：将更新事务获取时间戳时的阻塞从"全部读写"缩小到"仅冲突的写"，或引入行级意向锁
3. **优化写冲突检测**：移除共享端点即冲突的规则，改为基于实际行/属性级别的冲突检测
4. **增加 WAL Poison 保护**：在 WAL 写入失败后标记全局状态，阻止后续写操作
5. **压缩 Undo 日志**：对大事务的 Undo 日志引入磁盘溢出或压缩机制，或参考 Ladybug 的方案，将行级数据回滚改为版本标记
6. **统一错误处理路径**：确保 commit 和 abort 阶段对 sync_manager 失败采用一致的策略（全部失败或全部重试）
