# 内部 ID 稠密化与 CSR 行空间改造设计（阶段复盘与修订）

> 前置文档：
> - `docs/analysis/csr_vertex_id_space.md` — 问题背景、代码上下文、量化分析（问题本身）
> - `docs/analysis/csr_vertex_id_space_ladybug_对比分析.md` — Ladybug (Kuzu) 对照与改进建议
>
> 本文为本设计的**实施后复盘**：确认阶段一~三的落地情况，评估原阶段四方向，并给出修订后的后续计划。所有论断均经 linkrs 源码核实（`git log` 至 f46938e）。

---

## 1. 决策结论（修订）

**主线决策保持不变**："内部 ID 完全稠密化"（方案 C）+ "每行固定成本削减"（方案 B），放弃"边表持稠密行映射"（方案 A）。分段编码、Lazy 主块、稀疏溢出、比例扩容均按原设计落地。

**修订点**：
1. 原阶段四（行空间分组）**方向不再成立，予以否决**——其前提已被阶段一、三消除，收益上限 ≤25% 常数因子，成本是全路径间接寻址（详见第 4 节）；
2. 以**"收尾与验证"阶段**（新阶段四）替代：补齐两处残留扩容、一处冻结段容量、一处冷快照持久化缺口，并补上原文档承诺但未做的微基准；
3. 行空间分组**整体搁置**，记录重启触发条件，不删除设计知识。

---

## 2. 阶段一~三完成情况核实

### 2.1 阶段一：号段式编码 —— ✅ 已完成

`crates/graphdb-storage/src/storage/vertex/vertex_table/sharded.rs`：

| 设计项 | 实现位置 | 状态 |
|---|---|---|
| ID 布局 `(segment << K) \| slot`，K=12 | `sharded.rs:30-32`（`SEGMENT_SLOTS_BITS`） | ✅ |
| 分片交错段号 `segment = shard + ordinal × num_shards` | `encode_id`（:34-39）、`decode_id`（:41-47）纯位运算 | ✅ |
| 每分片 `local_counter`（段内取号） | `Shard.local_counter: AtomicU32`（:53） | ✅ |
| `current_segment` 缓存 + 全局 `segment_allocator` | :56、:70；`record_allocation`（:138-148）段耗尽 `claim_segment` | ✅ |
| encode/decode 为文件私有 | 顶层 `fn encode_id/decode_id`，全库按不透明 u32 消费 | ✅ |
| load 后恢复分配水位 | `load`（:570-594）刷新三组计数器 | ✅ |
| 压缩后刷新分配水位 | `compact_with_ts_collect_mapping`（:520-543）刷新 `local_counter`/`current_segment` | ✅ |
| 单元测试 | `test_encode_decode_id`、`test_segment_allocation_spans_boundaries`、`test_load_resumes_allocation`、`test_id_uniqueness_across_shards` | ✅ |

### 2.2 阶段二：削减每行固定成本 —— ✅ 已完成（2 处残留）

`crates/graphdb-storage/src/storage/edge/`：

| 设计项 | 实现位置 | 状态 |
|---|---|---|
| 主块首边惰性分配（零度行占 0 槽） | `mutable_csr.rs:201-212`（`allocate_primary_block`）、:245-248 调用点；`test_zero_degree_rows_hold_no_slots`（:1262） | ✅ |
| overflow 稀疏化（独立存储，仅高密度顶点持有） | `mutable_csr.rs:105`（`HashMap<u32, Vec<Vec<Nbr>>>`） | ✅ |
| 比例步进扩容（1.25×，弃用 next_power_of_two） | `mutable_csr.rs:79/191-197`（`VERTEX_GROWTH_FACTOR`）、`single_mutable_csr.rs:78/152-158` | ✅ |
| **残留**：Labeled/MultiSingle 变体仍 `*2` 扩容 | `labeled_mutable_csr.rs:118`、`multi_single_mutable_csr.rs:272` | ❌ |

**实测每行固定成本**（Phase 1 稠密化后行数 ≈ 顶点数）：
- `MutableCsr`：12B/行（`adj_offsets` 4B + `degrees` 4B + `primary_capacities` 4B，均为 u32），双 CSR = 24B/顶点 —— **优于原文档 36B 目标**；
- 不可变 `Csr`（冻结段/冷快照）：4B/行（仅 `offsets` u32 数组，`csr.rs:47-51`）。

### 2.3 阶段三：删除回收与顶点压缩接线 —— ✅ 已完成（1 处持久化缺口）

