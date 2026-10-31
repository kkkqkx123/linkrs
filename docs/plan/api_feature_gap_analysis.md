# API 功能矩阵缺口分析与修改方案

基于 `docs/api/feature_matrix.md` 对照当前代码逐项核查后的缺口清单。缺口分三类：

- **A 类：文档漂移** —— 矩阵描述与代码实际不一致，只需修正文档（或核实后修正代码认知）。
- **B 类：跨层能力缺口** —— 某层 API 确实缺少其他层已具备的能力，需要补代码。
- **C 类：设计性缺口** —— 有意为之（架构上不适用或用替代方案），维持现状，但矩阵/文档需明确标注理由。

---

## A 类：文档漂移

### A1. C API 一行描述严重过时

**现状**：`crates/linkrs-api/src/embedded/c_api/` 实际有 15 个 FFI 模块
（`batch` / `busy_handler` / `config` / `database` / `error` / `function` / `migration` /
`query` / `result` / `session` / `statistics` / `transaction` / `types` / `value` 等），
约 86 个 `extern "C"` 导出，覆盖远超矩阵所述的"查询/事务/Schema/validate/explain/kill/count 子集"：

| 实际已有能力 | FFI 证据 |
|---|---|
| 批量导入 `BatchInserter` | `batch.rs`（`linkrs_batch_*` 9 个） |
| UDF / 聚合 / 扩展加载 | `function.rs`（`linkrs_create_function/aggregate/load_extension` 等 11 个） |
| 配置构造与 setter | `config.rs`（`linkrs_config_*` 10 个） |
| 迁移执行/计划/回滚 | `migration.rs`（`linkrs_migration_*` 4 个） |
| 会话统计 | `statistics.rs`（`linkrs_session_get_statistics` 等 5 个） |
| busy handler / hooks | `busy_handler.rs`（commit/rollback/update hook、interrupt） |
| 保存点 | `transaction.rs`（`linkrs_txn_savepoint/release/rollback_to`） |

同时矩阵声称的 "Schema" 覆盖并不准确：C API 无独立 schema CRUD FFI，
空间/标签/边类型操作只能经 `linkrs_execute` 走 DDL 文本执行。

**修改方案**：

1. 修正 `docs/api/feature_matrix.md` C API 行：拆分描述为
   "查询/事务/保存点/Schema(DDL 文本)/validate/explain/kill/count + BatchInserter/UDF/扩展/配置构造/迁移/会话统计/hooks"。
2. 说明段（末条）同步改写，注明 Schema 覆盖方式是 DDL 文本而非结构化 FFI。
3. 顺带在说明中补记 C API 当前**确实缺失**的能力（见 B1），使矩阵 ⚠️ 标记有明确依据。

**验收**：矩阵 C API 行与 `c_api/` 目录导出清单一一对应，无夸大、无漏报。

---

## B 类：跨层能力缺口

### B1. C API 相对嵌入式 Rust API 的能力缺口

**现状**：`c_api` 是嵌入式 API 的 FFI 投影，但以下嵌入式能力未投影：

| 缺失能力 | 嵌入式实现位置 |
|---|---|
| 预编译语句 `prepare/bind/execute` | `crates/linkrs-api/src/embedded/session/prepared.rs` |
| 会话变量 `$name`（set/remove/snapshot） | `crates/linkrs-api/src/session_variables.rs`、`embedded/session/state.rs` |
| 运行时配置读写 `get_config_value/update_config_value` | `crates/linkrs-api/src/embedded/database.rs`（`config.rs` 仅有构造期 setter，无运行时读写） |
| `export_query_to_file` | `crates/linkrs-api/src/embedded/session/export.rs` |
| `database_overview` | `crates/linkrs-api/src/embedded/database.rs` |
| `list_transactions` / `transaction_metrics` | `embedded/session/transactions.rs`、`embedded/database.rs`（C API 仅 `transaction_count`/`kill_transaction`） |

**修改方案**（按 FFI 高频程度排序）：

