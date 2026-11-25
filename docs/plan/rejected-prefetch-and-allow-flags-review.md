# 被拒 CSR 预取钩子与 allow_* 开关复核

**日期**：2026-11-25
**范围**：复核 `docs/plan/production-gate-cleanup-tasks.md` 第五节中判定为"有意保留 / 开放排除"的两类门控
**方法**：先读决策文档（`docs/issue/linkrs-remaining-parity-issues.md` 第 45、46 节、`docs/plan/stage1-stage2-current-gap-remediation.md`），再逐文件核对代码现状与文档声明是否一致
**遵循规范**：`AGENTS.md`（文档用中文、代码注释不引用临时计划标识符、无向后兼容包袱、保持合理架构）

本文回答两个问题：其一，已被明确拒绝的 CSR 预取 A/B 钩子是否还应保留；其二，当前 `allow_*` 系列开关各自起什么作用、是否多余。结论对上一份清单的相关判定作**修订**，并给出可执行任务。

---

## 一、结论摘要

| 对象 | 上一份清单判定 | 本次复核判定 | 是否需改动 |
|---|---|---|---|
| CSR 预取 `prefetch_ahead` / `prefetch_head` A/B 钩子 | 开放排除，按文档保留验证 API | **应移除**——候选两轮被拒，且代码现状已与文档声明相反，普通入口成死代码 | 是（新增任务 7） |
| `allow_partition_metadata_count` | 有意保留的安全门 | **语义反转的多余门控**——名为 allow，从不放行，只换取一段拒绝文案 | 是（新增任务 8） |
| `allow_ordered_parallel_expand` | 有意保留 | 保留，是正式实现的配置化形式，不多余 | 否 |
| `allow_pure_count_chain` / `allow_global_aggregate` / `allow_parallel_count_frontier` | 有意保留的安全门 | 保留，但需澄清注释中"放行"语义的适用边界 | 否（仅文档，任务 9） |

---

## 二、CSR 预取 A/B 钩子：应从生产路径完整移除

### 2.1 决策事实链

- 整条钩子链由单次提交 `e71ff500`（提交名含 `temp`）引入，见 `-S'prefetch_ahead'` 与 `-S'get_packed_node_edge_headers_batch_with_prefetch'` 的检索结果。
- `docs/issue/linkrs-remaining-parity-issues.md` 第 46 节记录了两轮候选，均按预设门槛被拒：
  - 黑盒读候选：F1 密集 2048 为 3248→3295 us（+1.4%，噪声）、F5 为 4303→4276 us（-0.6%，噪声），分散 2048 为 8496→10169 us（**+19.7%，负收益**）。
  - x86_64 非阻塞 `_mm_prefetch(_MM_HINT_T0)` 候选：F1 密集基线 2983 us，四个距离 3018/3020/3016/3017 us；F1 分散基线 5698 us，四距离 5701/5702/5704/5783 us；F5 基线 4222 us，四距离 4170/4477/4520/4204 us。全部在噪声内，远未达 15% 门。
- 文档判定原文："拒绝该候选，生产拓扑读取路径保持上一节的按父直写；验证 API 与基准保留，用于后续有真实预取指令或局部性证据时重测"，并称"已删除 unsafe intrinsic，把 `prefetch_head` 恢复为有文档的空实现，仅保留 A/B 钩子与验证；不需要修改生产读取入口"。

### 2.2 代码现状与文档声明不一致（本次关键发现）

逐文件核对后，发现三处与上述声明矛盾的事实：

1. **生产入口已被改写，普通版成死代码**。普通入口 `packed_node_edge_headers_batch`（`reader/edge.rs:644`）虽传 `prefetch_ahead=0`，但其内部调用的仍是 `ctx.out_topology_batch_prefetched(...)` / `ctx.in_topology_batch_prefetched(...)`（`reader/edge.rs:742,751,831,839`）。普通版 `out_topology_batch` / `in_topology_batch`（`edge_ops.rs:1082,1173`）在全仓库（含 bench 与 tests）**零调用**。也就是说，"生产读取入口未修改"的声明不成立——生产调用链的函数名已经切到 A/B 版本，只是参数恒为 0。

