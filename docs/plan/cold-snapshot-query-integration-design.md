# ColdSnapshot 查询引擎集成方案

## 1. 现状分析

### ColdSnapshot 当前状态
- `crates/graphdb-storage/src/storage/cold/cold_snapshot.rs` — 独立数据结构，372 行
- 支持从 mmap 文件加载、二进制序列化、CSR 查询
- 提供 `get_out_edges(u32)`, `get_in_edges(u32)`, `get_edge(u32, VertexId)`, `degree(u32)` 方法
- 文件格式：Magic `LKCS` + version + metadata + 4 sections (out CSR, in CSR, property table, schema) + CRC32

### StorageReader trait 定义
`crates/graphdb-storage/src/storage/client.rs:22-236` — 约 30 个方法，分为：
- 顶点查询（5 个）
- 边查询（5 个核心：`get_edge`, `get_node_edges`, `scan_edges_by_type`, `scan_all_edges`, `count_edges_by_type` in/out/both）
- 带模式查询（4 个）
- 索引查找（1 个）
- 模式/空间元数据（10+ 个）
- 游标扫描（3 个，有默认实现）

### 已实现 StorageReader 的 GraphStorage
`crates/graphdb-storage/src/storage/engine/graph_storage.rs:349-724` — 委托至 `reader::` 模块方法。

GraphStorage 通过 `GraphStorageContext` 操作，其 `GraphDataStore` 以 `HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>` 管理边表。
`EdgeTableKey = (src_label, dst_label, edge_label)`。

### 关键差异
| 维度 | ColdSnapshot | StorageReader 边方法 |
|------|-------------|---------------------|
| 顶点标识 | 内部 CSR u32 index | `VertexId` (i64/string) |
| 边标签 | 单 label (`LabelId`) | 字符串 `edge_type` → 查 schema 得 `edge_type_id` |
| 出边查询 | `get_out_edges(src: u32)` | `get_node_edges(space, node_id, direction: Out)` |
| 单边查询 | `get_edge(src: u32, dst: VertexId)` | `get_edge(space, src, dst, edge_type, rank)` |
| 度查询 | `degree(src: u32)` | `count_edges_by_type(space, edge_type)` 聚合 |
| 外部 ID | 无 | 需 `get_external_id(src_label, internal_id, ts)` 转换 |
| MVCC | 快照时间戳固定 | 基于读取时间戳过滤 |

## 2. 设计原则

1. **最小侵入** — 不修改 ColdSnapshot 的内部数据结构，不增加热路径开销
2. **只读约束** — ColdSnapshot 为只读，所有写入路由到 active EdgeStore
3. **标签匹配** — ColdSnapshot 绑定单个 edge label，在 GraphStorageContext 中以 `HashMap<LabelId, ColdSnapshot>` 管理
4. **外部 ID 映射** — ColdSnapshot 内部只有 u32 index，查询时需要从 `VertexId` 映射到内部 u32 index（通过查找 vertex table 的 `internal_id`）
5. **非冷存储不参与** — 元数据方法（schema, space, index）仍然由 schema_manager 处理

## 3. 架构变更

### 3.1 GraphStorageContext 添加 cold_snapshots 字段

```rust
// crates/graphdb-storage/src/storage/engine/graph_storage/context.rs

pub struct GraphStorageContext {
    // ...现有字段...
    cold_snapshots: Arc<RwLock<HashMap<LabelId, ColdSnapshot>>>,
}
```

### 3.2 加载机制

在 `GraphStorage` 或 `PersistenceCoordinator` 启动时扫描快照目录：

```rust
impl GraphStorage {
    pub fn load_cold_snapshots(&self, snapshot_dir: &Path) -> StorageResult<()> {
        let mut snapshots = HashMap::new();
        for entry in std::fs::read_dir(snapshot_dir)? {
            let path = entry?.path();
            if path.extension().map_or(false, |e| e == "lkcs") {
                let snapshot = ColdSnapshot::open(&path)?;
                snapshots.insert(snapshot.label(), snapshot);
            }
        }
        *self.ctx.cold_snapshots.write() = snapshots;
        Ok(())
    }
}
```

