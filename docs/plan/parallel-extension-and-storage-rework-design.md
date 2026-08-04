# 剩余任务实施计划：存储列块改造 + 并行分区扩展

- 状态：待实施
- 依据：`docs/archive/benches/phase3-parallel-storage-validation.md`（2026-08-03 初测 / 2026-08-04 修复后重测）
- 上级文档：`docs/plan/columnar-optimization-phases-design.md`（Phase 1/2/3）、`docs/plan/columnar-phase3-parallel-storage-verification-design.md`
- 问题跟踪：`docs/issue/vertex-batch-insert-quadratic.md`、`docs/issue/traversal-query-pathology.md`

## 0. 立项依据（实测结论）

| 项 | 结论 | 数据 |
|----|------|------|
| P3.1 存储改造（查询路径消费列块） | **已立项** | 投影/全行 3.26~5.04x ≥ 1.5x；端到端存储读占比 R：Q1 33% / Q2 50% / Q3 92%（>30%） |
| P3.2 并行分区 | **已证明有效，可扩展** | 修复后 Q1/Q2 实测 `actual_workers = requested`（2/4/8），E(4) = **7.06 / 5.83** ≥ 2x，η(4) = 3.18 / 3.72 ≥ 0.5；Q3 图遍历按白名单正确回退串行 |
| 数据装载 | **需修复** | 批量顶点插入 O(n²)：200k 顶点 510s（10k 块 1.2s → 170k 块 358s） |

当前并行分区能力边界：**仅支持「恰好一个 tagged vertex scan」+ 上拉链**（`optimizer/partitioning.rs:96-100`），且仅 `PartitionSource::VertexId`（`arena_builder/partition.rs:73` 只处理 VertexId 分支）。存储列块消费路径未接通（`storage_scan.rs` 仍逐行物化 `Value`）。本计划补齐这两块。

---

## Part A 存储列块改造（P3.1 立项项）

### A1 查询路径列块消费

**现状**：`GraphVertexCursor` 只提供行式拉取 `next_batch`/`next_flat_batch`（`storage/cursor.rs:406/416`）；`storage_scan.rs:177` `next_cursor_chunk` 每批先逐行转 `Vec<Value>`，再由 `chunk.build_typed_columns()`（`storage_scan.rs:208`）二次构建 typed 列——行式 `Value` 物化是中间产物。而 `ColumnStore` 已有批量列解码（`column_store.rs:1246` `get_batch` / `:1262` `get_projected_batch`）、编码选择器 `EncodingSelector` 与 `next_column_batch` 所需的全部底层原语。

**A1.1 `graphdb-storage`：列块拉取 API**

- `GraphVertexCursor` 新增 `next_column_batch(prop_names, batch_size)`，返回列式批次（每个属性一列原始解码值 + 有效位），复用 `ColumnStore::get_projected_batch` 与 `EncodingSelector` 解码，跳过逐行 `VertexRecord`/`HashMap` 物化
- 对齐冷快照列式读取路径（`docs/archive/cold-snapshot-query-integration.md` 的列式读取语义）；`ScanOptions` 增加列块模式开关（默认关闭，行式兜底）
- 随机取属性（`GetVertices`）保持行式——实测投影随机访问无收益（窄表 1.29x，查找主导）

**A1.2 `graphdb-query`：列块产出 → typed 列直接求值**

- `source_operator/storage_scan.rs` `next_cursor_chunk` 增加列块分支：`next_column_batch` 产出直接填充 `chunk.rs` 现有 `TypedColumn::I64/F64/I32`（Phase 1 已就绪），跳过 `Vec<Value>` 中间层
- 过滤/投影在 typed 列批量求值（Phase 1 `eval_with_cache` typed 快路径已存在）
- 随机取属性路径不接列块；`ColumnarStats` 记录列块命中，`MemoryTracker` 记账列块分配

**验收**：
- Q1/Q2（100k，`taskset -c 0-7`）端到端较修复后基线 ≥1.5x，且 R ≤ 20%（当前 33%/50%）
- 列块模式开关关闭时行式行为完全不变（回归保障）
- 全量 `cargo test --test '*'` + clippy 全绿

### C1 批量顶点插入 O(n²) 修复

