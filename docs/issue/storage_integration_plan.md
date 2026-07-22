# GraphDB Storage 功能集成方案

## 概述

基于 clippy 分析和代码探索，本文档详细说明如何将未使用的功能正确集成到主流程中。分析发现 60 个未使用项，其中大部分是**功能不完整**而非**冗余设计**。

## 实施状态

| 阶段 | 状态 | 说明 |
|------|------|------|
| 1.1 MVCC 快照集成 | ✅ **已完成** | 在 `with_auto_commit_context` 中注册快照，在 `finalize_operation` 中注销 |
| 1.2 WAL 事务日志 | ✅ **已集成** | 通过 `commit_staged_writes_with_durability` → `append_transaction_with_durability` |
| 1.3 后台 GC | ✅ **已集成** | 通过 `compact_maintenance` → `compact_with_ts_collect`（与 `gc` 相同实现） |
| 2.1 批量顶点操作 | ⏸ **暂缓** | 方法已实现但仅在测试中使用；需业务需求驱动 |
| 2.2 索引 GC | ⏸ **暂缓** | `IndexGcManager` 已实现但未在 `BackgroundFreeze` 中循环调用 |
| 2.3 边缘物理删除 | ⏸ **暂缓** | 当前删除基于 MVCC 墓碑机制；物理删除需业务决策 |
| 3.1 编码反馈循环 | ⏸ **暂缓** | `should_reencode` 已实现但未被调用；压缩率优化 |
| 3.2 分段驱逐溢出 | ⏸ **暂缓** | `Spiller` 已实现但 `AccessClock` 未初始化；高级内存管理 |

---

## 1. 边缘删除功能集成

### 现状
- `delete_edge_by_offset` 在 `MutableCsr`、`EdgeStore`、`TimeTravelEdgeStore`、`SingleMutableCsr` 中定义但从未调用
- 当前删除通过 `MVCCManager` 的墓碑机制实现，而非物理删除

### 集成方案

#### 方案 A: 启用物理删除（推荐用于磁盘空间优化）

```rust
// 在 crates/graphdb-storage/src/storage/engine/transaction/ops.rs 中
// 修改 TransactionOps::delete_edge 方法

pub fn delete_edge(
    &self,
    params: DeleteEdgeParams,  // 目前未使用此结构体
) -> StorageResult<()> {
    // 当前实现：仅添加墓碑
    self.edge_table.mark_as_deleted(params.src, params.dst, params.ts)?;
    
    // 新增：如果启用物理删除且无活跃快照，执行物理删除
    if params.force_physical_delete && self.no_active_snapshots() {
        let offset = self.edge_table.find_edge_offset(params.src, params.dst)?;
        self.edge_table.delete_edge_by_offset(params.src, offset, params.ts)?;
    }
    
    Ok(())
}
```

**集成步骤**:
1. 在 `GraphStorageContext` 添加 `no_active_snapshots()` 检查方法
2. 在 `DeleteEdgeParams` 添加 `force_physical_delete: bool` 字段
3. 在事务提交时检查条件并调用 `delete_edge_by_offset`
4. 物理删除后更新 CSR 的 `live_edge_count` 元数据

#### 方案 B: 后台压缩清理（推荐用于性能敏感场景）

```rust
// 在 crates/graphdb-storage/src/storage/edge/edge_table/compaction.rs 中添加

pub fn compact_deleted_edges(&mut self, safe_ts: Timestamp) -> StorageResult<usize> {
    let mut deleted_count = 0;
    
    for segment in self.segments.iter_mut() {
        // 获取该段中所有已删除的边
        let deleted_edges = segment.get_tombstoned_edges_before(safe_ts);
        
        for (edge_id, _ts) in deleted_edges {
            let offset = segment.find_offset(edge_id)?;
            if segment.delete_edge_by_offset(edge_id.src(), offset, safe_ts)? {
                deleted_count += 1;
            }
        }
    }
    
    Ok(deleted_count)
}
```

