# CSR 顶点 ID 空间问题：Ladybug (Kuzu) 的处理方式与 linkrs 改进方案

> 本文基于 `linkrs/docs/analysis/csr_vertex_id_space.md` 描述的问题，对照 Ladybug（Kuzu 更名版，C++）存储层实际源码，分析其如何规避"稀疏内部 ID 直接作 CSR 行号"这一类问题，并给出 linkrs 的具体改进建议。所有结论均基于两个仓库的实际代码核实。

---

## 1. 问题回顾与 linkrs 现状核实

文档描述的四层放大在代码中全部核实成立：

| 缺陷 | 代码落点 | 核实结果 |
|---|---|---|
| ID 非稠密（16× 放大） | `vertex_table/sharded.rs` `encode_id`: `(local_id << 4) \| shard`，`SHARD_BITS = 4` | ✅ 成立，encode/decode 为文件私有 |
| 序号不复用 | `id_indexer.rs` `IdManager::insert`: `index = keys.len()`；`remove` 仅置 `None` | ✅ 成立，空洞永久化 |
| 幂等扩容 + 固定预分配 | `mutable_csr.rs` `ensure_vertex_capacity`: `next_power_of_two`；每行 4 个 64B `Nbr` 主槽 + 24B 空溢出 Vec ≈ 292B | ✅ 成立 |
| 无行向收缩 | `compact_with_ts` 循环 `0..vertex_capacity()` 重建，行数不变 | ✅ 成立（注：compact 会把零度行主块从 4 槽缩到 1 槽，有"列向"收缩，但行数不缩） |

核实中还发现文档**未提及的两个事实**：

1. **顶点侧已有重映射雏形但与边表脱节**：`IdManager::compact`（`id_indexer.rs`）已实现"排序重排 + 返回 old→new 映射"，并由 `vertex_table/optimizer.rs` / `compaction.rs` 的 `propagate_remap` 消费——但映射只传播到顶点侧列存储和时间戳，**不传播到边表 CSR 行号**。这不仅是浪费问题：一旦顶点压缩被触发，边表中的旧内部 ID 将全部失效，是**潜在的正确性 bug**。
2. **双 CSR 双倍放大**：`edge_table/core.rs` 同时维护 `out_csr` 和 `in_csr`（以及成对的 out/in 冻结段），所有行空间成本 ×2。
3. **冻结路径更糟**：`freeze.rs::freeze_delta` 中 `effective_capacity = max(delta.vertex_capacity(), max_vid + 1)`，段容量不仅继承可变 CSR 行数，遇更大 ID 还会进一步放大。

---

## 2. Ladybug 如何处理同一问题

Ladybug 从架构上根本不会出现这个问题，其防御由五层设计叠加构成。

### 2.1 内部 offset 严格稠密：来自计数器，而非编码

节点的内部 ID（`nodeOffset`）就是**表级全局行计数器**：

- `node_group_collection.cpp::appendToLastNodeGroupAndFlushWhenFull()`：`startOffset = numTotalRows`，插入后 `numTotalRows += numToAppend`；
- `node_table.cpp::commit()`（L611-653）：事务本地数据同样以 `getNumTotalRows()` 起点分配连续 offset。

offset 中**不编码任何分片/哈希信息**，0 起、单调、连续。"ID 数值 ≈ 行数"是不变式，而 linkrs 的 "ID 数值 = 16 × 片内序号" 从一开始就破坏了它。

### 2.2 外部主键与内部 offset 之间有显式映射层

任意稀疏的用户主键（int64 / string）由 `PrimaryKeyIndex`（磁盘可扩展哈希索引，`src/storage/index/hash_index.h/.cpp`）完成 `PK → offset_t` 映射：

- `NodeTable::insert()` 先 `validatePkNotExists` 再写索引；
- 查询侧 `lookup(key, offset_t& result)`。

即：**稀疏性被彻底隔离在哈希索引一层，存储布局（列存、CSR）只见稠密 offset**。这正是 linkrs 文档 4.1 节所说的"逻辑句柄与物理行号显式建映射"——但 Ladybug 是把映射放在**顶点表入口**（方案 C+映射层），而非边表内部（linkrs 的方案 A）。

### 2.3 Node Group 分组：行空间上界固定、按需创建

- 所有表按 `NODE_GROUP_SIZE = 2^17`（131072）个 offset 一组切分（`system_config.h.in`），`nodeGroupIdx = offset >> 17` 纯位运算定位；
- rel table 的 CSR 按 bound node 的 offset 分组，node group **惰性创建**（`node_group_collection.cpp::getOrCreateNodeGroup`）——没有边的 offset 区间根本不分配 CSR 结构；
- 组内 CSR header 行数由**实际最大有边 offset** 决定：`csr_node_group.cpp` L1040 `numNodes = csrIndex->getMaxOffsetWithRels() + 1`，而不是固定物化满 2^17。

对比 linkrs：`next_power_of_two` 是**全表单一数组**一次跳到 2^31 的根源；Ladybug 单组上界 2^17，且"插入一条 offset=10 亿的边"只会创建第 7629 号 node group 这一个组，不影响其他区间。

