# OLAP 能力提升改进分析

> 关联文档：
> - `docs/analysis/计划节点类型对比分析.md`（§5.3 因子化缺口 / §7 功能缺口）
> - `docs/analysis/linkrs_vs_ladybug_存储对比分析.md`（§5.7/§4 性能定性 / §6.2 核心矛盾）
> - `docs/analysis/因子化_重命名_扩展机制引入影响分析.md`（因子化引入成本）
> - `docs/plan/plan_node_remaining_issues_improvement_analysis.md`（P3 暂缓因子化）
> 核验基线：`crates/graphdb-storage/src/storage/edge/property_table.rs` / `csr_variant.rs:227` / `crates/graphdb-query/src/query/executor/streaming/` / `spec.rs:355` / `arena_builder/` / `benches/`

---

## 1. OLAP 负载特征与 linkrs 差距

**OLAP 典型**（LDBC SNB、图分析）：全量/大范围扫描、多跳 `Expand` + `Join` + `Aggregate`、高基数去重、排序/TopN、并发只读。

| 维度 | OLAP 需求 | linkrs 现状 | Ladybug 参照 |
|------|-----------|-------------|-------------|
| 数据扫描 | 仅读所需列、批量、零拷贝 | 边属性**行存** `PropertyTable`（整行读，`linkrs_vs_ladybug:3.3`），顶点列存已部分列存；`CsrVariant` 两级 `Multiple/Single` + overflow 有碎片 `csr_variant.rs:227` | 列式 `Column` + 密度自适应 CSR + Arrow 零拷贝 |
| 计算模型 | 向量化 2048/批 + 因子化压缩 + 多核 | 流式 `tuple-at-a-time` `SlotLayout`（`spec.rs:144`），`RecursiveFragmentSpec:950` 已原生但仍逐行 | 向量化 + 因子化 `factorized_table` + `semi_masker` + morsel 并行 |
| 并行 | morsel 并行、NUMA 感知 | `Exchange:Concatenate/RepartitionHash` 仅进程内分区，非 morsel；`data_store.rs:41` 整表 `RwLock` 写串行读阻塞 | morsel 并行 `docs/morsel_parallelism.md` |
| 数据接入 | `COPY FROM` 并行 CSV/Parquet | 仅 OLTP `InsertVertices/Edges` `plan_node_enum.rs:225` | 并行 CSV reader + Parquet + `COPY TO` |
| 索引/裁剪 | ART、zone maps、谓词下推 | BTreeMap 内存索引；列段 min/max 未用于谓词下推 | ART + min/max 下推 + `NODE_LABEL_FILTER` |
| 验证 | LDBC SNB 端到端 | Criterion 微基准 `benches/storage_bench.rs` | LDBC 数据集 + expected answers |

**结论**：linkrs 在 OLTP + 时序（`MVCC time-travel`）上领先，OLAP 天花板由**存储行存 + 流式 + 单线程**三重限制，需系统性改进，非补单一节点可得。

---

## 2. 需引入的改进（按层）

### 2.1 存储层 — 最高杠杆，需先行

| 改进 | 现状 | 目标 | 影响 | 成本 |
|------|------|------|------|------|
| **边属性列存化** | `property_table.rs` 行存 | 每属性一列，列段独立压缩（ALP/bitpacking/dict，同顶点 `ColumnStore`），列裁剪 + 谓词下推 | 扫 2 列 vs 整行，IO 降 5-10 倍 | 2-3 周，破坏性变更，需迁移工具 |
| **CSR 零拷贝/密度自适应** | `CsrVariant` 6 变体 + overflow zombie `fragmentation_stats.rs` | 单一密度自适应 + `PackedCSRInfo` 重分布 + Arrow 扫描 | 大度顶点遍历少间接，cache 友好 | 1-2 周 |
| **BufferManager** | 段 `residency` + Moka 缓存，无全局页池 | vmcache 风格 `BufferManager`（mmap + `MADV_DONTNEED` + 乐观读）`linkrs_vs_ladybug:3.6` | 全局内存复用，OLAP 大扫不 OOM | 1-2 周 |
| **zone maps / 统计** | 段统计缺失 | 列段 min/max/ndv 持久化，`ShowStats` 扩展 | 优化器基数估计 + 下推 | 0.5 周 |
| **细粒度并发** | `data_store.rs:554` 整表 `RwLock` | partition/stripe 锁或 MVCC 无锁读 | 解除 OLAP 读被写阻塞 | 1 周 |

**为何先行**：因子化/向量化均依赖列存零拷贝，否则"因子化"空转。

### 2.2 执行层 — 决定 OLAP 峰值