2. **`prefetch_head` 并非文档所称的空实现**。`mutable_csr/read.rs:36` 确为空实现且注释完整，但分派链下端的 `MappedFrozen::prefetch_head`（`edge/edge_table/checkpoint/snapshot/read.rs:221`）仍用 `std::hint::black_box` 读 `degree_at(idx)` 与 `offsets`。这正是文档声称"已删除"的黑盒读方案。完整分派链为：`EdgeStore::prefetch_out_head/in_head`（`core/reads.rs:371,378`）→ `node_group::prefetch_hot_head`（`read.rs:96`）→ `MappedFrozen::prefetch_head` 或 `CsrVariant::prefetch_head`（`csr_variant/read.rs:87-93`）→ 各 CSR 变体。

3. **A/B 参数贯穿生产 trait 与多模块**。`StorageReader` 公共 trait 新增方法 `get_packed_node_edge_headers_batch_with_prefetch`（`client/traits_reader.rs:461`），native 引擎 override（`reader_api.rs:237`），`impl` 携带 `prefetch_ahead` 并在 `edge_ops.rs:1145` 处检查 `> 0`。整条链涉及 7 个文件、1 个公共 trait 方法、1 个 benchmark 入口、1 个只读性单测。

### 2.3 判定与理由

**判定：应移除，而非继续保留。**

- 功能层面：两轮候选按预设门槛明确拒绝，无任何 ≥15% 微基准收益或 ≥10% 端到端收益，且分散父批出现 +19.7% 负收益；当前 `prefetch_head` 的黑盒读是被否定方案的残留，不是"空占位"。
- 架构层面（对应 `AGENTS.md`"无向后兼容包袱""保持合理架构"）：一个已被否决的实验不应让生产热路径的函数签名、公共 trait 与多层分派长期携带 A/B 参数。文档保留理由是"便于后续真实预取指令重测"，但该能力完全可以在实验模块内重建——它并非生产正确性或性能所依赖的路径。
- 一致性层面：代码现状与决策文档已经背离（普通入口成死代码、空实现声明不实）。保留这种"半落地"状态只会让后续维护者误判预取仍在生产可用，比直接清除风险更高。
- 反方考量：若未来确有真实非阻塞预取指令与局部性证据，重测需要钩子。但重测应通过独立 bench 模块直接调用 CSR 层原语来复现，而不是依赖生产 `StorageReader` trait 暴露的 A/B 方法；届时从 git 历史（`e71ff500`）恢复该提交即可，成本低于长期维护生产钩子。

### 2.4 移除任务（任务 7）

- **落点**：
  - `client/traits_reader.rs:457-475`：删除 trait 方法 `get_packed_node_edge_headers_batch_with_prefetch` 及其默认实现。
  - `engine/graph_storage/reader_api.rs:237-257`：删除该方法的 native override。
  - `engine/graph_storage/reader/edge.rs:644-844`：合并 `packed_node_edge_headers_batch_prefetched` 与 `packed_node_edge_headers_batch_impl` 回单一普通实现，去掉 `prefetch_ahead` 参数及各调用点的传参；生产入口恢复为普通版语义。
  - `engine/graph_storage/context/edge_ops.rs:1082-1230`：删除 `*_prefetched` 包装与 `*_impl` 的 `prefetch_ahead` 参数及 `if prefetch_ahead > 0` 分支，令 `out_topology_batch` / `in_topology_batch` 重新成为唯一入口。
  - `edge/edge_table/core/reads.rs:371-382`、`edge/node_group/read.rs:96-105`、`edge/csr_variant/read.rs:87-93`、`edge/csr_variant/snapshot/read.rs:221-228`、`edge/mutable_csr/read.rs:36`、`edge/csr_trait.rs:251`、`edge/mutable_csr/trait_impl.rs:125`：删除整条 `prefetch_head` / `prefetch_hot_head` / `prefetch_out_head` / `prefetch_in_head` 分派链。
  - `engine/graph_storage/writer/batch.rs`：该处 `prefetch` 是写入前读取旧属性的正当语义，与拓扑 A/B 钩子无关，**不得删除**；`crates/simvec` 中的 `_mm_prefetch` 是距离 kernel 的 SIMD 优化，同样无关，**不得删除**。
  - `benches/topology_read_bench.rs:51,164-232,388,429,583`：删除 `PREFETCH_AHEADS` 与 `prefetch_packed_sweep` / `prefetch_packed_dest_sweep` 及其调用。
  - `edge/csr_variant/tests.rs:18` `prefetch_head_is_read_only`：随分派链一并删除。
  - 同步修订 `docs/issue/linkrs-remaining-parity-issues.md` 第 46 节与 `docs/plan/stage1-stage2-current-gap-remediation.md` 第 91 行中"保留验证 API 与基准""A/B 钩子与非阻塞候选均已验证"的表述，改为"候选被拒、钩子已移除，重测需从历史恢复实验代码"。
