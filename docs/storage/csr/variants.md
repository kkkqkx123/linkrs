# CSR 变体详解

本文描述当前七种 `CsrVariant` 变体的实现细节。
历史形态（`MultiSingle`、`Labeled`、旧不可变 `Csr`、
`prop_offset` 行内属性偏移）均已删除，不再记录。

## 0. 通用基础：槽位两半与哨兵

除 `None` 外，所有变体共享同一槽位语义：

- 热端 `HotNbr { endpoint: u32, rank: i64, edge_id: EdgeId }`（24 字节）：
  遍历所需的全部拓扑；只走热端的扫描不触碰冷端缓存行。
- 冷端 `ColdStamps { delete_ts: Timestamp }`（8 字节）：物理 MVCC 副本，
  `Timestamp::MAX` 表示存活；`create_ts` 只存放在版本权威
  （`edge_timestamps`）中，行内不保留。
- API 边界组装为 `Nbr`（32 字节）：`HotNbr + ColdStamps`。
- 空位填充一律用 `Nbr::dead_gap()`（不可分配边 ID + 空删除窗口），
  永不存活；越界扫描报缺席而非幽灵边。

共享删除状态机（`csr_shared.rs` 的 `decide_slot_delete`）：
异戳墓碑上再删报写写冲突；同戳再删幂等返回 `false`；
回滚窗口仅允许撤销发生在回滚点及之前的删除。
共享顶点容量增长（`grown_vertex_capacity`，1.25 倍向上取整）。

---

## 1. Multiple（`MutableCsr`）：通用多边行

### 用途

默认通用形态：每顶点可有多条出边，度数无界。

### 布局

```
主块（hot_list / cold_list 连续扁平，VertexBookkeeping 寻址）：
+-----------------------------------+
|V0 主块 | V1 主块 | V2 主块 | ...  |  adj_offsets / degrees / primary_capacities
+-----------------------------------+
     |
     +--> 溢出块（按顶点分段稀疏表，每满一块追加一块，从不拷贝旧块）
          +---> [chunk1] -> [chunk2]   分级定长块

零度行：延迟分配，主块无槽位；每行固定成本 12 字节
（offset + degree + capacity），溢出与存活索引按段稀疏分配。
```

### 数据结构

```rust
pub struct MutableCsr {
    hot_list: Vec<HotNbr>,        // 主块拓扑一半
    cold_list: Vec<ColdStamps>,   // 主块时间戳一半（等长步进）
    rows: VertexBookkeeping,      // adj_offsets / degrees / primary_capacities
    overflow_chunks: OverflowStorage,      // 按顶点分段稀疏溢出表
    overflow_chunk_edges: usize,  // 每溢出块边数
    live_sets: LiveSetStorage,    // 宽行存活端点集（窄行直接扫描，无需建集）
    tombstone_reuse_cutoff: Timestamp, // 热路径墓碑复用水位（内存态，不持久化）
    reuse_hint: Vec<u32>,         // 每顶点首个已知可复用槽提示（内存态）
    live_counts: Vec<u32>,        // 窄行存活计数
    tombstone_counts: Vec<u32>,   // 窄行墓碑计数
    primary_sorted: Vec<bool>,    // 主块键序缓存（内存态，不持久化）
    edge_count: u64,
    total_edge_capacity: usize,
}
```

### 写入逻辑

```
insert_edge(src, dst, edge_id, ts):
  1. 宽行查存活集 / 窄行扫描：存活 (endpoint, rank) 重复 -> EdgeAlreadyExists
  2. 主块有空位（含水位下可复用墓碑）-> 写主块
  3. 否则追加到溢出尾块；尾块满则新分配一块（定长，不倍增拷贝）
  4. 更新度数与 edge_count；新键使该行 primary_sorted 置假
```

删除走共享状态机盖 `delete_ts` 戳；空溢出块在移除路径上立即摘除，
不存在不可达块。按顶点块数上限触发写路径重排，
移除上报逐边回调 `(edge_id, delete_ts)` 以便调用方集中提升删除。

### 行序约定

主块在维护路径上按 `(endpoint, rank, edge_id)` 排序为有序前缀，
溢出尾部保持插入序为无序后缀；新写回填主块空位或 spill 到溢出尾，
行即重新标记为无序。阈值扫描走 `primary_sorted` 缓存：
有序主块二分键窗口，无序行与溢出后缀线性扫描。

### 操作复杂度

| 操作 | 复杂度 | 说明 |
|---|---|---|
| `insert_edge` | O(1) 摊销 | 宽行集合判重 O(1)，窄行扫描 O(degree) |
| `delete_edge`（按 ID） | O(degree) | 主块 + 溢出定位 |
| `get_edge` / `edges_of` | O(degree) | 仅供测试的戳过滤；生产走版本权威 |
| `compact_vertex_with_reporting` | O(行宽) | 行内回收，消除溢出 |
| `compact_with_ts_reporting` | O(V + E) | 整表重建，消除溢出链 |

---

## 2. Single（`SingleMutableCsr`）：一对一

### 用途

