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

- `RecordFormPreference::Auto` 按模式推导安全默认值
  （无属性 → `Pure`，其余 → `Columnar`），
  选定后持久化在 `meta.bin`，加载时不再重推；建表日志报告推导结果，
  锁定后破坏形态前置条件的模式变更须走迁移（`migration_plan` /
  `migrate_record_form` / `switch_record_form_online`）并随后检查点，
  不做原地重解释。
- `RecordFormPreference::Columnar` 强制走策略路径。
- `RecordFormPreference::Bundled` 显式选入单标量内联形态：
  `Auto` 永不推导 `Bundled`，只有按名指定才承担内联限制
  （无 rank、无 MVCC 版本链、无在线改列）；不满足准入时建表直接失败。

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
  │                       └─ Columnar ─→ 见下策略分支
  │
  ├─ Bundled ──→ 准入校验 ──→ BundledCsr（失败直接报错）
  │
  └─ Columnar ──→ EdgeStrategy ──┬─ Multiple ──→ MutableCsr
                                 ├─ Single ────→ SingleMutableCsr
                                 └─ None ──────→ 占位（建表校验拒绝，仅组级回退）

运行期显式冻结/解冻：
  MutableCsr ──freeze──→ Frozen（ImmutableCsr）
  Frozen ──unfreeze──→ Multiple / Single（原策略）
  Frozen + 边车有效 ──load──→ 堆内 Frozen + 组级映射驻留（mmap 热点读）
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
    None { vertex_capacity: usize },
}
```

映射（`MappedFrozen`）不再是枚举分支，而是冻结组的派生驻留：
组槽在变体之外另持可选映射句柄，堆内冻结保持权威，
热点读在驻留有效时经映射服务。

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
            CsrVariant::None { .. } => $default,
        }
    };
    ($self:expr, $method:ident() -> $default:expr) => { /* 同上六分支 */ };
}
```

### 模式一：变更操作

```rust
impl MutableCsrTrait for CsrVariant {
    // 结果型写（插入、按编号删除、按偏移删除）：先查能力门，
    // 占位直接报拒绝错误，只读转发到底层以保留各自拒绝文案，
    // 以下 match 只服务可写形态。
    fn insert_edge(&mut self, src_vid: u32, dst: VertexId,
                   edge_id: EdgeId, ts: Timestamp) -> StorageResult<()> {
        if self.is_empty_placeholder() { return Err(...); }
        if self.is_read_only() {
            return match self {
                CsrVariant::Frozen(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
                _ => Err(...),
            };
        }
        match self {
            CsrVariant::Multiple(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Single(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Pure(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            _ => unreachable!(...),
        }
    }
    // delete_edge / delete_edge_by_offset 与插入同门。
    // delete_edge_by_dst 全匹配语义：一次调用删除全部存活匹配，返回删除计数；
    // 计数型删除（含上报变体）先查只读与占位门，命中直接返回零。
}
```

要点：match 展开分发（可内联，无虚表）；是否支持、
失败时返回零还是报错的决策集中在能力查询
（`is_writable/is_read_only/is_empty_placeholder/supports_positions/
supports_vertex_compact/supports_timestamp_reclaim/
has_reserved_capacity/promises_key_order/should_use_bisection`），
分发点只执行门结论。`Frozen` 须显式解冻，
无隐式解冻；只读臂转发到底层是为了保留拒绝文案，
决策本身仍在门上。`dispatch!` 只留给纯读与门后的可写转发，
不再作为写删的决策路径。

