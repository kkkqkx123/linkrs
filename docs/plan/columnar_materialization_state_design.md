# 阻塞物化态列存化设计（R4）—— 与 `graphdb-storage` 列存重构合并立项

> 基线：`kkkqkx123/linkrs` @ `9eabc369`（main）
> 上游分析：`linkrs_query_analysis.md` §6.2 **R4**；整改主方案：`docs/plan/factorization_invariant_remediation.md` §6.2
> 定位：**下一阶段任务**（设计文档，尚未落地代码）
> 规范依据：`AGENTS.md` —— 轻量单节点、No-backward-compatible、代码零中文、模块化 + 严格 DAG
> 合并动因：物化态列存化与 `graphdb-storage` 列存重构共享"列式数据布局 + 选择向量"底座，**合并立项**避免两套列存抽象。

---

## 0. 结论速览

现状：流式路径全程列存 `DataChunk` / `ColumnarBatch`，但**所有阻塞物化态退回行存** `Vec<Vec<Value>>`（必要时再叠 `HashSet<Vec<Value>>` 去重）；`Flatten`（交叉积物化）也以行缓冲重放。这带来三类代价：**逐行分配开销、缓存局部性差、列存↔行存反复转置**。

本设计提出：**把阻塞物化态统一为列存 `MaterializedBatch`**，并让 `Flatten` 直接对列批次做选择向量重放。**关键决策：不新建独立列存层，而是复用/共建 `graphdb-storage` 的列存重构成果**，合并立项、共享 `ColumnBatch` 抽象与编解码。

| 维度 | 现状 | 目标 |
|------|------|------|
| 物化态存储 | `Vec<Vec<Value>>` 行存 | 列存 `MaterializedBatch`（列式 + 选择向量） |
| `Distinct` 去重 | `HashSet<Vec<Value>>` | 列式哈希（按列行哈希，避免整行构造） |
| `Flatten` 展开 | 行缓冲重放 `Vec<usize>` | 列批次选择向量重放 |
| 落盘 | 行存 run 文件（仅 Distinct） | 列式 run 文件（与 R3 落盘统一） |
| 与 storage | 无共享 | 共享 `ColumnBatch` 编解码与列类型系统 |

**一句话**：R4 不是"查询层自造列存"，而是"查询层阻塞物化**接入**存储层列存底座"，合并立项后一次投入、两处受益。

---

## 1. 现状事实（file:line）

### 1.1 阻塞物化态全是行存

`crates/graphdb-query/src/executor/streaming/operators/blocking/materialize.rs`：

| 状态结构 | 行存字段 | 行 |
|---------|---------|----|
| `DistinctState` | `seen_rows: HashSet<Vec<Value>>`、`partition_seen: HashSet<Vec<Value>>`、`output_iter: Option<IntoIter<Vec<Value>>>` | `:9-19` |
| `MaterializeState` | `materialized_rows: Vec<Vec<Value>>`、`result_iter: Option<IntoIter<Vec<Value>>>` | `:22-27` |
| `DataCollectState` | `all_rows: Vec<Vec<Value>>` | `:30-34` |
| `RollUpApplyState` | `all_rows: Vec<Vec<Value>>` | `:37-40` |

### 1.2 流式路径已是列存

- `DataChunk`：`crates/graphdb-query/src/executor/streaming/chunk/core.rs:13`。
- `ColumnarBatch`：`crates/graphdb-query/src/executor/streaming/chunk/columnar_batch.rs:1046`。
- 列式子系统就在同目录：`chunk/{columnar_batch,kind,policy,selection,typed,schema}.rs`。

即：**列存底座已存在**，只是阻塞算子在收口时"降级"成行存。

### 1.3 `Flatten` 也走行缓冲

`crates/graphdb-query/src/executor/streaming/operators/flatten.rs`：

- `prepare_flatten_buffer`（`:20`）准备 `(Vec<usize>, DataChunk)` 选择向量。
- `flatten_next_batch`（`:52`）持 `buffered_chunk: &mut Option<DataChunk>`，按 group_pos 选择向量**重放**为输出行。
- `build_flatten_batch_chunk`（`:110`）按 `positions: &[usize]` 重建 chunk。

**要点**：`Flatten` 已经是"列批次 + 选择向量"思路，只是行数展开处仍构造行。这里应深化，而非重写。