每顶点至多一条存活边：一对一关系（配偶、现雇主等）。

### 布局

```
直接下标数组（槽序号即行号，无偏移数组）：
+---+---+---+---+
| V0| V1| V2| V3|  hot_slots[i] + cold_slots[i]
+---+---+---+---+
```

### 数据结构

```rust
pub struct SingleMutableCsr {
    hot_slots: Vec<HotNbr>,     // 每顶点一槽拓扑
    cold_slots: Vec<ColdStamps>,// 每顶点一槽时间戳
    edge_count: u64,
}
```

空槽为 `dead_gap()`；单行天然有序（`is_row_sorted` 恒真）。

### 并发与冲突语义

本层无时间戳排序检查：槽内有存活边时第二次插入直接报
`Conflict` 错误（从不静默覆盖，调用方须先删后建）；
墓碑槽或空槽接受任意时间戳重建。快照可见性由上层版本权威裁决。

### 操作复杂度

| 操作 | 复杂度 |
|---|---|
| `insert_edge` | O(1) |
| `delete_edge` / `delete_edge_by_dst`（0 或 1） | O(1) |
| `get_edge` / `edges_of` | O(1) |
| 回收（`compact_vertex_with_reporting`） | O(1)：截止线下墓碑清槽上报 |

---

## 3. Pure（`PureTopologyCsr`）：纯拓扑

### 用途

无 rank、无时间戳、无属性的纯拓扑边：12 字节/边，
rank 恒为 0。

### 布局

结构镜像 `MutableCsr`（主块 + 分段稀疏溢出表 + 宽行存活集），
但每槽仅 `(endpoint: u32, edge_id: u64)` 两列，
无热冷时间戳一半。读时现场组装 `Nbr`
（`rank = 0`，`delete_ts = Timestamp::MAX`），不存不查 MVCC 状态。

物理删除用 `INVALID_EDGE_ID` 覆写边 ID，端点槽保留以维持
位置引用有效。窄行阈值以上（`LIVE_SET_WIDTH_BOUND = 8`）建集，
窄行直接扫描。

### 约束

- rank 非零的写入被拒绝（纯拓扑无 rank 列）；
- 有属性的表不能选此形态（属性无处存放）。

---

## 4. Bundled（`BundledCsr`）：拓扑 + 内联单标量

### 用途

恰好一个内联数值属性、读多写少、模式稳定的边类型：
20 字节/边（拓扑 12 + 值 8）。类型编解码集中在
`encode_scalar` / `decode_scalar`，行内只存 64 位裸词，
类型由发布模式在读时解析，行内不存类型。

### 布局

值列与拓扑列槽位平行：`primary_values` / `primary_valid`
紧随主块端点与边 ID 块；每个拓扑溢出块有等长平行的
`BundledOverflowValues` 块。所有拓扑变更走共享纯拓扑入口
（定位插入、定位删除）或逐槽镜像其移动
（`rollback_insert`、`compact_vertex_with_reporting`），
两列永不漂移。

删除槽保留陈旧裸词但清有效位；定位回滚恢复保留裸词。

### 边界（有意不再扩展）

- 冻结直接按紧凑布局打包，不再经临时通用表全量重建；
  带有效值的组冻结仍被拒绝，需冻结时先迁移到列式形态，
  该前置检查在冻结规划阶段即报错并指引迁移；
- 多属性、不可编码类型、在线模式变更归列式形态——
  本形态限定单列，不再加列。
- 拓扑遍历与值读取必须配对：行迭代器只产出拓扑，
  值经 `visit_physical_with_values` 或 `bundled_value_*` 配对读取，
  单独遍历拓扑会静默丢值；范围查询按端点区间构造，排序键半键被显式忽略。

---

## 5. Frozen（`ImmutableCsr`）：冻结紧凑组

### 用途

读为主的冻结组：单段连续邻居 + 按行度数表。
空行无槽位；无容量数组、无溢出链、无存活索引、无锁：
冻结组仅付条目加两张小按行数组的成本。

### 布局

```
hot_entries / cold_entries 按行首尾相连，每行按
(endpoint, rank, edge_id) 排序；degrees[row] 为行长，
offsets[row] 为行内起始（内存态，打包与加载时重建，不持久化）。
```

冻结保留被打包可变组的全部邻居字节（含边 ID、删除戳与墓碑），
仅丢弃无边的预留空位哨兵。冻结改变物理布局与行序，
时间戳过滤读在前后观察到相同逻辑内容，而非相同字节序。
可见性权威仍在上层，行戳仍是物理副本。

点查在键区间内二分，返回区间内首个时间戳可见版本；
扫描在有序行上线性进行。所有变更入口拒绝写入：
冻结组须显式解冻回可变形态后方可再写，写路径无隐式解冻。
`Pure` / `Bundled` 组冻结直接按目标紧凑布局打包，
内容与经临时表重建一致但无全量拷贝与伪造时间戳；
逻辑内容在冻结前后一致，只允许物理顺序变化。

---

## 6. Mapped（`MappedFrozen`）：内存映射服务视图

### 用途

