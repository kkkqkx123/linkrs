# sync 包本地/远程后端设计问题清单与改造决策

**日期**：2026-11-22  
**范围**：`crates/linkrs-sync` 及直接接线的 `linkrs-fulltext`、`linkrs-server`、`linkrs-api`、`docs/archive`  
**基线**：`kkkqkx123/linkrs` main（克隆时 HEAD `8ec9667`），Rust stable 1.100.0

## 0. 目标与约束

目标：把上一轮设计评估中确认的问题转成可执行清单，逐阶段落地，并保证改动可编译、可验证、与项目规范一致。

约束（来自仓库 `AGENTS.md`）：

- 代码、注释、日志、错误信息一律英文；文档用中文；代码注释不得引用任何计划编号。
- 不 `unwrap`；测试里可用 `expect`。
- 最小化 `dyn`，优先具体类型；所有动态派发必须在 `docs/archive/dynamic.md` 登记。
- 子 crate 保持严格 DAG，不新增环。
- 项目处于开发期，不做向后兼容；但注册表属于持久化数据，新字段用 `#[serde(default)]` 容错。

行号均为改造前基线行号；落地后第 11 节同步更新。

## 1. 问题清单

| 序号 | 问题 | 证据（file:line） | 影响 | 决策 | 阶段 |
|---|---|---|---|---|---|
| 1 | fast path 准入只看 vector 后端，不看 fulltext 后端 | `linkrs-sync/src/manager/delivery.rs:936-947`（`fast_path_deployment_ready`）、`:957-977`（`fast_path_eligible`）；`:440-462`（`apply_index_mutation` 分发 fulltext）、`:982-992`（fast apply 调用链） | `vector=local + fulltext=es` 时，ES 网络写入可能绕过持久 outbox，崩溃后无法从 frontier 重放 | **修复**：在两处准入判断中加入 fulltext 后端 `is_local()` 检查 | 1 |
| 2 | `VectorBackend::supports_payload_index` / `supports_streaming` 恒返回 `true`，且全仓无调用 | `linkrs-sync/src/backend.rs:1784-1793` | 硬编码能力位制造"扩展点"假象，未来后端需改派发点；与 AGENTS.md 最小化原则冲突 | **删除**（死代码，不做 trait 化） | 2 |
| 3 | `VectorIndexManager::supports_online_rebuild` 恒返回 `true`，且全仓无调用；fulltext 侧同名方法是真实能力查询 | `linkrs-sync/src/vector_index_manager.rs:330-332`；`linkrs-fulltext/src/manager.rs:120-122`；`linkrs-server/src/http/handlers/rebuild.rs:198` | 两套同名方法语义不一致，vector 侧是假能力位 | **删除** vector 侧死方法，保留 fulltext 侧真实能力查询 | 2 |
| 4 | `Arc<dyn VectorEngineHandle>` / `MetricsEngineHandle` / `Arc<dyn FulltextSearchEngine>` 未登记 | `linkrs-sync/src/backend.rs:40,1265`；`metrics_handle.rs:28`；`linkrs-fulltext/src/manager.rs:25`；`docs/archive/dynamic.md:58,164-186` | 违反 AGENTS.md "所有动态派发需登记" | **补文档**：新增条目并说明理由 | 3 |
| 5 | Qdrant 发布指针 `collection_overrides` / `promote_backups` 不持久化，重启靠 generation + 命名约定重采纳 | `linkrs-sync/src/vector_index_manager.rs:41-46,151-189,904-920`；`vector_rebuild.rs:877-921`；`docs/plan/fulltext-vector-convergence-plan.md:127,183` | 注册表回落派生名；恢复依赖 `*.rebuild-{gen}` 解析和 outbox 活跃 generation，crash 窗口内可能读到旧 collection | **修复**：扩展注册表持久化结构，并让所有改写点触发保存 | 4 |
| 6 | trait 暴露 `local_engine()` / `qdrant_manager()` 具体类型逃生口 | `linkrs-sync/src/backend.rs:61-69`；`linkrs-server/src/startup.rs:174-200`；`linkrs-api/src/embedded/database.rs:1097-1102` | 抽象泄漏，新增后端需实现/默认这些方法 | **本次不改，列开放点**：现有用途是启动指标采样与断言，属合理具体类型需求；替换需先设计统一的指标探针接口 | 开放点 |