调用方契约（计数与结果的区分有意保留，决策已收敛到能力门）：
计数型删除（按目的删除及其上报变体）对只读形态与空占位返回零，
结果型删除（插入、按编号删除、定位与偏移删除）对同等形态返回拒绝错误。
审计与回滚路径统一按“计数型看数量、结果型看错误”处理，
表层写路径在冻结组上提前拒绝，调用方不再逐入口记忆差异。

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
3    Frozen（堆内权威字节；映射身份不持久）
4    Pure
5    Bundled
```

`dump()` 首字节打标签；`load()` 按首字节分发重建。
标签 3 只承载堆内 `Frozen` 字节：转储保留权威堆字节，
加载一律重建堆内 `Frozen`。需要映射加速的调用方走检查点边车
路径挂载组级驻留，不得假设视图类型可往返。`dump_into()` 与
`dump()` 字节一致，供检查点零拷贝追加；带暂存转储同样共享
该标签行为。各形态载荷尾带 CRC32，加载先验签再解析；边 ID
计数等结构字段做重算校验，篡改与截断均拒绝。

### 模式四：迭代器分发

**位置**：`csr_variant/iter.rs`

```rust
pub enum CsrIterator<'a> {
    Multiple(...), Single(...), Pure(...), Bundled(...),
    Frozen(...), None,
}
```

行内顺序按形态承诺，不做全局承诺：可变 /
Pure / Bundled 行为插入序、无序承诺；Frozen / Single 行按
键有序并承诺该顺序与键区间二分（`Single` 行至多一槽，天然有序）。
冻结、回收、压缩与服务重建可改变顺序，查询层不得依赖未承诺顺序。
`is_row_sorted` 只在承诺有序的形态上选择二分，其余走线性扫描；
内存态有序标志不持久，加载后重建，计划缓存不得跨重启复用该标志。
`Bundled` 行迭代器复用纯拓扑类型，只产出拓扑：遍历与读值必须配对
（`visit_physical_with_values` 或 `bundled_value_*`），单独走拓扑会静默丢值。
范围查询在 `Pure` / `Bundled` 上显式忽略排序键半键：调用方按端点区间
构造查询，跨形态复用同一二元组区间时由调用方收紧。

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
  │       ├─ Frozen ────→ 有序行二分 / 线性扫描（驻留有效时经映射同序服务）
  │       └─ None ──────→ 空
  │
  └─→ Columnar：按 EdgeId 查属性列式存储；Pure/Bundled：属性 stub/内联
```

生产批量走热端与配对批量：邻居与度数批量走 `visit_hot` 热端，
避免属性解码；Bundled 表默认走配对批量
（组 `fill_physical_with_values_batch_into`、表
`AdjacencyBatchAccessor::fill_many_with_values_into`），
拓扑批量只用于无需值的遍历。缺席组在各层一律读空：
计数删除回零，结果型删除在稀疏洞上回 `Ok(false)`，
仅无边策略表 fail-closed 报错，与占位变体一致。

## 回收与维护

无整表重建式回收接口：生产回收走按行
`compact_vertex_with_reporting`，整表 `compact_with_ts_reporting`
仅用于恢复与离线重建；单测直接练习上报入口。

**各变体行为**：

| 变体 | 行级回收 | 整表回收 |
|---|---|---|
| `Multiple` | 合并溢出、丢截止线下墓碑并逐边上报 | 同行级并消除溢出链 |
| `Single` | 丢截止线下单槽墓碑并上报（无整表 reserve 参数） | 同行级语义 |
| `Pure` / `Bundled` | 行级回收（Bundled 双列同步移动） | 无操作，离线重建同样不走整表 |
| `Frozen` | 无操作（只读） | 堆内回收并上报 |
| `None` | 无操作（零边） |

碎片口径覆盖持有预留行容量的形态（`Multiple` / `Pure` / `Bundled`），
其余形态报告零或空。墓碑复用水位仅 `Multiple` 生效，
`fresh_variant` 仅在列式分支传播水位，`Pure` / `Bundled` 分支不传播。

## 设计原则

### 1. 零虚表开销

枚举 `match` 分发可内联；无运行时间接调用；
编译器可按变体优化。

### 2. 统一接口

所有变体实现 `CsrBase` + `MutableCsrTrait`；
单个枚举类型简化代码（无泛型参数）；
模式匹配保证编译期类型安全。

### 3. 显式失败

- `Frozen` 写拒绝错误，不静默成功；
- 无位置寻址能力的形态对定位写 fail-closed；
- `None` 读返回空；计数删返回零，结果型删返回拒绝错误；
  组层稀疏洞结果型删回 `Ok(false)`（无边策略表除外）；
- 校验失败（CRC、计数重算、尾部多余字节）一律拒绝加载。

### 4. 可扩展

新增变体检查清单（存于本文档而非代码注释，上线前逐项核对，
不实际新增变体时只做走查）：枚举定义、`dispatch!` 全部分支、
能力查询（可写、只读、占位、位置、行回收、整表回收、
预留容量、键序承诺、二分计划）、持久化标签与加载分支、
行/全表迭代器枚举、读遍历三形态、维护与回收分支、内联值分支、
组容器构造出口、表配对批量、检查点边车分支、
本文档与变体文档同步。组批量（`fill_physical_batch_into` 与
`fill_physical_with_values_batch_into`）按段借 shard 后走变体单行
入口，不设逐形态分支，新增形态无需改动组批量。宏使样板最小化，但不能代替逐项核对。
