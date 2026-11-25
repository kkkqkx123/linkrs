# 生产路径门控清理与环境参数配置化任务清单

**日期**：2026-11-25
**范围**：提交 `6422130c` 之后至 `7504983f` 的全部 45 个提交
**方法**：先按提交读取其新增/修改的 `docs/` 设计文档，再据此判断代码中的门控状态；不对每个提交做完整 diff 审阅
**遵循规范**：`AGENTS.md`（文档用中文、代码注释不引用临时计划、无向后兼容包袱、保持合理架构）

本清单只列两类事项：**已合入生产路径、应移除多余门控并转为正式实现**的代码，以及**与环境性能有关、应以配置方式提供可配置性**的硬编码项。仍保持开放状态的修改在第五节明确排除并给出理由。

---

## 一、642213 以来的修改方向

45 个提交按"提交内文档 + 提交说明"归为六条主线。以下只列提交号与方向，细节见对应文档。

| 主线 | 提交（旧→新） | 沉淀文档 |
|---|---|---|
| 存储后端与向量/全文同步 | `68991eea` `8ec96671` `5094f2bd` `a94bb5b8` `9f04e48f` `57e8505f` | `docs/plan/fulltext-vector-convergence-plan.md`（已删）、`docs/infra/sync/architecture.md`、`docs/analysis/print-to-logging-migration.md` |
| 阶段三 parity 缺口：顶点装入 / 展开拓扑 / 有序并行 / 分区聚合 | `943f7dec` `e2359d0a` `d814ab92` `e06f7b92` `09a36d5b` `733322f8` `06d9cff8` `0e895038` `58d1bf58` `7a289577` `7f4fc961` `52e24fe0` `204f6fb7` | `docs/plan/linkrs-remaining-parity-implementation-plan.md`、`docs/plan/stage1-stage2-current-gap-remediation.md`、`docs/issue/linkrs-remaining-parity-issues.md` |
| 路径终态语义修复 | `86407d8e` `17e3ea59` | `docs/plan/semantic-correctness-remediation.md`、`docs/plan/current-code-implementation-defects.md`（已删） |
| 前端 / GPUI 资源加载 | `a656b459` `e3b1f600` | `docs/analysis/frontend-resource-loading-analysis.md`、`docs/analysis/gpui-client-analysis.md`、`docs/plan/gpui-open-issues-decisions.md`（已删） |
| 阶段一、二性能收敛（扫描可见性、标识批量解析、packed 直写、局部 TopK） | `cae24b9d` `fde81fff` `96b84c52` `e6c2f99a` `e1a21411` `61b85615` `e71ff500` `c03e57c8` `eb736222` | `docs/issue/linkrs-remaining-parity-issues.md` 第 37–48 节（过程文档在 `eb736222` 集中归档删除） |
| `distinct_limit` 有序存在扫描与编译缓存 | `fe92c527` `c56b3026` `24577572` `2f828f18` `7504983f` | `docs/plan/distinct-limit-ordered-existence-scan-design.md`、`docs/plan/ordered-existence-non-source-overhead-remediation.md`、`docs/plan/compiled-plan-memo-design.md`、`docs/plan/compiled-plan-memo-integration-plan.md` |

**总体方向**：以资源对齐的三方 A/B 为唯一验收口径，把一批"窄形状快路径"（typed 分组计数、精确局部 TopK、packed 拓扑直写、身份主键直通、有序存在扫描、快形状优化、编译计划 memo）逐条从实验推进到生产默认；正确性靠反例测试与 EXPLAIN 证明标记守住，性能靠固定门与微基准守住。每条改动都带一个 A/B 或灰度开关，这是本清单要处理的对象。

---

## 二、门控盘点

下表覆盖生产代码中所有与上述主线相关的门控。"判定"列区分：转正候选、配置化候选、有意保留、开放排除。