| 改进 | 现状 | 目标 | 影响 | 成本 |
|------|------|------|------|------|
| **向量化批次** | 逐行 | `DataChunk:2048` 批量 + `ListVector` 嵌套 | 分支预测友好，SIMD 友好 | 2 周 |
| **因子化** `SEMI_MASKER/MULTIPLICITY_REDUCER` | `Dedup/Filter` 事后 | `FactorizedTable` + 半连接掩码在 `Expand` 期裁剪 | 多跳 `3..5` 中间结果不物化 | 2-3 周（依赖 2.1） |
| **morsel 并行** | `Exchange` 进程内 | morsel 调度 + work-stealing，`Sort/Aggregate/Expand` 并行 | 多核线性扩展 | 2 周 |
| **Hash 聚合/排序优化** | 通用 `Aggregate/Sort` | 分区预聚合 `PartialAggregate/FinalAggregate` `spec.rs:289` 已定义但未并行化 + `RadixSort` | `GROUP BY` 大分组加速 | 1 周 |
| **Spill/内存池** | 无 spill | 算子级内存池 + 外排（Sort/Join/Agg 阈值 spill） | 大查询不 OOM | 1 周 |

### 2.3 规划/优化层

- **CBO 增强**：`optimizer/cost_based/row_estimates.rs` 按列存 ndv/zone maps 重算基数；因子化代价模型（压缩因子 vs 行数）。
- **Join 重排**：`join_order/join_plan_solver` DP（Ladybug `subplans_table.h`）替代当前启发式。
- **谓词/投影下推**：`Filter` 下推至 `StorageScan` + 列裁剪（与 2.1 联动），`PlanNodeCategory::Access.is_leaf()` 已具备语义。
- **`Subgraph` 节点**：`nebula kSubgraph` 等价，补 `AllPaths` 之外的子图抽取（`plan_node_category_analysis.md:230` 未实现）。

成本：1-2 周。

### 2.4 数据接入/运维层

- **`COPY FROM CSV/Parquet` 并行导入**：`SourceSpec:71 StorageScan` 复用 + `csv/parquet` crate 并行解析 + `COPY TO` 导出，`ParallelCsvReader` 参照 Ladybug `reader/csv/`。成本 3-5 天，OLAP 建图刚需。
- **LDBC SNB 基准套件**：`dataset/` + `queries/` + expected answers，替代微基准，成本 1 周但为唯一可复现验证。

### 2.5 索引/扩展（可选）

- **ART 索引**替代 BTreeMap（范围/前缀），与 Ladybug 一致。
- **GDS 函数库** `function/gds` 轻量版（PageRank/BFS）以纯函数形式，非 `INSTALL EXTENSION` 动态库（见扩展机制分析 `P3 不改`）。

---

## 3. 引入顺序（依赖驱动）

```
Phase 0 基准（1 周）
  └─ LDBC SNB SF1 数据导入 + 3 条多跳/聚合查询 bench，确立基线（否则无法量化收益）

Phase 1 存储（3-4 周，必做）
  ├─ 边属性列存化 + 压缩
  ├─ 细粒度锁（解除写阻塞读）
  └─ zone maps + 统计

Phase 2 接入（1 周，可并行）
  └─ COPY FROM CSV 并行

Phase 3 执行向量化（2-3 周）
  ├─ DataChunk 批量 + 零拷贝 CSR 扫描
  └─ morsel 并行（Sort/Agg/Expand）

Phase 4 因子化（2-3 周，条件）
  └─ FactorizedTable + SEMI_MASKER（需 Phase1 列存）

Phase 5 CBO/下推（1-2 周）
  └─ 基数估计 + Join DP + 谓词下推
```

**总计**：最小 OLAP 可用（Phase 0-3）约 **6-8 周**；完整因子化约 **9-11 周**。单独引入任一层（如仅因子化）收益有限，需按序。

---

## 4. 优缺点与风险

| 改进 | 优点 | 缺点/风险 |
|------|------|----------|
| **存储列存化** | 撬动所有上层（向量化/因子化/下推），IO 降数倍，最值得 | 破坏性变更，需数据迁移；`freeze.rs:120` 等脆弱路径需重验证 |
| **向量化+morsel** | 多核扩展性立竿见影，LDBC 查询 3-5 倍 | 需重写算子，`unsafe` 边界增多（`docs/archive/unsafe.md`） |
| **因子化** | 多跳分析天花板质变 | 依赖列存，复杂度最高，仅分析负载受益，OLTP 零收益 |
| **COPY/LDBC** | 立即可验证，投入小 | 非查询加速本身，但缺之则无法证明 OLAP 提升 |

**不做的影响**：维持现状 = OLAP 保持"流式 + 行存 + 单线程"天花板，仅适合 OLTP/小图分析，无法承载 SNB 规模。

---

## 5. 最小可行建议

若目标为"**可演示 OLAP**"而非"对标 Kuzu"：

1. **必做**：Phase 0 基准 + Phase 1 边属性列存 + 细粒度锁 + Phase 2 `COPY FROM CSV`（约 4-5 周），即可让 `MATCH (a)-[:KNOWS*2]->(b) RETURN count` 等查询从"慢"变"可用"。
2. **次优**：Phase 3 向量化批次 + morsel（再 2 周），多跳聚合进入 Ladybug 的 30-50% 水平。
3. **暂缓**：完整因子化（待 Phase 1-3 验证后，按 SNB 最慢查询决定是否值得）。

> 与 `plan_node_remaining_issues_improvement_analysis.md:102` 一致：因子化单独立项不值得，需存储先行；OLAP 能力是系统性工程，应以基准驱动分阶段，避免为单一算子重构偏离轻量定位。