**集成步骤**:
1. 在 `BackgroundFreeze` 中添加定期压缩任务
2. 调用 `compact_deleted_edges` 清理墓碑
3. 更新 `EdgeDeletionBloomFilter` 以反映新的删除状态

---

## 2. 分段驱逐与溢出集成

### 现状
- `CsrSegment::evict_to_spill`、`reload_from_spill` 已实现但未调用
- `AccessClock` 完全未使用
- `SegmentEvictionEngine` 的 `access_clock` 字段未初始化

### 集成方案

#### 步骤 1: 启用访问时钟

```rust
// 在 crates/graphdb-storage/src/storage/edge/edge_table/segment.rs 中

impl CsrSegment {
    pub fn new(...) -> Self {
        Self {
            // ... existing fields ...
            access_clock: Arc::new(AccessClock::new()),  // 初始化时钟
            last_access_ts: AtomicU64::new(0),
        }
    }
    
    // 在每次访问时记录时间戳
    pub fn get_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<Edge> {
        self.access_clock.tick();  // 更新时间戳
        self.record_access(self.access_clock.now());  // 记录访问时间
        // ... existing logic ...
    }
}
```

#### 步骤 2: 集成到内存管理器

```rust
// 在 crates/graphdb-storage/src/storage/engine/spiller.rs 中添加

impl Spiller {
    pub fn try_reserve_with_spill(
        &self,
        requested_bytes: u64,
        category: MemoryCategory,
    ) -> Option<u64> {
        // 如果内存充足，直接分配
        if self.available_memory() >= requested_bytes {
            return Some(0);  // 无溢出
        }
        
        // 内存不足，尝试驱逐冷数据
        let cold_segments = self.collect_cold_segments(category);
        let mut spilled_bytes = 0u64;
        
        for segment in cold_segments {
            if self.available_memory() + spilled_bytes >= requested_bytes {
                break;
            }
            
            let spill_path = self.spill_dir.join(format!("segment_{}.bin", segment.id()));
            match segment.evict_to_spill(&spill_path) {
                Ok(size) => {
                    spilled_bytes += size;
                    // 更新段状态为 Evicted
                    segment.mark_as_evicted(spill_path, size);
                }
                Err(_) => continue,
            }
        }
        
        if spilled_bytes >= requested_bytes {
            Some(spilled_bytes)
        } else {
            None  // 无法释放足够内存
        }
    }
    
    fn collect_cold_segments(&self, category: MemoryCategory) -> Vec<Arc<CsrSegment>> {
        let mut segments = self.segments_by_category(category);
        // 按访问时间排序，最冷的在前
        segments.sort_by_key(|s| s.last_access_ts());
        segments.into_iter().take(5).collect()  // 每次最多驱逐 5 个段
    }
}
```

#### 步骤 3: 在查询时自动重新加载

```rust
// 在 CsrSegment 中添加

pub fn try_optimistic_read<F, R>(&self, func: F) -> Option<R>
where
    F: FnOnce(&Self) -> R,
{
    // 如果段已被驱逐，尝试重新加载
    if self.is_evicted() {
        match self.reload_from_spill() {
            Ok(_) => Some(func(self)),
            Err(_) => None,  // 重新加载失败
        }
    } else {
        Some(func(self))
    }
}
```

**集成步骤**:
1. 在 `GraphStorageContext` 初始化时创建 `Spiller`
2. 在内存分配时调用 `try_reserve_with_spill`
3. 在段访问时调用 `try_optimistic_read` 自动处理驱逐/重新加载
4. 后台任务定期调用 `record_access` 更新时钟

---

## 3. 编码反馈循环集成

### 现状
- `EncodingSelector::should_reencode` 仅测试中使用
- `record_compression_result` 被调用但结果未用于决策
- `EncodingThresholds` 的 `reencode_threshold` 未实际使用

### 集成方案

#### 步骤 1: 在列刷新时检查重新编码