1. **会话变量**：`c_api/session_variables.rs` 新增
   `linkrs_session_set_variable / linkrs_session_remove_variable / linkrs_session_get_variable / linkrs_session_variables_snapshot`，
   复用 `session_variables.rs` 的事务感知逻辑；快照返回 JSON 字符串由 `linkrs_free_string` 释放。
2. **运行时配置**：在 `c_api/config.rs` 增补
   `linkrs_config_get_value / linkrs_config_update_value / linkrs_config_reset_value / linkrs_config_bulk_update`，
   直接转发 `embedded::Database` 的对应方法。
3. **事务可观测**：`c_api/transaction.rs` 增补 `linkrs_transaction_list`（JSON）与 `linkrs_transaction_metrics`。
4. **预编译语句**：`c_api/prepared.rs` 新增
   `linkrs_prepare / linkrs_stmt_bind_* / linkrs_stmt_execute / linkrs_stmt_free`，
   handle 生命周期对齐现有 `linkrs_txn_*` 风格。
5. **其余**：`database_overview`（JSON 返回）、`export_query_to_file`（路径 + 格式参数）并入 `c_api/database.rs` / 新 `c_api/export.rs`。

统一约定：错误码走 `linkrs_errcode`，字符串/JSON 一律堆分配 + `linkrs_free_string`，
不引入 `unsafe` 之外的新约束（FFI 本身属 `docs/archive/unsafe.md` 记录的低层操作）。
每个新模块附 C 侧集成测试（对齐现有 `c_api` 测试风格）。

**验收**：矩阵 C API 行升级为 ✅（除用户管理/同步运维/Web 管理外）；
新增 FFI 有测试覆盖；`c_api` 特性门控构建通过。

### B2. 无 gRPC 客户端封装

**现状**：`crates/linkrs-client` 仅 `HttpClient`（reqwest，基址固定 `/v1`）；
gRPC 调用方须以 `proto/` + 生成的 `LinkrsService` 直连，与 HTTP 侧开箱体验不对等。
CLI 远程模式也因此只走 HTTP。

**修改方案**（二选一，建议方案 1）：

1. **新增 `GrpcClient`**：在 `crates/linkrs-client` 增加 `grpc` feature，
   基于 tonic 封装 `LinkrsServiceClient`，方法集与 `HttpClient` 对齐
   （查询/事务/会话/会话变量/查询管理/配置/导入导出/流式），
   DTO 复用 `linkrs-wire`，流式接口返回 tonic `Streaming` 的薄包装。
   CLI 远程模式可增 `--transport grpc` 选项（后续再议）。
2. **明确不做**：在 feature_matrix 说明段记录"gRPC 客户端以 proto 直连为准"为长期决策，
   矩阵保持现状。

**验收**（方案 1）：`GrpcClient` 与 `HttpClient` 公共方法集合做一次 diff 对齐并有测试；
矩阵客户端列区分 HTTP/gRPC 两小列。

### B3. 嵌入式缺 Schema 版本历史读

**现状**：版本历史读是服务端 store 专属（HTTP `/schema/versions/*`、gRPC `GetVersionHistory`、
Web `schema_ext/migration_read.rs` 只读），嵌入式仅 `migration` 计划/执行/回滚，矩阵标 ⚠️。

**修改方案**：

1. 先确认版本历史数据的实际存放位置（服务端运行时 store 还是持久化于 `linkrs-storage`）。
2. 若持久化在存储层：把读取逻辑下沉为 `linkrs-migration` 或 `linkrs-storage` 的公共 API，
   嵌入式 `Database` 暴露 `list_schema_versions` / `get_version_history`，
   HTTP/gRPC/Web 三侧改调同一实现，消除目前各层各自读 store 的重复。
3. 若历史仅存在于服务端内存 store：属数据模型问题，需先在迁移执行时把版本记录落盘，
   再做第 2 步；此路径工作量大，排在 B1/B2 之后。

**验收**：矩阵该行嵌入式列由 ⚠️ 升 ✅（或维持 ⚠️ 并注明数据落盘前提）。

### B4. 嵌入式缺 system/search/freeze 统计