**根因（已代码级确认）**：`batch_insert_vertices`（`writer.rs:534`）批量前的预分配只做了 `reserve_vertex_capacity` → `reserve_id_capacity`（`core.rs:531`），**仅预留 `IdIndexer` 容量**，未预留 `ColumnStore` 与 `timestamps`。插入时 `FixedWidthColumn::set`（`column_store.rs:133`）对 `Vec<u8>` 执行 `data.resize(offset + element_size, 0)`——每次插入只增长一个元素，超出容量即整体 realloc+copy，**逐行成本随表规模线性增长 → 总量 O(n²)**。对比：批量边插入走独立线性路径（600k 边 1.6s），故仅顶点路径病态。

**修复步骤**：
1. `reserve_id_capacity`（或新增 `reserve_insert_capacity`）同时预留：`ColumnStore` 各列 `Vec<u8>`（按 `column_len × element_size` + 容量余量）、null 位图、`timestamps` 向量
2. `writer.rs` 批量预计数阶段（`:546-559` 已有每 tag 计数）直接驱动上述预留，避免逐 tag 重复解析
3. 顺带核对 `timestamps` 增长路径是否同病（若为逐行 `resize` 一并预留）
4. 单行/小批量插入路径不动（不额外预留，避免空批量放大内存）

**验收**：
- 100k 顶点批量插入 ≤ 10s（当前 ~370s）；200k ≤ 20s；600k 边保持线性（1.6s 级）
- `storage_bench` bulk 用例无回归；基准数据准备（`parallel_scale_bench` setup）随之受益

### A2 图遍历路径（与 E4 联动，先评估）

- 依赖 `docs/issue/traversal-query-pathology.md`：锚定 1-hop 单次 5.15s（R=92%）、无锚 2-hop >5min
- 方向：Expand 谓词下沉（Phase 2 P2.4）+ Expand 路径列块/属性裁剪（A1 列块扩展到 Expand 邻居读取），目标锚定 1-hop ≤ 100ms、无锚 2-hop（100k×3）≤ 2s
- **不做前置投入**：A2 在 A1/C1 落地前不启动；A2 是 E4（遍历分区）的前置

---

## Part B 并行分区扩展到其他模块

### 扩展原则

- 以 B 链（修复后基线）为门槛：每个增量必须保持 `EXPLAIN ANALYZE` 的 `actual = requested` 与逐值正确性，串行路径（分区关闭）行为不变
- 分区源扩展复用既有 `PartitionSource::EdgeId/Index`（`execution_plan.rs:17-24`）与 `edge_src_id_range`（`spec.rs` ScanEdges 已有 `partition_range`）
- 不可分区形态必须记录 fallback reason（B1.3 已建立的可观测性），禁止静默串行

### E1 多 tagged vertex scan 分区（放宽「恰好一个 scan」）

**E1a 独立扫描分区（低风险，先做）**

- 场景：多个 tagged scan 相互独立，组合算子是「无键亲和」形态（UNION、各自聚合后合并、cross join）——每侧 scan 独立分区、`Exchange` 汇聚后再走全局算子
- 改动：`partitioning.rs:96-100` 从 `scans.len() != 1` 放宽为「每个分支恰好一个 tagged scan 且分支间无 equality-join 键依赖」；`partition.rs` 为每个 scan 生成独立分区片段组，共享根 exchange/全局链
- 拒绝：出现两个分区侧参与 equality join 的计划（无 hash-exchange 前直接分片会丢行/重复行）

**E1b 键亲和 join 分区（hash exchange，中等风险，独立增量）**

- 场景：`MATCH (a:A)-[:R]->(b:B)` 类两侧分区 join；两个分区侧需按 join 键共分区
- 改动：`ExchangeOperator` 增加 hash 分布（当前只有 `Concatenate`，`gather_operator.rs:110`/`exchange_operator.rs:138` 的并行分支基于 `children.len()>1`）；新增 hash-repartition 算子 + `PartitionedInputs` 的键分布描述；hash join 两侧分区键一致时走共分区直连
- 前置：E1a 稳定 + Exchange 并发回填路径（E5）成熟后再做；独立验收

### E2 ScanEdges 分区

- 场景：纯边表扫描（`MATCH ()-[e:E]->() RETURN ...`，投影边属性 + 聚合），不含 Expand/顶点属性 join
- 改动：`partitioning.rs` 增加边扫描分支 → `PartitionSource::EdgeId { edge_type }`；`partition.rs` 增加 EdgeId 分支，本地片段 `StorageScanEdges` 绑定 `edge_src_id_range`（按外部 src-id 范围切分，分区完备且不相交）
- 约束：分区只允许「边行自身足够」的链（边属性过滤/聚合）；需要 dst/src 顶点属性时拒绝（缺顶点侧 join 键），记录 fallback reason
- 与 P2.4 谓词下沉联动：边属性谓词下沉到 CSR 遍历后再分区，收益叠加

