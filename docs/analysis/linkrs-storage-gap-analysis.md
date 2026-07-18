# LinkRS 存储模块不足分析与改进建议

## 前置说明

本文档基于对 LinkRS `graphdb-storage` crate 全部 122 个源文件的深度审查，从 WAL/持久化、MVCC/并发、索引、边/顶点存储、配置 5 个维度逐一分析。每个发现均标注：(1) 是否为架构选型下的合理取舍，(2) 是否为考虑不全面之处，(3) 具体的改进方向。

---

## 一、WAL 与持久化

### 1.1 撤销日志仅在内存中 (崩溃不安全)

**问题描述**：`UndoLogManager` 将撤销条目存储在 `Vec<UndoLogEntry>` 中，完全驻留在内存。若活动事务期间发生崩溃，所有 undo 条目丢失。重启后 WAL 恢复会重放已提交的 redo 条目，但正在执行中的事务缺少 undo 信息，无法回滚部分完成的操作——例如，一条边被插入后事务尚未提交就崩溃了，恢复后该边会残留在数据库中。

- 代码位置：`transaction/undo_log.rs` 第 546-548 行
- 严重程度：严重 —— 可能导致事务原子性被破坏

**判定：考虑不全面。** 内存优先架构不意味着 undo 也必须是纯内存的。即使是内存数据库（如 VoltDB、MemSQL），undo 日志也会持久化以保证事务原子性。

**改进建议**：
- 将 undo 日志写入 WAL（与 redo 一起记录），或写入独立的 undo 段
- 恢复时先重放 WAL 识别未完成的事务，再根据持久化的 undo 信息回滚
- 短期缓解：在 WAL entry 中增加 `transaction_state` 标记（BEGIN/COMMIT/ABORT），恢复时自动丢弃未标记 COMMIT 的事务的 redo 操作

### 1.2 损坏的 WAL 条目被静默跳过

**问题描述**：WAL 恢复过程中遇到未知 OpType 或反序列化失败时，仅递增 `errors_encountered` 计数器并继续。损坏的条目被永久丢弃，对应数据静默丢失。没有死信队列、审计日志或阈值告警机制。

- 代码位置：`recovery.rs` 第 187-207 行
- 严重程度：严重 —— 生产环境中磁盘静默损坏可能导致数据丢失且无感知

**判定：考虑不全面。** 任何持久化系统都必须区分"可恢复错误"与"不可恢复错误"。WAL 条目损坏属于不可恢复的信号，应触发显式告警。

**改进建议**：
- 将损坏条目写入独立的死信文件（`wal_dead_letter.bin`），供事后审计
- 当 `errors_encountered` 超过阈值（如 1% 的 WAL 条目或绝对值 100）时，恢复以错误终止并要求人工干预
- 为每个 WAL 条目增加独立 CRC（目前 CRC 仅在压缩层存在），实现条目级完整性校验

### 1.3 WAL LSN 先于 fsync 更新

**问题描述**：`LocalWalWriter::write_entry` 在实际 fsync 之前就更新了内存中的 `current_lsn`。如果同步策略为 `Never` 或同步中途失败，内存 LSN 已推进但数据未落盘，后续 checkpoint 可能引用不存在的 WAL 位置。

- 代码位置：`wal/local.rs` 第 663-677 行
- 严重程度：高

**判定：考虑不全面。** LSN 的语义是"此位置之前的数据已安全持久化"。过早更新违反了这一契约。

**改进建议**：
- 将 LSN 更新移到 fsync 确认之后
- 增加 `committed_lsn` 与 `flushed_lsn` 双指针，区分"已接受"和"已持久化"

### 1.4 Checkpoint 发布与 WAL 截断之间无原子性

**问题描述**：checkpoint manifest 发布 (`publish_checkpoint_manifest`) 先于 WAL 截断执行。若进程在两者之间崩溃，checkpoint 引用的是已被截断（或不存在）的 WAL 条目范围。