### 3.3 查询集成 — reader.rs 扩展

修改 `reader.rs` 中 4 个边查询方法，在查询 hot edge table 后合并 cold 结果。

#### 辅助函数

```rust
/// 将 VertexId 转为 CSR 内部 u32 index
fn vertex_id_to_internal(
    ctx: &GraphStorageContext,
    label_id: LabelId,
    vid: &VertexId,
    ts: Timestamp,
) -> Option<u32> {
    if let Some(i) = vid.as_int64() {
        // int64 ID: 通过 vertex table 查找 internal_id
        ctx.get_internal_id(label_id, i, ts)
    } else if let Some(s) = vid.as_str() {
        ctx.get_internal_id_str(label_id, s, ts)
    } else {
        None
    }
}
```

#### get_edge 集成

```rust
pub(crate) fn get_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<Option<Edge>> {
    // 1. 现有 hot 查询逻辑不变
    let edge_info = ctx.schema_manager().get_edge_type(space, edge_type)?;
    // ...解析 edge_label_id, src_label_id, dst_label_id...
    let ts = ctx.get_read_timestamp();
    // ...hot 查询...

    // 2. 如果 hot 未命中，查询 cold snapshots
    if result.is_none() {
        let cold = ctx.cold_snapshots.read();
        if let Some(snapshot) = cold.get(&edge_label_id) {
            let src_internal = vertex_id_to_internal(ctx, src_label_id, src, ts);
            let dst_internal = vertex_id_to_internal(ctx, dst_label_id, dst, ts);
            if let (Some(s), Some(d)) = (src_internal, dst_internal) {
                // ColdSnapshot.get_edge 不支持 rank，需要近似匹配
                // 方案：使用 dst 的 VertexId 而非 internal u32（因 CSR 存储的就是 VertexId）
                if let Some(nbr) = snapshot.get_edge(s, *dst) {
                    // 通过 nbr.prop_offset 读取属性
                    // 构建 Edge 返回
                }
            }
        }
    }

    Ok(result)
}
```

#### get_node_edges 集成

在现有循环中，每个 edge_type 查询完成后追加 cold 结果：

```rust
for edge_info in &edge_types {
    let edge_label_id = edge_info.edge_type_id;
    // ...现有 hot out_edges/in_edges 查询...

    // 追加 cold 边
    let cold = ctx.cold_snapshots.read();
    if let Some(snapshot) = cold.get(&edge_label_id) {
        let ts = ctx.get_read_timestamp();
        let internal_id = vertex_id_to_internal(ctx, src_label_id, node_id, ts);
        if let Some(internal) = internal_id {
            match direction {
                EdgeDirection::Out => {
                    for nbr in snapshot.get_out_edges(internal) {
                        let dst_external = resolve_external_id(
                            ctx, dst_label_id, &nbr.neighbor, ts
                        );
                        let edge = cold_nbr_to_edge(
                            &nbr, snapshot, edge_type_name, node_str, &dst_external
                        );
                        edges.push(edge);
                    }
                }
                EdgeDirection::In => {
                    for nbr in snapshot.get_in_edges(internal) {
                        let src_external = resolve_external_id(
                            ctx, src_label_id, &nbr.neighbor, ts
                        );
                        let edge = cold_nbr_to_edge(
                            &nbr, snapshot, edge_type_name, &src_external, node_str
                        );
                        edges.push(edge);
                    }
                }
                EdgeDirection::Both => {
                    // 合并 out + in
                }
            }
        }
    }
}
```

#### scan_edges_by_type 集成

在 `edge_tables` 扫描后追加：

```rust
let cold = ctx.cold_snapshots.read();
if let Some(snapshot) = cold.get(&edge_label_id) {
    // ColdSnapshot 不存完整 scan（无全表迭代器）
    // 方案1: 仅对部分支持（如指定 vertex_id 时的过滤）
    // 方案2: 实现 ColdSnapshot::scan_all() 通过 CSR 遍历所有 vertex
    // 当前推荐：scan_edges_by_type 不查询 cold，对于全表扫描由
    // scan_edges_with_schema 兜底
}
```

#### count_edges_by_type 集成