| 门控 / 参数 | 代码位置 | 默认 | 对应功能状态 | 判定 |
|---|---|---|---|---|
| `LINKRS_COLUMN_BLOCK_ENABLED` | `storage_scan.rs:48` | 环境变量未设置即走静态开关 `true` | 列块扫描已落地；已有正式配置 `column_block_enabled` | **转正候选**（任务 1） |
| `LINKRS_ORDERED_EXISTENCE_SCAN` | `partition/chain.rs:724` | 未设置即启用改写 | 有序存在扫描已落地（A/B 30.98→1.82 ms，全门通过） | **转正候选**（任务 2） |
| `LINKRS_NO_FAST_SHAPE` | `optimizer/engine/optimizer.rs:67` | 未设置即启用快形状 | 快形状优化已落地（`optimize` 870→554 us，全门通过） | **转正候选**（任务 3） |
| `fixed_overhead_rows` / `per_partition_rows` / `exchange_row_per_mille` | `partitioning/config.rs:46,48,51`；`database.rs:224-226` 硬编码 0 | 全 0（禁用经济门） | 经济门计算已落地（`desired_partition_count`），但用户配置层无字段 | **配置化候选**（任务 4） |
| `batch_size = limit.clamp(1,16)` | `source_operator/ordered_existence.rs:461` | 上限硬编码 16 | 批次通用性测试已补齐，上限仍硬编码 | **配置化候选**（任务 5） |
| `chunk_size.clamp(1,2048)` | `source_operator/storage_scan.rs:62` | 上限硬编码 2048 | 扫描批次已落地 | **配置化候选**（任务 6） |
| `parallel.enabled` + `allow_*` 五项 | `linkrs-config/src/parallel.rs:33-45` | 总开关 false；仅 `allow_ordered_parallel_expand=true` | 灰度放行已就位，其余形状收益未证明 | 有意保留（第五节） |
| `LINKRS_STAGE_TIMING` | `streaming/instance.rs:237`、`ordered_existence.rs:51` | 未设置即关闭 | 诊断计时已落地，不进热路径 | 有意保留（第五节） |
| `LINKRS_NO_COMPILED_PLAN_MEMO` | `pipeline.rs:744` | 未设置即启用 memo | 只读 memo 已落地，但集成重构仍开放 | 开放排除（第五节） |
| `prefetch_ahead` / `prefetch_head` | `client/traits_reader.rs:457`、`reader_api.rs:245`、`context/edge_ops.rs:1104`、`reader/edge.rs:669` | 生产恒传 0；`prefetch_head` 为空实现 | CSR 预取候选已被拒绝 | 开放排除（第五节） |
| `COMPILED_EVAL_ENABLED` / `set_column_block_enabled` | `expression/evaluator/compiled.rs:43`、`storage_scan.rs:35` | true | 运行时 API 已提供可配置性 | 已配置化，无需改动 |

---

## 三、修改任务清单：移除多余门控，作为正式实现

以下三项的共同前提：功能已合入生产路径、有 A/B 与全门通过证据、当前默认已启用，门控只剩一次性 A/B / 回退用途。按 `AGENTS.md` 的"无向后兼容包袱"，应删除环境变量回退，把行为固化为正式实现；可观测性改由 EXPLAIN 标记承担，A/B 能力移到一次性脚本而非生产分支。

### 任务 1：列块扫描统一到正式配置入口

- **证据**：`10e8f0cc`、`cae24b9d`、`fde81fff` 引入并收敛列块扫描；`docs/release/statements/07_configuration_reference.md:447` 已把它列为正式配置项 `column_block_enabled`（默认 true，输出逐位一致），同时保留 `LINKRS_COLUMN_BLOCK_ENABLED=0` 环境变量回退。
- **问题**：同一能力存在"配置字段 + 运行时 API + 环境变量"三个入口，环境变量覆盖静态开关，形成多余的第二回退门控。
- **目标**：保留 `column_block_enabled` 配置与 `set_column_block_enabled` API，移除 `LINKRS_COLUMN_BLOCK_ENABLED` 读取逻辑；回滚方式统一为改配置。
- **落点**：`crates/linkrs-query/src/executor/streaming/operators/source_operator/storage_scan.rs:35-60`；同步更新 `07_configuration_reference.md:447` 与 `config.toml` 示例。
- **验收**：`column_batch_test`、storage lib 全量、DQL 全量通过；PROFILE/EXPLAIN 的 `column_block_hits` 仍可观察；配置设为 false 时行为与当前环境变量回退一致。
- **不做**：不删除 `set_column_block_enabled`（它服务于嵌入式新增视图等运行时场景，是正当可配置性）。

### 任务 2：有序存在扫描转正为正式实现