- **验收**：devs release 编译通过；storage lib 原有 1415 项（扣除已删只读性单测）全绿；删除后全仓 `grep -r 'prefetch_ahead\|prefetch_head'` 仅剩写入路径与 simvec 距离 kernel 的无关命中；拓扑读取微基准结果与生产默认同量级、不因清理回归。
- **不做**：不借清理改动第 45 节保留的按父直写与身份主键直通；不触碰 `docs/archive/unsafe.md` 中已记录的历史事实。

---

## 三、allow_* 开关：作用机制与冗余判定

### 3.1 作用机制

`allow_*` 是配合作业总开关 `parallel.enabled`（默认 `false`）的**形状级灰度放行白名单**。核心判定在 `optimizer/partitioning/planner/core.rs` 的 `shape_allowlisted`（第 459-485 行）：

```
若 enabled=false 且 shape_allowlisted(root)=false → 回退串行；
否则进入后续分区门槛（自证明域、行数、结构检查等）。
```

放行矩阵（`enabled=false` 时）：

| 形状判定 | 控制开关 | 结果 |
|---|---|---|
| `is_ordered_parallel_expand` | `allow_ordered_parallel_expand`（默认 true） | 命中即放行 |
| `is_parallel_count_frontier` | `allow_parallel_count_frontier`（默认 false） | 命中即放行 |
| `is_partition_metadata_count` | `allow_partition_metadata_count`（默认 false） | 命中则**明确否决**（返回 false） |
| 其它 Aggregate root | `allow_pure_count_chain` / `allow_global_aggregate` | 按其值放行 |
| 非 Aggregate root | 无 | 否决 |

五个开关均参与计划缓存哈希（`pipeline/compiler.rs:472-476`），因此任何取值变化都会使缓存失效。它们经 `linkrs-api/embedded/database.rs:227-231` 与 `linkrs-server/graph_service/factory.rs:427-431` 从用户配置透传，属于正式配置面，不是临时代码。

### 3.2 逐项判定

| 开关 | 默认 | 实际作用 | 是否多余 |
|---|---|---|---|
| `allow_ordered_parallel_expand` | true | 放行已证明的 rowless 单跳展开前端（group-count / 有序 distinct）；是生产并行能力真正的启用开关 | **不多余**。`07_configuration_reference.md:366,370` 明确其为正式配置并定义回滚方式，属于"以配置方式提供可配置性" |
| `allow_pure_count_chain` | false | 放行纯计数链；收益未稳定证明 | **不多余**。安全门，文档反复要求保持关闭（`stage1-stage2...md:108`），是灰度放行的正当载体 |
| `allow_global_aggregate` | false | 放行其它聚合根；需额外证明 | 同上，保留 |
| `allow_parallel_count_frontier` | false | 放行 count-only 展开前沿；需额外证明 | 同上，保留 |
| `allow_partition_metadata_count` | false | **从不放行任何形状**；打开它只会在被拒时输出更具体的原因文本 | **多余且命名误导**，见 3.3 |

### 3.3 关键发现：`allow_partition_metadata_count` 语义反转

该开关名为 allow，实际行为是 deny，且从不改变放行结果：

- `shape_allowlisted`（`core.rs:475-477`）：`if allow_partition_metadata_count && is_partition_metadata_count(root) { return false; }` —— 打开时反而明确否决该形状，防止其误入通用 scan 分区路径。
- `decide_with_layout`（`core.rs:207-211`）：`if allow_partition_metadata_count && is_partition_metadata_count(root) { return fallback("partitioning is disabled; metadata count has no shard-local storage proof"); }` —— 打开时仅替换为更精确的拒绝文案。
- 关闭时：走通用 `fallback("partitioning is disabled")`，同样拒绝。

因此该字段唯一的可观测作用是"一段更精确的错误文本"，与放行决策无关；它却参与计划缓存哈希，改动一次会让全部计划重新规划，是纯开销。它与其它 `allow_*` 的"放行白名单"语义不一致，属于历史遗留的冗余配置项。

### 3.4 移除任务（任务 8）