- 代码位置：`persistence_coordinator.rs` 第 526-534 行
- 严重程度：高

**判定：考虑不全面。** 应遵循"先截断 WAL，再发布 checkpoint"或使用两阶段提交模式。

**改进建议**：
- 调换操作顺序：先截断 WAL，再发布 checkpoint manifest
- 或在 manifest 中记录 WAL 截断后的 LSN 边界，恢复时交叉验证

### 1.5 架构选型下的合理取舍

以下 WAL 设计特点属于架构选型下的合理取舍：

| 取舍项 | 理由 |
|--------|------|
| 无组提交 (Group Commit) | 内存优先架构下事务提交延迟已很低（内存写+WAL append），组提交带来额外延迟复杂度，收益有限 |
| postcard 序列化格式 | 比 protobuf 更轻量，适合纯 Rust 生态，减少依赖 | 
| WAL 采用追加写模式 | 简化恢复逻辑，避免随机 I/O |
| 无 WAL 分段/轮转 | 内存优先意味着 WAL 尺寸小（仅保上次 checkpoint 之后的增量），不需要复杂分段管理 |

---

## 二、MVCC 与并发控制

### 2.1 长时间运行的事务无限期阻塞 GC

**问题描述**：GC 的高水位线由 `min_active_snapshot_ts()` 决定（所有活跃快照的最小时间戳）。任何长时间运行的事务会将该水位线钉在极低的值，导致在它之前创建的所有墓碑永远无法被回收。没有看门狗、超时或强制终止机制。

- 代码位置：`mvcc.rs` 第 51 行
- 严重程度：严重 —— 在 OLTP+OLAP 混合负载中，一个持续的分析查询会导致 GC 完全停滞

**判定：考虑不全面。** 几乎所有 MVCC 数据库（PostgreSQL、MySQL InnoDB、CockroachDB）都有长事务检测和干预机制。

**改进建议**：
- 增加快照最大存活时间配置（如 `max_snapshot_age: 300s`），超时后强制驱逐
- 引入"GC 安全水位线"概念：当最老快照存活超过阈值时，GC 仍可推进到第二老的快照时间戳，仅保留最老快照可能访问的版本（而非所有版本）
- 暴露 `list_active_snapshots` API 供运维人员排查

### 2.2 墓碑冷层无界增长

**问题描述**：`TieredTombstoneManager` 的冷层（已排序 `Vec`）仅在 `gc()` 时清理，而 gc 又被快照水位线阻塞。在持续写入+一个长事务的场景下，冷层可能增长到数百万条目（极端情况下 > 80MB），且无上限。

- 代码位置：`mvcc.rs` 第 80-81 行，第 139-167 行
- 严重程度：高

**判定：考虑不全面。** 分级墓碑管理是好的设计思路（借鉴 JVM GC 的分代思想），但需要配套的背压机制。

**改进建议**：
- 为冷层设置绝对上限（如 `max_cold_tombstones: 1_000_000`），超出后触发强制 GC 或拒绝新写入（背压）
- 监控冷层大小并在 metrics 中暴露
- 对于极端情况，冷层可溢出到磁盘（磁盘上的墓碑文件）

### 2.3 全局写锁导致跨标签写入串行化

**问题描述**：`GraphDataStore` 对所有顶点表使用单一 `RwLock<HashMap<LabelId, VertexTable>>`，对所有边表同理。即使两个并发写入操作修改不同标签（完全无冲突），仍需竞争同一把全局锁。

- 代码位置：`data_store.rs` 第 41-42 行
- 严重程度：严重 —— 多租户或多标签场景下写入吞吐量受限于单锁

**判定：架构选型下的合理初版实现，但需要演进。** 单锁在原型阶段是合理的（Rust 的 `RwLock<HashMap>` 模式简单安全），但不应作为最终方案。

**改进建议**：
- 将 `HashMap` + `RwLock` 替换为 `DashMap`（分片锁，内置并发安全）
- 或使用 `Arc<RwLock<VertexTable>>` 逐表加锁模式
- 权衡：`DashMap` 的迭代器性能较差（需收集快照），需评估迭代频率