## 2. 设计决策总表

| 决策点 | 选择 | 理由 |
|---|---|---|
| fast path 修复位置 | 改 `SyncManager` 的两个准入函数，不改 `apply_committed_fast` | 准入集中，fallback 路径已存在（`:1035-1043`），改动面最小 |
| fulltext 本地判定的信息来源 | 新增 `FulltextIndexManager::is_local()`，经 `FulltextSyncCoordinator` 暴露 | `supports_online_rebuild()` 语义是"能否在线重建"，不能直接当"是否本地"用；ES 两项都是 false，但概念不同 |
| 三个硬编码能力位 | 直接删除而非下沉到 trait | 全仓无调用；下沉会扩大本已偏大的 `VectorEngineHandle`，与 AGENTS.md 相悖。真需要时再按实际调用点设计 |
| 动态派发文档 | 新增条目，不改动运行时代码 | 零风险，纯合规 |
| 远程发布映射持久化 | 在 `PersistedLogicalIndex` 增加 `collection_name` / `live_name` / `collection_override` / `promote_backup`，全部 `#[serde(default)]`；所有 setter 触发保存 | 显式状态优先于隐式约定；保存实际名可绕开 load 时 granularity 尚未注入的时序问题；保留重采纳作为 crash 窗口兜底 |
| 注册表名不要重算 | load 时优先用持久化的 `name` | 现实现 `:222` 用 `collection_name_for` 重算，load 发生在 `set_granularity` 之前，粒度时序有隐患 |
| trait 逃生口 | 保留，登记为开放点 | 替换成本 > 当前收益，且用途正当 |

## 3. 阶段划分

| 阶段 | 内容 | 涉及文件 | 风险 |
|---|---|---|---|
| 1 | fast path fulltext 后端准入 | `linkrs-fulltext/src/manager.rs`、`linkrs-sync/src/fulltext_sync.rs`、`linkrs-sync/src/manager/delivery.rs` | 低 |
| 2 | 删除三个死代码能力位 | `linkrs-sync/src/backend.rs`、`linkrs-sync/src/vector_index_manager.rs` | 低 |
| 3 | 动态派发文档登记 | `docs/archive/dynamic.md` | 无 |
| 4 | 远程发布映射持久化 | `linkrs-sync/src/vector_index_manager.rs`、`vector_rebuild.rs`、`docs/plan/fulltext-vector-convergence-plan.md` | 中 |
| 5 | 编译验证 + 文档状态同步 | 全仓 `cargo check`、本文档第 11 节 | — |

## 4. 阶段 1 diff（fast path 准入）

`crates/linkrs-fulltext/src/manager.rs`，在 `supports_online_rebuild`（:120）附近新增：

```rust
    /// Whether this manager serves indexes in-process rather than through a
    /// remote cluster. Delivery fast paths require every target to be local.
    #[cfg(feature = "fulltext")]
    pub fn is_local(&self) -> bool {
        self.config.backend == FulltextBackendKind::Local
    }
```

`crates/linkrs-sync/src/fulltext_sync.rs`，在 `impl FulltextSyncCoordinator` 内新增：

```rust
    /// Whether the fulltext backend runs in-process. Network-backed indexes
    /// must stay on the durable outbox path.
    #[cfg(feature = "fulltext")]
    pub fn is_local(&self) -> bool {
        self.fulltext_manager.is_local()
    }
```

`crates/linkrs-sync/src/manager/delivery.rs`，在 `fast_path_deployment_ready`（:940 之后）与 `fast_path_eligible`（:961 之后）各插入同一段：

