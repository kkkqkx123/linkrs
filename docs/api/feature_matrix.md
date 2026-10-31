# Linkrs API Feature Matrix

本矩阵对比各层 API 对功能模块的支持情况。图例：

- ✅ 已支持
- ⚠️ 部分支持 / 仅查询层或部分操作
- ❌ 未支持
- — 不适用 / 非该层职责

| 功能模块 | 嵌入式 Rust API | HTTP Server API | gRPC Server API | Web 管理 API | HttpClient（仅 HTTP） | CLI |
|---|---|---|---|---|---|---|
| 健康检查与指标 | ⚠️ `StatsManager`/`transaction_metrics`/`database_overview`（无 `/health`、`/metrics` 端点） | ✅ `/health*`, `/metrics` | ✅ `HealthCheck`（无 metrics 流，见说明） | — | ✅ `health_check` | —（可用 `\status` 看本地） |
| 认证与用户管理 | —（进程隔离，无多租户） | ✅ `/auth/*`, `/users/*` | ✅ `Login/Logout/AuthMe` + 用户 CRUD | — | ✅ 用户相关方法 | ✅ `\users` |
| 会话管理 | ✅ `session()` | ✅ `/sessions/*`（含变量子资源，见下） | ✅ `Create/Get/Close/List/KillSession` | —（经 `/v1` 代理执行） | ✅ 会话方法 | ✅ `\connect`, `\sessions` |
| 会话变量（`$name`） | ✅ `set/remove/variable_value/variables_snapshot`（事务感知） | ✅ `/sessions/{id}/variables*`（GET/POST/DELETE） | ✅ `List/Set/DeleteSessionVariables` | — | ✅ `list/set/delete_session_variables` | ⚠️ 本地 `\set` + 远程 `LET`（未自动同步，见说明） |
| 查询执行 | ✅ `execute` + `execute_with_params*` | ✅ `/query` | ✅ `ExecuteQuery` | —（经 `/v1` 代理执行） | ✅ `execute_query*` | ✅ SQL 输入 |
| 预编译语句 | ✅ `prepare`/`PreparedStatement`（`bind/execute/execute_batch`，本地校验复用） | —（用带参查询代替） | —（用带参查询代替） | — | — | —（CLI 用带参输入代替） |
| 查询管理（运行中查询） | —（单进程用 `interrupt()` 协作取消） | ✅ `/queries` + `/sessions/{id}/queries/{qid}/kill` | ✅ `ListQueries`/`KillQuery` | — | ✅ `list_queries`/`kill_query` | ✅ `\queries list|kill` |
| 流式查询 | —（`QueryResult` 直接返回，无需 SSE） | ✅ `/query/stream` | ✅ `ExecuteQueryStream` | — | ✅ `execute_query_stream` | ✅ `\stream` |
| 结果游标 | —（内存分页/迭代代替） | ✅ `/query/cursor/*` | ✅ `Open/Fetch/CloseCursor` | — | ✅ `open/fetch/close_cursor` | ✅ `\cursor-*` |
| 查询校验/解释 | ✅ `validate_query`, `explain_query` | ✅ `/query/validate`, `/query/explain` | ✅ `ValidateQuery`, `ExplainQuery` | — | ✅ `validate_query`, `explain_query` | ✅ `\validate`, `\explain` |
| 事务控制 | ✅ `begin/commit/rollback` | ✅ `/transactions/*` | ✅ `Begin/Commit/RollbackTransaction` | — | ✅ `begin/commit/rollback_transaction` | ✅ `\begin`, `\commit`, `\rollback` |
| 保存点 | ✅ `create/rollback/release/list` | ✅ `/transactions/{id}/savepoints*` | ✅ `Create/Rollback/Release/ListSavepoint` | — | ✅ `create/list/rollback/release_savepoint` | ✅ `\transactions savepoint-*` |
| 事务列表/指标/终止 | ✅ `list_transactions`、`transaction_metrics`、`kill_transaction` | ✅ `/transactions`, `/transactions/metrics`, `/kill` | ✅ `List/KillTransaction`, `GetTransactionMetrics` | — | ✅ `list/kill_transaction`, `transaction_metrics` | ✅ `\transactions list/metrics/kill` |
| Schema 管理 | ✅ 空间/标签/边类型 `create/drop/list/get` + DDL | ✅ spaces/tags/edge-types（含 `GET` 详情） | ✅ 空间/标签/边类型 CRUD（含 `Get*`） | ✅ 扩展 Schema 查询 | ✅ 空间/标签/边类型方法 | ✅ DDL + `\show` |
| Schema 版本与迁移 | ✅ `migration` 计划/执行/回滚 + `get_label_version_history`/`get_applied_versions`（版本历史读已下沉存储层） | ✅ `/schema/versions/*`, `/migration/*` | ✅ `GetVersionHistory`, `Migrate*`, `StreamMigrationProgress` | ✅ 版本历史/迁移历史状态只读 | ✅ 版本/迁移/流式进度 | ✅ `\migration`, `\schema-*` |
| 批量操作 | ✅ `BatchInserter` | ✅ `/batch/*` | ✅ `Create/Add/Execute/...Batch` | — | ✅ `create/add/execute/cancel/delete_batch` | ✅ `\batch` |
| 向量索引 | ⚠️ 原生读写（`create/drop/search`，`vector` 门控）+ 同步运维走服务端 | ✅ `/v1` vector 路由（`vector` 特性门控） | ✅ `Create/Search/...Vector`（含 payload/scroll/count/rebuild） | — | ✅ 完整向量方法集 | ✅ `\vector`（含 `scroll/set-payload/merge/delete/rebuild/status/clear`） |
| 全文索引 | ⚠️ 原生搜索 + 索引管理（`fulltext` 特性门控） | ✅ fulltext 路由（`fulltext` 特性门控） | ✅ `Rebuild/GetStatus/Clear/ListInconsistent` | — | ✅ 全文方法集 | ✅ `\fulltext` |
| 统计信息 | ⚠️ 会话/查询统计 + `database_overview`（无 system/search/freeze 全局注册表；有意维持，嵌入式以单进程自监控为主） | ✅ `/statistics/*`（overview/system/search/freeze/profile） | ✅ `Get*Statistics` | — | ✅ 统计方法集 | ✅ `\statistics`（含 `freeze/profile`） |
| 配置管理 | ✅ `DatabaseConfig` + `get/update/reset/bulk_update_config`（`storage.*` 即时生效，WAL/sync 下次 open 生效） | ✅ `/config/*`（含 `PUT /config` 批量） | ✅ `Get/Update/Reset/BulkUpdateConfig` | — | ✅ 含 `bulk_update_config` | ✅ `\config`（含 `bulk`） |
| 函数管理 | ⚠️ 本地注册/扩展加载（无服务端持久化） | ✅ `/functions/*` | ✅ `Register/List/...Function` | — | ✅ 函数方法集 | ✅ `\functions` |
| 同步/Outbox | — | ✅ `/sync/*`（status/outbox/diagnostics/dead/requeue/degraded/retention） | ✅ `GetSyncStatus`, `RetryOutbox`, degraded/retention 系列 | — | ✅ 同步方法集（outbox/diagnostics/dead/requeue/degraded/retention） | ✅ `\sync` |
| 导入/导出 | ⚠️ `export_query_to_file` + `BatchInserter`（文件导入走服务端） | ✅ `/import`, `/export` | ✅ `ImportData`, `ExportData`, `GetImportStatus` | — | ✅ `server_import_file`, `server_export_*` | ✅ `\import --server`, `\export --server` |
| Web 管理 | — | —（`/api` 挂载 Web 后端，见说明） | — | ✅ `/api/v1/queries|schema|data|graph`（历史/收藏/数据浏览/图查询；执行经 `/v1`） | — | — |
| C API（FFI） | ⚠️ 嵌入式 API 的 FFI 投影：查询/事务/保存点/预编译/会话变量/批量/UDF/扩展/配置（构造+运行时）/迁移/统计/hooks/导出/overview/validate/explain/kill（`c_api` 特性门控，隐含 `embedded`；Schema 经 DDL 文本；缺向量/全文/版本历史读） | — | — | — | — | — |
| 输出/脚本/分页 | — | — | — | — | — | ✅ CLI 本地能力（脚本/变量/pager/输出重定向） |