### 2.4 缓存 TTL 过长导致 MVCC 陈旧数据窗口

**问题描述**：RecordCache 默认 TTL 为 3600 秒（1 小时）。顶点被修改后，旧的缓存条目在最多 1 小时内仍然可被命中。失效仅在标签级别（`invalidate_vertices_by_label`），而非单条记录级别。

- 代码位置：`record_cache.rs` 第 224 行，`config.rs` 第 29 行
- 严重程度：高 —— 高频更新场景下，用户可观察到长达 1 小时的陈旧数据

**判定：配置不当 + 失效粒度不足。** 缓存机制本身是合理的（减少热顶点重复读取），但失效策略过于粗糙。

**改进建议**：
- 将默认 TTL 从 3600s 降至 60s（或用户可配置）
- 在顶点写入路径中增加精确失效：`cache.invalidate(vertex_id)`
- 引入"写入穿透（write-through）"模式作为可选项：更新时间步更新缓存

### 2.5 compact_with_ts 忽略时间戳参数

**问题描述**：`compact_with_ts(&mut self, _ts: u32, ...)` 完全忽略其 `_ts` 参数，直接移除所有 `delete_ts != u32::MAX` 的边，不进行 MVCC 安全验证。如果调用者传入错误的快照时间戳，可能删除尚被其他快照引用的边。

- 代码位置：`mutable_csr.rs` 第 735 行
- 严重程度：中 —— 需要调用者正确使用，但 API 签名暗示了不存在的安全性

**判定：考虑不全面。** 这是一个 API 契约问题。函数签名承诺了 MVCC 安全的 compact，但实现忽略安全保障。

**改进建议**：
- 方案 A：修复实现，在 compact 时用 `ts` 过滤，仅删除 `delete_ts < ts` 的边
- 方案 B：若当前行为是故意的，将参数改为 `force_compact()` 并文档化"忽略 MVCC 安全性，调用者需确保无活跃快照引用被删除的边"
- 方案 A 更安全

### 2.6 架构选型下的合理取舍

| 取舍项 | 理由 |
|--------|------|
| 快照隔离 (SI) 而非可串行化 (SSI) | SSI 需要写意图跟踪，复杂度高；SI 对图数据库已足够（多数图查询是读密集型） |
| 无死锁检测 | Rust 的 `parking_lot` 锁不中毒、不暴露死锁检测钩子；图数据库的事务依赖图通常较浅 |
| SnapshotGuard RAII 模式 | 虽然 drop 失败时可能泄漏，但这是 Rust 的普遍取舍——`Mutex::lock()` 中毒后也不自动恢复 |
| 单表 `&mut self` 模式（无法并发读写） | Rust 所有权模型的天然约束；要支持需要在表内部引入内部可变性，增加复杂度 |

---

## 三、索引

### 3.1 BTreeMap 内存无界且无溢写磁盘

**问题描述**：`GenericIndexManager` 使用纯内存 `BTreeMap<SecondaryIndexKey, IndexRecord>`，无内存上限，无磁盘溢写。百万级索引条目可导致 OOM。

- 代码位置：`generic_index_manager.rs` 第 30-31 行
- 严重程度：严重 —— 索引大小通常与数据大小成正比，内存优先架构下索引成为内存瓶颈

**判定：考虑不全面。** 数据可通过 flush 释放内存，但索引始终驻留在内存中，这破坏了"内存优先架构可通过 flush/checkpoint 控制内存使用"的前提。

**改进建议**：
- 引入基于磁盘的 B+Tree 或 LSM 索引作为可选项（如使用 `sled` 或自定义 page-based B+Tree）
- 短期改进：为索引设置内存预算（`max_index_memory`），超出后触发索引 flush + 部分驱逐
- 对等值查询场景，建议增加哈希索引选项（O(1) 且内存开销更低）

### 3.2 GC 全量扫描阻塞所有操作