```rust
        #[cfg(feature = "fulltext")]
        if let Some(coordinator) = self.sync_coordinator.as_ref() {
            if !coordinator.is_local() {
                return false;
            }
        }
```

该模块本身在 `fulltext` feature 下编译相关代码，字段 `sync_coordinator` 亦由 `#[cfg(feature = "fulltext")]` 保护，不需额外 feature 组合处理。

## 5. 阶段 2 diff（删除死代码能力位）

`crates/linkrs-sync/src/backend.rs`：删除 `:1780-1793` 的 `supports_payload_index` 与 `supports_streaming` 两个方法及其文档注释。

`crates/linkrs-sync/src/vector_index_manager.rs`：删除 `:327-332` 的 `supports_online_rebuild` 及其文档注释。

删除前已全仓 grep 确认：这三个符号只有定义、没有调用点，也没有跨 crate 引用。

## 6. 阶段 3 diff（动态派发登记）

`docs/archive/dynamic.md` 新增两节（中文），并入"保留的 dyn 使用"表：

- `Arc<dyn VectorEngineHandle>`：`backend.rs:40/1265`，向量后端运行时选择 local / Qdrant，I/O 密集、虚调用开销可忽略；`MetricsEngineHandle` 为同 trait 的装饰器。
- `Arc<dyn FulltextSearchEngine>`：`linkrs-fulltext/src/manager.rs:25`，本地 Tantivy 与 ES 后端按配置运行时选择。

## 7. 阶段 4 diff（远程发布映射持久化）

`crates/linkrs-sync/src/vector_index_manager.rs`：

1. `PersistedLogicalIndex`（:69-77）增加字段：

```rust
    #[serde(default)]
    collection_name: Option<String>,
    #[serde(default)]
    live_name: Option<String>,
    #[serde(default)]
    collection_override: Option<String>,
    #[serde(default)]
    promote_backup: Option<String>,
```

2. `save_logical_indexes`（:154-189）的 entry 构造增加：

```rust
                collection_name: Some(entry.value().name.clone()),
                live_name: Some(self.live_collection_name(&entry.key())),
                collection_override: self
                    .collection_overrides
                    .get(entry.key())
                    .map(|target| target.clone()),
                promote_backup: self
                    .promote_backups
                    .get(&self.live_collection_name(&entry.key()))
                    .map(|backup| backup.clone()),
```

3. `load_logical_indexes`（:191-231）改为用持久化值恢复，而不是无条件重算：

```rust
            if let Some(target) = entry.collection_override {
                self.collection_overrides.insert(location.clone(), target);
            }
            if let Some(backup) = entry.promote_backup {
                self.promote_backups
                    .insert(self.live_collection_name(&location), backup);
            }
            let name = entry
                .collection_override
                .clone()
                .or(entry.collection_name)
                .unwrap_or_else(|| self.collection_name_for(&location));
```

   注意 `entry.collection_override` 需先借用再移动，实现时改成先取 `let remap = entry.collection_override.clone();` 再用 `remap` 插入与解析名。

4. 改写点触发保存：`set_collection_override`（:904）、`set_logical_collection_name`（:911）、`clear_collection_override`（:918）末尾调用 `self.save_logical_indexes()`；`publish_temp_field` 本地分支写入 `promote_backups` 后（:971-973）、`restore_promote_backup` 远程分支写 override 后（:1197-1200）同样调用保存。

5. `vector_rebuild.rs:877-921` 的重采纳逻辑保留为兜底：它覆盖"publish 已发生但持久化未完成"的 crash 窗口；持久化命中后重采纳算出的 temp 与 override 相同，重复设置幂等。

`docs/plan/fulltext-vector-convergence-plan.md`：把 :127 与 :183 中"未持久化 `collection_overrides`，属于既有遗留"更新为"已纳入 `sync-backend-design-decisions.md` 阶段 4 并落地"。