### 2.4 CSR 行空间成本近零：header 两列 + 列压缩

Ladybug 的 CSR 每行成本不是 292B，而是两个 UINT64 列（`ChunkedCSRHeader{offset, length}`，`csr_chunked_node_group.h`）：

- 零度顶点：`length = 0`，`start == end`，邻接数据列中占 **0 行**；
- header 列经 `ColumnChunk` 压缩（`IntegerBitpacking` / `CONSTANT`，`compression.h`），大段全零的 length 列可压成常量，**每行实际成本远小于 8B**；
- 没有任何"每行预物化邻居槽位"——linkrs 的 256B/行主块槽在 Ladybug 中不存在。

### 2.5 PMA 式 CSR gap：用受控空隙替代每行预分配

Ladybug 为吸收未来插入预留的是**区间级空隙**而非行级槽位（Packed Memory Array 思想）：

- 常量：`PACKED_CSR_DENSITY = 0.8`、`LEAF_HIGH_CSR_DENSITY = 1.0`（`constants.h` L80-81）；leaf region 大小 2^10 个节点；
- `computeGapFromLength`：每个 leaf region 末尾留 `len/0.8 - len` ≈ 20% gap（`csr_chunked_node_group.cpp`）；
- checkpoint 时构建 calibrator 树（高度 = 17−10 = 7，`PackedCSRInfo`），密度阈值随层级从 1.0 线性收紧到 0.8；某 region 插入超密度则升级到父区间局部重排（`mergeRegionsToCheckpoint` / `isWithinDensityBound`），触顶才全组 `redistributeCSRRegions`；
- 只重写受影响 region 的列段，摊销重排成本。

即 gap 与**实际边数成正比**（20%），而 linkrs 的预分配与**行数**成正比（每行 292B，零度行同样占满）。

### 2.6 删除与回收

- **顶点 offset 不复用**：`NodeTable::delete_()` 只在 `VersionInfo` 打标记，注释明确说明不能搬移行否则会破坏边引用。这点与 linkrs 相同——但因为 offset 稠密、node group 惰性、header 近零成本，"计数器不回退"的代价可忽略；
- **页级空间回收**：checkpoint 时 `reclaimStorage()` → `FreeSpaceManager`（按 2 的幂分级 free list）回收删除行的页空间供复用；
- **边删除**：内存态 `csrIndex->setInvalid()` 标记；checkpoint 重写 region 时物理剔除，旧页归还 FreeSpaceManager。

### 2.7 小结：为什么 Ladybug 没有这个问题

| 维度 | linkrs 现状 | Ladybug |
|---|---|---|
| 内部 ID 来源 | `(local<<4)\|shard` 编码，稀疏 | 全表行计数器，稠密 |
| 稀疏外部 ID 隔离 | 无（IdIndexer 只做片内映射，编码引入新稀疏性） | PK 哈希索引，稀疏性不进入存储层 |
| 行空间粒度 | 全表单数组，`next_power_of_two` | 2^17/组，按需创建，组内按 max 有边 offset 截断 |
| 每行固定成本 | ~292B（×2 双 CSR） | 2×8B 列，可压缩至近零 |
| 插入预留 | 每行 4 槽（与行数成正比） | region 级 20% gap（与边数成正比） |
| 删除回收 | 无（标记 + 序号不复用） | offset 不复用但页空间经 FreeSpaceManager 回收 |

**核心教训：行号必须来自稠密分配器，稀疏标识只能经映射层进入存储布局；预留空间应与数据量成正比，而不是与 ID 空间成正比。**

---

## 3. linkrs 改进建议

### 3.1 对文档三个候选方案的重新评估

Ladybug 的实证结果修正了文档 4.2 的方案取舍：

- **方案 A（边表持稠密行映射）**：文档推荐，但 Ladybug 的做法说明这不是业界主流。在边表内做 `内部ID → 行号` 哈希映射，意味着**每次 CSR 访问都多一次哈希查找**，且 out/in 两个 CSR、每个冻结段、每个 MVCC 快照都要各自维护或共享映射与反查数组，复杂度分散在所有热点路径。
- **方案 C（内部 ID 完全稠密化）**：文档因"放弃分片写并发隔离"而弃选，但这个顾虑站不住——Ladybug 用一个表级计数器分配 offset 也支撑了多核并行写（分配是 O(1) 原子操作，真正的并发瓶颈在数据写入而非 ID 分配）。linkrs 完全可以**保留 8 分片的物理写隔离，只改 ID 分配为全局稠密**：分片选择继续按外部 ID 哈希，但内部 ID 从全局 `AtomicU32` 计数器取号，分片内部维护 `内部ID → 片内槽位` 即可（或干脆让片内序号 = 全局号，分片只是锁域）。
- **方案 B（局部缓解）**：其中"主块预分配归零"和"扩容步进改比例"与稠密化不冲突，可作为叠加优化。

**修订建议：以"方案 C + 显式映射层"为主线（即 Ladybug 路线），叠加方案 B 的每行成本削减，而非文档推荐的方案 A。**

### 3.2 分阶段改造路径