### 1.4 落盘是行存 run 文件

`crates/graphdb-query/src/executor/streaming/spill.rs`：

- `RunWriter::write_row(&[Value])` / `write_rows(&[Vec<Value>])`（`:311` / `:326`）——行存序列化。
- `RunReader::read_row() -> Option<Vec<Value>>`（`:491`）、`read_all() -> Vec<Vec<Value>>`（`:511`）。
- run 文件头 `RunHeader`（`:111`）+ 魔数 `GRSP`（:53）。
- 仅 `Distinct` 真正落盘；`Materialize`/`DataCollect`/`RollUpApply` 走 `spill_not_supported`（见 R3）。

### 1.5 storage 侧已有列存

`crates/graphdb-storage/src/` 存在列批次相关实现：`cursor/column_batch.rs`、`cursor/predicates.rs`、`edge/property_schema.rs`、`aggr/` 等（`grep columnar` 命中多文件）。**这是合并立项的本钱**：列式编解码、列类型、选择向量可共建。

---

## 2. 问题分析

### 2.1 逐行分配（分配开销）

行存 `Vec<Vec<Value>>` 每行一次 `Vec` 分配，`Value` 往往是枚举（含堆指针）。N 行 × M 列 = N 次行分配 + 值内部可能再分配；列存为 M 次列分配 + 紧凑缓冲。对 `Materialize` 的大结果集，差距随 N 线性放大。

### 2.2 缓存局部性差

行存按行连续，投影/过滤只取部分列时仍需跨列跳跃（bad locality）；列存同列连续，`Flatten` 的选择向量重放、`Distinct` 的按列哈希都更友好。

### 2.3 列存↔行存反复转置

链路：列存 `DataChunk` →（阻塞算子）→ 行存 `Vec<Vec<Value>>` →（`Flatten`）→ 行缓冲 → 列存 chunk。每次阻塞边界都发生一次转置。若物化态保持列存，可消掉"收口转置 + 重放转置"两次。

### 2.4 与 R3 落盘的耦合

物化态若列存化，落盘格式也应列存化（否则又转回行存再序列化）。因此 **R3（阻塞算子统一落盘）与 R4 应同批设计**：落盘格式定为列式 run。

---

## 3. 设计方案

### 3.1 新增 `MaterializedBatch`（列存物化态）

在 `executor/streaming/chunk/`（或与 storage 共建的 `graphdb-columnar` 共享 crate）引入：

```rust
/// Column-oriented materialized state shared by all blocking operators.
///
/// Backed by a set of typed column buffers plus an optional selection
/// vector, so projection/filter/distinct/flatten all operate on columns
/// without transposing to rows. Replaces `Vec<Vec<Value>>` in
/// `MaterializeState` / `DataCollectState` / `RollUpApplyState` /
/// `DistinctState`.
pub struct MaterializedBatch {
    /// One buffer per output column, in schema order.
    columns: Vec<ColumnBuffer>,
    /// Logical row count (sum of selection, not buffer capacity).
    num_rows: usize,
    /// Optional selection vector; `None` == identity over `0..num_rows`.
    selection: Option<SelectionVector>,
    /// Shared schema fingerprint for spill compatibility checks.
    schema_fingerprint: u64,
}
```

- `ColumnBuffer`：复用 `chunk/typed.rs` 的类型化缓冲（或 storage 列类型）。
- `SelectionVector`：复用 `chunk/selection.rs`。
- `schema_fingerprint`：复用 `spill.rs:190 schema_fingerprint`（同一套列名指纹）保证落盘兼容。

### 3.2 阻塞算子改造

| 算子 | 现状 | 目标 |
|------|------|------|
| `Materialize` | `materialized_rows: Vec<Vec<Value>>` | `MaterializedBatch`；`next_materialize` 逐 chunk **追加列**而非逐行 push |
| `DataCollect` | `all_rows: Vec<Vec<Value>>` | `MaterializedBatch` |
| `RollUpApply` | `all_rows: Vec<Vec<Value>>` | `MaterializedBatch` |
| `Distinct` | `HashSet<Vec<Value>>` + `partition_seen` | 列式 `HashSet<RowKeyRef>`（按列算行哈希，`seen` 存行号/紧凑键） |