```rust
// 在 crates/graphdb-storage/src/storage/vertex/vertex_table/persistence.rs 中

pub fn flush_with_encoding<P: AsRef<Path>>(
    &self,
    path: P,
    encoding_selector: &mut EncodingSelector,
) -> StorageResult<()> {
    // ... existing encoding logic ...
    
    // 记录压缩结果（已有）
    encoding_selector.record_compression_result(
        column_name,
        encoding_type,
        original_size,
        compressed_size,
    );
    
    // 新增：检查是否需要重新编码
    if encoding_selector.should_reencode(encoding_type) {
        log::info!(
            "Column {} should be re-encoded from {:?} to {:?}",
            column_name,
            encoding_type,
            encoding_selector.recommend_encoding(column_stats)
        );
        
        // 标记该列需要重新编码
        self.mark_for_reencode(column_name, encoding_type);
    }
}
```

#### 步骤 2: 添加重新编码触发器

```rust
// 在 EncodingSelector 中添加

pub fn recommend_encoding(&self, stats: &ColumnStats) -> EncodingType {
    // 基于历史反馈推荐最佳编码
    let candidates = vec![
        EncodingType::None,
        EncodingType::Dictionary,
        EncodingType::RLE,
        EncodingType::ALP,
        EncodingType::FSST,
    ];
    
    candidates
        .into_iter()
        .max_by(|a, b| {
            let ratio_a = self.average_ratio(*a).unwrap_or(1.0);
            let ratio_b = self.average_ratio(*b).unwrap_or(1.0);
            ratio_a.partial_cmp(&ratio_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(EncodingType::None)
}

pub fn should_rebuild_fsst(&self) -> bool {
    // 当 FSST 符号表需要更新时返回 true
    self.fsst_symbols_inserted >= self.fsst_max_symbols as usize
}
```

#### 步骤 3: 在查询规划中使用统计信息

```rust
// 在 query planner 中集成列统计

pub fn estimate_selectivity(
    &self,
    column_name: &str,
    predicate: &Predicate,
) -> f64 {
    let stats = self.column_stats(column_name).expect("column exists");
    
    match predicate {
        Predicate::Eq(value) => {
            // 使用基数估计
            1.0 / stats.cardinality as f64
        }
        Predicate::Range(min, max) => {
            // 使用值范围估计
            let range = max - min;
            let total_range = stats.max_value - stats.min_value;
            range as f64 / total_range as f64
        }
        // ... other predicates ...
    }
}
```

**集成步骤**:
1. 在 `ColumnStore` 中暴露 `column_stats()` 方法
2. 在 query planner 中使用统计信息进行选择率估计
3. 在后台任务中定期检查 `should_reencode` 并触发重新编码
4. 重新编码时更新 `EncodingFeedback` 历史记录

---

## 4. MVCC 快照系统集成

### 现状
- `SnapshotHandle` 在测试中使用但生产代码未调用
- `VertexTable::gc` 仅测试中调用
- `TieredTombstoneManager` 在 `PropertyTable` 中使用但 `VertexTable` 未使用

### 集成方案

#### 步骤 1: 在事务开始时注册快照

```rust
// 在 crates/graphdb-storage/src/storage/engine/transaction/mod.rs 中添加

pub fn begin(&self) -> StorageResult<Transaction> {
    let ts = self.get_read_timestamp();
    
    // 注册快照以阻止 GC 清理该时间戳之前的数据
    let snapshot_handle = self.vertex_table.register_snapshot(ts)?;
    
    Ok(Transaction {
        read_ts: ts,
        snapshot_handle: Some(snapshot_handle),
        // ... other fields ...
    })
}

pub fn commit(&mut self) -> StorageResult<CommitResult> {
    // ... existing commit logic ...
    
    // 提交后释放快照
    if let Some(handle) = self.snapshot_handle.take() {
        self.vertex_table.unregister_snapshot(handle)?;
    }
    
    Ok(commit_result)
}

pub fn rollback(&mut self) -> StorageResult<()> {
    // ... existing rollback logic ...
    
    // 回滚后也释放快照
    if let Some(handle) = self.snapshot_handle.take() {
        self.vertex_table.unregister_snapshot(handle)?;
    }
    
    Ok(())
}
```