```rust
pub(crate) fn count_edges_by_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<u64> {
    let mut count = /* hot table count */;

    let cold = ctx.cold_snapshots.read();
    if let Some(snapshot) = cold.get(&edge_label_id) {
        count += snapshot.edge_count();
    }

    Ok(count)
}
```

### 3.4 ColdSnapshot 增强

添加以下方法以支持外部 ID 转换和 scan：

```rust
impl ColdSnapshot {
    /// 全量边迭代（通过 CSR 内部遍历所有 vertex）
    pub fn scan_edges(&self) -> Vec<ColdEdgeRecord>;

    /// 从 Nbr 构建 Edge（配合属性读取）
    pub fn nbr_to_edge_record(&self, nbr: &Nbr, src: u32, dst: VertexId) -> EdgeRecord;
}
```

### 3.5 PropertyTable 暴露

当前 `PropertyTable` 在 `edge_table.rs` 中实现，未提供按 `prop_offset` 查询的公共方法。
ColdSnapshot 需要从 `Nbr.prop_offset` 读取属性值 → `Edge.properties`。

```rust
impl PropertyTable {
    /// 根据 prop_offset 反向映射到属性键值对
    pub fn read_properties(&self, offset: u32) -> Vec<(String, Value)>;
}
```

### 3.6 GraphDataStore 集成

`GraphDataStore` 新增 cold snapshots 管理接口：

```rust
impl GraphDataStore {
    pub fn with_cold_snapshots<R>(
        &self,
        op: impl FnOnce(&HashMap<LabelId, ColdSnapshot>) -> R,
    ) -> R;

    pub fn with_cold_snapshots_mut<R>(
        &self,
        op: impl FnOnce(&mut HashMap<LabelId, ColdSnapshot>) -> R,
    ) -> R;
}
```

## 4. 边界情况处理

### 4.1 数据重复
- Hot EdgeStore 和 ColdSnapshot 可能包含相同边（cold 是导出时间点的快照）
- 去重策略：按 `(src, dst, edge_type, rank)` 去重，hot 优先。
- `get_edge`: hot 命中直接返回，不查 cold。
- `get_node_edges`: 使用 `HashSet<(VertexId, VertexId, String)>` 去重。
- `count_edges_by_type`: 返回 `hot_count + cold_count`（允许重复计数是一致性取舍）。

### 4.2 时间戳不一致
- ColdSnapshot 的 `snapshot_ts` 是固定时间戳
- MVCC 读取时间戳 `ts` 可能大于 `snapshot_ts`（正常）或小于 `snapshot_ts`（读历史）
- 当 `ts < snapshot_ts` 时，ColdSnapshot 不应被查询（快照尚未创建），跳过 cold

### 4.3 外部 ID 映射失败
- CSR 存储的 `ImmutableNbr.neighbor` 是 `VertexId` 而非 u32
- 因此 `ColdSnapshot::get_out_edges(src: u32)` 返回的 `Nbr.neighbor` 为 `VertexId`
- 可直接作为外部 ID，无需反向映射 → 简化设计

## 5. 生命周期管理

### 5.1 加载时机
- 数据库 `open()` 时从 `{db_path}/cold_snapshots/` 加载所有 `.lkcs` 文件
- 热加载：运行时通过 API 手动加载某个 `.lkcs` 文件

### 5.2 卸载
- Remove API：`GraphStorage::remove_cold_snapshot(label: LabelId)`
- 引用计数：暂不需要，使用 `Arc<RwLock<>>` + 按 label 管理

### 5.3 Flush/Checkpoint 跳过
- 实现 `StoragePersistenceOps` 时跳过 cold section
- ColdSnapshot 文件是只读的，不参与 WAL/checkpoint

## 6. 实现步骤