要点：
- 入口处 `chunk.append_into(&mut batch)`（新增方法）**直接列追加**，避免行化。
- 出口处按需 `batch.to_chunks(rows_per_chunk)` 分批吐列存 chunk。
- `Distinct` 的 `seen` 从 `HashSet<Vec<Value>>` 改为按列构造的紧凑行键（避免整行 `Vec` 分配）。**注意碰撞/等价语义必须与现测试一致**（`Null`、浮点 NaN、类型混排需逐项对齐）。

### 3.3 `Flatten` 列批次重放

```rust
/// Replay a buffered column batch by a selection vector, producing an
/// expanded column batch without materializing rows.
fn flatten_batch_columns(
    chunk: &DataChunk,
    positions: &SelectionVector,
    out: &mut MaterializedBatch,
);
```

- 现状 `build_flatten_batch_chunk`（`flatten.rs:110`）按行重建；目标改为**逐列 gather**（`out.col[j][k] = in.col[j][positions[k]]`）。
- `buffered_chunk` 保持 `DataChunk`，避免行化。

### 3.4 落盘列式化（与 R3 合并）

- `RunWriter` 增加列式写：`write_batch(&MaterializedBatch)`；`RunReader` 增加 `read_batch()`。
- run 文件头 `RunHeader` 增加 `layout: Row | Column` 标记与列类型表（向后不兼容可接受，No-backward-compatible）。
- `Distinct` 的 `HashPartitionSpiller` 分区哈希从 `hash_row_partition`（`spill.rs:556`）扩展为 `hash_column_partition`。
- `Materialize`/`DataCollect`/`RollUpApply` 补 `spill` 实现（R3），**一次到位用列式格式**。

---

## 4. 与 `graphdb-storage` 列存重构的合并边界

### 4.1 共建什么（共享 crate）

建议抽出共享抽象（新建 `graphdb-columnar` 或复用 storage 现有模块公开接口）：

| 共享物 | 责任 | 来源 |
|--------|------|------|
| `ColumnBuffer` / 列类型 | 类型化列缓冲与编码 | storage `cursor/column_batch.rs`、`edge/property_schema.rs` |
| `SelectionVector` | 选择/重放 | query `chunk/selection.rs`（下沉共享） |
| 列式编解码 | run 文件列式序列化 | 新增（基于现有 `RunWriter` 扩展） |
| `Value` 列化/反列化 | 行↔列转换唯一实现 | 两端共建，避免双份 |

### 4.2 不合并什么（保持各自职责）

- **查询执行语义**（`Flatten` 重放规则、`Distinct` 等价、聚合 state）留在 `graphdb-query`。
- **存储布局/持久化**（CSR、属性列、冷热分层）留在 `graphdb-storage`。
- DAG 约束：`graphdb-storage` 不得依赖 `graphdb-query`；共享列存抽象需放在**更底层**（`graphdb-core` 或独立 crate），或由 query 依赖 storage 的公开列存 API。

> **关键约束**：按 `AGENTS.md` DAG（`…→storage→query→…`），共享列存类型**不能**定义在 query；应定义在 `graphdb-core` 或独立 `graphdb-columnar`，两端共同依赖。

### 4.3 落地顺序（建议）

1. **P0**：抽象出共享 `ColumnBuffer`/`SelectionVector`（下沉到 core/独立 crate），storage 与 query 各自接入，行为不变。
2. **P1**：query 阻塞算子改 `MaterializedBatch`（`Materialize`/`DataCollect`/`RollUpApply`），保持行存 fallback 开关以便灰度。
3. **P2**：`Distinct` 列式去重 + `Flatten` 列批次重放。
4. **P3**：列式 run 文件 + 阻塞算子补落盘（R3 合并落地）。

---

## 5. 风险与缓解

| 风险 | 说明 | 缓解 |
|------|------|------|
| `Value` 语义对拍 | 行↔列转换若字节/语义不一致会静默错值 | 单一转换实现 + 全量对拍测试（行存结果 == 列存结果） |
| `Distinct` 等价 | NaN/Null/类型混排哈希等价 | 复用现有 hash 契约，加专项等价测试；必要时退化行存 fallback |
| 内存预算 | 列存缓冲峰值与 `MemoryTracker` 记账口径变化 | `MaterializedBatch` 暴露 `memory_size()`，接入 `MemoryTracker::try_reserve_row` |
| 落盘兼容 | run 文件格式变更 | No-backward-compatible 允许；加 `layout` 标记 + 版本 |
| 双份列存 | query/storage 各造一套 | 合并立项，共享底层 crate（§4.1） |
| 性能回退 | 小结果集列存摊销反而不划算 | 阈值：行数 < 阈值时保持行存快速路径；基准验证 |

