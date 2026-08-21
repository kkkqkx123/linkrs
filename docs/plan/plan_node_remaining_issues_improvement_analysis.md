# PlanNode 剩余问题改进必要性分析

> 前置文档：`docs/analysis/计划节点类型对比分析.md`（Ladybug/linkrs/Nebula 计划节点类型对比，聚焦 §5-§7）
> 核验范围：`crates/graphdb-query/src/query/planning/plan/core/nodes/base/plan_node_enum.rs:567`、`macros/enum_methods.rs:176`、`__analysis__/plan_node_category_analysis.md:3`、`__analysis__/plan_node_dependency_analysis.md:3`、`spec.rs:950`、`Cargo.toml:33`、`arena_builder/` 等
> 结论：7 项中 3 项已闭环（5.1/5.2/5.5），剩余需决策的仅 5.3/5.6/5.7/§7（5.4/§6 为过时文档口径，已删除）。

---

## 1. 判定原则

本分析不以"与 Ladybug 对齐"为唯一标准，而以三原则判定是否需改进：

1. **是否阻塞正确性/可维护性**：会否导致线上错、协作成本、文档误导
2. **是否与项目定位一致**：linkrs 定位 `AGENTS.md` 为轻量单节点、聚焦本地部署、可服务端（非嵌入式分析引擎），部分 Ladybug 能力属"互补定位"而非"缺陷"
3. **投入产出比**：改动成本（代码/存储/执行器重构）vs 收益（负载覆盖、性能、协作效率）

分级：`P0 必改（低成本高收益）` / `P1 建议改（中期）` / `P2 按需/暂缓（有条件）` / `P3 不改（设计取舍）`

---

## 2. 剩余问题总览

| 编号 | 原文定位 | 现状（核验后） | 是否需进一步改进 | 优先级 |
|------|---------|---------------|----------------|--------|
| 5.3 | 缺乏因子化 `SEMI_MASKER/MULTIPLICITY_REDUCER` | 仍无，流式 `Dedup/Filter` 近似 | **P3 不改**（暂缓，见 §3.1） | P3 |
| 5.6 | 样板代码 882 行、新增改动面大 | `define_all_plan_nodes!` 已收敛 4 元信息，但仍需同步改 `is_*/as_*/as_mut_` 三表 | **P1 建议改**（生成器/校验） | P1 |
| 5.7 | 分类粒度/命名历史包袱 `GetVertices/GetNeighbors` | 仍对齐 Nebula，非最优但兼容 | **P3 不改** | P3 |
| §7-1 | 批量导入导出 `COPY FROM/TO` CSV/Parquet/NPY | 仍缺，`spec.rs` 无 `CopySpec` | **P2 按需** | P2 |
| §7-2 | 扩展机制 `INSTALL/LOAD EXTENSION`、DB `attach/detach` | 仍缺 | **P3 不改**（与轻量定位冲突） | P3 |
| §7-3 | 分布式管理面 `AddHosts/Balance/Job/Snapshot/Zone` | 仍缺（§5.5 已明确需扩充而非缩减） | **P2 按需**（分布式路线触发时） | P2 |

> 5.1/5.2/5.5 已在 `2026-08-16` 闭环（`plan_node_enum.rs:699` 6 项一致性测试门禁 + `RecursiveFragmentSpec:950` 四变体 + `variable_length_path_planner.rs` + DDL 1743 行落地），本次不再展开。

---

## 3. 逐项分析

### 3.1 5.3 因子化执行 — P3 不改（暂缓）

**现状**：Ladybug 的因子化依赖三件套 `factorized_table`/`semi_masker`/`multiplicity_reducer`（`src/processor/result/factorized_table.cpp` 等），与列式存储 + 向量化 `ListVector` 深度绑定。linkrs 为行存边属性 + 流式 `tuple-at-a-time` + `SlotLayout`（`spec.rs:144`），无等价节点是**存储/执行模型选择的结果**，非简单"补节点"。

**影响**：多跳 `MATCH (a)-[:KNOWS*3..5]->(b)` 中间结果物化放大，因子化可在去重/半连接层面裁剪。linkrs 当前用 `Dedup` + `Filter` 近似，正确性无损，性能有损。

**为何暂缓**：
- 引入成本极高：需重做存储列化（`PropertyTable` 行存→列存）、向量化批次（`DataChunk` 2048）、`FactorizedTable` 中间表示、 morsel 调度，≈ 重写执行器，与 `docs/analysis/linkrs_vs_ladybug_存储对比分析.md:5.7` 的存储层重构耦合。
- 与定位不匹配：linkrs 强项为 OLTP + 时序 + 多租户 RBAC（`tests/dcl/`），Ladybug 强项为 OLAP 嵌入式分析，二者互补。当前无 LDBC SNB 类分析负载基准证明瓶颈（同文档 §5.8 基准方法论缺口）。
- 已有缓解：`RecursiveFragmentSpec` 已将最痛的递归遍历从 `Loop` 模拟升级为原生算子，边际收益已收敛。

**建议**：**不立项**。在 `docs/analysis` 显式标注"因子化为分析负载可选演进，非 P0"，待出现真实分析 SLAs 再以 `bench: ldbc_snb` 驱动决策。若确需，先做列存边属性 + 零拷贝 CSR（存储先行），再谈因子化节点。

---

