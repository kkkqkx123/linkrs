# API 功能缺口分析与整改方案

## 0. 文档信息

| 项 | 内容 |
|---|---|
| 分析对象 | `docs/api/feature_matrix.md`（各层 API × 功能模块矩阵） |
| 代码基线 | 当前工作区：`crates/linkrs-api`、`crates/linkrs-server`、`crates/linkrs-client`、`crates/linkrs-cli`、`crates/linkrs-wire`、`proto/linkrs.proto` |
| 关联文档 | `docs/api/embedded/README.md`、`docs/plan/cli_feature_gap_analysis.md`、`docs/plan/txn-sync-dlq-remediation.md` |
| 输出 | 本文件是缺口与整改的唯一清单；任务编号只允许出现在本文件，代码注释不得引用 |

## 1. 结论速览

矩阵中的 ⚠️ / — 可以分成三类，不能一概当作“待补功能”：

1. **真实能力缺口**：补齐后能消除层间能力差，且底层已有可复用能力。
   - 可观测性：嵌入式没有进程内 health/metrics；gRPC 没有 metrics；client 与 CLI 没有 metrics 命令。
   - Schema 版本历史：嵌入式可以执行迁移，但不能读版本历史/变更/破坏性判定；`linkrs-storage` 已经提供读接口。
   - 统计面：嵌入式只有 `database_overview` 与事务指标，缺 search/freeze/query profile。
   - 同步运维：嵌入式没有暴露 `SyncApi`，即使配置启用了 outbox。
   - 向量/全文运维：嵌入式只有最小读写，缺 list/get/rebuild/clear/inconsistent/scroll/payload/count。
   - 文件导入：server 与 CLI 有，嵌入式只有 `BatchInserter`。
2. **文档失真**：能力已经在代码里，但矩阵/文档没更新。
   - C API 实际已覆盖 batch、config、migration、UDF、统计、busy handler 等，矩阵仍写“查询/事务/Schema/validate/explain/kill/count 子集”。
   - `docs/api/embedded/README.md` 声称“不再维护 C API”，与 `c_api` feature、`include/linkrs.h`（272 处 `linkrs_*` 标识）、`tests/integration_c_api.rs` 矛盾。
   - CLI 已有 `\health`（远程探针）与 `\set --remote` / `\unset --remote`（服务端 `$` 变量），矩阵未反映。
3. **有意非目标**：不建议为了“矩阵全绿”而实现。
   - 嵌入式流式查询 / 结果游标：进程内迭代与分页足够。
   - 嵌入式用户/会话管理：单进程无多租户。
   - Web `/api` 承担产品侧历史/收藏/浏览；运维页面走 `/v1`，不需要在 `/api` 复制管理面。
   - 协议级预编译语句：所有传输已支持 `@name` 参数绑定；嵌入式 `PreparedStatement` 只是本地校验封装。
   - 仓库内 gRPC 客户端封装：按 `proto/linkrs.proto` 直连即可。

另外有两处矩阵没有记录的问题：

- **架构倒置**：gRPC handler 直接调用 HTTP handler（统计、配置、函数、向量、全文），使“gRPC 已对齐”建立在 HTTP 层之上；HTTP 关闭时 gRPC-only 部署没有指标获取通道。
- **C API 定位矛盾**：代码、头文件、测试都在维护 C API，嵌入式文档却声明已放弃。

## 2. 判定口径

| 口径 | 说明 |
|---|---|
| 层职责 | 嵌入式是库，不应提供 `/health` 这类网络端点；但“进程内可观测/可运维”仍应提供函数式入口。 |
| 可复用性 | `api_core` 或底层 crate 已具备能力，而某传输层未暴露 → 记为缺口；仅 server 运行时才有的能力（host/sysinfo、监听器、用户库）→ 记为合理差异。 |
| 数据来源 | 矩阵单元格与代码不一致时，以代码为准；先修文档，再决定是否补功能。 |
| 非目标 | 明确写进矩阵说明，避免后来者把“—”误读为遗漏。 |