### E3 配置化暴露（默认关闭）

- `graphdb-config` `Config`（`config.rs:95`）新增 `parallel` 段：`enabled`、`workers`、`min_rows_per_partition`、`max_partitions`、`max_buffered_chunks`、`vertex_id_range`（可选）
- 接线：配置 → `OptimizerEngine::set_partitioning_config` → `QueryPipelineManager`（`compiler.rs` 已从 optimizer 读 `max_workers`/分区配置）；`SharedScheduler` 按 `workers` 建池
- 默认 `enabled=false, workers=1`（与 `PartitioningConfig::default` 一致，行为零变化）；用户显式开启才进入并行
- 验收：`config.toml` 配 `parallel.workers=4` 后 `EXPLAIN ANALYZE` 显示 `requested=4`；未配置时与现状完全一致

### E4 图遍历型渐进分区（条件启动）

- 前置：A2（遍历路径性能改善，目标锚定 1-hop ≤ 100ms）后启动
- 设计：锚点 scan 按 vid 范围分区，每分区本地跑有界遍历，结果全局汇聚 + 去重；跨分区路径（无锚 2-hop）暂不分区
- 现状保持：`partitioning.rs:183-198` 白名单继续拒绝 `Expand/Traverse/Loop/...` 并记录 fallback reason，直到 A2 达标

### E5 跨切割分与调优

- **分区粒度**：`desired = rows/min_rows`（`partitioning.rs:116`）改为「目标每分区行数 × 核数」动态约束；小表（< 2×min_rows）维持阈值拒绝
- **Exchange 汇聚开销**：E(8) 回落（Q1 7.06→6.60、Q2 5.83→4.58，storage R 升至 455%/529%）——评估 Gather 串行回填（`gather_operator.rs`）与 `max_buffered_chunks` 背压参数，避免 worker 饥饿
- **plan cache 与 layout_version**：`PartitionSpec.layout_version` 当前恒 `None`；规划器对每个 range 布局签名（范围、来源、数据布局版本）做缓存键，数据布局变更（分片/重分布）时失效
- **可观测性**：`EXPLAIN ANALYZE` 已输出 `partition_spec_description`/`actual_workers`/`fallback_reason`；为 E1/E2 新增 `EXPLAIN` 单测（分区片段形态、exchange 契约）

---

## 验收总则（每个增量）

1. 正确性：新形态分区执行结果与串行逐值相等（集成测试经 `QueryPipelineManager` 全链路）
2. 行为：`EXPLAIN ANALYZE` `actual = min(partitions, workers)`、fallback_reason 为空或可解释
3. 性能：`benches/parallel_scale_bench.rs` 扩展 E1/E2 查询组，`taskset -c 0-7` 固定 CPU；新形态 E(4) ≥ 2x 且 η(4) ≥ 0.5
4. 回归：`cargo test --test '*'` 全过 + clippy 全绿；串行路径（分区关闭）零变化；`operator_bench`/`query_bench`/`end_to_end_bench` 无回归
5. 环境：记录 CPU 型号与并发负载（沿用 `phase3-parallel-storage-validation.md` §5 规范）

## 里程碑

| 里程碑 | 内容 | 产出 |
|--------|------|------|
| S1 | C1 批量顶点插入修复 | 100k ≤ 10s；`storage_bench` 无回归 |
| S2 | A1 列块消费（A1.1 + A1.2） | Q1/Q2 ≥1.5x 且 R ≤ 20%；开关回退 |
| S3 | E3 配置化暴露 + E1a 独立扫描分区 | 用户可开启；星型/UNION 类多扫描并行 |
| S4 | E5 调优 + E2 边扫描分区 | E(8) 不再回落或原因明确；边扫描查询并行 |
| S5 | A2 + E4 遍历路径（条件） | 锚定 1-hop ≤ 100ms；遍历分区渐进 |
| S6 | E1b hash exchange + 共分区 join（条件） | 键亲和 join 分区 |

## 非目标

- 并行写路径（规划器白名单已拒绝写/事务边界）；跨事务/跨实例并行（单节点约束）
- 手工 SIMD / 全量列式化（字符串列、图实体列保持行式）
- Validity Bitmap 引入（实测不立项）
- 对齐旧版 `coordinator.rs`（已废弃）
