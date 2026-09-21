# CSR 选择与分发

## 总览

边存储的运行时多态分两级：建表时由记录形态（`RecordForm`）与
边策略（`EdgeStrategy`）决定每组装什么 `CsrVariant`；
调用时由单个 `dispatch!` 宏做枚举分发，无虚表开销。

## 入口

### 两级选择：RecordForm × EdgeStrategy

**位置**：`node_group/core.rs` 的 `fresh_variant()`（组级唯一构造出口）

```rust
match self.record_form {
    RecordForm::Pure => CsrVariant::Pure(...),
    RecordForm::Bundled => CsrVariant::Bundled(...),
    RecordForm::Columnar => CsrVariant::from_strategy_with_overflow(
        self.strategy, self.group_size(), 0, self.overflow_chunk_edges,
    )?,
}
```

- `RecordFormPreference::Auto` 按模式自动推导形态
  （无属性 → `Pure`，单可编码标量 → `Bundled`，其余 → `Columnar`），
  选定后持久化在 `meta.bin`，加载时不再重推；
  破坏形态前置条件的模式变更须离线重建。
- `RecordFormPreference::Columnar` 强制走策略路径。

### CsrVariant::from_strategy_with_overflow：列式形态工厂

**位置**：`csr_variant/core.rs`

```rust
pub fn from_strategy_with_overflow(
    strategy: EdgeStrategy,       // 仅 Multiple / Single / None
    vertex_capacity: usize,
    edge_capacity: usize,
    overflow_chunk_edges: usize,  // 每溢出块边数（须 > 0）
) -> StorageResult<Self> {
    match strategy {
        EdgeStrategy::Multiple => Ok(CsrVariant::Multiple(...)),
        EdgeStrategy::Single => Ok(CsrVariant::Single(...)),
        EdgeStrategy::None => Ok(CsrVariant::None { vertex_capacity }),
    }
}
```

旧 `from_strategy`（三参、无溢出块参数）已删除。
`overflow_chunk_edges == 0` 在 `CsrShardSet::new` 即被拒绝。

### 决策流

```
建表 RecordFormPreference
  │
  ├─ Auto ──→ 模式推导 ──→ Pure ──────→ PureTopologyCsr
  │                       ├─ Bundled ──→ BundledCsr
  │                       └─ Columnar ─→ 见下策略分支
  │
  └─ Columnar ──→ EdgeStrategy ──┬─ Multiple ──→ MutableCsr
                                 ├─ Single ────→ SingleMutableCsr
                                 └─ None ──────→ 占位（建表校验拒绝，仅组级回退）

运行期显式冻结/解冻：
  MutableCsr ──freeze──→ Frozen（ImmutableCsr）
  Frozen ──unfreeze──→ Multiple / Single（原策略）
  Frozen + 边车有效 ──open──→ Mapped（mmap 视图）
```

## CsrVariant 枚举

**位置**：`csr_variant.rs`

```rust
pub enum CsrVariant {
    Multiple(Box<MutableCsr>),
    Single(SingleMutableCsr),
    Pure(Box<PureTopologyCsr>),
    Bundled(Box<BundledCsr>),
    Frozen(Box<ImmutableCsr>),
    Mapped(Box<MappedFrozen>),
    None { vertex_capacity: usize },
}
```

跨层遍历契约：行位置（`EdgePosition` 块/槽对、主块偏移、
溢出下标）是变体私有的，永不过层边界。层间交接
（读→写、扫描→点查、存活表→迁移）一律先按边 ID 键重解目标；
从一个变体拿到的位置永不拿到另一变体解释。
无位置寻址能力的形态对定位写 fail-closed（报错），
不静默回退到 ID 扫描。

## 分发：单个宏

**位置**：`csr_variant.rs`

可变与不可变调用共用一个 `dispatch!` 宏（可变性随接收者走，
不保留第二宏）：

```rust
macro_rules! dispatch {
    ($self:expr, $method:ident($($arg:expr),+ $(,)?) -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method($($arg),+),
            CsrVariant::Single(csr) => csr.$method($($arg),+),
            CsrVariant::Pure(csr) => csr.$method($($arg),+),
            CsrVariant::Bundled(csr) => csr.$method($($arg),+),
            CsrVariant::Frozen(csr) => csr.$method($($arg),+),
            CsrVariant::Mapped(csr) => csr.$method($($arg),+),
            CsrVariant::None { .. } => $default,
        }
    };
    ($self:expr, $method:ident() -> $default:expr) => { /* 同上七分支 */ };
}
```

### 模式一：变更操作

```rust
impl MutableCsrTrait for CsrVariant {
    fn insert_edge(&mut self, src_vid: u32, dst: VertexId,
                   edge_id: EdgeId, ts: Timestamp) -> StorageResult<()> {
        // 拓扑与属性解耦：CSR 只存拓扑，无 prop_offset 参数。
        // 插入用显式 match（非 dispatch!）：None 分支报拒绝错误，
        // Frozen / Mapped 由底层实现拒绝写入。
        match self {
            CsrVariant::Multiple(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Single(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Pure(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Frozen(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Mapped(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::None { .. } => Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            )),
        }
    }
    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp)
        -> StorageResult<bool> {
        dispatch!(self, delete_edge(src_vid, edge_id, ts) -> Err(...))
    }
    // delete_edge_by_dst 全匹配语义：一次调用删除全部存活匹配，返回删除计数
}
```