**问题描述**：`gc_tombstones` 需对 forward 和 reverse 两个 BTreeMap 做全量 O(n) 扫描，期间持有写锁，所有读写被阻塞。在百万条目下，GC 可能耗时数秒。

- 代码位置：`generic_index_manager.rs` 第 73-113 行
- 严重程度：严重

**判定：考虑不全面。** 墓碑 GC 应该在后台增量执行，而非作为阻塞式的全量操作。

**改进建议**：
- 将 GC 改为始终增量模式（`gc_tombstones_incremental`），并在后台定期调度（如每 30s 执行一个 batch）
- 修复增量 GC 的已知 bug：当 forward batch 填满时跳过 reverse_index 的问题
- 引入"epoch-based GC"：维护一个 `gc_progress` 游标，每次只扫描有限数量

### 3.3 Flush 期间持有读锁阻塞写操作

**问题描述**：`flush_data()` 在持有 BTreeMap 读锁期间完成整个序列化。对于大索引，这意味着写操作被长时间阻塞。

- 代码位置：`generic_index_manager.rs` 第 193-196 行
- 严重程度：严重

**判定：考虑不全面。** Flush 应使用快照拷贝或在释放锁后执行序列化。

**改进建议**：
- 克隆 BTreeMap 的 Arc 引用，释放锁后再序列化（利用 BTreeMap 的 `Clone` 特性）
- 或使用"双缓冲"模式：flush 时交换 active/inactive 索引，在新 inactive 索引上进行序列化

### 3.4 不支持前缀/部分键查找

**问题描述**：`forward_index` 和 `reverse_index` 仅支持精确匹配（`BTreeMap::get()`），没有范围扫描、前缀扫描或 `lower_bound`/`upper_bound` API。

- 代码位置：`generic_index_manager.rs` 第 30-74 行
- 严重程度：中 —— 限制了复合索引和 LIKE 查询的能力

**判定：考虑不全面。** BTreeMap 天生支持范围查询（`range()` 方法），只需暴露适当的 API。

**改进建议**：
- 增加 `scan_range(start_key, end_key)` API
- 增加 `scan_prefix(prefix)` API（利用 BTreeMap 的有序性）
- 对于等值查询的索引，建议仍然使用哈希索引变体（性能更好）

### 3.5 架构选型下的合理取舍

| 取舍项 | 理由 |
|--------|------|
| BTreeMap 而非 ART/Radix Tree | BTreeMap 实现简单、经过充分测试、Rust 标准库支持；ART 在字符串键上有优势但实现复杂度高 |
| 正向+反向双索引 | 支持双向查找是必要的（从键查记录 + 从记录查键），写放大在可接受范围 |
| Manifest 使用线性搜索 | 当前分片数量少（通常 < 10），二分搜索收益不大；可随分片数量增长演进 |

---

## 四、边存储 (CSR)

### 4.1 超级节点的溢出块无界翻倍

**问题描述**：`expand_vertex_capacity()` 每次将容量翻倍（`old_cap * 2`），默认初始容量为 4。一个拥有 100K 条边的超级节点在 15 次扩展后消耗 4M 个槽位，且旧块标记为"僵尸"等待 compact 回收。极端情况下是最常见图模式（幂律分布）。

- 代码位置：`mutable_csr.rs` 第 237-261 行
- 严重程度：严重 —— 社交网络、知识图谱等实际场景中超级节点普遍存在

**判定：考虑不全面。** 倍增策略适用于均匀分布的顶点度数，但实际图的度数分布服从幂律。

**改进建议**：
- 为超级节点采用不同的存储策略：超过阈值（如 10000 条边）的顶点使用 `BTreeMap<rank, Nbr>` 而非连续数组
- 或设置单顶点的最大内联容量（如 65536），超出部分自动使用溢出页
- 引入 `max_overflow_capacity_per_vertex` 配置项

### 4.2 VertexId 编码溢出时静默截断