#### 步骤 2: 在后台运行 GC

```rust
// 在 crates/graphdb-storage/src/storage/engine/background_freeze.rs 中添加

pub fn run_vertex_gc(&self, safe_ts: Timestamp) -> StorageResult<usize> {
    let mut total_freed = 0;
    
    // 获取最小活跃快照时间戳
    let min_snapshot_ts = self.vertex_table.min_active_snapshot_ts();
    
    // 只 GC 早于最小快照时间戳的数据
    let gc_ts = std::cmp::min(safe_ts, min_snapshot_ts);
    
    total_freed += self.vertex_table.gc(gc_ts)?;
    
    // 同时清理属性表的墓碑
    total_freed += self.property_table.gc_tombstones(gc_ts)?;
    
    Ok(total_freed)
}
```

#### 步骤 3: 集成顶点批量操作

```rust
// 在 TransactionOps 中添加批量操作支持

pub fn batch_insert_vertices(
    &self,
    vertices: Vec<InsertVertexParams>,
) -> StorageResult<Vec<VertexId>> {
    let ts = self.get_timestamp();
    
    // 使用批量插入而非逐个插入
    let external_ids: Vec<&str> = vertices.iter().map(|v| v.external_id.as_str()).collect();
    let values: Vec<Vec<Value>> = vertices.iter().map(|v| v.properties.clone()).collect();
    
    self.vertex_table.batch_insert(&external_ids, &values, ts)?;
    
    Ok(vertices.into_iter().map(|v| v.vertex_id).collect())
}

pub fn batch_delete_vertices(
    &self,
    vertex_ids: Vec<VertexId>,
) -> StorageResult<usize> {
    let ts = self.get_timestamp();
    
    // 批量删除顶点及其关联的边
    let mut deleted_count = 0;
    
    for vertex_id in &vertex_ids {
        // 先删除所有出边和入边
        self.delete_edges_for_vertex(*vertex_id, ts)?;
        deleted_count += 1;
    }
    
    // 批量删除顶点
    let external_ids: Vec<String> = vertex_ids.iter()
        .filter_map(|vid| self.vertex_table.get_external_id(*vid))
        .collect();
    
    self.vertex_table.batch_delete(&external_ids.iter().map(|s| s.as_str()).collect(), ts)?;
    
    Ok(deleted_count)
}
```

**集成步骤**:
1. 在事务生命周期中集成快照注册/注销
2. 在 `BackgroundFreeze` 中添加定期 GC 任务
3. 启用顶点批量操作方法
4. 确保 GC 不会清理活跃快照需要的数据

---

## 5. 索引系统集成

### 现状
- `VertexIndexManager` 和 `EdgeIndexManager` 的多数方法未使用
- 索引游标通过其他路径打开
- 索引墓碑 GC 未启用

### 集成方案

#### 步骤 1: 在 schema 变更时更新索引

```rust
// 在 crates/graphdb-storage/src/storage/engine/graph_storage/context/mod_schema.rs 中添加

pub fn create_index(&self, index_def: IndexDefinition) -> StorageResult<()> {
    // ... existing index creation logic ...
    
    // 根据索引类型调用对应的管理器
    match index_def.target {
        IndexTarget::Vertex(label_id) => {
            self.vertex_index_manager.update_vertex_indexes(
                label_id,
                &index_def,
                self.get_timestamp(),
            )?;
        }
        IndexTarget::Edge(label_id) => {
            self.edge_index_manager.update_edge_indexes(
                label_id,
                &index_def,
                self.get_timestamp(),
            )?;
        }
    }
    
    Ok(())
}

pub fn drop_index(&self, index_name: &str) -> StorageResult<()> {
    // ... existing index drop logic ...
    
    // 清理索引管理器中的元数据
    self.vertex_index_manager.delete_vertex_indexes(
        index_name,
        self.get_timestamp(),
    )?;
    
    self.edge_index_manager.delete_edge_indexes(
        index_name,
        self.get_timestamp(),
    )?;
    
    Ok(())
}
```