- **落点**：`linkrs-config/src/parallel.rs:46-49,66`、`optimizer/partitioning/config.rs:39-42,67`、`pipeline/compiler.rs:476`、`linkrs-api/embedded/database.rs:231`、`linkrs-server/graph_service/factory.rs:431`、`config.toml:261`，以及相关 tests/benches 的结构体字面量（用 `..Default::default()` 处无需改，显式赋值处删除该行）。
- **目标**：整字段删除；`decide_with_layout` 中该分支随之删除，metadata count 形状统一走通用拒绝原因；`shape_allowlisted` 中对应否决分支删除（默认即否决，无需显式判断）。
- **替代方案**（若团队希望保留更精确的诊断文案）：不改放行逻辑，但把字段重命名为 `explain_metadata_count_rejection` 并移出 `allow_*` 命名族、同时从计划缓存哈希中移除，明确其纯诊断定位。二选一，推荐直接删除。
- **验收**：配置默认值测试、partitioning 全量、`parallel_partition_execution`、计划哈希单测通过；`config.toml` 与 `07_configuration_reference.md` 字段一致；metadata count 形状仍走串行、结果与当前逐值一致。

### 3.5 语义澄清任务（任务 9，仅文档/注释）

`allow_*` 的"放行"仅在 `enabled=false` 时生效；`enabled=true` 时 `shape_allowlisted` 被短路（`core.rs:205`），白名单全部失效，所有形状都过总开关。这一点应在 `linkrs-config/src/parallel.rs` 与 `optimizer/partitioning/config.rs` 的字段注释、`07_configuration_reference.md` 第 353-370 行写清，避免使用者误以为打开某个 `allow_*` 即可单独放行某一形状而总开关仍受控。本任务只改注释与文档，不改行为，也不打开任何当前关闭的门。

---

## 四、与上一份清单的关系

- 上一份清单第 120 行把 CSR 预取判为"开放排除、按文档有意保留"。本次复核以代码事实推翻该判定：普通入口已被改写、空实现声明不实、钩子贯穿生产 trait。据此新增任务 7，并将该行改列为"应移除"。
- 上一份清单第 126 行把五个 `allow_*` 统一列为"有意保留的安全门"。本次复核细分：四个确为安全门保留；`allow_partition_metadata_count` 单列为多余项（任务 8）。
- 上一份清单的任务 1-6 不受影响，执行顺序不变；本文任务 7-9 为新增，建议排在任务 1-6 之后。

---

## 五、执行顺序与硬门槛

1. **任务 9**（纯注释/文档）→ **任务 8**（删单个多余配置项）→ **任务 7**（移除 CSR 预取钩子链，改动面最大）。
2. 统一硬门槛沿用上一份清单第六节：正确性门（partitioning 49/49、typed group、local topk、parallel 27/27、DQL、storage 全量、`column_batch_test`）、结果门（`group_by_count [14|59]`、`distinct_limit [14]`、`result_stable=true`、`spill_rows=0`）、性能门（同源资源对齐口径，结果不回归）。
3. **不做项**：不打开任何当前关闭的 `allow_*`；不改动第 45 节保留的按父直写与身份直通；不触碰 `parallel.enabled` 默认值与已证明的 `allow_ordered_parallel_expand=true`；不删除写入路径与 simvec 距离 kernel 中无关的 `prefetch` 调用。

---

## 六、执行结果（2026-11-25）

任务 7-9 已全部实施：

- **任务 7**：CSR 预取钩子链完整移除——公共 trait 方法、native override、`reader/edge.rs` 与 `edge_ops.rs` 的 A/B 入口、`prefetch_head` 五级分派、基准 sweep 与只读性单测全部删除；全仓 `grep` 确认仅剩写入路径与 simvec 距离 kernel 的无关 `prefetch`。决策文档第 46 节与 stage1-stage2 表格已补"后续处置"说明。
- **任务 8**：`allow_partition_metadata_count` 整字段删除，连带删除死代码 `is_partition_metadata_count`；原验证该 flag 的集成测试改写为 `metadata_count_without_storage_proof_stays_serial`。
- **任务 9**：`linkrs-config` 与 `linkrs-query` 的 `ParallelConfig` 注释、`07_configuration_reference.md` 均补齐白名单适用边界说明。

验证：全量 `cargo check --workspace --all-targets` 通过；`storage` lib 1416/1416、`parallel_partition_execution` 27/27、`partitioning` 49/49 全绿。任务清单详细执行结果见 `production-gate-cleanup-tasks.md` 第七节。