**问题描述**：VertexId 编码使用 `(endpoint, rank)` 的 16 字节组合。当 endpoint 超出 `i64` 范围时，`as_int64().unwrap_or(0) as u32` 静默截断为 0，导致多条边的端点被映射到相同的错误标识符。

- 代码位置：`edge_table/core.rs` 第 98 行（`undo.rs` 中也有类似模式）
- 严重程度：严重 —— 数据损坏

**判定：考虑不全面。** 这是一个防御性编程的缺失。

**改进建议**：
- 改为显式错误返回：`as_int64().ok_or(EncodingError::VertexIdOverflow)?`
- 或使用 128 位 VertexId 以容纳更大范围
- 添加 `debug_assert!` 在开发构建中捕获此类溢出

### 4.3 写入可触发同步 Freeze 链

**问题描述**：`check_and_apply_write_backpressure()` 在每次 `insert_edge()` 后调用。若可变 CSR 内存超过 `max_mutable_csr_bytes`，会触发 freeze → merge → compact → rebuild sparse index 的完整链路。所有这些在单次 `&mut self` 借用中完成，意味着一条边的插入可能阻塞数百毫秒甚至数秒。

- 代码位置：`edge_table/core.rs` 第 1300-1325 行
- 严重程度：高 —— 用户在低延迟写入场景下会观察到不可预测的延迟峰值

**判定：架构选型下的合理初版实现，但需要异步化。** Freeze 触发写入背压是必要的（防止 OOM），但同步执行会破坏延迟可预测性。

**改进建议**：
- 将 freeze 触发改为异步：写入操作标记"需要 freeze"，后台线程执行实际的 freeze
- 在 freeze 期间，新写入进入新的 delta CSR（双缓冲）
- 引入"soft backpressure"（内存使用 > 80% 时触发异步 freeze）和"hard backpressure"（> 95% 时阻塞写入）

### 4.4 Compact 操作阻塞所有并发读取

**问题描述**：`compact_with_ts()` 获取 `&mut self`，在 O(V+E) 的紧凑操作期间阻塞所有读取器。

- 代码位置：`mutable_csr.rs` 第 735 行
- 严重程度：中

**判定：Rust 所有权模型的固有约束，但可通过架构优化。** 对于读密集型负载，这是一个显著的可用性问题。

**改进建议**：
- 使用双缓冲模式：compact 写入新的 CSR 结构，完成后原子交换指针
- 在 compact 过程中不阻塞读取（读取仍访问旧 CSR）
- 这需要将 CSR 包装在 `ArcSwap` 或类似结构中

### 4.5 无自动碎片触发 Compact

**问题描述**：`fragmentation_ratio()` 返回碎片比例，但代码中无自动触发 compact 的逻辑。调用者需自行监控并手动触发。

- 代码位置：`mutable_csr.rs` 第 830-832 行
- 严重程度：中

**判定：考虑不全面。** 碎片管理应是存储引擎的内置功能，不应依赖外部协调。

**改进建议**：
- 在 flush 前检测碎片率，超过阈值（如 2.0）自动触发 compact
- 增加配置项 `auto_compact_fragmentation_threshold`
- 在 metrics 中暴露碎片率

### 4.6 架构选型下的合理取舍

| 取舍项 | 理由 |
|--------|------|
| 6 种可变 CSR 类型 | 图数据库的边模式多样（一对一、一对多、带标签），多态 CSR 为每种模式提供最优存储。虽增加代码复杂度，但在性能关键路径上值得 |
| 冻结机制（可变→不可变） | 不可变 CSR 支持 O(1) 无锁读取，冻结成本可接受。与 Ladybug 的 Shadow Paging 相比，LinkRS 的方式更简单直接 |
| 属性表采用行存 | 边属性通常较小（几条到几十条），行存避免了列存的随机访问开销 |
| 段合并的 LSM 模式 | 借鉴 LSM-tree 的分层思想，在不可变段之间做归并，平衡写放大与读放大 |

---

## 五、顶点存储