## 3. 缺口清单

| 编号 | 矩阵位置 | 现状证据 | 缺口结论 | 类型 | 优先级 |
|---|---|---|---|---|---|
| G1 | 健康检查与指标 | HTTP `/v1/metrics` 渲染逻辑在 `http/handlers/metrics.rs:53`；`StatsManager` 在 `embedded/database.rs:840` 可访问；proto 只有 `HealthCheck`（`proto/linkrs.proto:7`） | 嵌入式无进程内 health/metrics；gRPC 无 metrics；client/CLI 无 metrics | 真实缺口 | P1 |
| G2 | Schema 版本与迁移 | `storage/client/traits_reader.rs:437` 起已有版本历史读接口；`schema/version.rs:59` 只在 HTTP 层组装返回 | 嵌入式不能读版本历史、版本区间变更、破坏性变更 | 真实缺口 | P1 |
| G3 | 统计信息 | `embedded/database.rs:630` 只有 overview 子集；`linkrs-metrics` 已有 `get_query_profile`、`get_search_latency_percentiles` | 嵌入式缺 search/freeze/profile/slow-query 等统计 | 真实缺口 | P1 |
| G4 | 同步/Outbox | `session.rs:79` 内部持有 `sync_manager: Option<Arc<SyncManager>>`；`api_core/sync_api.rs` 已封装 | 嵌入式无同步运维入口 | 真实缺口 | P2 |
| G5 | 向量/全文索引 | 嵌入式 `session/vector.rs` 只有 create/drop/simvec；`session/fulltext.rs` 只有 search；`api_core/vector_api.rs`、`fulltext_api.rs` 已有索引元数据接口 | 嵌入式缺索引管理/重建/状态/scroll/payload/count | 真实缺口 | P2 |
| G6 | 导入/导出 | server 的文件解析与语句构造在 `http/handlers/import.rs:49/72`；嵌入式只有 `export_query_to_file` 与 `BatchInserter` | 嵌入式不能按文件导入 CSV/JSON/JSONL | 真实缺口 | P2 |
| G7 | 多行文档失真 | 见第 1 节 | 矩阵与嵌入式 README 过时 | 文档 | P0 |
| G8 | gRPC 各能力列 | `grpc/statistics.rs:145/162/179`、`grpc/config.rs:44`、`grpc/function.rs:28`、`grpc/vector.rs:345`、`grpc/fulltext.rs:52` 直接调用 `crate::http::handlers` | gRPC 未形成独立用例层；HTTP 关闭时部分能力不可用 | 架构 | P2 |
| G9 | C API（FFI） | `crates/linkrs-api/src/embedded/c_api/*` 已覆盖批处理/配置/迁移/UDF/统计/busy handler；`docs/api/embedded/README.md:7,48` 与代码矛盾 | 文档失真 + 少量高价值 FFI 仍缺 | 文档/能力 | P1（文档）/ P2（代码） |
| G10 | 预编译语句 | 嵌入式 `session/prepared.rs:25` 有本地封装；HTTP/gRPC 仅参数绑定 | 协议级预编译未提供 | 非目标决策 | P2 |

### 3.1 G1：可观测性

- HTTP 的 Prometheus 文本渲染是 server 专属实现（`http/handlers/metrics.rs`），但输入只有 `StatsManager`，没有任何 server 依赖。
- 嵌入式的 `StatsManager` 可通过 `GraphDatabase::stats_manager()` 拿到，却无法导出 Prometheus 文本，也无法给出结构化 health。
- gRPC 只有 `HealthCheck`，且 `grpc/session.rs:25` 恒返回 `healthy = true`，没有探测 storage/事务管理状态。
- `linkrs-client` 只有 `health_check` / `health_detail`，没有 metrics；CLI 有 `\health`，没有 `\metrics`。

影响：嵌入式宿主、gRPC-only 部署和 CLI 用户无法用同一套指标做监控。

### 3.2 G2：Schema 版本历史