| 设计项 | 实现位置 | 状态 |
|---|---|---|
| 压缩触发接线到维护路径 | `context/mod_maintenance.rs:50`（`compact_with_ts_collect_mapping`） | ✅ |
| old→new 映射传播到边表 CSR 行号 + 邻居键 | `edge/edge_table/remap.rs:164-229`（`remap_vertex_ids`：out/in CSR、冻结段、稀疏顶点索引、快照缓存、属性索引） | ✅ |
| 传播到冷快照 | `mod_maintenance.rs:112-124`（内存态） | ✅（见缺口） |
| 行空间截断为"最大有边行 + 1" | `remap.rs:69-84/131-149/264-266`（对齐 Ladybug `getMaxOffsetWithRels()+1`） | ✅ |
| 墓碑条目保留（时间旅行可见性） | `remap.rs:340-364` 测试 + `iter_all` 重建路径 | ✅ |
| 冻结段行容量按需 | `edge_table/freeze.rs:111-128`（`max(delta_capacity, max_vid+1)`） | ⚠️ 见残留 |
| **缺口**：冷 `.lkcs` 文件在内存 remap 后不重写 | `cold/cold_snapshot.rs:291-298` 注释自认："backing .lkcs must be re-exported" | ❌ |
| 压缩后行空间物理回收 + 分配水位重置 | `sharded.rs:537-540` + `id_indexer.rs:250`（`compact` 返回 old→new） | ✅ |
| 回归测试 | `remap.rs` 测试×7（行/邻居/段/墓碑/单向标签）、`core_tests.rs:419`、`cold_snapshot.rs:614-657` | ✅ |

### 2.4 影响面核实（原文档 4.3）

| 位置 | 结论 |
|---|---|
| `Vertex.id` 查询结果 / `add_vertex` 返回值 | 数值变为稠密，接口形状不变（`transaction/ops.rs:113`）；既有断言已更新，测试通过 |
| WAL / 恢复 | 存外部 ID，重放重新解析，零影响（`recovery.rs`） |
| 既有测试硬编码内部 ID | 已随重构提交更新（631 个 lib 测试通过） |

### 2.5 测试现状

`cargo test -p graphdb-storage --lib`：**631 通过 / 9 失败**，9 个失败均为**既存无关问题**：
- `schema_engine` 版本自增断言×5（schema 版本管理模块，与 ID 无关）；
- `index::tests::resolve_split_crash_recovery_*`×4（`tests.rs:239` 写 postcard、读时期待 "LNKF" 魔数——index generation 文件格式与测试不同步）。

---

## 3. 阶段四原方向评估：否决

原设计：全表单一行数组按 `内部 ID >> K'` 分组、组惰性创建、组内按最大有边行截断，收益为"冻结按组、冷文件随数据收缩、为 PMA 留结构位"。

### 3.1 前提已被消除

原分组的动机是"单个大 ID / 稀疏行空间拖大整表"。阶段一后最大行号 ≈ N + 8×2^K（常数），阶段三压缩后截断为"最大有边行+1"。**剩余行空间膨胀仅为常数因子**：
- 1.25× 扩容尾部（阶段二，≤25%）；
- 压缩间隔期内删除顶点暂留的行（有界，由压缩节奏控制）。

稠密 ID 下"组内按最大有边行截断"退化为无收益：每组的最高有边行 ≈ 组基址 + 组规模，组边界截断与整表截断等价。

### 3.2 宣称收益不成立

| 收益 | 评估 |
|---|---|
| 冻结按组进行 | freeze 是按时间批量 delta→段；ID 分组不提供任何冻结触发语义 |
| 冷文件随数据收缩 | 冷快照行空间已 ≈ N（offsets 4B/行）；分组最多省 1.25× 尾部，但每行访问需组查找 + 基址偏移 |
| PMA region 结构位 | 投机性；与现有 segment/merge/compact 架构冲突，且 linkrs 的"压缩+重映射+截断"模型已将行空间限定在常数因子内 |

### 3.3 成本

分组触及每一次行访问路径（查询、remap、freeze、merge、dump/load、冷格式），引入新的组边界失败模式，换来 ≤25% 的常数因子收益。**不划算。**

### 3.4 搁置触发条件（未来若满足其一再重启）

1. 内部 ID 重新变得稀疏（例如引入外部 ID 保留语义的插入路径）；
2. 冷存储迁移到 mmap/磁盘、需要按 region 粒度换页读取；
3. 单 label 顶点数达到 10^8 量级且实测 offsets 数组成为主导成本。

---

## 4. 修订后的阶段四：收尾与验证

### 4.1 完成阶段二残留：两种 CSR 变体比例扩容

