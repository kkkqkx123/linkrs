# Time-Travel 运行时可选设计（修订 v2）

## 上一版方案的核心问题

上一份文档 [linkrs_time_travel_feature_flag.md](file:///workspace/linkrs_time_travel_feature_flag.md) 推荐了**编译期 feature flag**（Cargo `#[cfg]`），这在以下场景下存在严重缺陷：

### 问题 1：一个二进制文件无法同时支持两种模式

Linkrs 有 **server 模式**（[main.rs](file:///workspace/linkrs/src/main.rs)），以单一二进制文件部署运行。如果 Time-Travel 是编译期 feature：

```
场景：用户有两个 Space
  - Space "OLTP"  → 不需要 Time-Travel，追求极致性能
  - Space "Audit"  → 需要 Time-Travel，记录所有历史变更

编译期 feature flag 方案：
  - 编译时只能选一种：带 time-travel 或不带
  - 不可能用一个二进制同时服务两个 Space
  - 需要部署两套服务 → 运维灾难
```

**结论：对于 server 形态的产品，编译期 feature flag 是不可接受的。**

### 问题 2：内部散落 `#[cfg]` 分支

条件编译会让所有调用方代码都散落 `#[cfg(feature = "time-travel")]` 分支。用户期望的是"入口处一次分发，内部无需感知"。

---

## 正确参考：SQL Server 和 Oracle 的 Time-Travel

SQL Server 和 Oracle 都原生支持 Time-Travel，但都采用**运行时**机制，而非编译期开关：

### SQL Server：时态表（Temporal Table）

```
┌──────────────────────────────────────┐
│          CREATE TABLE 时决定          │
│  SYSTEM_VERSIONING = ON | OFF       │
│                                      │
│  ON  → 当前表 + 历史表（自动维护）     │
│  OFF → 普通表（无历史开销）            │
└──────────────────────────────────────┘
```

核心设计理念：
- **每张表独立决定**是否启用时态功能
- 当前数据存当前表，历史数据存历史表，物理隔离
- 查询当前数据只访问当前表，零额外开销
- 历史查询通过 `FOR SYSTEM_TIME` 语法访问历史表
- 一个 SQL Server 实例可同时包含时态表和普通表

### Oracle：闪回（Flashback）

```
┌──────────────────────────────────────┐
│       依赖 UNDO 表空间（回滚段）        │
│  闪回查询 → 读取 UNDO 中旧版本数据      │
│  保留时间受 UNDO_RETENTION 参数限制    │
└──────────────────────────────────────┘
```

核心设计理念：
- 不需要单独的历史表，利用 UNDO 日志
- 所有表自动具备闪回能力（受 UNDO 保留期限制）
- 查询当前数据零额外开销
- 历史查询从 UNDO 中重建

### 对 Linkrs 的启示

| 特性 | SQL Server | Oracle | 适合 Linkrs |
|------|-----------|--------|------------|
| 粒度 | 表级 | 全局（UNDO 限制） | **边类型级（Edge Type）** |
| 当前查询开销 | 零 | 零 | 必须零 |
| 历史存储 | 独立历史表 | UNDO 日志 | 独立 segment |
| 运行时决策 | 建表时决定 | 始终开启 | 建边类型时决定 |

**关键启示：SQL Server 的"当前表 + 历史表"模式最适合 Linkrs。** Linkrs 的 segment 天然就是"历史表"，Mutable CSR 是"当前表"。核心问题是：是否每个边类型都需要历史表？

---

## 为什么用枚举而非 trait object

### 当前存储方式

[data_store.rs](file:///workspace/linkrs/crates/graphdb-storage/src/storage/engine/data_store.rs#L38) 中，EdgeTable 是**值类型**存储在 HashMap 中：

```rust
edge_tables: RwLock<HashMap<EdgeTableKey, EdgeTable>>
```

### 两种多态方案对比

| 维度 | `Box<dyn EdgeStore>` | `enum EdgeStore` |
|------|---------------------|------------------|
| 内存布局 | 堆分配（Box） + 指针间接 | **栈内联（值类型）** |
| 存储方式 | `HashMap<K, Box<dyn EdgeStore>>` | `HashMap<K, EdgeStore>`（与现状一致） |
| 分发开销 | vtable 指针跳转（~2ns） | match 分支跳转（~0.5ns，CPU 分支预测） |
| 缓存局部性 | 差（数据在堆上，指针追蹤） | **好（数据在 HashMap 内部连续）** |
| 新增第三变体 | 容易（加 impl） | 需要加 match 臂（但编译器强制检查） |
| 虚函数调用 | 每次调用都走 vtable | 无虚函数，全静态分发 |

### 为什么只有 2 个变体时枚举更优

1. **无堆分配**：`EdgeStore` enum 直接存储在 HashMap 中，不需要 `Box`。当前 `EdgeTable` 就是值类型，改为 `Box<dyn>` 会引入不必要的堆分配。

2. **无虚函数开销**：trait object 每次方法调用需要：读取 vtable 指针 → 跳转到函数指针。而 `match` 只有 2 个分支时，编译器优化为单次条件跳转，CPU 分支预测器完美命中。

3. **更好的缓存局部性**：HashMap 遍历时，`EdgeStore` 的数据紧邻存储，减少 cache miss。

4. **编译器穷尽检查**：`match` 强制覆盖所有变体，新增变体时编译器报错，不会遗漏。

5. **Send + Sync 自动推导**：enum 直接继承变体的 Send/Sync，无需 `dyn EdgeStore: Send + Sync` 约束。

```rust
// trait object 方式（差）
edge_tables: RwLock<HashMap<EdgeTableKey, Box<dyn EdgeStore>>>
// 每次访问：HashMap 查找 → 读 Box 指针 → 堆解引用 → vtable 跳转 → 函数调用

// enum 方式（优）
edge_tables: RwLock<HashMap<EdgeTableKey, EdgeStore>>
// 每次访问：HashMap 查找 → match 分支 → 直接函数调用
// 与现有代码完全兼容，无需改变存储结构
```

**结论：仅有 2 个变体时，枚举在性能、内存、代码简洁性上全面优于 trait object。**

---

## 修正后的设计方案：枚举 + 运行时 per-Edge-Type 配置

### 核心原则

1. **一个二进制文件**服务所有 Space 和 Edge Type
2. **每个 Edge Type 独立决定**是否启用 Time-Travel
3. **入口处分发一次**（工厂函数返回枚举变体），内部代码通过 `match` 分发
4. 当前时间查询 **零额外开销**

### 架构设计

```
                     ┌────────────────────────────────────┐
                     │      enum EdgeStore {              │
                     │        TimeTravel(TimeTravelStore),│
                     │        Simple(SimpleEdgeStore),    │
                     │      }                             │
                     │                                    │
                     │  impl EdgeStore {                  │
                     │    fn out_edges(&self, ..) {       │
                     │      match self {                  │
                     │        Self::TimeTravel(s) =>      │
                     │          s.merged_edges_of(..),    │
                     │        Self::Simple(s) =>          │
                     │          s.out_csr.edges_of(..),   │
                     │      }                             │
                     │    }                               │
                     │  }                                 │
                     └──────┬──────────────┬──────────────┘
                            │              │
              ┌─────────────▼──┐   ┌───────▼──────────────┐
              │ TimeTravelStore│   │   SimpleEdgeStore    │
              │ (time_travel   │   │   (time_travel       │
              │  = true)       │   │    = false)          │
              │                │   │                      │
              │ Mutable CSR    │   │   单段 CSR            │
              │ + Segments     │   │   轻量删除标记        │
              │ + MVCCManager  │   │   无 freeze/merge    │
              │ + freeze/merge │   │   仅当前时间查询      │
              └────────────────┘   └──────────────────────┘
                            ▲              ▲
                            │              │
                            └──────┬───────┘
                                   │
                     ┌─────────────┴──────────────┐
                     │  EdgeStore::create(schema)  │
                     │                             │
                     │  match schema.time_travel   │
                     │    true  → TimeTravel(..)   │
                     │    false → Simple(..)       │
                     └─────────────────────────────┘
```

### 入口分发（仅此一处）

```rust
// —— schema_engine.rs ——
// 创建 Edge Type 时，根据 schema 中的 time_travel_enabled 字段
// 选择对应的枚举变体。此后的所有操作通过枚举 match 分发。

pub fn create_edge_type(
    ctx: &GraphStorageContext,
    name: &str,
    src_label: LabelId,
    dst_label: LabelId,
    properties: Vec<StoragePropertyDef>,
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
    time_travel: bool,  // ← 新增参数
) -> StorageResult<LabelId> {
    // ... 现有校验逻辑 ...

    let schema = EdgeSchema {
        label_id,
        label_name: name.to_string(),
        src_label,
        dst_label,
        properties,
        oe_strategy,
        ie_strategy,
        schema_version: 1,
        time_travel_enabled: time_travel,  // ← 新增字段
    };

    // ★ 唯一的入口分发点 ★
    // 返回 EdgeStore 枚举（值类型），直接存入 HashMap
    let table = EdgeStore::create(schema, config)?;

    // 后续代码通过枚举方法操作，HashMap 存储结构不变
    ctx.data_store().edge_tables().write().insert(key, table);
    // ...
}
```

### 调用方代码：完全不变

```rust
// —— reader.rs / writer.rs ——
// 所有读写操作通过 EdgeStore 枚举的方法调用：

fn get_node_edges(ctx, space, node_id, direction) -> Result<Vec<Edge>> {
    let edge_tables = ctx.data_store().edge_tables().read();
    for table in edge_tables.values() {
        // table 是 &EdgeStore（枚举引用），调用时内部 match 分发
        let edges = table.out_edges(src_u32, ts);
        //                 ↑ 内部 match self { TimeTravel(s) => ..., Simple(s) => ... }
    }
}
```

**关键点：** HashMap 存储结构完全不变（`HashMap<EdgeTableKey, EdgeStore>` 替代 `HashMap<EdgeTableKey, EdgeTable>`），调用方代码无需任何改动。`EdgeStore` 枚举的方法内部通过 `match` 分发到具体实现，这是**唯一的分发点**。

---

## 详细设计

### 1. EdgeStore 枚举定义

```rust
/// 边存储的统一枚举
/// 仅 2 个变体：TimeTravel（多段）和 Simple（单段）
#[derive(Debug)]
pub enum EdgeStore {
    TimeTravel(TimeTravelEdgeStore),
    Simple(SimpleEdgeStore),
}

impl EdgeStore {
    /// 工厂方法：根据 schema 创建对应变体
    pub fn create(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        if schema.time_travel_enabled {
            Ok(EdgeStore::TimeTravel(
                TimeTravelEdgeStore::with_config(schema, config)?
            ))
        } else {
            Ok(EdgeStore::Simple(
                SimpleEdgeStore::with_config(schema, config)?
            ))
        }
    }

    // —— 基本 CRUD ——

    pub fn insert_edge(
        &mut self,
        src: u32, dst: u32, rank: i64,
        props: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.insert_edge(src, dst, rank, props, ts),
            EdgeStore::Simple(s)     => s.insert_edge(src, dst, rank, props, ts),
        }
    }

    pub fn delete_edge(
        &mut self,
        src: u32, dst: u32, rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match self {
            EdgeStore::TimeTravel(s) => s.delete_edge(src, dst, rank, ts),
            EdgeStore::Simple(s)     => s.delete_edge(src, dst, rank, ts),
        }
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        match self {
            EdgeStore::TimeTravel(s) => s.out_edges(src, ts),
            EdgeStore::Simple(s)     => s.out_edges(src, ts),
        }
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        match self {
            EdgeStore::TimeTravel(s) => s.in_edges(dst, ts),
            EdgeStore::Simple(s)     => s.in_edges(dst, ts),
        }
    }

    pub fn get_edge(
        &self, src: u32, dst: VertexId, rank: i64, ts: Timestamp
    ) -> Option<Nbr> {
        match self {
            EdgeStore::TimeTravel(s) => s.get_edge(src, dst, rank, ts),
            EdgeStore::Simple(s)     => s.get_edge(src, dst, rank, ts),
        }
    }

    pub fn has_edge(&self, src: u32, dst: VertexId, rank: i64, ts: Timestamp) -> bool {
        match self {
            EdgeStore::TimeTravel(s) => s.has_edge(src, dst, rank, ts),
            EdgeStore::Simple(s)     => s.has_edge(src, dst, rank, ts),
        }
    }

    // —— 属性 ——

    pub fn get_property(&self, edge_id: EdgeId, prop_name: &str) -> Option<Value> {
        match self {
            EdgeStore::TimeTravel(s) => s.get_property(edge_id, prop_name),
            EdgeStore::Simple(s)     => s.get_property(edge_id, prop_name),
        }
    }

    pub fn set_property(
        &mut self, edge_id: EdgeId, prop_name: &str, value: Value
    ) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.set_property(edge_id, prop_name, value),
            EdgeStore::Simple(s)     => s.set_property(edge_id, prop_name, value),
        }
    }

    // —— 扫描 ——

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        match self {
            EdgeStore::TimeTravel(s) => s.scan(ts),
            EdgeStore::Simple(s)     => s.scan(ts),
        }
    }

    // —— 持久化 ——

    pub fn flush(&self, path: &Path, compression: CompressionType) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.flush(path, compression),
            EdgeStore::Simple(s)     => s.flush(path, compression),
        }
    }

    pub fn load(&mut self, path: &Path) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.load(path),
            EdgeStore::Simple(s)     => s.load(path),
        }
    }

    // —— 元数据 ——

    pub fn schema(&self) -> &EdgeSchema {
        match self {
            EdgeStore::TimeTravel(s) => s.schema(),
            EdgeStore::Simple(s)     => s.schema(),
        }
    }

    pub fn label(&self) -> LabelId {
        match self {
            EdgeStore::TimeTravel(s) => s.label(),
            EdgeStore::Simple(s)     => s.label(),
        }
    }

    pub fn is_open(&self) -> bool {
        match self {
            EdgeStore::TimeTravel(s) => s.is_open(),
            EdgeStore::Simple(s)     => s.is_open(),
        }
    }

    pub fn close(&mut self) {
        match self {
            EdgeStore::TimeTravel(s) => s.close(),
            EdgeStore::Simple(s)     => s.close(),
        }
    }

    // —— Time-Travel 专用 ——

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        match self {
            EdgeStore::TimeTravel(s) => s.export_snapshot(ts),
            EdgeStore::Simple(_) => Err(StorageError::unsupported(
                "Time-Travel snapshot is not enabled for this edge type"
            )),
        }
    }

    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.freeze_csr_only(ts),
            EdgeStore::Simple(_) => 0,  // Simple 不需要 freeze
        }
    }

    pub fn merge_segments(&mut self, current_ts: Timestamp) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_segments(current_ts),
            EdgeStore::Simple(_) => 0,  // Simple 不需要 merge
        }
    }

    // —— 统计 ——

    pub fn segment_count(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.segment_count(),
            EdgeStore::Simple(_) => 0,
        }
    }

    pub fn edge_count(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.edge_count(),
            EdgeStore::Simple(s)     => s.edge_count(),
        }
    }
}
```

### 2. TimeTravelEdgeStore（现有逻辑迁移）

```rust
/// 启用 Time-Travel 的边存储实现
/// 保留现有的所有逻辑：Mutable CSR + Segments + freeze + merge + tombstone
pub struct TimeTravelEdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    pub out_segments: Vec<CsrSegment>,
    pub in_segments: Vec<CsrSegment>,
    pub out_segment_index: Vec<(Timestamp, usize)>,
    pub in_segment_index: Vec<(Timestamp, usize)>,
    pub mvcc: MVCCManager,
    pub properties: PropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<Arc<StatsManager>>,
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    pub property_index_cache: HashMap<String, usize>,
}

impl TimeTravelEdgeStore {
    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        // 现有 EdgeTableCore::with_config 的全部逻辑
        // ...
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        // 现有 merged_edges_of 逻辑（遍历 segment + 去重 + tombstone 检查）
        // 内部可加入自动降级优化：
        //   if ts == u32::MAX && self.out_segments.len() <= 3 {
        //       return self.fast_merged_out_edges(src);
        //   }
        self.merged_edges_of(&self.out_csr, &self.out_segments, src, ts)
    }

    pub fn insert_edge(&mut self, src: u32, dst: u32, rank: i64,
                       props: &[(String, Value)], ts: Timestamp) -> StorageResult<()> {
        // 现有逻辑：写入 Mutable CSR → 可能触发 freeze
        // ...
    }

    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        // 现有 freeze 逻辑（freeze.rs）
    }

    pub fn merge_segments(&mut self, current_ts: Timestamp) -> usize {
        // 现有 merge 逻辑（merge.rs）
    }

    // ... 其他方法保持现有逻辑不变 ...
}
```

### 3. SimpleEdgeStore（新增实现）

```rust
/// 不启用 Time-Travel 的边存储实现
/// 单段 CSR + 轻量删除追踪，零历史开销
pub struct SimpleEdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,        // 单一 CSR，无 segment
    pub in_csr: CsrVariant,         // 单一 CSR，无 segment
    pub properties: PropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<Arc<StatsManager>>,

    /// 轻量删除追踪（替代 tombstone 三层结构）
    /// 方案 A：物理删除（如果不需要事务隔离则直接移除行）
    /// 方案 B：HashSet 标记（如果还需要事务隔离）
    pub deleted_edges: HashSet<EdgeId>,
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    pub property_index_cache: HashMap<String, usize>,
}

impl SimpleEdgeStore {
    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        let out_csr = CsrVariant::from_strategy(
            schema.oe_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
        )?;
        let in_csr = CsrVariant::from_strategy(
            schema.ie_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
        )?;

        let mut properties = PropertyTable::with_capacity(config.initial_edge_capacity);
        for prop in &schema.properties {
            properties.add_property(prop.name.clone(), prop.data_type.clone(), prop.nullable);
        }

        Ok(Self {
            label: schema.label_id,
            label_name: schema.label_name.clone(),
            src_label: schema.src_label,
            dst_label: schema.dst_label,
            schema,
            out_csr,
            in_csr,
            properties,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            deleted_edges: HashSet::new(),
            version_history: Arc::new(Mutex::new(LabelVersionHistory::new(
                schema.label_id, schema.label_name.clone(), SchemaObjectType::Edge,
            ))),
            property_index_cache: HashMap::new(),
        })
    }

    pub fn out_edges(&self, src: u32, _ts: Timestamp) -> Vec<Nbr> {
        // ★ 关键：单次 CSR 查找，零 segment 遍历，零 HashSet 去重 ★
        self.out_csr.edges_of_with_position(src)
            .filter(|(_, edge)| !self.deleted_edges.contains(&edge.edge_id))
            .map(|(_, edge)| Nbr::new(edge.neighbor, edge.edge_id, edge.prop_offset, edge.timestamp))
            .collect()
    }

    pub fn get_edge(&self, src: u32, dst: VertexId, rank: i64, _ts: Timestamp) -> Option<Nbr> {
        self.out_csr.edges_of_with_position(src)
            .find(|(_, e)| e.neighbor == dst && !self.deleted_edges.contains(&e.edge_id))
            .map(|(_, e)| Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp))
    }

    pub fn insert_edge(&mut self, src: u32, dst: u32, rank: i64,
                       props: &[(String, Value)], _ts: Timestamp) -> StorageResult<()> {
        // 直接写入 CSR，无 freeze 触发
        let edge_id = self.next_edge_id;
        self.next_edge_id.0 += 1;
        // ... CSR insert + PropertyTable insert ...
        Ok(())
    }

    pub fn delete_edge(&mut self, src: u32, dst: u32, rank: i64, _ts: Timestamp) -> StorageResult<bool> {
        // 方案 A：物理删除（最简单）
        // 方案 B：标记删除（如果还需要事务隔离）
        // ...
        self.deleted_edges.insert(edge_id);
        Ok(true)
    }

    pub fn segment_count(&self) -> usize {
        0
    }

    // ... 其他方法 ...
}
```

### 4. Schema 和 Config 扩展

```rust
// —— EdgeSchema 新增字段 ——
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,
    pub ie_strategy: EdgeStrategy,
    pub schema_version: u64,
    // ★ 新增 ★
    pub time_travel_enabled: bool,
}

// —— EdgeTableConfig 新增字段 ——
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    pub initial_edge_capacity: usize,
    pub max_segments_per_direction: usize,
    pub max_mutable_csr_bytes: usize,
    pub segment_merge_threshold: usize,
    pub merge_keep_newest: usize,
    // ★ 新增 ★
    /// 是否启用 Time-Travel（历史查询、segment 管理）
    /// 由 EdgeSchema.time_travel_enabled 初始化
    pub time_travel_enabled: bool,
}
```

### 5. 持久化加载时的分发

```rust
impl EdgeStore {
    /// 从磁盘加载，根据 meta.bin 中的 time_travel_enabled 标志选择变体
    pub fn load(path: &Path) -> StorageResult<Self> {
        let meta = read_meta(path)?;  // 读取 time_travel_enabled 标志
        if meta.time_travel_enabled {
            let mut store = TimeTravelEdgeStore::empty(meta.schema, meta.config);
            store.load_data(path)?;
            Ok(EdgeStore::TimeTravel(store))
        } else {
            let mut store = SimpleEdgeStore::empty(meta.schema, meta.config);
            store.load_data(path)?;
            Ok(EdgeStore::Simple(store))
        }
    }
}
```

### 6. 从 Simple 迁移到 TimeTravel

```rust
impl TimeTravelEdgeStore {
    pub fn migrate_from(simple: SimpleEdgeStore) -> Self {
        // 将 Simple 的单段 CSR 作为第一个 segment
        let out_segment = CsrSegment::from_csr(simple.out_csr, 0..u32::MAX);
        let in_segment = CsrSegment::from_csr(simple.in_csr, 0..u32::MAX);

        TimeTravelEdgeStore {
            out_csr: CsrVariant::new_empty(),
            in_csr: CsrVariant::new_empty(),
            out_segments: vec![out_segment],
            in_segments: vec![in_segment],
            // ... 其他字段继承 ...
        }
    }
}
```

---

## 配置粒度分析

### 三种可选粒度

| 粒度 | 配置位置 | 优点 | 缺点 |
|------|---------|------|------|
| **全局（Server 级）** | `Config` 或命令行参数 | 最简单 | 缺乏灵活性 |
| **Space 级** | 建 Space 时指定，Space 内所有边类型继承 | 逻辑清晰 | 同一 Space 内无法混合 |
| **Edge Type 级**（推荐） | 建 Edge Type 时指定 | 最灵活，匹配 SQL Server 模型 | 需要边类型级别配置 |

### 推荐：Edge Type 级 + Space 级默认值

```sql
-- 创建 Space 时指定默认值（可选，不指定则用全局默认）
CREATE SPACE audit_space WITH time_travel_default = true;

-- 创建边类型时可覆盖 Space 默认值
CREATE EDGE KNOWS(src Person, dst Person)
    WITH time_travel = true;   -- 显式启用

CREATE EDGE VIEWS(src User, dst Page) 
    WITH time_travel = false;  -- 显式禁用（高频 OLTP 边）

-- 不指定时继承 Space 默认值
CREATE EDGE LIKES(src User, dst Post);
-- 如果 Space 默认 time_travel = true，则启用
```

对应的 DSL 语法扩展：

```rust
// CreateEdgeTypeParams 新增字段
pub struct CreateEdgeTypeParams {
    pub name: String,
    pub user_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,
    pub ie_strategy: EdgeStrategy,
    pub time_travel: Option<bool>,  // None = 继承 Space 默认值
}
```

---

## 与 SQL Server 的类比

| SQL Server 概念 | Linkrs 对应 | 说明 |
|----------------|------------|------|
| 当前表 | `Mutable CSR`（TimeTravel）或 `Single CSR`（Simple） | 存储当前有效数据 |
| 历史表 | `CsrSegment[]` | 冻结的历史数据段 |
| `SYSTEM_VERSIONING = ON` | `time_travel_enabled = true` | 启用时自动维护 segment |
| `SYSTEM_VERSIONING = OFF` | `time_travel_enabled = false` | 使用 SimpleEdgeStore，无 segment |
| `ValidFrom / ValidTo` | `Segment.create_ts_min / create_ts_max` | 时间范围 |
| `FOR SYSTEM_TIME AS OF` | `out_edges(src, historical_ts)` | 历史时间点查询 |
| 当前查询（无 FOR SYSTEM_TIME） | `out_edges(src, u32::MAX)` | 只查当前数据 |
| 每表独立决定 | 每 Edge Type 独立决定 | 灵活性和隔离性 |

---

## 性能对比：两种实现在同一进程中

```
┌─────────────────────────────────────────────────────────┐
│              同一个 Linkrs Server 进程                     │
│                                                         │
│  Space "OLTP"                    Space "Audit"           │
│  ┌─────────────────────┐        ┌─────────────────────┐ │
│  │ Edge "TRANSFERS"    │        │ Edge "TRANSFERS"    │ │
│  │ → Simple(SimpleStore)│       │ → TimeTravel(TTStore)│ │
│  │                     │        │                     │ │
│  │ out_edges(src, MAX) │        │ out_edges(src, MAX) │ │
│  │  → match → Simple   │        │  → match → TimeTravel│ │
│  │  → 1次 CSR 查找     │        │  → 50段遍历+去重    │ │
│  │  → ~10μs           │        │  → ~500μs           │ │
│  │                     │        │                     │ │
│  │ out_edges(src, 100) │        │ out_edges(src, 100) │ │
│  │  → 同上（忽略ts）   │        │  → 历史时间查询     │ │
│  │                     │        │  → 时间范围剪枝     │ │
│  └─────────────────────┘        └─────────────────────┘ │
│                                                         │
│  HashMap<EdgeTableKey, EdgeStore>  → 值类型存储，无堆分配  │
│  match 分发 → 2 分支，CPU 分支预测器完美命中               │
└─────────────────────────────────────────────────────────┘
```

---

## 枚举 vs trait object 的编译产物

```rust
// 枚举方案的 out_edges 编译后大致等价于：
fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
    // 读取枚举 discriminant（0 = TimeTravel, 1 = Simple）
    // cmp discriminant, 0
    // je  .time_travel_branch
    // jmp .simple_branch
    //
    // .time_travel_branch:
    //   call TimeTravelEdgeStore::out_edges(self.data, src, ts)
    //   ret
    // .simple_branch:
    //   call SimpleEdgeStore::out_edges(self.data, src, ts)
    //   ret
}

// trait object 方案的 out_edges 编译后大致等价于：
fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
    // 读取 vtable 指针（在堆上，可能 cache miss）
    // 读取 vtable[out_edges_offset] 函数指针
    // call *function_ptr(self.data, src, ts)
    // ret
}
```

枚举方案的 `match` 在仅 2 分支时就是一次条件跳转 + 直接函数调用，编译器可内联或优化。trait object 需要额外的指针间接。

---

## 实施路径

### 阶段 1：将 EdgeTable 改为枚举（1-2 天）

将现有 `EdgeTableCore` 重命名为 `TimeTravelEdgeStore`，定义 `EdgeStore` 枚举（目前仅 `TimeTravel` 变体），所有方法加 `match` 分发。此阶段**不改变任何行为**，仅做枚举包装。

```
改动范围：
  - edge/edge_table/core.rs  → 重命名为 TimeTravelEdgeStore
  - edge/edge_table/mod.rs   → 定义 enum EdgeStore { TimeTravel(TimeTravelEdgeStore) }
  - 枚举方法内 match 分发（目前仅 1 臂，后续加 Simple 臂）
  - 所有引用 EdgeTable 的代码 → 改为 EdgeStore（类型名变化）
```

### 阶段 2：实现 SimpleEdgeStore（2-3 天）

实现 `SimpleEdgeStore` 结构体，覆盖所有方法。核心逻辑：
- `out_edges` / `in_edges` / `get_edge` → 直接 CSR 查询
- `insert_edge` / `delete_edge` → 直接 CSR 操作
- `freeze_csr_only` / `merge_segments` → 空操作
- `export_snapshot` → 返回错误

### 阶段 3：Schema 扩展 + 枚举工厂（1 天）

- `EdgeSchema` 添加 `time_travel_enabled: bool`
- `EdgeTableConfig` 添加 `time_travel_enabled: bool`
- 实现 `EdgeStore::create()` 工厂方法
- 修改 `schema_engine.rs` 调用 `EdgeStore::create()`

### 阶段 4：持久化兼容（1 天）

- 在 `meta.bin` 中写入 `time_travel_enabled` 标志
- 加载时根据标志选择反序列化路径
- 向后兼容：旧数据（无此标志）默认视为 `time_travel = true`

### 阶段 5：DSL 语法扩展（1-2 天）

- 在 `CREATE EDGE` 语句中添加 `WITH time_travel = true|false` 子句
- 在 `CREATE SPACE` 中添加 `WITH time_travel_default = true|false`
- 在 `SHOW CREATE EDGE` 中展示 time_travel 状态

---

## 总结

| 对比维度 | 编译期 feature flag | trait object (Box<dyn>) | **枚举 + match（最终方案）** |
|---------|-------------------|------------------------|--------------------------|
| 二进制数量 | 需要 2 个 | 1 个 | **1 个** |
| 不同 Space 混合 | 不可能 | 支持 | **支持** |
| 不同 Edge Type 混合 | 不可能 | 支持 | **支持** |
| 内部代码分支 | 散落 `#[cfg]` | 零分支（虚函数） | **match 分发（仅枚举方法内）** |
| 内存布局 | 编译期消除 | 堆分配（Box） | **栈内联（值类型）** |
| HashMap 存储 | N/A | `HashMap<K, Box<dyn>>` | **`HashMap<K, EdgeStore>`（与现状一致）** |
| 分发开销 | 零 | vtable 指针跳转 ~2ns | **match 分支跳转 ~0.5ns** |
| 缓存局部性 | N/A | 差（堆指针追逐） | **好（连续内存）** |
| 编译器穷尽检查 | N/A | 无（trait 可被任意实现） | **有（match 强制覆盖所有变体）** |
| 增加新变体 | 新编译产物 | 容易（加 impl） | 需加 match 臂（编译器报错引导） |
| 参照模型 | 无 | 无 | **SQL Server 时态表** |

**最终结论：采用 `enum EdgeStore { TimeTravel(...), Simple(...) }` + match 分发方案。** 仅 2 个变体时，枚举在性能（无堆分配、无虚函数、更好缓存局部性）、内存（值类型内联）、代码安全性（编译器穷尽检查）上全面优于 trait object。入口处一次分发（`EdgeStore::create()`），内部代码通过枚举方法调用，无需任何 `#[cfg]` 或额外分支。