---

## 6. 验收标准

1. **正确性**：`cargo test -p graphdb-query -j 2` 全绿；新增"行存 vs 列存对拍"测试，覆盖 `Materialize`/`DataCollect`/`RollUpApply`/`Distinct`/`Flatten`。
2. **等价性**：同查询在行存与列存物化态下结果逐行相等（含 `Null`/NaN/多类型）。
3. **内存**：大结果集 `Materialize` 在固定预算下峰值内存下降（给出前后对比数据）。
4. **落盘**：阻塞算子在超预算时溢写列式 run 并正确回放（R3）。
5. **基准**：LDBC SNB 或合成大宽表下，端到端 P50/P95 不劣化，宽表投影/过滤场景有提升。

---

## 7. 与 R1–R6 的关系

- **R2**（不变量改 `Result`）：已落地（`docs/plan/factorization_invariant_remediation.md`），与本设计正交，先行的错误处理基础。
- **R3**（落盘不一致）：与本设计**合并落地**（§3.4）。
- **R4**（本文）：物化态列存化 + 与 storage 列存重构合并立项。
- **R5**（巨型 match 漂移）：列存化会触及算子分发，建议同步推进 visitor 收敛。
- **R6**（别名三路径）：与本设计无关。

---

## 8. 待拍板开放点清单

1. 共享列存抽象落在 **`graphdb-core`** 还是**独立 `graphdb-columnar` crate**？（DAG 约束下两者皆可，需定归属与版本策略。）
2. 是否保留**行存 fallback 开关**用于灰度与回归对比？（建议保留一个 feature/配置开关，稳定后移除。）
3. `Distinct` 列式去重的**等价契约**：是否与现 `HashSet<Vec<Value>>` 的 `Value` 哈希/Debug 语义完全对齐，还是借机修正（可能影响既有行为）？
4. 列存化**阈值**：多大结果集才切列存（避免小结果集摊销倒挂）？
5. 落盘格式：**列式 run 是否要求压缩**（现 `RunCompression`，`spill.rs:92`）？
6. 与 storage 列存重构的**排期对齐**：R4 的 P0 抽象是否等待 storage 侧接口冻结后再动？

---

## 9. 核验记录（file:line）

| 事实 | 引证 |
|------|------|
| 阻塞物化态行存字段 | `crates/graphdb-query/src/executor/streaming/operators/blocking/materialize.rs:9`, `:22`, `:30`, `:37` |
| `DataChunk` 定义 | `crates/graphdb-query/src/executor/streaming/chunk/core.rs:13` |
| `ColumnarBatch` 定义 | `crates/graphdb-query/src/executor/streaming/chunk/columnar_batch.rs:1046` |
| `Flatten` 行缓冲重放 | `crates/graphdb-query/src/executor/streaming/operators/flatten.rs:20`, `:52`, `:110` |
| 落盘行存写/读 | `crates/graphdb-query/src/executor/streaming/spill.rs:311`, `:326`, `:491`, `:511` |
| run 文件头 + 魔数 + 压缩 | `crates/graphdb-query/src/executor/streaming/spill.rs:111`, `:53`, `:92` |
| 行分区哈希 | `crates/graphdb-query/src/executor/streaming/spill.rs:556` |
| schema 指纹 | `crates/graphdb-query/src/executor/streaming/spill.rs:190` |
| storage 列存既有实现 | `crates/graphdb-storage/src/cursor/column_batch.rs`, `cursor/predicates.rs`, `edge/property_schema.rs` |
| R3 落盘不一致（合并动因） | `linkrs_query_analysis.md` §6.2 R3 |
| R4 原始判定 | `linkrs_query_analysis.md` §6.2 R4；`docs/plan/factorization_invariant_remediation.md` §6.2 |
| DAG 约束 | `AGENTS.md`（`…→storage→query→…`） |