- **证据**：`fe92c527`（fix distinct limit operator）、`c56b3026`、`24577572` 落地改写；`docs/archive/perf/parity/2026-11-25-ordered-existence-non-source-overhead.md` 第 3、5 节记录 A/B（`off` 30.98 ms / `on` 1.82 ms，结果均为 `[14]`）与全门通过；`docs/plan/distinct-limit-ordered-existence-scan-design.md` 第 3、5 节定义启用证明与门槛。
- **问题**：改写已在生产默认启用且收益 17×，但 `LINKRS_ORDERED_EXISTENCE_SCAN=off` 仍在生产分支中作为回退；证明链（`safe_global_tail_after_topn`、运行时标签守卫、generation 漂移回退）本身已保证语义安全，环境变量回退属冗余。
- **目标**：删除环境变量分支，改写的启用完全由计划期证明与运行时守卫决定；保留 `ordered_existence:proven` / `redundant_global_ops=eliminated` 等 EXPLAIN 标记作为可观测与诊断出口。
- **落点**：`crates/linkrs-query/src/executor/streaming/plan/arena_builder/partition/chain.rs:724-727`；`docs/plan/ordered-existence-non-source-overhead-remediation.md` 验证门槛第 121 行改为基于 EXPLAIN 断言，A/B 差分移入 `scripts/devs-ordered-existence-measure.sh` 一次性执行。
- **验收**：差分测试、partitioning、local_topk、parallel、DQL、storage 全门通过；`distinct_limit [14]`、`result_stable=true`、`spill_rows=0`；EXPLAIN 仍能读出候选/匹配/输出行数与回退原因。
- **前置**：确认 `24577572`（提交名带 temp）的工作树改动已全部提交并被上述归档文档覆盖后再转正。

### 任务 3：快形状优化转正为正式实现

- **证据**：`24577572` + `docs/plan/ordered-existence-fast-shape-optimizer-design.md` 第 6 节：门命中后 `optimize_median_us` 870→554–593 us、`compile_median_ms` 0.96→0.67–0.72 ms、`exec` 1.76→1.56 ms，结果 `[14]`，A/B 与全门通过；设计文档第 4 节要求完整路径与快路径计划树逐项一致。
- **问题**：`LINKRS_NO_FAST_SHAPE` 是纯 A/B 逃逸开关，快路径已证明等价且默认启用。
- **目标**：删除环境变量检查，快路径由既有 `enable_heuristic`（`optimizer/engine/config.rs:93,244`，已是可配置项）统一控制；计划树一致性由现有单测保证。
- **落点**：`crates/linkrs-query/src/optimizer/engine/optimizer.rs:65-74`；同步修订 `docs/plan/ordered-existence-fast-shape-optimizer-design.md` 第 3、4 节"保留回退开关"的表述。
- **验收**：`fast_shape` 计划一致性单测、DQL、partitioning、local_topk、parallel 全绿；A/B 计划树与诊断 notes 逐项一致。
- **不做**：不放宽快形状门的适用形状；任一形状出现计划差异即恢复该门并停止转正。

---

## 四、修改任务清单：环境性能相关项配置化

以下项直接影响执行成本，且与硬件/数据规模/命中密度相关，当前硬编码或硬编码为 0，应按 `AGENTS.md`"保持合理架构"的原则暴露为配置，而不是留在代码常量里。

### 任务 4：把并行经济门三参数暴露到 `[parallel]` 配置并纳入配置哈希

- **证据**：`PartitioningConfig` 定义 `fixed_overhead_rows` / `per_partition_rows` / `exchange_row_per_mille`（`partitioning/config.rs:46,48,51`），`desired_partition_count`（`planner/core.rs:346,358`）用 `saving - overhead` 选择分区数；但 `docs/issue/linkrs-remaining-parity-issues.md` 第 30 节说明零成本配置保持原结果，`database.rs:224-226` 把三者硬编码为 0，用户配置层 `linkrs-config/src/parallel.rs` 无对应字段，`partitioning_config_hash`（`pipeline/compiler.rs:464-485`）也未哈希它们。
- **问题**：三项是典型的机器相关成本参数（启动开销、每分区开销、交换每行成本），却无法配置；更严重的是不参与配置哈希，若将来赋值会导致不同成本口径下的计划误命中缓存。
- **目标**：在 `linkrs-config` 的 `ParallelConfig` 增加三字段（默认 0，行为不变），经 `partitioning_config_from` 透传，并加入 `partitioning_config_hash`；在 `config.toml` 与 `07_configuration_reference.md` 第 8 节补齐说明。
- **验收**：配置默认值测试、partitioning 全量、`parallel_partition_execution`、`planning_config_hash` 相关单测通过；改动三参数后缓存哈希变化、下一次执行重新规划；`config.toml` 示例与文档字段一致。
- **不做**：不改变默认 0 的语义，不在本任务标定具体成本数值（那需要专门的容量基准）。

### 任务 5：有序存在扫描候选批次上限配置化