- `linkrs-storage` 的 `StorageReader` 已提供 `get_vertex_version_history`、`get_edge_version_history`、`get_vertex_schema_changes`、`get_edge_schema_changes`、`detect_*_breaking_changes`。
- HTTP `schema/version.rs` 在 handler 内组装 `versions` 数组与变更详情；嵌入式 `SchemaApi` 完全没有这些方法。
- 结果是嵌入式用户能做迁移、不能审计迁移前后的 schema 变化。

### 3.3 G3：统计面

- 嵌入式 `database_overview()` 只拼了 spaces、事务指标、`StatsManager` 计数器。
- `linkrs-metrics` 已有 search 延迟分位、查询画像、错误快照、慢查询、timeseries；`linkrs-storage` 有 `get_storage_stats()` 与 `get_freeze_stats()`。
- 嵌入式没有对应入口，导致“单机部署也要起 HTTP 才能看 search/freeze/profile”。

`system` 统计依赖 `sysinfo` 与监听器/会话管理器，属于 server 运行时能力，保持 server-only。

### 3.4 G4：同步/Outbox

- 嵌入式 `GraphDatabaseInner` 已持有 `sync_manager`，但没有任何公开访问器。
- `api_core::SyncApi` 已经封装了 status 所需的 outbox 统计、diagnostics、dead letter 列表/重入队、degraded range、retention。
- 嵌入式启用 fulltext/vector 同步时，用户无法在进程内查看/重试/清理。

### 3.5 G5：向量/全文运维

- 嵌入式的向量只有 `create_vector_index` / `drop_vector_index` / `simvec*`；全文只有 `search_fulltext*`。
- `api_core::VectorApi` 与 `FulltextApi` 已能 list/get/inconsistent；`linkrs-sync` 已有 rebuild/status/clear 协调器；`VectorBackend` 有 scroll/payload/count。
- 嵌入式缺这些入口，矩阵只能标 ⚠️。

### 3.6 G6：文件导入

- server 的 `import_file` 在 handler 内完成 multipart、格式识别、语句构造、分组提交。
- 解析与语句构造是纯函数，和 HTTP 无关；嵌入式无法复用，只能让用户自己解析文件再走 `BatchInserter`。
- 空间级导入/导出在 `linkrs-storage`/CLI 已有实现，但 `api` 未暴露。

### 3.7 G8：gRPC 与 HTTP 的耦合

gRPC handler 直接调用 axum handler，意味着：

- gRPC 的“完整度”实际依赖 HTTP 层编译与存在。
- HTTP 关闭时（配置允许 `server.http.enabled = false`），`Get*Statistics`、向量/全文重建、配置更新等 gRPC 调用仍能编译，但缺少独立的业务入口层。
- 统计 handler 直接返回 axum 类型，gRPC 只能靠 `statistics_http_status` 转换错误。

### 3.8 G9：C API 定位与覆盖

- 代码、头文件、集成测试都在维护 C API；嵌入式 README 却写“不再维护”。
- 矩阵低估了实际覆盖，也漏掉了真正缺失项：会话变量、版本历史、health/metrics、事务列表/指标、批量配置更新。
- 头文件为手工维护，缺少“声明与导出一致”的校验，容易继续漂移。

## 4. 整改方案

### T1（P0）文档校准

**目标**：让矩阵与代码一致，先把“假缺口”消掉。

改动：

1. `docs/api/feature_matrix.md`
   - 健康检查行：CLI 列改为 `\health`（远程 health/live/ready），嵌入式写明“无网络端点；进程内能力见后续 `health()`/`metrics_text()`”。
   - C API 行：按实际覆盖重写为“查询/事务/保存点/Schema/批量/配置/迁移/UDF/统计/busy handler；缺会话变量、版本历史、health/metrics、导出等”。
   - 向量/全文嵌入行列：明确“create/drop/search 已具备；list/get/rebuild/clear/inconsistent/scroll/payload/count 未具备”。
   - CLI 列：`\set --remote` / `\unset --remote` 已存在；说明本地变量与服务端 `$` 变量不自动同步是有意设计，不是缺口。
   - Web 列：注明运维页走 `/v1`，`/api` 只承担产品侧历史/收藏/数据浏览/图查询。
   - 增加说明：gRPC 统计/配置/函数/向量/全文 handler 当前复用 HTTP handler，属已知架构债（见 T8）。