### 5.1 无 Schema 类型迁移能力

**问题描述**：`VertexTable` 支持 add/remove/rename 属性，但不支持类型迁移（如 Int→String、Float→Double）。同时 `add_property()` 不提供默认值或数据回填机制，新增列对存量数据填充 null。

- 代码位置：`vertex_table/schema.rs`
- 严重程度：高 —— 生产环境中 schema 变更不可避免

**判定：考虑不全面。** 这是图数据库成熟度的关键指标。

**改进建议**：
- 增加 `ALTER PROPERTY TYPE` 操作，通过写入时转换 + 后台回填实现
- 为 `add_property()` 增加 `DEFAULT <value>` 语义
- Schema 变更应支持在线执行（不阻塞读写），可通过"写入双写新旧列、读取优先新列"的方式实现

### 5.2 VertexIterator 在有删除间隙时低效

**问题描述**：迭代器使用 `total_count()` 作为结束边界，遍历所有 ID 0..count，对每个 ID 调用 `get_by_internal_id()` 检查是否存在。若存在大量被删除的顶点间隙，效率极低。

- 代码位置：`vertex_table/core.rs` 第 739-752 行
- 严重程度：中

**判定：考虑不全面。** 需要一个"有效顶点位图"或链表来跳过已删除的 ID。

**改进建议**：
- 维护一个 `roaring_bitmap` 标记有效的顶点 ID
- 或使用 freelist 在删除时回收 ID（但需处理好外部引用的悬空问题）
- 迭代时通过位图跳过空洞

### 5.3 快照注册无数量限制

**问题描述**：`register_snapshot()` 无最大快照数限制。客户端可创建任意数量快照而不释放，导致 `active_snapshots` HashMap 无限增长。

- 代码位置：`vertex_table/core.rs` 第 632-644 行
- 严重程度：中

**判定：考虑不全面。** 这是一个典型的资源泄漏向量。

**改进建议**：
- 增加 `max_active_snapshots` 配置（如 1000），超出后拒绝新快照
- 增加快照创建时间的追踪，暴露最老快照信息供运维排查
- 对连接断开时自动释放关联快照

### 5.4 架构选型下的合理取舍

| 取舍项 | 理由 |
|--------|------|
| 列式存储 (ColumnStore) | 顶点属性通常按列批量访问（如查询所有用户的年龄），列存天然支持向量化操作和列级压缩 |
| IdIndexer 外部ID→内部ID映射 | 解耦外部标识符与内部存储布局，支持高效的 ID 重分配和压缩 |
| 无行级 TTL | 图数据库的顶点生命周期通常与应用逻辑绑定，TTL 会增加 GC 复杂度 |

---

## 六、配置与运维

### 6.1 flush_threshold 默认值不当

**问题描述**：`flush_threshold: 1000` 意味着每 1000 次操作触发一次 flush。对于 1M ops/s 的写入负载，每秒触发 1000 次 flush，造成严重的 I/O 抖动。

- 代码位置：`config.rs` 第 21 行
- 严重程度：高

**判定：配置不当。** 1000 对于测试场景合理，但不适合生产。

**改进建议**：
- 将默认值调整为 50000 或引入基于时间间隔的 flush 策略（如"每 10000 次操作或每 30 秒"）
- 将 flush 从"计数触发"改为"内存压力触发"（如 dirty 数据超过 50MB）

### 6.2 Persistence 模式忽略用户配置

**问题描述**：`new_with_persistence()` 创建 `PersistenceCoordinator` 时使用硬编码的 `PropertyGraphConfig::default()`，忽略调用者传入的配置。

- 代码位置：`mod.rs` 第 177 行
- 严重程度：高 —— 生产环境中所有的持久化调优参数均不生效

**判定：Bug。** 这明显是遗漏。

**改进建议**：
- 修改构造函数签名以接受 `PropertyGraphConfig` 参数
- 或从外部配置文件读取

### 6.3 缺失的生产级配置项