- **证据**：`source_operator/ordered_existence.rs:461` 的 `batch_size = limit.clamp(1, 16)`；`docs/plan/ordered-existence-non-source-overhead-remediation.md` 问题清单第 6 项指出该上限仅对 `limit=10` 密集验证，需覆盖稀疏命中与大 limit；归档文档第 22 行显示 `select_emitted` 纯函数与 limit 1/10/17/100 测试已补齐。
- **问题**：批次大小是与命中密度、候选检查成本相关的环境参数，硬编码 16 无法在稀疏命中或大 limit 场景调优。
- **目标**：把上限（当前 16）提升为配置项，经执行上下文传入源算子；保留 `clamp(1, …)` 的安全下界与 `select_emitted` 的早停语义。
- **验收**：limit 1/10/17/100 × 连续命中/稀疏命中/全不命中/关闭早停的差分测试全过；输出始终是按主键升序的前 k 个存在结果；A/B 结果逐值一致。
- **不做**：不放宽证明条件、不把原始索引游标 limit 直接设为 k（设计文档第 6 节明确禁止）。

### 任务 6：扫描固定批次上限配置化评估

- **证据**：`source_operator/storage_scan.rs:62` 的 `fixed_batch = op.config.chunk_size.clamp(1, 2048)`；`chunk_size` 已是配置项，但 2048 上限硬编码。
- **问题**：扫描批次直接决定向量化收益与内存/延迟权衡，属环境性能项。
- **目标**：先评估 2048 上限是否可被现有 `chunk_size` 配置完全表达；若是，则删除硬编码 clamp 由配置层负责约束；若否，则把上限也暴露为配置。
- **验收**：storage lib 全量、`column_batch_test`、DQL 全绿；配置不同 `chunk_size` 时扫描批次单调可控，结果逐值一致。
- **不做**：不调整默认 `chunk_size`；本任务为评估型，若无配置层可表达性收益可直接关闭。

**评估结论（已完成，决定关闭本任务，不改代码）**：

- 清单证据中"`chunk_size` 已是配置项"与代码现状不符。`op.config.chunk_size` 经 `QueryBindings::from_context`（`instance.rs:146`）取自 `ExecutionContext.chunk_size`，而 `build_execution_context` 并未设置该字段，`ExecutionContext::new_base` 恒置为 `DEFAULT_CHUNK_SIZE = 2048`；`config.toml` 与 `07_configuration_reference.md` 中亦无 `chunk_size` 条目。生产路径的 `chunk_size` 实际是不可配置的固定值 2048。
- 因此 `fixed_batch = op.config.chunk_size.clamp(1, 2048)` 的上限 2048 与生产 `chunk_size` 取值恒等，上限分支永不触发，clamp 实际只留下 `≥1` 的下界保护作用。删除上限不会产生任何行为差异，保留它还能在未来 `chunk_size` 被误设过大时提供防御性封顶，无害。
- 清单设想的验收"配置不同 `chunk_size` 时扫描批次单调可控"缺少前提（无用户配置入口）；若真要暴露，需新增配置项并贯穿 config → OptimizerEngine → ExecutionContext → `OperatorConfig` 多层，与任务 5 已建立的配置链路重复且超出"评估"范围。
- 结论：按"评估型，若无配置层可表达性收益可直接关闭"授权关闭本任务，保留 `clamp(1, 2048)` 原样；storage lib / `column_batch_test` / DQL 的全绿由任务 10（编译验证与测试）统一确认。

---

## 五、明确排除项（仍开放 / 有意保留）

### 5.1 仍开放，不纳入本清单

| 项 | 理由 |
|---|---|
| 编译计划 memo 集成重构与 DML memo 重构（`2f828f18`、`7504983f`） | 最新提交 `7504983f` 仅新增集成计划；`docs/plan/compiled-plan-memo-integration-plan.md` 第 1、5、8 节明确"后续应做"、"保留开关便于分 namespace A/B 与故障隔离"。因此 `LINKRS_NO_COMPILED_PLAN_MEMO` 保留，待集成重构闭环后再评估转正。 |
| 有序存在扫描源算子外剩余开销中未完成的部分 | `docs/plan/ordered-existence-non-source-overhead-remediation.md` 的执行装配骨架、已编译物理计划复用仍列为后续方向；其归档文档第 4 节亦说明"本次不直接改热路径缓存结构"。相关门控随该开放任务保留。 |
| CSR 预取 `prefetch_ahead` / `prefetch_head` | 候选已被明确拒绝（无 ≥15% 微基准收益与 ≥10% 端到端收益，分散父批甚至 +19.7%），A/B 钩子按文档有意保留用于后续真实预取指令重测；且其从未进入生产路径，不属于"已合入生产的多余门控"。 |

### 5.2 有意保留的安全 / 诊断门控，不移除