要点：match 展开分发（可内联，无虚表）；`Frozen` / `Mapped`
在底层实现侧拒绝写入（须显式解冻，无隐式解冻）；
`dispatch!` 的 `None` 分支返回调用点给出的 `$default`
（读取类为空值，删除类为 0 / 报错），
插入用显式 `match` 使 `None` 直接报错。

### 模式二：读操作

读分测试原语（戳过滤：`get_edge`、`edges_of`）与
生产路径（经版本权威的访问者 / 缓冲填充）两类，
后者统一行视图三形态：`visit_physical`（出借遍历）、
`fill_physical_into`（调用方缓冲）、`physical_edges_of`
（分配式，仅测试与离线）。`None` 的读一律返回空。

### 模式三：持久化（序列化标签）

**位置**：`csr_variant/persistence.rs`

```
Tag  变体
0    None
1    Multiple
2    Single
3    Frozen / Mapped（Mapped 按 Frozen 标签转储，共享标签 3）
4    Pure
5    Bundled
```

`dump()` 首字节打标签；`load()` 按首字节分发重建。
`dump_into()` 与 `dump()` 字节一致，供检查点零拷贝追加。
各形态载荷尾带 CRC32，加载先验签再解析；边 ID 计数等
结构字段做重算校验，篡改与截断均拒绝。

### 模式四：迭代器分发

**位置**：`csr_variant/iter.rs`

```rust
pub enum CsrIterator<'a> {
    Multiple(...), Single(...), Pure(...), Bundled(...),
    Frozen(...), Mapped(...), None,
}
```

行内顺序按形态承诺，不做全局承诺：可变 / Single /
Pure / Bundled 行为插入序、无序承诺；Frozen / Mapped 行按
`(endpoint, rank, edge_id)` 紧凑有序并承诺该顺序与键区间二分。
冻结、回收、压缩与服务重建可改变顺序，查询层不得依赖未承诺顺序。
`MutableCsr::is_row_sorted` 给出行级有序建议，供计划选择。

## 集成：EdgeSchema → EdgeStore → CsrVariant

### 表创建

```
EdgeSchema { oe_strategy, ie_strategy, record_form, ... }
  │  validate：两方向须同为启用（非 None），单向表构造即拒
  ├─→ RecordFormPreference::Auto/Columnar → RecordForm（持久化）
  └─→ 每方向 CsrShardSet { strategy, group_bits, record_form }
        └─→ 每组 fresh_variant() → CsrVariant
```

组按 `group = vid >> group_bits`、`local = vid & (group_size - 1)`
划分端点区间；组稀疏存放（`BTreeMap`），缺席组读为空、
写时按需建组；邻居键保留全局端点值，仅行寻址本地化。

### 查询执行

```
Query("traverse edges")
  │
  ├─→ EdgeStore.edges_of(src, ts)
  │   │
  │   └─→ 路由到属组 → group.variant.edges_of(src, ts)  // CsrVariant 分发
  │       │
  │       ├─ Multiple ──→ 主块 + 溢出链（宽行走存活集）
  │       ├─ Single ────→ O(1) 直接槽
  │       ├─ Pure ──────→ 主块 + 溢出链（现场组装 Nbr）
  │       ├─ Bundled ───→ 同 Pure 并附值列
  │       ├─ Frozen ────→ 有序行二分 / 线性扫描
  │       ├─ Mapped ────→ 同 Frozen，经 mmap 按需分页
  │       └─ None ──────→ 空
  │
  └─→ Columnar：按 EdgeId 查属性列式存储；Pure/Bundled：属性 stub/内联
```

## 回收与维护

无整表重建式回收接口：生产回收走按行
`compact_vertex_with_reporting`，整表 `compact_with_ts_reporting`
仅用于恢复与离线重建；单测直接练习上报入口。

**各变体行为**：

| 变体 | 行为 |
|---|---|
| `Multiple` | 整表回收：合并溢出、丢截止线下墓碑并逐边上报 |
| `Single` | 丢截止线下单槽墓碑并上报（无整表 reserve 参数） |
| `Pure` / `Bundled` | 行级回收（Bundled 双列同步移动） |
| `Frozen` / `Mapped` | 无操作（只读） |
| `None` | 无操作（零边） |

仅 `Multiple` 的碎片诊断有意义：

```rust
pub fn fragmentation_ratio(&self) -> f32 {
    match self {
        CsrVariant::Multiple(csr) => csr.fragmentation_ratio(),
        _ => 0.0,
    }
}
```

## 设计原则

### 1. 零虚表开销

枚举 `match` 分发可内联；无运行时间接调用；
编译器可按变体优化。

### 2. 统一接口

所有变体实现 `CsrBase` + `MutableCsrTrait`；
单个枚举类型简化代码（无泛型参数）；
模式匹配保证编译期类型安全。

### 3. 显式失败

- `Frozen` / `Mapped` 写拒绝错误，不静默成功；
- 无位置寻址能力的形态对定位写 fail-closed；
- `None` 读返回空、删返回未找到；
- 校验失败（CRC、计数重算、尾部多余字节）一律拒绝加载。

### 4. 可扩展

新增变体只需：实现 trait 的新结构体 +
`dispatch!` 七分支加一臂 + 持久化标签 +
迭代器枚举加一臂。宏使样板最小化。