2. `docs/api/embedded/README.md`
   - 删除/改写“不再维护独立 C API”的段落；明确 `c_api` feature、`include/linkrs.h`、集成测试的现状。
   - 若团队决定放弃 C API，则改为“移除计划”，并同步删除 T9 的代码任务；默认按“仍在维护”处理。

验收：

- `rg "不再维护" docs/api/embedded/README.md` 无命中。
- 矩阵每个 ⚠️ 都能对应本文件的 G 编号或明确的非目标说明。
- C API 行与 `crates/linkrs-api/src/embedded/c_api/*` 模块清单一致。

### T2（P1）可观测性对齐

**目标**：HTTP / gRPC / 嵌入式 / client / CLI 使用同一份指标渲染与 health 语义。

改动：

1. 下沉 Prometheus 渲染。
   - 新建 `crates/linkrs-metrics/src/prometheus.rs`，把 `http/handlers/metrics.rs:53` 的 `render_prometheus`、`push_sample`、`seconds` 及稳定排序逻辑迁入；导出 `render_prometheus(&StatsManager) -> String` 与 `PROMETHEUS_CONTENT_TYPE`。
   - `http/handlers/metrics.rs` 只保留 axum 适配与响应头，调用 `linkrs_metrics::render_prometheus`。
   - 把确定性测试一并迁到 `linkrs-metrics`。
2. 嵌入式入口。
   - `GraphDatabase::metrics_text() -> String`：调用 `render_prometheus(self.inner.stats_manager)`。
   - `GraphDatabase::health() -> serde_json::Value`：返回与 HTTP `/v1/health` 同构的 `status/service/version/ready/live/checks/optional`；`storage` 用 `storage.read().get_storage_stats()` 探测，`transaction_manager` 用事务管理器存活状态，`sync` 按 `sync_manager` 是否存在给出 `ok`/`disabled`。
   - 同时给 `Session` 增加同名转发方法，便于 CLI 嵌入模式复用。
3. gRPC metrics。
   - `proto/linkrs.proto` 增加：
     ```proto
     rpc GetMetrics(GetMetricsRequest) returns (GetMetricsResponse);
     message GetMetricsRequest {}
     message GetMetricsResponse {
       string content_type = 1;
       string body = 2;
     }
     ```
   - 新建 `crates/linkrs-server/src/grpc/metrics.rs`，在 `service.rs`/`server.rs` 接线。
   - 顺带让 `grpc/session.rs:25` 的 `HealthCheck` 探测 storage，而不是恒 `true`。
4. client 与 CLI。
   - `linkrs-client::HttpClient::metrics() -> Result<String>`，请求 `GET /v1/metrics`。
   - CLI 新增 `MetaCommand::Metrics`：远程打印原始 Prometheus 文本；嵌入模式调用 `Session::metrics_text()`；更新 parser、completion、help、executor。
   - CLI 嵌入模式的 `\health` 改为输出 `GraphDatabase::health()` JSON，而不是固定字符串。

验收：

- 同一 `StatsManager` 状态下，`/v1/metrics`、gRPC `GetMetrics`、`metrics_text()` 输出逐字节一致（测试比对）。
- gRPC-only 配置（`server.http.enabled = false`）可获取指标。
- `cargo test -p linkrs-metrics`、`cargo test -p linkrs-api --features embedded`、CLI 元命令测试通过。

### T3（P1）嵌入式 Schema 版本历史

**目标**：嵌入式可读版本历史、区间变更、破坏性变更，返回结构与 HTTP 对齐。

改动：