## 8. 测试策略与落地情况

- 阶段 1：`FulltextIndexManager::is_local()` 是对配置字段的纯判断，构造 ES 管理器需要额外配置接线，故未新增单测；由 `fulltext-es` feature 的编译校验 + 代码审查覆盖。
- 阶段 4：已新增 `vector_index_manager::registry_tests::publish_routing_state_survives_reopen`——写入 override 与 promote backup，重新 `attach_registry_directory`，断言 `collection_name_for` 返回 override、`promote_backup_for` 返回 backup。
- 全阶段：`cargo check` 覆盖 default、fulltext、fulltext-es、vector-qdrant 组合；`cargo test` 覆盖 sync lib 全量。

## 9. 验证

以本机 rsproxy 源和 stable 1.100.0 执行。`vector-client` 的 protobuf 生成需要 `protoc`，已在本机安装 libprotoc 3.21.12，四个 feature 组合全部验证成功。

**预存警告清理（后续追加，已全部修复）**：首轮验证时四个组合共出现 38+ 条 warning，来源分三类：

1. 代码级 unused（`backend.rs` 的 `CollectionDefaults`、`vector_sync.rs` 的 `VectorCoordinatorError`、`manager/types.rs` 的函数内 `use ClassifySearchError`、`traits_reader.rs` 的 `handles`、`batch.rs` 的 `space`、`vector-client/config.rs` 的三个 Hint 类型）：按实际使用域收敛——仅特定 feature 用的加 `#[cfg]`、仅测试用的移入 `#[cfg(test)] mod tests`、未用参数加 `_` 前缀。
2. 死代码与文档（linkrs-storage 的 `clear_unavailable_columns` 仅测试调用 → **删除生产方法，测试自定义辅助函数**（见下方说明）；linkrs-query 的 doc comment 挂在 `thread_local!` 宏上 → 改普通注释；ddl.rs 五个 `pub use *` 可见性超过被导出项 → 降为 `pub(in ...arena_builder)`）。
3. manifest 级 `cargo::redundant_homepage`（Cargo 1.100 新 lint，homepage 与 repository 同值）：删除 19 个 Cargo.toml 的冗余 `homepage` 字段；`linkrs-cli` 未使用的 `async-trait` 依赖一并移除。

最终 `cargo check --workspace` 及三个 sync feature 组合均 **0 warning / 0 error**，测试 54 passed。修复明细与设计取舍见第 12 节。

## 10. 待拍板开放点

1. **trait 逃生口**：是否引入统一的 `VectorMetricsProbe` 接口替代 `as_local()` / `as_qdrant_manager()`？需先决定指标采样接口归属 crate（可能触发 DAG 调整）。
2. **能力位下沉**：若未来新增不支持 streaming 或 payload index 的后端，是走 trait 默认方法还是关联类型？本次按"用到再设计"处理。
3. **ES 在线重建**：alias swap 是否立项？当前仍由 `supports_online_rebuild()` 显式拒绝。
4. **持久化文件格式演进**：`PersistedLogicalIndex` 未来若需多代备份，是否改为顶层对象而非扁平数组？本次保持数组 + `#[serde(default)]`。

> 以上四项的逐步设计分析、推荐结论与触发条件见第 12 节。第 12 节为设计建议，未改动运行时代码，仍需项目方拍板后才进入实施阶段。

## 11. 实施状态

所有阶段已落地，验证结果如下。

| 阶段 | 状态 | 验证结果 |
|---|---|---|
| 1 | 已落地 | `cargo check -p linkrs-sync --features fulltext,vector` 与 `--features fulltext-es,vector` 均通过 |
| 2 | 已落地 | 全仓 grep 确认三个符号无残留引用；上述 check 通过 |
| 3 | 已落地 | 文档变更，无编译影响 |
| 4 | 已落地 | `publish_routing_state_survives_reopen` 通过；同步更新 `fulltext-vector-convergence-plan.md` 的过期描述 |
| 5 | 已落地 | `cargo check --workspace` 通过；`cargo check -p linkrs-sync --features vector-qdrant` 通过；`cargo test -p linkrs-sync --features fulltext,vector --lib` 54 passed / 0 failed |