#### 步骤 2: 在查询时打开索引游标

```rust
// 在 GraphStorageContext 中添加索引查询方法

pub fn query_by_index(
    &self,
    index_name: &str,
    key: &IndexKey,
    read_ts: Timestamp,
) -> StorageResult<Vec<EntityRef>> {
    // 根据索引类型选择管理器
    let index_manager = self.get_index_manager_for(index_name)?;
    
    // 打开游标进行查询
    let cursor = index_manager.open_index_cursor(index_name, read_ts)?;
    
    // 使用游标迭代匹配的实体
    let mut results = Vec::new();
    while let Some(entity_ref) = cursor.next()? {
        if self.is_visible(entity_ref, read_ts)? {
            results.push(entity_ref);
        }
    }
    
    Ok(results)
}
```

#### 步骤 3: 后台 GC 索引墓碑

```rust
// 在 BackgroundFreeze 中添加索引 GC

pub fn run_index_gc(&self, safe_ts: Timestamp) -> StorageResult<usize> {
    let mut total_freed = 0;
    
    // GC 顶点索引墓碑
    total_freed += self.vertex_index_manager.gc_tombstones(safe_ts)?;
    
    // GC 边缘索引墓碑
    total_freed += self.edge_index_manager.gc_tombstones(safe_ts)?;
    
    // 如果墓碑数量超过阈值，触发增量 GC
    if self.vertex_index_manager.tombstone_count() > self.gc_threshold {
        total_freed += self.vertex_index_manager.gc_tombstones_incremental(
            safe_ts,
            self.gc_batch_size,
        )?;
    }
    
    Ok(total_freed)
}
```

**集成步骤**:
1. 在 schema 变更时调用索引管理器的更新方法
2. 在查询引擎中集成索引游标
3. 在后台任务中运行索引 GC
4. 监控墓碑数量并触发增量 GC

---

## 6. WAL 与持久化集成

### 现状
- `WalManager::append_transaction` 未使用
- `PersistenceCoordinator::inject_failure` 等调试功能未使用
- 版本检查通过内联方式实现

### 集成方案

#### 步骤 1: 在事务提交时写入 WAL

```rust
// 在 Transaction::commit 中添加

pub fn commit(&mut self) -> StorageResult<CommitResult> {
    // ... existing pre-commit checks ...
    
    // 在提交前写入 WAL
    let write_set = self.collect_write_set();
    let wal_entry = WalEntry {
        txn_id: self.txn_id,
        timestamp: self.commit_ts,
        operations: write_set.serialize()?,
        checksum: self.calculate_checksum(),
    };
    
    self.wal_manager.append_transaction(wal_entry)?;
    
    // 确保持久化后提交
    self.wal_manager.sync()?;
    
    // ... existing commit logic ...
    
    Ok(commit_result)
}
```

#### 步骤 2: 启用故障注入测试

```rust
// 在测试中使用 inject_failure

#[test]
fn test_persistence_recovery() {
    let coordinator = PersistenceCoordinator::new(...);
    
    // 注入故障点在 manifest 写入时
    coordinator.inject_failure(PersistenceFaultPoint {
        phase: FaultPhase::ManifestWrite,
        fail_count: 1,
    });
    
    // 执行 checkpoint，应该触发故障
    let result = coordinator.create_checkpoint(...);
    assert!(result.is_err());
    
    // 验证恢复逻辑
    coordinator.recover_from_checkpoint(...)?;
}
```

#### 步骤 3: 统一版本检查

```rust
// 在 persistence.rs 中添加统一的版本检查入口

pub fn check_version(version: u32, expected: u32) -> StorageResult<()> {
    if version != expected {
        return Err(StorageError::version_mismatch(version, expected));
    }
    Ok(())
}

// 在各 deserializer 中使用
impl ColumnFileHeader {
    pub fn deserialize<R: Read>(reader: &mut R) -> StorageResult<Self> {
        let (version, _section_id) = read_header(reader)?;
        check_version(version, COLUMN_FILE_VERSION)?;  // 统一检查
        
        // ... existing deserialization logic ...
    }
}
```