与 `Frozen` 相同只读内容的内存映射视图：查询直接走 mmap
的扁平列文件按需分页，而非 open 时把整组解码进堆。

### 文件布局（小端）

- 魔数（u32）、行数 / 条目数 / 存活边数（u64 × 3）
- 五组 `(offset u64, length u64)` 列描述子：
  度数、端点、rank、边 ID、删除戳
- 列区：`rows` 个 u32 度数；`entries` 个 u32 端点、
  i64 rank、u64 边 ID、u64 删除戳
- 尾部 CRC32（覆盖之前全部字节，与堆检查点同校验模式）

列宽固定，任意槽一次小端解码即可寻址，无需全文件解码；
行偏移在 open 时内存重建，不持久化。

### 服务缓存状态机（派生缓存，检查点为准）

| 状态 | 含义 |
|---|---|
| 缺席 | 无边车文件；冻结/映射基在冲盘时缺则补，可变基保持缺席 |
| 有效 | 边车通过校验且与基一致；加载直接映射，跳过权威解码 |
| 陈旧 | 基被重写为可变或组已消失；冲盘删边车，读路径不可见 |
| 过期 | 边车校验失败或有只读视图无法吸收的追加增量；回退权威基并重建，计数告警而不静默、不失败加载 |

句柄引用计数（`Arc<Mmap>`）：克隆共享同一映射，
持视图的读者在边车替换期间保持旧文件存活。
`clear()` 语义特殊：映射在堆外无法清空，组回退为 `None` 占位
并保留顶点容量；该类型跃迁由组层显式承载，后续冻结、回收、
属性分支一律按占位处理，不得按原类型调度。
持久化不对称：映射与堆共享标签 3 转储权威字节，
加载一律重建堆内冻结形态，映射身份不持久；
检查点加载优先经边车重开映射，失败则回退权威基。

---

## 7. None：占位

### 用途

模式存在但该方向不存边。仅存顶点容量，不存边。

```rust
None { vertex_capacity: usize }  // 仅容量，无边
```

### 行为

| 操作 | 结果 |
|---|---|
| `edge_count()` | 0 |
| `insert_edge()` | 报拒绝错误（`invalid_operation`，写路径到此即判错） |
| `delete_edge()` | 报拒绝错误（同上） |
| `delete_edge_by_dst()` | 0（无边可删） |
| `get_edge()` | `None` |
| `edges_of()` / 遍历 | 空 |
| 内存 | `sizeof(usize)` 量级 |

注意 `EdgeSchema::validate` 拒绝任一方向为 `None` 的表，
因此 `None` 只出现在组级回退（如 `Mapped` 清空后）与
单测构造中，不出现在正常建表的持久模式里。

### 序列化

```
dump(): [0u8, vertex_capacity (8 字节小端)]
load(): 反序列化容量，重建 None 变体
```

---

## Trait 实现矩阵

| Trait 方法 | Multiple | Single | Pure | Bundled | Frozen | Mapped | None |
|---|---|---|---|---|---|---|---|
| `vertex_capacity` | 有 | 有 | 有 | 有 | 有 | 有 | 有 |
| `edge_count` | 有 | 有 | 有 | 有 | 有 | 有 | 有（0） |
| `dump` / `load` / `dump_into` | 有 | 有 | 有 | 有 | 有 | 有（读边车/堆） | 有 |
| `insert_edge` | 有 | 有 | 有 | 有（经拓扑） | 拒绝 | 拒绝 | 拒绝（报错） |
| `delete_edge` 系列 | 有 | 有 | 有（物理） | 有（经拓扑） | 拒绝/0 | 拒绝/0 | 报错/`Ok(false)`/0（按入口） |
| `get_edge` / `edges_of`（测试原语） | 有 | 有 | 有（组装） | 有（组装，值需配对读取） | 有 | 有 | 空 |
| `visit_physical` / `fill_physical_into` | 有 | 有 | 有 | 有（拓扑，需配对读值） | 有 | 有 | 空 |
| `compact_vertex_with_reporting` | 有 | 有（清槽） | 有 | 有（双列） | 无操作 | 无操作 | 无操作 |
| `compact_with_ts_reporting`（整表） | 有 | 有 | 无操作 | 无操作 | 有（堆内） | 无操作 | 无操作 |
| `used_memory_size` | 有 | 有 | 有 | 有 | 有 | 有 | 有 |

## 选型对照：何时用谁

| 场景 | 记录形态 + 变体 | 原因 |
|---|---|---|
| 好友、关注（通用多边） | Columnar + `Multiple` | 默认，最灵活 |
| 配偶、现雇主（一对一） | Columnar + `Single` | O(1)，内存省，冲突显式报错 |
| 无属性纯拓扑大图 | `Pure` | 12 字节/边，最省 |
| 单数值属性且读多写少 | `Bundled` | 20 字节/边，免属性表一次跳转 |
| 读为主、长期不变的组 | `Frozen` / `Mapped` | 紧凑有序行；mmap 视图免全量解码 |
| 模式存在但该方向无边 | `None`（组级占位） | 零开销 |