**阶段一：内部 ID 稠密化（根治 16× 放大）**

1. `sharded.rs`：废除 `(local<<4)|shard` 编码。新增表级 `AtomicU32` 分配器，`insert` 时取全局稠密 ID；分片仍按外部 ID 哈希选择（锁域不变），片内 `IdIndexer` 维护 `外部ID → 全局稠密ID`；
2. 由于 encode/decode 本就是 `sharded.rs` 私有函数、全库把内部 ID 当不透明 u32，此改动的外溢面与第一轮修复相当；
3. `decode_id` 的反向需求（由内部 ID 找分片）需补一个 `稠密ID → shard` 查找——可用一个全局 `Vec<u8>`（每顶点 1 字节，随稠密 ID 追加）或直接在 ID 里保留低位分片但改为"分配时全局取号、分片号仅作路由缓存"。

**阶段二：削减每行固定成本（292B → ~12B）**

1. `mutable_csr.rs`：`DEFAULT_VERTEX_DEGREE` 主块预物化归零，首条边到达时才分配主块（参考 Ladybug"零度顶点占 0 行数据"）；
2. `overflow_chunks: Vec<Vec<Vec<Nbr>>>` 的空 Vec 24B/行可改为 `Option<Box<...>>`（8B）或独立稀疏 map；
3. `ensure_vertex_capacity` 的 `next_power_of_two` 改为比例步进（如 1.25×）——ID 稠密化后行数 ≈ 顶点数，幂扩容的尾段浪费上界从"数亿行"降为"顶点数的 25%"。

**阶段三：行空间分组（对齐 node group 思想，可选但推荐）**

将全表单一行数组改为按 `内部ID >> K`（如 K=16，每组 65536 行）分组、组惰性创建、组内按最大有边行截断。收益：
- 与冻结段天然对齐（freeze 可按组进行，`effective_capacity` 不再被单个大 ID 拖大）；
- 冷快照 `.lkcs` 文件尺寸随实际数据收缩；
- 为将来 region 级 gap/重排（PMA）留好结构位。

**阶段四：打通删除回收与顶点压缩**

1. 短期：修复 `IdManager::compact` 的重映射不传播到边表的问题——要么在 `propagate_remap` 中同步重建 out/in CSR 行号，要么在边表未同步前禁止顶点压缩触发（当前是正确性隐患，优先级应高于内存优化）;
2. 长期：稠密 ID 下顶点删除可效仿 Ladybug——offset 不复用（保证边引用稳定），但 compact/merge 重建时按存活顶点重排行号并同步更新映射，空间在重写时物理回收。

### 3.3 量化预期

以 100 万顶点、平均度 8 为例（单边表，out+in 双 CSR）：

| 项 | 现状 | 阶段一后 | 阶段二后 |
|---|---|---|---|
| CSR 行数 | ~2^25（16× + 幂扩容） | ~2^21（幂扩容尾段） | ~1.25M（比例步进） |
| 行固定成本 | 292B × 行数 × 2 ≈ **19.6GB** | ≈ 1.2GB | ~12B × 1.25M × 2 ≈ **30MB** |
| 冻结段/冷文件行空间 | 同步放大 | 同步收缩 | 同步收缩 |

（现状列按 2^25 行估算对应文档 3.2 的实测跳变规律；实际数值取决于写入历史。）

### 3.4 验证路径

沿用文档 4.3 的建议，补充两点：

1. 微基准中增加"稀疏写入模式"（只写少量高号顶点的边），这是现状 vs 稠密化差距最大的场景；
2. 增加"顶点压缩触发后边表查询正确性"回归用例——覆盖 3.2 阶段四第 1 点的现存隐患。

---

## 附：关键源码索引

| 主题 | Ladybug 位置 | linkrs 位置 |
|---|---|---|
| 稠密 offset 分配 | `src/storage/table/node_group_collection.cpp` (`appendToLastNodeGroupAndFlushWhenFull`) | `sharded.rs` `encode_id`（对照物） |
| PK → offset 映射 | `src/storage/index/hash_index.cpp`, `hash_index.h` L343 | `id_indexer.rs`（仅片内） |
| Node group 切分 | `storage_utils.h` L59-62; `system_config.h.in`（NODE_GROUP_SIZE_LOG2=17） | 无 |
| CSR header | `csr_chunked_node_group.h` L97-130 (`ChunkedCSRHeader`) | `mutable_csr.rs` L87-98（每行四件套） |
| CSR gap / PMA | `csr_chunked_node_group.cpp` (`computeGapFromLength`, `populateStartCSROffsetsFromLength`); `csr_node_group.cpp` (`mergeRegionsToCheckpoint`, `redistributeCSRRegions`); `constants.h` L80-81 | 无（每行 4 槽预分配替代） |
| 行数截断 | `csr_node_group.cpp` L1040 (`getMaxOffsetWithRels() + 1`) | 无（`next_power_of_two`） |
| 空间回收 | `free_space_manager.cpp`（分级 free list） | 无 |
| 删除语义 | `node_table.cpp::delete_`（标记，offset 不复用） | `id_indexer.rs::remove`（同为标记） |