---

## 7. 溢出管理器 (Spiller) 完整集成

### 现状
- `Spiller` 的多数方法未使用
- `SpillFile` 的 `category` 和 `spilled_bytes` 字段未读取
- 内存预算与溢出未联动

### 集成方案

#### 步骤 1: 在内存预算中集成溢出

```rust
// 在 ResourceBudget 中添加溢出感知

pub fn try_allocate(&self, bytes: u64, category: MemoryCategory) -> AllocationResult {
    // 首先尝试正常分配
    match self.try_allocate_without_spill(bytes, category) {
        Ok(result) => return Ok(result),
        Err(AllocationError::OutOfMemory) => {
            // 内存不足，尝试溢出
            if let Some(spilled) = self.spiller.try_reserve_with_spill(bytes, category) {
                self.metrics.spilled_bytes += spilled;
                return Ok(AllocationResult::WithSpill(spilled));
            }
        }
        Err(e) => return Err(e),
    }
    
    Err(AllocationError::OutOfMemory)
}
```

#### 步骤 2: 监控溢出状态

```rust
// 在 GraphStorageContext 中添加溢出监控

pub fn memory_stats(&self) -> MemoryStats {
    MemoryStats {
        total_bytes: self.budget.total(),
        used_bytes: self.budget.used(),
        spilled_bytes: self.spiller.active_spills()
            .iter()
            .map(|f| f.spilled_bytes)
            .sum(),
        spill_files: self.spiller.active_spills().len(),
        cold_segments: self.edge_table.cold_segment_count(),
    }
}
```

#### 步骤 3: 在查询时处理溢出段

```rust
// 在边查询中集成溢出处理

pub fn get_edges(&self, src_vid: u32) -> StorageResult<Vec<Edge>> {
    let segment = self.get_segment_for(src_vid)?;
    
    // 使用 try_optimistic_read 自动处理溢出
    match segment.try_optimistic_read(|s| s.get_edges(src_vid)) {
        Some(result) => result,
        None => {
            // 溢出重新加载失败，尝试从磁盘直接读取
            let spill_path = segment.spill_path().expect("segment should be evicted");
            let edges = Self::load_from_spill(&spill_path, src_vid)?;
            Ok(edges)
        }
    }
}
```

---

## 优先级建议

| 优先级 | 功能 | 复杂度 | 收益 |
|--------|------|--------|------|
| P0 | MVCC 快照集成 | 中 | 高 - 防止数据丢失 |
| P0 | WAL 事务日志 | 中 | 高 - 崩溃恢复 |
| P1 | 批量顶点操作 | 低 | 高 - 性能提升 |
| P1 | 索引 GC | 中 | 中 - 磁盘空间 |
| P2 | 编码反馈循环 | 中 | 中 - 压缩率优化 |
| P2 | 边缘物理删除 | 低 | 中 - 磁盘空间 |
| P3 | 分段驱逐溢出 | 高 | 中 - 内存管理 |
| P3 | 索引查询游标 | 中 | 低 - 查询优化 |

---

## 实施路线图

### 第一阶段（P0 - 数据安全性）
1. 集成 MVCC 快照到事务生命周期
2. 启用 WAL 事务日志
3. 添加后台 GC 任务

### 第二阶段（P1 - 核心功能）
1. 启用顶点批量操作
2. 集成索引更新和 GC
3. 启用边缘物理删除

### 第三阶段（P2-P3 - 优化功能）
1. 集成编码反馈循环
2. 实现分段驱逐和溢出
3. 完善索引查询游标

---

## 测试策略

每个功能集成后需要：
1. 单元测试验证功能正确性
2. 集成测试验证与其他组件的交互
3. 压力测试验证性能和稳定性
4. 故障注入测试验证恢复逻辑