1. 在 `linkrs-storage/src/schema/version_history.rs` 增加投影方法，例如 `LabelVersionHistory::summary_json()`，把 `versions + changes + timestamps` 的组装从 `http/handlers/schema/version.rs` 迁出；HTTP 改为调用该方法，保持现有 JSON 形状不变。
2. `api_core/schema_api.rs` 增加：
   ```rust
   pub fn get_vertex_version_history(&self, space: &str, tag: &str) -> CoreResult<Option<LabelVersionHistory>>;
   pub fn get_edge_version_history(&self, space: &str, edge: &str) -> CoreResult<Option<LabelVersionHistory>>;
   pub fn get_schema_changes(&self, space: &str, label: &str, is_edge: bool, from: u64, to: u64) -> CoreResult<Vec<PropertyChange>>;
   pub fn detect_breaking_changes(&self, space: &str, label: &str, is_edge: bool, from: u64, to: u64) -> CoreResult<Vec<PropertyChange>>;
   ```
3. `Session` 增加同名方法；`GraphDatabase` 增加便捷包装（内部 `session()`）。

验收：

- 同一 DB 上，嵌入式 `summary_json()` 与 `GET /v1/schema/versions/{space}/{label}` 返回一致。
- 迁移前后版本号/变更条目测试通过。
- `cargo test -p linkrs-api --features embedded`。

### T4（P1）嵌入式统计补齐

**目标**：嵌入式能拿到除 `system` 外的完整统计面。

改动：

1. 在 `embedded/statistics.rs` 或 `embedded/database.rs` 增加：
   - `statistics_overview() -> serde_json::Value`：`database`（storage stats + 计数器）、`query_latency_us`、`errors`、`storage`、`transaction`、`sync`、`timeseries`；`system` 明确不提供。
   - `search_statistics() -> serde_json::Value`：复用 `StatsManager::get_search_latency_percentiles` 与 search 相关计数器。
   - `freeze_statistics() -> Option<FreezeStats>`：`storage.read().get_freeze_stats()`。
   - `query_profile(trace_id: &str) -> Option<QueryProfile>`：`StatsManager::get_query_profile`。
   - `slow_queries(limit: usize) -> Vec<QueryProfile>`。
2. 返回 `serde_json::Value`/底层类型，避免 `linkrs-api` 反向依赖 `linkrs-wire`；需要与 HTTP 响应字段对齐时，在文档注明差异（仅 `system` 缺失）。
3. 可选后续：把 HTTP statistics handler 的组装逻辑抽成共享函数，供 T8 使用；本任务先保证嵌入式可用，不阻塞。

验收：

- 执行查询后，overview 中 query/latency/errors 非零；search 在 fulltext/vector 特性下非空。
- 与 HTTP 对应 section 的字段名一致（`system` 除外）。
- 嵌入式的 `profile` 能按 `trace_id` 命中与 HTTP 查询画像相同的记录。

### T5（P2）嵌入式同步运维

**目标**：启用同步时，嵌入式可查看与处理 outbox/死信/降级区间。

改动：

1. `GraphDatabase` 增加 `sync_api(&self) -> Option<SyncApi>`；由 `inner.sync_manager.clone()` 构造 `SyncApi::new`。
2. 通过 `SyncApi` 暴露：`is_running`、`outbox_stats`、`sync_diagnostics`、`list_dead_letters`、`requeue_dead_letter(s)`、`list_degraded_ranges`、`clear_degraded_range`、`retention_lsn`、`run_retention_once`。
3. 同步未启用时返回 `None`；不要在嵌入式里造“假空状态”，避免调用方误判。
4. 文档补充“嵌入式同步仅在本进程启用 fulltext/vector 同步时存在”。

验收：

- 未启用同步：`sync_api()` 为 `None`。
- 启用并制造一次失败投递后，能看到 dead letter 并成功重入队。
- `cargo test -p linkrs-api --features "embedded,fulltext"`。

### T6（P2）嵌入式向量/全文运维

**目标**：补齐索引管理面，与 HTTP 路由语义对齐。

改动：