- **改动文件**：`edge/labeled_mutable_csr.rs:118`、`edge/multi_single_mutable_csr.rs:272`
- **改动**：`resize((src_vid + 1).max(vertex_capacity * 2))` → `resize((min_capacity * 1.25).ceil())`，对齐 `VERTEX_GROWTH_FACTOR`。
- **验证**：两变体既有测试 + 新增行容量断言（行数 ≤ 1.25 × 最大有边行）。

### 4.2 冻结段严格截断

- **改动文件**：`edge/edge_table/freeze.rs:111-128`
- **改动**：段容量改为"本空间最大有边行 + 1"。当前 `max_vid` 同时取 src 与邻居（:111-118），但 out 段行空间只属于 src 空间、邻居是值——取 `max(src)` 即可，消除跨标签最大 ID 拖大段容量；同时弃用 `max(delta_capacity, …)` 继承的 1.25× 尾部。
- **注意**：in 段同理取 dst 空间最大行。
- **验证**：freeze/merge 既有测试 + 断言段容量 = max 有边行 + 1。

### 4.3 冷快照 remap 持久化

- **改动文件**：`cold/cold_snapshot.rs`、`context/mod_maintenance.rs:112-124`
- **方案**（二选一）：
  a. 维护路径中内存 remap 后**重写 `.lkcs`**（复用 `ColdSnapshot::create` 序列化路径）；
  b. 若冷快照视为可重建缓存，改为 remap 后**标记失效并从边表重新导出**（当前已有 `export_snapshot_file`，`context/mod_edge_ops.rs:166`）。
- **验证**：新增用例——压缩触发 → 冷快照查询正确 → 重启重载 `.lkcs` 后仍正确（roundtrip）。

### 4.4 稠密化微基准（原文档第 7 节承诺项）

- **改动文件**：`benches/storage_bench.rs`
- **新增基准**：稀疏高号外部 ID 写入模式（少量大号 ID 顶点，修复前后差距最大的场景），指标：CSR 行数、实际 RSS、插入耗时。
- **回归断言**（测试级）：任意写入序列后 `out/in CSR 行数 ≤ 1.25 × (最大有边行+1)`（已有雏形 `edge_table.rs:844-855`，扩展覆盖压缩与冻结路径）。

### 4.5 既存失败测试（记录，不在本阶段范围）

`schema_engine` 版本自增 ×5、index generation 文件格式 ×4（见 2.5）。建议单独开任务修复，与 ID 稠密化解耦。

---

## 5. 量化核对（实施后实测口径）

以 100 万顶点、平均度 8、单边表（out+in 双 CSR）为例：

| 项 | 原文档估计 | 实施后实测口径 |
|---|---|---|
| CSR 行数 | ~2^20 → ~1.25M | max 行号 ≈ N + 16K；行容量 = 1.25×(N+16K)（比例扩容，含尾部） |
| 每行固定成本 | 292B → ~36B | `MutableCsr` 12B/行；双 CSR = 24B/顶点（优于目标）；不可变 Csr 4B/行 |
| 冻结段行空间 | 同步收缩 | 冻结时继承 delta 容量（1.25× 尾部），合并/remap 后截断为 max 行 + 1 |
| 冷快照行空间 | 同步收缩 | = 导出时 CSR 行容量，≤ 1.25×(N+16K) |

---

## 6. 验证路径

1. `cargo test -p graphdb-storage --lib`（631 通过 / 9 既存失败，见 2.5）；
2. 阶段四各项：4.1/4.2 新增容量断言；4.3 roundtrip 用例；4.4 微基准 + 回归断言；
3. 全量 `cargo clippy --all-targets --all-features`。

---

## 7. 风险与开放问题

1. **跨 label ID 重叠**（既有）：不同 label 顶点表 ID 空间独立，段式编码不改变该现状；边表层 (label, id) 复合键或全局 ID 空间超出本次范围，记录待议；
2. **`Vertex.id` 语义正式化**：该字段仍为半公开"内部实现泄漏"，建议后续显式定义（稳定序列号 vs 移除），避免依赖数值；
3. **段式编码与顶点压缩交互**：压缩重排片内局部号时 slot 随之变化，必须走"重建边表行号"路径——已由 `remap_vertex_ids` 承担并测试（`remap.rs`）；
4. **K 参数**：12（4096/段）已落地；若未来需要微调，仅改 `SEGMENT_SLOTS_BITS` 并回归 `test_encode_decode_id`；
5. **冷快照持久化缺口**（4.3）：在修复前，内存 remap 后 `.lkcs` 与内存态不一致，重启后行号回退到 remap 前——是当前唯一已知的持久化一致性风险，优先处理。