### 3.2 5.6 样板代码 — P1 建议改（中期）

**现状**：`plan_node_enum.rs` 978 行中，`define_enum_is_methods!` / `define_enum_as_methods!` / `define_enum_as_mut_methods!` / `define_all_plan_nodes!` 四表 + 手写 `DOCUMENTED_NAMES:808` 镜像，共 5 处需同步。`define_all_plan_nodes!` 已将 `type_name/category/describe/ALL_VARIANT_NAMES` 收敛，但 `is_*/as_*/as_mut_` 仍独立，且 `DOCUMENTED_NAMES` 为手写镜像（测试 enforce）。

**影响**：新增节点改动面 5 文件/5 表，遗漏即编译错或测试红（虽有门禁，但上手成本高）。

**改进必要性：建议改**，非阻塞但持续磨损协作效率。

**方案（择一，推荐 A）**：
- **A. 代码生成（推荐）**：单一 `plan_nodes.toml`/`build.rs` 生成 `is_*/as_*` + `ALL_VARIANT_NAMES` + `DOCUMENTED_NAMES`，消除手写镜像。成本 1-2 天，新增节点改为 1 处声明。
- **B. 轻量**：保留宏，增加 `cargo xtask check-plan-nodes` 一键校验，PR 模板勾选。成本 0.5 天，不根治但降低遗漏率。

**不改的代价**：可接受（已有测试兜底），但每次新增节点仍需跨 5 表比对，长期维护税。

---

### 3.3 5.7 历史包袱 `GetVertices/GetNeighbors/Expand` — P3 不改

**现状**：Nebula 血统的细粒度拆分（`GetVertices` 多输入按上游 ID、`GetNeighbors` 多输入按上游顶点、`Expand` 多输入、`Traverse` 单输入）在 `plan_node_traits.rs:319` 的 `Zero/Single/Binary/Multiple` 体系中已文档化，且 `Spec` 层有合并（`GraphSpec:355`）。

**为何不改**：重命名/合并为统一 `Scan+谓词下推` 需同步改 `planner.rs`、`arena_builder`、`optimizer`、`EXPLAIN` 输出，破坏 `NebulaGraph` Rust 重写的心智一致性，收益仅为"更优雅"，无性能/正确性收益。标记为设计债，文档中已说明取舍即可。

---

### 3.4 §7 功能缺口 — 分级处置

#### §7-1 批量导入导出 `COPY FROM/TO` — P2 按需

**现状**：Ladybug 的并行 CSV/Parquet 读写 + `COPY FROM/TO` 是数据工程刚需，linkrs 仅有 OLTP 式 `InsertVertices/Edges`（`plan_node_enum.rs:225`）。

**判定**：若 linkrs 面向"本地图库 + 定期批量建图"，则**必补**；若仅面向在线小写，则可缓。建议**P2**：先提供最小 `COPY FROM CSV`（`SourceSpec:71 StorageScan` 复用 + `csv` crate 并行解析），Parquet/NPY 按需。成本 3-5 天，收益明确。

#### §7-2 扩展/DB 管理 `INSTALL EXTENSION / ATTACH DATABASE` — P3 不改

与"轻量单节点、聚焦本地部署"冲突，属嵌入式生态特性。linkrs 已有 `FulltextManage/VectorManage` 插件式管理，通用扩展机制无需求则不做。

#### §7-3 分布式管理面 `AddHosts/Balance/Job/Snapshot` — P2 按需（触发式）

`§5.5` 已更正：分布式演进时需**扩充**而非缩减。当前单节点无需，立项时机为明确分布式路线图时（参考 `vector_local_engine_plan.md` 的分阶段思路），届时以 `SpaceManageNode` 子枚举扩容为 `ClusterManageNode` 即可，枚举膨胀问题已由参数化解决。

---

## 4. 优先级与路线图

| 阶段 | 项 | 工作量 | 产出 |
|------|----|--------|------|
| **P1 下迭代** | 样板代码生成器 | 1-2 天 | `build.rs`/`plan_nodes.toml` 单源生成，删除手写 `DOCUMENTED_NAMES` |
| **P2 按需** | `COPY FROM CSV` 批量导入 | 3-5 天 | `CopyFromSpec` + 并行 CSV reader + `EXPLAIN` + bench |
| **P2 触发式** | 分布式管理面扩充 | 待 T-shirt 评估 | `ClusterManageNode` 子枚举 + `DdlSpec::ClusterManage` |
| **P3 不做** | 因子化/重命名/扩展机制 | — | 标注为"设计取舍/互补定位"，不再列为缺陷 |

**不建议立项**：完整因子化/向量化/morsel 重构（成本数周，与存储列化耦合，当前无基准驱动）。

---

## 5. 验证方式

- P1：新增哑节点仅改一处声明，`cargo test -p graphdb-query --lib plan_node_enum::tests -- --nocapture` 6 项绿。
- P2：`COPY FROM` 端到端 `cargo test --test integration_import` + `benches/import_bench.rs`。

---

## 6. 结论

剩余问题中**无 P0 必改**；唯一值得投入的是**样板代码生成（P1）**；因子化、历史命名、扩展机制属**定位差异，不应视为缺陷**；批量导入与分布式管理面为**条件触发（P2）**。按此可在 <2 天内闭环可维护性痛点，避免为 OLAP 能力过度投入。