| 步骤 | 文件 | 描述 |
|------|------|------|
| 1 | `cold_snapshot.rs` | 新增 `scan_edges()`, `nbr_to_edge_record()` 方法 |
| 2 | `edge/property_table.rs` | 暴露 `read_properties(offset)` 方法 |
| 3 | `data_store.rs` | 新增 `cold_snapshots` 字段和管理方法 |
| 4 | `context.rs` | `GraphStorageContext` 暴露 cold snapshots 访问 |
| 5 | `reader.rs` | 4 个边查询方法集成 cold 查询 |
| 6 | `graph_storage.rs` | 新增 load/unload API, `StorageReader` 委托不变 |
| 7 | `cold_snapshot.rs` | 单元测试覆盖 `scan_edges` 和属性读取 |
| 8 | `tests/` | 集成测试覆盖 cold + hot 混合查询 |

## 7. 后续版本规划

### v2 — 查询覆盖率补齐

**目标**：覆盖全部 StorageReader 边查询方法，补齐缺漏项。

| 功能 | 变更 |
|------|------|
| `scan_edges_by_type_paginated` | ColdSnapshot 实现 `scan_edges()` 返回分页迭代器；reader 层先分页 hot，不足时从 cold 补 |
| `scan_edges_with_schema` | 基于 `scan_edges()` 结果附加 `EdgeTypeInfo` 和序列化属性 |
| `scan_all_edges` | 遍历所有 edge label，对每个 label 的 cold snapshot 调用 `scan_edges()` |
| `get_edge_with_schema` | 类似 `get_edge` 集成路径，命中后附加 schema 信息 |

**实现要点**：

```rust
impl ColdSnapshot {
    /// 全量扫描：遍历所有 vertex 的 CSR 出边，跳过空行
    pub fn scan_edges(&self) -> Vec<ColdEdgeRecord>;
    /// 分页版本
    pub fn scan_edges_paginated(&self, offset: usize, limit: usize) -> Vec<ColdEdgeRecord>;

    /// 行协议：封装出边 + 入边元组
    pub struct ColdEdgeRecord {
        pub src_internal: u32,
        pub dst_vid: VertexId,
        pub nbr: Nbr,
        pub properties: Option<Vec<(String, Value)>>,
    }
}
```

`scan_edges` 关键路径 — 遍历 `[0..vertex_capacity)`，对每个 index 调用 `edges_of(i)`：

```rust
pub fn scan_edges(&self) -> Vec<ColdEdgeRecord> {
    let cap = self.vertex_capacity;
    let mut results = Vec::with_capacity(self.edge_count as usize);
    for src in 0..cap {
        for nbr in self.out_csr.edges_of(src) {
            results.push(ColdEdgeRecord {
                src_internal: src as u32,
                dst_vid: nbr.neighbor,
                nbr: Nbr::new(nbr.neighbor, nbr.edge_id, nbr.prop_offset, nbr.timestamp),
                properties: None, // lazy load
            });
        }
    }
    results
}
```

---

### v3 — 索引查询支持

**目标**：冷快照可参与 `lookup_index` 查询，支持按属性值过滤。

**设计**：

ColdSnapshot 需要包含属性索引。当前 `PropertyTable` 是列式存储，无二级索引。

```rust
pub struct ColdSnapshot {
    // ...现有字段...
    property_index: Option<HashMap<String, HashMap<Value, Vec<u32>>>>,
    // 索引映射: prop_name -> value -> [prop_offset..]
}
```

**索引构建时机**：导出 snapshot 时一并构建：

```rust
impl EdgeStore {
    pub fn export_snapshot_file(&self, path: &Path) -> StorageResult<ColdSnapshot> {
        let exported = self.export_snapshot(ts)?;
        // 构建索引（若 EdgeStore 对应属性有索引）
        let index = if self.has_property_index() {
            Some(build_property_index(&exported.properties))
        } else {
            None
        };
        ColdSnapshot::create_with_index(&exported, index, path)
    }
}
```

**文件格式扩展**：v2 格式新增可选 section — index section。

```
[4]  Magic "LKCS"
[4]  Version (u32 LE, v2)
...
--- 现有 4 个 sections ---
[8]  Out CSR length + data
[8]  In CSR length + data
[8]  Property table length + data
[8]  Schema length + data (JSON)
--- 新增 section (v2) ---
[8]  Index length (u64 LE, 0 = 无索引)
[N]  Index data (bincode 序列化)
[4]  CRC32
```

**reader 集成**：