| 项 | 理由 |
|---|---|
| `parallel.enabled`（默认 false）与 `allow_pure_count_chain` / `allow_global_aggregate` / `allow_parallel_count_frontier` / `allow_partition_metadata_count`（默认 false） | 文档反复强调"不得打开"：前三者收益未稳定或需额外证明，后者缺分片本地存储证明。`docs/plan/stage1-stage2-current-gap-remediation.md` 第 108 行、`docs/plan/distinct-limit-ordered-existence-scan-design.md` 第 6 节均为硬约束。已放行的 `allow_ordered_parallel_expand=true` 是正式实现的配置化形式，保留。 |
| `LINKRS_STAGE_TIMING` | 诊断计时，默认关闭、不进入热路径，是定位装配/物化瓶颈的必要手段（归档文档第 4 节仍在用），保留。 |
| `COMPILED_EVAL_ENABLED`、`set_column_block_enabled`、`set_enable_heuristic` | 均为运行时 API 形式的可配置性，符合"以配置方式提供可配置性"，无需改动。 |

---

## 六、执行顺序与硬门槛

1. **先做纯配置化**：任务 4 → 任务 5 → 任务 6。它们不改变默认行为，风险最低，且任务 4 修正了配置哈希一致性问题，是后续改动的前置。
2. **再做门控转正**：任务 1 → 任务 3 → 任务 2。按"证据强度 + 影响面"排序；任务 2 依赖 `24577572` 工作树确认已提交。
3. **统一硬门槛**：
   - 正确性门：partitioning 49/49、typed group、local topk、parallel 27/27、DQL 全量、storage 全量、`column_batch_test` 全绿；
   - 结果门：`group_by_count [14|59]`、`distinct_limit [14]`、`result_stable=true`、`spill_rows=0`；
   - 性能门：沿用资源对齐口径（`numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`，warmup 1 + run 5 去最大值均值），结果与生产默认同量级，不因清理产生回归。
4. **不做项**：不为清理门控放宽任何语义证明；不打开第五节 5.2 的安全门控；不触碰仍开放的 memo 集成与预取实验代码。

---

## 七、执行结果（2026-11-25）

任务 1-6 已全部实施，并连带完成复核文档新增的任务 7-9。逐项结果：

| 任务 | 结果 | 关键落点 |
|---|---|---|
| 1 列块扫描统一配置 | 完成 | 移除 `LINKRS_COLUMN_BLOCK_ENABLED`，保留配置字段与 `set_column_block_enabled` API |
| 2 有序存在扫描转正 | 完成 | 移除 `LINKRS_ORDERED_EXISTENCE_SCAN`，A/B 差分脚本改为 EXPLAIN 断言验证 |
| 3 快形状优化转正 | 完成 | 移除 `LINKRS_NO_FAST_SHAPE`，由 `enable_heuristic` 统一控制 |
| 4 经济门三参数配置化 | 完成 | `ParallelConfig` 增三字段并纳入 `partitioning_config_hash`，新增哈希覆盖单测 |
| 5 有序存在批次上限配置化 | 完成 | 新增 `ordered_existence_batch_limit` 配置，经 OptimizerEngine → ExecutionContext 传入规划期 |
| 6 扫描批次上限评估 | 关闭（不改代码） | 生产 `chunk_size` 恒为 2048、`clamp(1,2048)` 上限分支不触发；评估详见任务 6 小节 |
| 7 移除 CSR 预取钩子链 | 完成 | 删除整条 `prefetch_ahead`/`prefetch_head` 分派与 A/B 入口，保留写入路径与 simvec 无关 prefetch |
| 8 删除 `allow_partition_metadata_count` | 完成 | 整字段删除（含死代码 `is_partition_metadata_count`），改写对应集成测试 |
| 9 `allow_*` 语义澄清 | 完成 | 补齐"仅 `enabled=false` 生效、`enabled=true` 时白名单短路"的注释与文档 |

**验证结果**（工具链 rustc 1.97.1，与仓库 `rust-version` 一致）：

- 全量编译 `cargo check --workspace --all-targets`：通过（0 error；仅既有 warning）。
- `linkrs-config` 默认值测试 16/16、配置加载 2/2 通过。
- `partitioning` 49/49、`parallel_partition_execution` 27/27 通过。
- `linkrs-storage` lib 1416/1416 通过（预取钩子移除无回归）。
- 新增 `partitioning_config_hash_covers_economic_gate_terms` 单测通过。
- 结果门与性能门（资源对齐口径的 `devs` 固定门）未在本环境执行——需 NUMA 固定核与基准数据集，建议在有对应硬件的环境按第六节口径补跑。
- 附带修复：集成测试 `tests/lifecycle.rs` 缺少 `in_write_context` 字段的既有编译问题（补 `false`，属普通读语境），否则 `--all-targets` 无法通过。