最终改动文件：

- `crates/linkrs-fulltext/src/manager.rs`（阶段 1）
- `crates/linkrs-sync/src/fulltext_sync.rs`（阶段 1）
- `crates/linkrs-sync/src/manager/delivery.rs`（阶段 1）
- `crates/linkrs-sync/src/backend.rs`（阶段 2）
- `crates/linkrs-sync/src/vector_index_manager.rs`（阶段 2、4 + 新测试）
- `docs/archive/dynamic.md`（阶段 3）
- `docs/plan/fulltext-vector-convergence-plan.md`（阶段 4 文档同步）
- 本文档

## 12. 预存警告修复与开放点设计分析

### 12.1 预存警告修复清单

追加修复（原第 9 节标注为"既有、未纳入范围"的全部 warning），逐项如下。行号为修复前基线。

| 警告 | 位置 | 根因 | 修复手段 |
|---|---|---|---|
| unused import `CollectionDefaults` | `linkrs-sync/src/backend.rs:30` | 仅在 `QdrantEngineHandle`（:744/1322/1416）使用，非 qdrant 组合下悬空 | import 加 `#[cfg(feature = "vector-qdrant")]` |
| unused import `VectorCoordinatorError` | `linkrs-sync/src/vector_sync.rs:17` | 全文件仅此一处，实际只用 `VectorCoordinatorResult` | 从 use 列表移除 |
| unused import `ClassifySearchError`（函数内 use） | `linkrs-sync/src/manager/types.rs:121` | impl 目标已是全路径 `impl ...ClassifySearchError for SyncError`，块内 use 冗余 | 删除该行 |
| unused variable `handles` | `linkrs-storage/src/client/traits_reader.rs:199` | trait 默认方法签名保留该参数但实现未用 | 改名 `_handles` |
| unused variable `space` | `linkrs-storage/.../writer/batch.rs:221` | `batch_insert_grouped` 全程只用 `space_id` | 改名 `_space` |
| method `clear_unavailable_columns` never used | `linkrs-storage/.../core/maintenance.rs:65` | 生产路径不走它，仅 `vertex_table/sharded/write/tests.rs:393` 调用 | 删除生产方法；在 `write/tests.rs` 定义测试辅助函数，经 `pub(super)` 的 `columns` 字段直调底层 `ColumnStore::clear_unavailable_columns()`（该项为 `pub`） |
| unused doc comment | `linkrs-query/.../copy/from.rs:361` | `///` 挂在 `thread_local!` 宏上，宏不产生文档项 | 改 `//` |
| 5× glob import 不重导出 pub 项 | `linkrs-query/.../specs/ddl.rs:9-13` | `pub use *` 可见性（pub）超过被导出项（最高 `pub(in arena_builder)` / `pub(crate)`） | 降为 `pub(in crate::executor::streaming::plan::arena_builder)`，consumer `specs.rs:14` 的 `pub(super)` 仍可消费 |
| unused imports 三个 Hint | `vector-client/src/engine/http/config.rs:5-6` | 仅 `#[cfg(test)] mod tests`（:485+）使用 | 从顶层 use 移除，在 tests 模块内 use |
| unused dependency `async-trait` | `linkrs-cli/Cargo.toml:32` | cli 源码无 `#[async_trait]` 或该 crate 引用 | 删除依赖行 |
| 19× `package.homepage` redundant | 根 + 17 个 crate 的 Cargo.toml + `simvec` | Cargo 1.100 `cargo::redundant_homepage`，homepage 与 repository 同值 | 删除冗余 `homepage` 字段（语义无损，cargo 官方建议） |