## 说明

- 嵌入式 API 以查询语言和事务 API 为主；服务端专属能力（用户管理、同步运维）在嵌入式侧标记为 —。
  嵌入式已补齐：`validate_query`/`explain_query`、`get_space/get_tag/get_edge_type`（对齐 `Get*`）、
  `kill_transaction`（多线程嵌入式；单线程优先 `Transaction::rollback` + `Session::interrupt`）、
  `get/update/reset/bulk_update_config`（`cache.*`/`timeout.*`/`storage.read_only` 即时生效）、
  `database_overview`（spaces + 事务指标 + `StatsManager` 计数器子集）、
  `PreparedStatement`（`prepare/bind/execute[_update|_batch]`，`$` 变量仍走快照）、
  `export_query_to_file`（csv/jsonl，程序化导入仍用 `BatchInserter`）。
  版本历史读已下沉存储层：`get_label_version_history`/`get_applied_versions`
 （未知标签返回 `None`，与 `get_tag` 一致）。
- HTTP 与 gRPC 已对齐：用户管理、`ListSessions`/`KillSession`、`AuthMe`、`BulkUpdateConfig` 两侧一致；
  新增查询管理（HTTP `/queries` + `/sessions/{id}/queries/{qid}/kill`，gRPC `ListQueries`/`KillQuery`，
  语义等价 `SHOW QUERIES`/`KILL QUERY`）与会话变量
 （HTTP `/sessions/{id}/variables*`，gRPC `List/Set/DeleteSessionVariables`，事务感知与嵌入式一致）。
  远程侧有意不提供预编译语句：带参查询已覆盖解析复用与注入安全的主要收益，保留演进空间。
