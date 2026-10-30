# Linkrs API Feature Matrix

本矩阵对比各层 API 对功能模块的支持情况。图例：

- ✅ 已支持
- ⚠️ 部分支持 / 仅查询层或部分操作
- ❌ 未支持
- — 不适用 / 非该层职责

| 功能模块 | 嵌入式 Rust API | HTTP Server API | gRPC Server API | Web 管理 API | Client (`HttpClient`) | CLI |
|---|---|---|---|---|---|---|
| 健康检查与指标 | — | ✅ `/health*`, `/metrics` | ✅ `HealthCheck` | — | ✅ `health_check` | — |
| 认证与用户管理 | — | ✅ `/auth/*`, `/users/*` | ❌ | — | ✅ 用户相关方法 | ✅ `\users` |
| 会话管理 | ✅ `session()` | ✅ `/sessions/*` | ✅ `Create/Get/CloseSession` | — | ✅ 会话方法 | ✅ `\connect`, `\sessions` |
| 查询执行 | ✅ `execute` | ✅ `/query` | ✅ `ExecuteQuery` | — | ✅ `execute_query*` | ✅ SQL 输入 |
| 流式查询 | ❌ | ✅ `/query/stream` | ✅ `ExecuteQueryStream` | — | ✅ `execute_query_stream` | ✅ `\stream` |
| 结果游标 | ❌ | ✅ `/query/cursor/*` | ✅ `Open/Fetch/CloseCursor` | — | ✅ `open/fetch/close_cursor` | ✅ `\cursor-*` |
| 查询校验/解释 | ❌ | ✅ `/query/validate`, `/query/explain` | ✅ `ValidateQuery`, `ExplainQuery` | — | ✅ `validate_query`, `explain_query` | ✅ `\validate`, `\explain` |
| 事务控制 | ✅ `begin/commit/rollback` | ✅ `/transactions/*` | ✅ `Begin/Commit/RollbackTransaction` | — | ✅ `begin/commit/rollback_transaction` | ✅ `\begin`, `\commit`, `\rollback` |
| 保存点 | ✅ `create/rollback/release` | ✅ `/transactions/{id}/savepoints*` | ✅ `Create/Rollback/ReleaseSavepoint` | — | ✅ `create/list/rollback/release_savepoint` | ✅ `\transactions savepoint-*` |
| 事务列表/指标/终止 | ❌ | ✅ `/transactions`, `/transactions/metrics`, `/kill` | ✅ `List/KillTransaction`, `GetTransactionMetrics` | — | ✅ `list/kill_transaction`, `transaction_metrics` | ✅ `\transactions list/metrics/kill` |
| Schema 管理 | ⚠️ 空间创建/删除/列表 | ✅ spaces/tags/edge-types | ✅ 空间/标签/边类型 CRUD | ✅ 扩展 Schema 查询 | ✅ 空间/标签/边类型方法 | ✅ DDL + `\show` |
| Schema 版本与迁移 | ✅ `migration` 模块 | ✅ `/schema/versions/*`, `/migration/*` | ✅ `GetVersionHistory`, `Migrate*`, `StreamMigrationProgress` | — | ✅ 版本/迁移/流式进度 | ✅ `\migration`, `\schema-*` |
| 批量操作 | ✅ `BatchInserter` | ✅ `/batch/*` | ✅ `Create/Add/Execute/...Batch` | — | ✅ `create/add/execute/cancel/delete_batch` | ✅ `\batch` |
| 向量索引 | ⚠️ 通过查询/配置 | ✅ `/v1` vector 路由 | ✅ `Create/Search/...Vector` | — | ✅ 完整向量方法集 | ✅ `\vector` |
| 全文索引 | ⚠️ 通过查询 | ✅ fulltext 路由 | ✅ `Rebuild/GetStatus/Clear/ListInconsistent` | — | ✅ 全文方法集 | ✅ `\fulltext` |
| 统计信息 | ⚠️ 查询/会话统计 | ✅ `/statistics/*` | ✅ `Get*Statistics` | — | ✅ 统计方法集 | ✅ `\statistics` |
| 配置管理 | ✅ `DatabaseConfig` | ✅ `/config/*` | ✅ `Get/Update/Reset/BulkUpdateConfig` | — | ⚠️ 单键读写，无 bulk | ✅ `\config` |
| 函数管理 | ❌ | ✅ `/functions/*` | ✅ `Register/List/...Function` | — | ✅ 函数方法集 | ✅ `\functions` |
| 同步/Outbox | ❌ | ✅ `/sync/*` | ✅ `GetSyncStatus`, `RetryOutbox`, ... | — | ✅ 同步方法集 (9) | ✅ `\sync` |
| 导入/导出 | ❌ | ✅ `/import`, `/export` | ✅ `ImportData`, `ExportData`, `GetImportStatus` | — | ✅ `server_import_file`, `server_export_*` | ✅ `\import --server`, `\export --server` |
| Web 管理 (历史/收藏/数据浏览/图查询) | — | — (挂载 `/api`) | — | ✅ `/api/*` | — | — |
| 输出/脚本/变量/分页 | — | — | — | — | — | ✅ CLI 本地能力 |

## 说明

- 嵌入式 API 以查询语言和事务 API 为主；服务端专属能力（用户管理、同步、导入导出、Web 管理）不适用。
- HTTP 与 gRPC 基本对齐；gRPC 额外提供 `BulkUpdateConfig`，HTTP 提供 `PUT /config` 批量更新，客户端尚未封装 bulk 更新。
- CLI 同时支持本地嵌入模式与远程模式；新增的保存点子命令、`--server` 导入/导出、`--follow` 迁移进度在远程模式下走 HTTP。