```rust
pub(crate) fn lookup_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    value: &Value,
) -> StorageResult<Vec<Value>> {
    let mut results = /* hot index lookup */;

    let edge_label_id = /* 从 index_name 反查 edge_label */;
    if let Some(snapshot) = ctx.cold_snapshots.read().get(&edge_label_id) {
        if let Some(index) = &snapshot.property_index() {
            if let Some(offsets) = index.get(index_name).and_then(|m| m.get(value)) {
                for offset in offsets {
                    let edge = /* 通过 offset 还原 Edge */;
                    results.push(Value::from(edge));
                }
            }
        }
    }

    Ok(results)
}
```

---

### v4 — 全表游标 (Cursor) 集成

**目标**：实现 `StorageReader::create_edge_cursor` 的冷感知版本，让查询引擎通过统一的 `EdgeCursor` trait 访问 cold 数据。

**设计**：

```rust
pub struct ColdEdgeCursor {
    snapshot: Arc<ColdSnapshot>,
    src_cursor: usize,          // 当前 vertex index
    edge_cursor: usize,         // 当前 vertex 的边索引
    batch: Vec<ColdEdgeRecord>, // 预取 batch
    schema: Option<EdgeTypeInfo>,
    options: ScanOptions,
}

impl EdgeCursor for ColdEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Vec<EdgeRecord> { ... }
    fn schema(&self) -> Option<EdgeTypeInfo> { ... }
    fn close(self) { ... }
}
```

**GraphStorageContext API**：

```rust
pub fn create_cold_edge_cursor(
    &self,
    edge_label: LabelId,
    options: &ScanOptions,
) -> Option<Box<dyn EdgeCursor>> {
    let cold = self.cold_snapshots.read();
    cold.get(&edge_label).map(|snapshot| {
        Box::new(ColdEdgeCursor::new(snapshot.clone(), options.clone()))
    })?;
}
```

**reader 集成**：

```rust
pub(crate) fn create_edge_cursor(
    ctx: &GraphStorageContext,
    space: &str,
    options: &ScanOptions,
) -> Result<Box<dyn EdgeCursor>, StorageError> {
    // 1. 先尝试 hot cursor（已有实现）
    // 2. 若无 hot cursor 或 hot 数据不完整，追加 cold cursor
    // 3. 返回 MultiSourceEdgeCursor 包装 hot + cold cursor
    let hot_cursor = /* ... */;
    let cold_cursors = /* 收集所有匹配 label 的 cold cursor */;
    Ok(Box::new(MultiSourceEdgeCursor::new(hot_cursor, cold_cursors)))
}
```

---

### v5 — 自动冷热分层

**目标**：基于策略自动将冷数据（访问频率低、写入时间久）导出为 ColdSnapshot，减少热数据内存占用。

**架构变更**：

```
┌─────────────────┐     触发条件（任一）
│  AutoFreezeManager │ ── ① EdgeTable 行数 > threshold
└────────┬────────┘    ② 指定时间无写入
         │             ③ 手动触发的 freeze API
         ▼
┌──────────────────────┐
│ 1. freeze: EdgeStore → ExportedEdgeSnapshot  │
│ 2. persist: ExportedEdgeSnapshot → .lkcs file │
│ 3. warm: 保留最近 N 条边的软副本             │
│ 4. evict: 从 hot EdgeStore 删除已冻结数据     │
└──────────────────────┘
```

**配置项** (`PropertyGraphConfig`)：

```rust
pub struct ColdTierConfig {
    pub enabled: bool,                       // 是否启用自动分层
    pub trigger_row_count: u64,              // 单 label 超过该行数触发
    pub trigger_idle_seconds: u64,           // 无写入超过该时间触发
    pub max_cold_snapshots_per_label: usize, // 每 label 保留的最新快照数
    pub preserve_recent_edges: u64,          // freeze 时保留最近 N 条边在 hot
    pub snapshot_dir: PathBuf,               // .lkcs 存储目录
}
```

**冻结策略**：