**现状**：嵌入式有会话/查询统计 + `database_overview`（`StatsManager` 计数器子集），
无 system/search/freeze 全局注册表（服务端 `/statistics/*` 有）。

**修改方案**：评估把 system/search 统计注册表下沉到 `linkrs-metrics`（DAG 最底层，天然可被全链路依赖），
freeze/profile 能力经 `StatsManager` 暴露到嵌入式 `database_overview` 或独立方法。
优先级低；若无嵌入式运维场景需求，可维持 ⚠️ 并在矩阵注明"嵌入式以单进程自监控为主"。

---

## C 类：设计性缺口（维持，需标注理由）

### C1. 预编译语句仅嵌入式有（HTTP/gRPC/CLI 用带参查询代替）

远程侧无计划缓存语义，带参查询已覆盖注入安全与解析复用的大部分收益。
若未来出现"高频重复查询 + 解析成为瓶颈"的场景，再在 gRPC 增加 `Prepare/ExecutePrepared`
并让 HTTP 复用。矩阵维持 "—"，在说明段补充该决策依据（现有说明已提及，可加一句"有意不做"）。

### C2. CLI `\set` 与服务端 `$` 变量不同步

两者概念不同（本地替换变量 vs 服务端会话变量），不自动同步是正确决策。
**低成本体验改进**（可选）：`LET` 执行后提示对应远程会话变量；CLI 增加
`\variables`（或 `\sessions variables`）查看远程会话变量快照，减少用户混淆。

### C3. 嵌入式无用户管理/同步运维/Web 管理

进程隔离、单节点架构决定非嵌入式职责，矩阵标记 — 合理，无需变更。

### C4. Web 管理 API 覆盖窄

现状 web `/api/v1` 仅：查询历史/收藏（`metadata.rs`）、Schema 扩展只读+迁移只读
（`schema_ext/`）、数据浏览（`data_browser.rs`）、图数据（`graph_data.rs`）；
会话/事务/配置/同步运维均无 Web 端 API，执行靠前端代理 `/v1`。

若 Web 管理需要运维能力，建议按需增量补齐（复用 `http` 层 service，不重复实现业务）：

1. **优先**：运行中查询列表 + kill（Web 已有查询执行场景，用户常需终止慢查询）、
   会话列表/查看。
2. **其后**：事务列表/metrics、配置查看与修改、同步运维只读面板、statistics 面板。
3. 每补一项同步更新矩阵 Web 列。

当前若无产品需求，矩阵 Web 列维持 —，不预先建设（避免 over-engineering）。

---

## 优先级排序

| 顺序 | 事项 | 类别 | 理由 |
|---|---|---|---|
| 1 | 修正矩阵 C API 行 + 说明段 | A1 | 零代码成本，消除误导 |
| 2 | C API 补会话变量 + 运行时配置读写 | B1-1/2 | FFI 高频能力，嵌入式已有实现可直接投影 |
| 3 | C API 补事务列表/metrics、prepared、export、overview | B1-3/4/5 | 完成 FFI 投影对齐 |
| 4 | gRPC 客户端决策（做/不做）+ 落地 | B2 | 影响调用方选型，需尽早定调 |
| 5 | 嵌入式版本历史读（视数据落盘情况） | B3 | 涉及存储层改动，需先调研 |
| 6 | Web 管理按需扩展（查询 kill/会话先行） | C4 | 依赖产品需求 |
| 7 | 嵌入式统计扩展、CLI 变量体验改进、服务端预编译 | B4/C2/C1 | 低频，按需 |

## 涉及文件一览

- 文档修正：`docs/api/feature_matrix.md`
- B1：`crates/linkrs-api/src/embedded/c_api/`（新增 `prepared.rs`、`session_variables.rs`，扩展 `config.rs`/`transaction.rs`/`database.rs`）
- B2：`crates/linkrs-client/`（新增 grpc 模块或子 crate）、`proto/`
- B3：`crates/linkrs-migration` / `crates/linkrs-storage`（读取下沉）、`crates/linkrs-api/src/embedded/database.rs`
- B4：`crates/linkrs-metrics`
- C4：`crates/linkrs-server/src/web/handlers/`