**取舍说明**：

- `simvec/Cargo.toml` 是递归拉取的 git 子模块（独立仓库，gitlink 指向 `0327092`）。其冗余 homepage 属上游代码，本次一并删除以使工作区编译零警告。**代价**：主仓库 `git diff` 会显示子模块 dirty（`crates/simvec`），但该改动不在主仓库提交内，上游 `git submodule update` 会覆盖。建议后续在上游 simvec 仓库单独提交同一行删除，避免反复出现。
- `_handles` / `_space` 选择"保留参数改名"而非删除参数：二者都是 trait 方法签名或内部函数签名的一部分，改名零风险，删除需改所有调用点且破坏接口对称性。
- **`clear_unavailable_columns` 不用 `#[cfg(test)]`**：生产代码不保留任何测试专用方法。该 `VertexTable` 方法仅一个测试调用，直接删除；测试在 `write/tests.rs` 内自定义辅助函数，经 `pub(super)` 的 `columns` 字段调用本就是 `pub` 的 `ColumnStore::clear_unavailable_columns()`。原则：仅测试使用的生产代码是多余的，辅助逻辑归测试自身；生产代码不出现任何形式的测试条件编译。
- 未对任何 warning 使用 `#[allow(...)]` 全局压制：全部按真实作用域收敛，避免掩盖后续新增问题。

### 12.2 开放点 1：trait 逃生口（`as_local()` / `as_qdrant_manager()`）

**现状事实**：

- trait `VectorEngineHandle` 在 `linkrs-sync/src/backend.rs:41` 定义，含两个具体类型逃生口：`local_engine() -> Option<Arc<LocalVectorEngine>>`（:62）与 `qdrant_manager() -> Option<Arc<VectorManager>>`（:68，cfg vector-qdrant）。
- 实际调用点仅两处生产用途 + 一处测试：`linkrs-server/src/startup.rs:174-200` 用它们取引擎/连接信息启动指标采样器（`vector_metrics.rs:423/442`）；`linkrs-api/src/embedded/database.rs:1100` 用 `as_local()` 做断言。
- 依赖方向：`linkrs-server` → `linkrs-sync` →（`simvec` / `vector-client`），`linkrs-metrics` 被 `linkrs-sync` 依赖（Cargo.toml:28）但指标采样器实现在 `linkrs-server`。

**分析**：逃生口之所以存在，是因为指标采样需要**具体引擎句柄**（local 采样读 `LocalVectorEngine` 内部统计；remote 采样读连接配置发 HTTP）。这是宿主层（server）的横切关注点，不是 sync 层的同步职责——sync trait 为服务宿主的观测需求而泄漏具体类型，边界不清。

**推荐设计（分两步，按触发条件推进）**：

1. **现在不做**。当前仅两个引擎、采样逻辑稳定，逃生口的收益（类型安全宿主集成）大于成本（trait 多两个方法）。
2. **触发条件**：当出现第 3 个向量后端（如远程 HTTP-only 引擎）时，改为**指标快照探针**：在 `linkrs-metrics` 定义 `VectorBackendProbe` trait，方法返回后端无关的采样数据（如 `fn sample(&self) -> VectorBackendSnapshot`，snapshot 结构放 `linkrs-metrics`），各引擎句柄实现它；`linkrs-sync` 的 `VectorBackend` 暴露 `fn metrics_probe(&self) -> Arc<dyn VectorBackendProbe>`。`linkrs-server` 只依赖 probe，不再关心具体引擎。
   - 归属 crate 选 `linkrs-metrics` 而非 `linkrs-sync`：避免 sync 依赖观测抽象，也符合该 crate "任何 crate 可依赖、不拉 core 数据模型"的定位（`linkrs-metrics/src/lib.rs:1-6`）。不触发 DAG 环——`linkrs-metrics` 已是叶子层。
   - 代价：新增一个 trait + snapshot 结构，server 侧采样器改为面向 snapshot。收益：trait 真正引擎无关，新增后端不再碰 sync trait。