```rust
impl BackgroundFreezeManager {
    pub fn evaluate_cold_tier(&self, config: &ColdTierConfig) {
        let edge_tables = self.data_store.with_edge_tables(|tables| {
            tables.iter()
                .filter(|(_, store)| {
                    // 排除已有 cold snapshot 的 label
                    let guard = store.read();
                    guard.0.edge_count() > config.trigger_row_count
                        && self.idle_since(guard.0.label()) > config.trigger_idle_seconds
                })
                .map(|(key, store)| (*key, store.clone()))
                .collect::<Vec<_>>()
        });

        for (key, store) in edge_tables {
            let guard = store.read();
            let ts = self.version_manager().read_ts();
            // 保留最近 N 条边
            let snapshot = guard.export_snapshot_with_retention(ts, config.preserve_recent_edges);
            // 写入文件并加载为 ColdSnapshot
            let path = config.snapshot_dir.join(format!("{}.lkcs", snapshot.label));
            ColdSnapshot::create(&snapshot, &path)?;
            // 从 hot 中删除已冻结行
            guard.freeze_edges_before(ts, config.preserve_recent_edges)?;
            // 注册到 GraphStorageContext
            self.register_cold_snapshot(snapshot.label, path)?;
        }
    }
}
```

**注意事项**：
- freeze 操作的原子性：freeze 步骤在事务中完成，文件写入在事务外
- 恢复流程：`open()` 时 scan 目录重建 cold snapshots 映射
- 监控指标：`cold_edge_count`, `cold_snapshot_count`, `last_freeze_timestamp`

---

### v6 — 增量快照 (Delta / CDC)

**目标**：支持两个时间点之间的冷快照差异查询，用于增量备份和时间旅行 diff。

**增量格式**：

```text
[4]  Magic "LKCD" (Cold Delta)
[4]  Version
[8]  Base snapshot timestamp
[8]  Delta timestamp
[4]  Label ID
--- Delta sections ---
[8]  Added out CSR + data（新增边）
[8]  Added in CSR + data
[8]  Removed out CSR + data（已删除边的 u32 offset 列表）
[8]  Removed in CSR + data
[8]  Property delta + data（新增/修改属性）
[4]  CRC32
```

**查询语义**：`delta_between(ts1, ts2)` = 基快照 + 增量链累积

```rust
impl ColdSnapshot {
    pub fn apply_delta(&self, delta: &ColdDelta) -> StorageResult<Self> {
        // 合并 CSR 邻接表 + 属性表
    }
}
```

**使用场景**：
- 时间旅行：`get_node_edges_at(ts)` 将 `ts` 映射到最近的 `snapshot_ts + delta` 链
- 增量导出：避免每次全量导出

---

### v7 — 多维时间旅行

**目标**：保留多个时间点的快照，查询时自动选择最合适的版本。

**时间戳路由层**：

```rust
pub struct ColdSnapshotTimeMachine {
    /// 按 (label, timestamp) 排序的不可变快照列表
    shelves: HashMap<LabelId, BTreeMap<Timestamp, Arc<ColdSnapshot>>>,
}

impl ColdSnapshotTimeMachine {
    /// 选择最接近但不大于 ts 的快照
    fn snapshot_at(&self, label: LabelId, ts: Timestamp) -> Option<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)?
            .range(..=ts)
            .next_back()
            .map(|(_, v)| v.clone())
    }
}
```

**存储目录结构**：

```
{cold_snapshot_dir}/
  edges/
    {label_name}/
      1000.lkcs   # snapshot at ts=1000
      2000.lkcs   # snapshot at ts=2000
      3000.lkcs   # snapshot at ts=3000
```

**reader 集成**：在 `get_node_edges` / `scan_edges_by_type` 中，当 `ts < current_ts` 且 hot 中有该时间点的数据时，回退到对应 cold snapshot（跳过 hot 查询）。

---

### v8 — 压缩与编码优化

**目标**：减小 .lkcs 文件体积，提升 mmap 加载速度。

| 技术 | 说明 | 预期收益 |
|------|------|----------|
| 字典编码 | VertexId 去重，id → dict_id 映射 | CSR 邻接表减少 30-50% |
| 帧压缩 | Zstd 压缩属性表 section | 属性表减少 60-80% |
| 位图索引 | 空行标记：`RoaringBitmap` 标记哪些 vertex 有边 | scan 跳过空 vertex 加速 10x |
| 列式存储 | 属性按列独立编码，支持 projection pushdown | 只读所需列的边加快查询 |