1. 向量（`vector` feature）：
   - 使用 `api_core::VectorApi` 暴露 `list_indexes`、`list_index_descriptors`、`get_index_info`、`inconsistent_indexes`。
   - 使用同步协调器暴露 `rebuild_vector_index`、重建状态、`clear_vector_index`。
   - 使用 `VectorBackend` 暴露 `scroll`、`set_payload`、`set_payload_fields`、`delete_payload`、`count`。
   - 异步操作复用 `GraphDatabaseInner` 的 `vector_runtime`，不要临时创建 runtime。
2. 全文（`fulltext` feature）：
   - 暴露 `list_indexes`、`get_index_info`、`inconsistent_indexes`、`rebuild_fulltext_index`、重建状态、`clear_fulltext_index`。
3. 所有新方法沿用 HTTP 的参数/返回语义；feature 关闭时不编译。

验收：

- `cargo test -p linkrs-api --features "embedded,vector"` 与 `"embedded,fulltext"` 通过。
- 与 HTTP `/v1/vector/*`、`/v1/fulltext/*` 的索引列表/状态一致。
- 重建期间状态可查；清空后列表为空。

### T7（P2）嵌入式文件导入

**目标**：把 server 的文件解析/语句构造下沉为可复用用例，嵌入式提供文件导入。

改动：

1. 新建 `crates/linkrs-api/src/api_core/import.rs`：
   - 迁入 `http/handlers/import.rs:49/72` 的 `vertex_statement_from_csv`、`edge_statement_from_csv`、`format_value`、JSON array/JSONL 解析。
   - 定义 `ImportFormat { Csv, Json, Jsonl }`、`ImportTarget { Vertex, Edge }`、`ImportStats`。
2. server `import_file` 只保留 multipart/Job 状态与权限，调用共享解析器。
3. 嵌入式 `Session::import_file(path, format, space, target_type, target_name, batch_size)`；按现有分组提交路径执行，返回导入/失败行数。
4. 不在 `api_core` 引入 HTTP 类型；仅新增 `csv` 依赖（与 server 已有依赖一致）。

验收：

- 同一 CSV/JSON/JSONL 样本，server 与嵌入式导入后行数、失败行数、目标属性一致。
- 非法行只计数不中断，错误信息包含行号。
- `cargo test -p linkrs-api --features embedded` 与 server 导入测试通过。

### T8（P2）gRPC 用例层去 HTTP 化

**目标**：让 HTTP 与 gRPC 都调用传输无关用例，消除 `grpc → http::handlers` 反向依赖。

分域推进，每个域独立可验收：

1. 统计：把 `overview/search/migration/freeze/profile` 的组装从 `http/handlers/statistics/*` 抽到 `api_core/statistics_api.rs`（或 server 内的 `statistics/service.rs`，但不得引用 axum 类型）。
2. 配置：把 `apply_config_update`/`get_config_value` 从 `http/handlers/config.rs` 抽到 `api_core/config_api.rs`。
3. 函数：把 `register_udf_from_source`/`unregister_udf_by_name` 抽到 `api_core/function_api.rs`。
4. 向量/全文：把 `scroll/count/rebuild/status/clear/search` 的用例逻辑抽到 `api_core` 扩展或 `linkrs-sync` 的协调器调用层。
5. HTTP/gRPC 两侧都变成薄适配器，错误映射各自做。

验收：

- `rg "crate::http::handlers" crates/linkrs-server/src/grpc` 无命中（允许引用 `crate::http::AppState`，但不得调用 handler）。
- `server.http.enabled = false` 时 gRPC 全量集成测试通过。
- 各域 HTTP 响应形状不变。

### T9（P1 文档 / P2 代码）C API 对齐

**目标**：先明确 C API 是否仍受支持，再决定补功能还是移除。

默认方案（仍在维护）：

1. 文档：修正 `docs/api/embedded/README.md` 与矩阵；把 C API 写成“嵌入式 Rust API 的薄 FFI 投影，覆盖查询/事务/保存点/Schema/批量/配置/迁移/UDF/统计/busy handler”。
2. 补高价值 FFI：
   - 会话变量：`linkrs_session_set_variable`、`_get_variable`、`_remove_variable`、`_list_variables`。
   - 观测：`linkrs_health_json`、`linkrs_metrics_text`。
   - Schema 版本：`linkrs_schema_version_history_json`。
   - 事务观测：`linkrs_transactions_json`、`linkrs_transaction_metrics_json`。
   - 配置批量：`linkrs_bulk_update_config`。