**结论**：维持现状到第 3 个后端出现；届时按 `VectorBackendProbe` 方案重构，归 `linkrs-metrics`。开放点保留。

### 12.3 开放点 2：能力位下沉（streaming / payload index）

**现状事实**：

- 上一阶段已删除 `VectorBackend::supports_payload_index` / `supports_streaming` 与 `VectorIndexManager::supports_online_rebuild` 三个硬编码 `true` 的死代码（全仓无调用）。
- 真正生效的能力查询在 `VectorEngineHandle`：`engine_name()`（:43）、`is_in_process()`（:49）——后者是 trait 默认方法 + 各实现覆写，调用方按能力而非引擎名分支。全文侧 `supports_online_rebuild()`（`linkrs-fulltext/src/manager.rs:127`）是真实配置判断。

**分析**：问题的本质是"能力建模方式"。两种候选：

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| trait 默认方法 | 每个能力一个 `fn supports_x(&self) -> bool { false }`，新后端按需覆写 | 简单、对象安全、调用点直观；与现有 `is_in_process` 一致 | 能力增长会持续膨胀 trait；无法编译期保证"声明了就实现" |
| 关联类型 / const | `type Capabilities` 或 `const CAPS: CapSet` 位标志 | 能力集中声明，新增能力不动 trait 方法集；可做编译期组合 | 位标志可读性差；`dyn` 无法用关联类型（对象安全破坏），而本项目明确用 `Arc<dyn VectorEngineHandle>` |

**推荐**：**走 trait 默认方法，不要关联类型**。决定性理由是对象安全——`VectorBackend` 持有 `Arc<dyn VectorEngineHandle>`（`backend.rs:1265`），关联类型会使 trait 失去对象安全，与 `docs/archive/dynamic.md` 已登记的架构直接冲突。默认 bool 方法与现有 `is_in_process` 同构，一致性最好。

**触发条件**：仅当某新后端确实不支持某能力时，才在 trait 加对应 `supports_x()` 默认方法并让该后端保持 `false`、其余用默认 `true`。**现在不加任何能力位**（沿用"用到再设计"）。

**结论**：方案定为"trait 默认 bool 方法"，关联类型因破坏对象安全被排除；仍按用到再设计，本次不动代码。开放点可关闭（方向已定）。

### 12.4 开放点 3：ES 在线重建 alias swap

**现状事实**：

- `FulltextIndexManager::supports_online_rebuild()`（`linkrs-fulltext/src/manager.rs:127-129`）仅在 `backend == Local` 时返回 true。
- `create_rebuild_engine()`（:967-985）对 ES 直接返回 `SearchError::NotSupported`，注释明确："ES 需要 alias swap，尚未实现；增量同步完全支持"。
- 本地重建靠临时目录 + 原子目录 rename 发布；ES 侧对应原语应是索引 alias 原子切换（`indices.update_aliases`）。
- 向量侧已有可借鉴范式：Qdrant 用 `collection_overrides` + generation 逻辑指针重映射（阶段 4 已持久化）。

**分析**：ES alias swap 是**功能增强而非设计缺陷**。当前设计"显式拒绝在线重建 + 增量同步兜底"是自洽且安全的——它避免了"建了 scratch 却无法发布"的半成品状态。立项与否取决于是否有真实的 ES 大规模重建需求（停机窗口不可接受）。

**推荐设计（若立项）**：

1. 发布原语：ES 侧建 `<live>-v<generation>` 新索引 → 数据灌入 → `POST /_aliases` 原子执行 `remove(live→旧) + add(live→新)`。
2. 状态建模：复用向量侧思路，把"当前 alias 指向的物理索引"作为显式状态持久化（类似阶段 4 的 `collection_override`），不要靠索引名解析猜测。
3. 能力闸门：`supports_online_rebuild()` 改为读后端配置/特性开关，不再硬编码后端类型判断，为未来其他远程后端留口。
4. 回滚：alias 指回旧索引；旧索引保留 N 代后清理（与开放点 4 的多代备份呼应）。
5. 测试：需要真实 ES 或 testcontainers 做端到端 alias 切换验证——这是当前测试体系的缺口（本地有集成测试，ES 只有构造/单测）。