```rust
pub struct ColdSnapshotV3 {
    // ...现有字段...
    /// 编码格式版本
    encoding: EncodingScheme,
    /// 可选：字典映射 [{internal_id: original_vertex_id}]
    dict: Option<HashMap<u32, VertexId>>,
    /// 可选：空行位图
    vertex_presence: Option<RoaringBitmap>,
    /// 可选：压缩属性区
    compressed_properties: Option<Vec<u8>>,
}
```

---

### v9 — API 与 CLI 管理

**目标**：提供完整的快照管理接口，支持运维操作。

**StorageSnapshotOps 扩展**：

```rust
pub trait StorageSnapshotOps: Send + Sync {
    // ...现有方法...

    // ── ColdSnapshot 管理 ──
    fn list_cold_snapshots(&self) -> Result<Vec<ColdSnapshotInfo>, StorageError>;
    fn load_cold_snapshot(&self, path: &Path) -> Result<ColdSnapshotInfo, StorageError>;
    fn remove_cold_snapshot(&self, label: LabelId) -> Result<(), StorageError>;
    fn export_cold_snapshot(&self, label: LabelId, path: &Path) -> Result<ColdSnapshotInfo, StorageError>;
    fn merge_cold_snapshots(&self, labels: &[LabelId]) -> Result<ColdSnapshotInfo, StorageError>;
}

pub struct ColdSnapshotInfo {
    pub label: LabelId,
    pub label_name: String,
    pub snapshot_ts: Timestamp,
    pub edge_count: u64,
    pub file_path: String,
    pub file_size: u64,
    pub checksum: u32,
}
```

**CLI 命令**（`graphdb-cli/src/commands/snapshot.rs`）：

```
graphdb snapshot list                                # 列出所有冷快照
graphdb snapshot create --label knows [--path ...]   # 手动创建快照
graphdb snapshot load --path snapshot.lkcs           # 加载外部快照
graphdb snapshot remove --label knows                # 卸载快照
graphdb snapshot merge --labels knows,likes          # 合并多个标签快照
graphdb snapshot info --label knows                  # 快照详情
graphdb snapshot diff --from 1000 --to 2000          # 时间点差异
```

---

### v10 — 跨节点分发

**目标**：在只读副本间分发 cold snapshot，实现冷数据共享。

**机制**：

```
主节点                                   只读副本
  │                                        │
  ├─ export_cold_snapshot(label)           │
  ├─ 生成 .lkcs 文件                        │
  ├─ push (gRPC stream) ─────────────────► │
  │                                        ├─ load_cold_snapshot
  │                                        ├─ 注册到本地 GraphStorageContext
  │                                        ├─ 查询引擎自动感知 cold 数据
```

**gRPC 扩展**（`proto/cold_snapshot.proto`）：

```protobuf
service ColdSnapshotService {
    rpc PushSnapshot(PushSnapshotRequest) returns (PushSnapshotResponse);
    rpc PullSnapshot(PullSnapshotRequest) returns (stream SnapshotChunk);
    rpc ListRemoteSnapshots(ListRequest) returns (ListResponse);
}
```

**安全保障**：
- 校验和验证（CRC32）
- 签名认证（Ed25519）
- 限速传输

---

### 版本路线图总览

| 版本 | 主题 | 依赖 | 优先级 |
|------|------|------|--------|
| v1 | 基础查询集成（本方案 1-6 章） | — | P0 |
| v2 | 查询覆盖率补齐 | v1 | P1 |
| v3 | 索引查询支持 | v2 | P1 |
| v4 | 全表游标集成 | v1 | P2 |
| v5 | 自动冷热分层 | v1, config | P2 |
| v6 | 增量快照 | v1, file format v2 | P3 |
| v7 | 多维时间旅行 | v6 | P3 |
| v8 | 压缩与编码优化 | v1 | P3 |
| v9 | API 与 CLI 管理 | v1 | P2 |
| v10 | 跨节点分发 | v9, grpc | P4 |