- 客户端列指 `HttpClient`（`crates/linkrs-client`，HTTP）：`bulk_update_config` 直调 `PUT /v1/config`；
  含 `list_queries`/`kill_query`、`list/set/delete_session_variables`。
  同 crate 新增 `grpc` 特性门控的 `GrpcClient`（tonic，方法命名对齐 `HttpClient`，直接收发
  proto 生成的请求/响应类型）：覆盖 health/认证/会话+变量/查询（含流式、游标）/事务/Schema/
  配置/统计/批量/迁移/导入导出；用户/函数/向量/全文/同步 RPC 有意暂不封装，流式接口返回
  `tonic::Streaming` 由调用方消费。CLI 远程模式仍走 HTTP。
- CLI 同时支持本地嵌入模式与远程模式（远程走 HTTP）；`\queries list|kill`、保存点子命令、
  `--server` 导入/导出、`--follow` 迁移进度在远程模式下走 HTTP；
  `\config bulk <json|@file>` 对齐服务端批量更新。`\set` 是 CLI 本地替换变量，
  服务端 `$` 变量请用 `LET` 或 `/sessions/{id}/variables`（两者概念不同，不自动同步）。
- 向量（`vector`）与全文（`fulltext`）路由受特性门控编译；未启用特性时对应 HTTP/gRPC/嵌入式能力不可用。
- Web 列指 Web 后端 `/api/*`（历史/收藏/数据浏览/图数据/Schema 扩展只读查询）；实际查询执行仍经 `/v1`，
  版本/迁移历史在 Web 侧只读。
- C API（`crates/linkrs-api/src/embedded/c_api/`，`c_api` 特性门控并隐含 `embedded`）
  是嵌入式 Rust API 的 FFI 投影：查询/事务/保存点/预编译语句/会话变量/批量/UDF/扩展加载/
  配置（构造期 + 运行时 get/update/reset/bulk）/迁移/会话统计/hooks/`validate`/`explain`/
  `kill_transaction`/`list_transactions`/`transaction_metrics`/`database_overview`/
  `export_to_file`。Schema 管理经 `linkrs_execute` 执行 DDL 文本，无结构化 schema FFI；
  向量/全文搜索与版本历史读暂未投影；不覆盖用户管理、同步运维、Web 管理。