**结论**：**暂不立项**。理由：增量同步 + durable outbox 已保证一致性，在线重建是优化；且缺失 ES 端到端测试基础设施，先建测试再谈 alias swap。维持当前显式拒绝。开放点保留，触发条件＝"出现 ES 大索引重建停机不可接受的实际需求"。

### 12.5 开放点 4：持久化文件格式演进（多代备份）

**现状事实**：

- `PersistedLogicalIndex`（`linkrs-sync/src/vector_index_manager.rs:69-85`）是扁平结构：一个逻辑索引一条记录，阶段 4 新增 4 个 `#[serde(default)]` 的 `Option<String>` 字段（`collection_name` / `live_name` / `collection_override` / `promote_backup`）。
- 文件是这些记录的数组（`vector_metadata.json`），load 时逐条恢复。
- `promote_backup` 当前只存**单个**备份名——即"最近一次发布前的 live collection"。

**分析**：问题在于"多代备份"需求下，单个 `promote_backup` 不够。三种演进路径：

| 方案 | 结构 | 兼容性 | 适用 |
|---|---|---|---|
| A. 保持数组 + 单字段 | 现状 | 完全兼容 | 只需 1 代回滚 |
| B. 数组 + 备份列表字段 | 加 `#[serde(default)] promote_backups: Vec<BackupEntry>` | 兼容（旧文件 default 空） | 需要 N 代回滚 |
| C. 顶层对象 | `{ version, indexes: [...] }` | 需读旧数组并迁移 | 需要 schema 版本治理、元数据扩展 |

**推荐**：**现在保持 A；需要多代时选 B，暂不上 C**。理由：

- B 用 `#[serde(default)] Vec` 可无缝兼容现有文件，改动局部（一个字段 + load/save 逻辑），且 `BackupEntry { name, generation, created_at }` 能表达多代与保留策略。
- C（顶层对象）的价值是 schema 版本治理，但代价是必须实现"旧数组 → 新对象"的迁移读取（否则升级即丢注册表）。当前只有一代扁平结构、无版本字段，引入 C 的迁移成本此刻不划算。
- 触发条件：当注册表需要**第二个非索引级字段**（如全局 granularity、schema version）时再上 C，届时一次性做迁移读取，避免两次破坏性格式变更。

**结论**：本次维持数组 + `#[serde(default)]` 不变；多代备份走"加 `Vec<BackupEntry>` 字段"（方案 B）；顶层对象（C）推迟到出现第二个全局字段时再连同迁移一起做。开放点保留，方向已明确。

### 12.6 四项结论汇总

| 开放点 | 推荐 | 是否需项目方拍板 | 触发条件 |
|---|---|---|---|
| 1 trait 逃生口 | 维持到第 3 个后端；届时用 `VectorBackendProbe`（归 `linkrs-metrics`） | 是（涉及新 crate 接口） | 第 3 个向量后端出现 |
| 2 能力位下沉 | trait 默认 bool 方法；排除关联类型（破坏 dyn 对象安全） | 否（方向已定） | 新后端缺某能力时按需加 |
| 3 ES alias swap | 不立项；维持显式拒绝 + 增量兜底 | 是（功能优先级） | ES 大索引重建停机不可接受 |
| 4 格式演进 | 现维持数组；多代走 `Vec<BackupEntry>`；顶层对象推迟 | 部分（顶层对象需拍板） | 第二个全局字段出现时上顶层对象 |

本节为设计分析，未改动运行时代码；落地时按各自触发条件另起实施阶段并更新第 11 节状态表。