以下配置项在 LinkRS 中完全缺失：

| 缺失配置 | 影响 |
|----------|------|
| 全局最大内存限制（数据 + 索引） | 索引可能撑爆内存 |
| 索引专用内存预算 | 索引和数据争抢内存 |
| WAL sync 间隔 / 缓冲区大小 | WAL 性能不可调 |
| GC 调度间隔 | 墓碑无限累积 |
| Operation timeout | freeze/merge 可能无限挂起 |
| 压缩算法可选（LZ4/Snappy） | CPU 开销不可控 |
| 最大快照数 / 最老快照年龄 | 快照泄漏无防护 |

**判定：考虑不全面。** 上述配置项在 Ladybug、Neo4j、JanusGraph 等同类系统中均为标准配置。

**改进建议**：
- 逐项增加对应配置项，参照 Ladybug 的配置体系
- 为每项提供合理的默认值
- 在文档中说明各配置项的适用场景和权衡

### 6.4 LSMSegmentLevel 硬编码

**问题描述**：段合并的分层大小和触发条件硬编码在 `config.rs` 第 261-301 行，对小图过于激进（过早合并），对大图过于保守（合并不够频繁）。

**判定：考虑不全面。** 应像 RocksDB 的 `target_file_size_base` / `target_file_size_multiplier` 那样可配置。

**改进建议**：
- 暴露 `level0_segment_size`、`size_multiplier`、`max_levels` 等配置
- 提供 `small` / `medium` / `large` 预设配置档

---

## 七、总结与优先级建议

### 7.1 问题严重程度总览

| 优先级 | 数量 | 典型问题 |
|--------|------|---------|
| P0 (立即修复) | 5 | undo 纯内存、WAL 静默丢数据、VertexId 截断、全局写锁、索引 OOM |
| P1 (近期修复) | 7 | 长事务阻塞 GC、墓碑无界增长、缓存陈旧数据、flush_threshold 默认值、配置不生效 bug、超级节点翻倍、Schema 无迁移 |
| P2 (规划改进) | 6 | compact 阻塞读、索引 GC 全量扫描、无前缀查找、VertexIterator 低效、LSM 硬编码、缺失配置项 |

### 7.2 架构选型合理性评估

LinkRS 存储模块的**核心架构决策基本合理**：

- 内存优先 + CSR 原生存储是图数据库的正确方向（Neo4j 也走类似路线）
- 6 种 CSR 变体的精细化设计超出了同类系统（Ladybug 仅 1 种 CSR 实现）
- 编码选择器（CompressionSelector）基于列统计的自动选择是领先的设计
- 分层持久化（WAL→Flush→Checkpoint→Snapshot）的责任链清晰

**主要不足集中在"工程完备性"层面**，而非架构缺陷：

- 错误处理路径不完整（静默丢弃、无重试）
- 资源边界不明确（无界增长、无背压）
- 配置体系不成熟（默认值不当、关键配置缺失）
- 恢复路径考虑不周全（部分恢复 path 是空操作）

### 7.3 与 Ladybug 的差距

| 维度 | LinkRS 不足 | Ladybug 做法 |
|------|-----------|-------------|
| 崩溃恢复 | undo 不持久化、WAL 静默丢数据 | WAL 完整恢复 + ShadowFile replay |
| 事务安全 | 无事务超时/强制终止 | 组提交 + poison 机制 |
| 内存安全 | 索引/墓碑无界增长 | BufferManager 严格控制页面内存 |
| Schema 变更 | 不支持类型迁移 | 完整的 ALTER TABLE 语义 |
| 运维可观测 | 配置项不足、metrics 有限 | 丰富的统计信息和可配置性 |

这些差距不意味着 LinkRS 的架构选型有误；相反，LinkRS 的"内存优先 CSR"设计在正确的使用场景（中小规模图、热数据集可完全放入内存）下性能优于 Ladybug。差距主要体现在生产环境的鲁棒性和运维成熟度上，而这些是可以通过工程迭代弥补的。