3. 更新 `include/linkrs.h`；为每个新增函数补 `tests/integration_c_api.rs` 用例（含空指针与非法参数）。

替代方案（若决定放弃 C API）：

- 删除 `c_api` feature、`embedded/c_api` 模块、`include/linkrs.h`、C API 测试，矩阵 C API 行改为 `—`；本任务代码部分取消。

验收：

- 头文件声明的 `linkrs_*` 与 Rust 导出一一对应（可加一个头文件覆盖测试）。
- `cargo test --features c_api --test integration_c_api` 通过。
- 文档不再出现“维护/不维护”自相矛盾的表述。

### T10（P2）预编译语句决策

**目标**：明确协议级预编译是否是非目标，避免矩阵长期挂一个含糊的 `—`。

建议：

1. 保持“参数化查询是统一契约”：HTTP `QueryRequest.parameters`、gRPC `QueryParameters`、嵌入式 `execute_with_params` 都已支持。
2. 矩阵说明从“用带参查询代替”改为“非目标：协议级预编译；嵌入式 `PreparedStatement` 仅为本地校验封装”。
3. 不新增 HTTP/gRPC 语句句柄。若后续基准证明解析/校验是端到端耗时的主要热点，再评估在 `QueryApi` 内加有界文本缓存（按 schema 版本失效），而不是新增传输 API。
4. 如果产品明确需要 `PREPARE/EXECUTE/DEALLOCATE` 生命周期，再单独立项；本方案不实现。

验收：

- 矩阵该行有“明确非目标”说明。
- 无新增公开 API；现有参数绑定路径回归测试通过。

## 5. 执行顺序

1. **T1**：先修文档，避免团队按错误矩阵排期。
2. **T2 → T3 → T4**：P1 能力补齐，均可独立交付；T2 会新增 proto RPC，先做可减少后续 gRPC 改动冲突。
3. **T5 → T6 → T7**：P2 嵌入式运维与导入；T6 依赖 feature 组合测试，放在 T2/T4 之后。
4. **T8**：按域拆分，统计域可在 T4 后立即做；向量/全文域可与 T6 合并设计。
5. **T9**：文档部分随 T1；FFI 部分最后做。
6. **T10**：决策项，随 T1 一起写进矩阵说明。

## 6. 非目标（保持并补充说明）

| 项 | 结论 |
|---|---|
| 嵌入式流式查询 / 游标 | 进程内 `QueryResult` 与迭代已覆盖；不引入 SSE/游标句柄 |
| 嵌入式用户/会话管理 | 单进程无多租户；保持 `—` |
| 嵌入式 Web 管理面 | Web `/api` 是产品侧历史/收藏/浏览；运维页继续走 `/v1` |
| 协议级预编译 | 参数绑定已覆盖；按 T10 决策 |
| 仓库内 gRPC 客户端 | 以 `proto` + `LinkrsService` 直连为准；如需 SDK 另行立项 |
| 嵌入式 `system` 统计 | 依赖 host/sysinfo 与监听器，保持 server-only |

## 7. 总体验收清单

- `cargo test --lib -- --nocapture` 通过。
- `cargo test -p linkrs-api --features embedded`、`--features "embedded,vector"`、`--features "embedded,fulltext"` 通过。
- `cargo test -p linkrs-server --features grpc`（含 gRPC-only 集成）通过。
- `cargo test --features c_api --test integration_c_api` 通过。
- `rg "crate::http::handlers" crates/linkrs-server/src/grpc` 无命中（T8 完成后）。
- `docs/api/feature_matrix.md` 中每个 ⚠️ 都能追溯到本文件 G 编号或“非目标”说明。
- `docs/api/embedded/README.md` 与 C API 代码/头文件不再矛盾。
