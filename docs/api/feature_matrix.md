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
| 预编译语句 | ✅ `prepare`/`PreparedStatement`（`bind/execute/execute_repeated`，本地校验复用） | —（用带参查询代替） | —（用带参查询代替） | — | — | —（CLI 用带参输入代替） |
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
| C API（FFI） | ⚠️ 嵌入式 API 的 FFI 投影（`c_api` 特性门控，隐含 `embedded`，172 个导出函数）：库/会话/结果句柄生命周期（含内存库一键开启）、查询（含位置/命名带参、参数加会话变量联合执行、`validate`/`explain`）、结果（含按名 `bool/float`、行与结果集 JSON 导出）、事务（含配置化开启、事务信息、保存点枚举与按名查找、会话与事务两侧的命名/联合执行、`kill`/`list`/`metrics`）、预编译（含批量绑定、原文回读、参数元信息、批量执行）、会话变量、批量（含多条添加、结果汇总 JSON）、UDF（含聚合、扩展加载/列表/安装）、配置（构造期向量开关 setter 与内存/路径查询 + 运行期 `get/update/reset/bulk`）、迁移（`plan`/`execute`/`rollback`）、版本历史读、结构化 Schema（空间/标签/边类型 `create/drop/list/get`，属性定义走 JSON）、统计（`changes`/`last-id`/`statistics`/`overview`）、hooks（`trace`/`commit`/`rollback`/`update`）/中断（含中断状态查询）、向量/全文/混合搜索（运行时门控，见说明）、结果空值判断、结果句柄直接导出、错误槽（`errcode/errmsg/errstr/offset/extended`）、调用方自驱动的 `busy_handler` + 建议式 `busy_timeout` | — | — | — | — | — |
| 输出/脚本/分页 | — | — | — | — | — | ✅ CLI 本地能力（脚本/变量/pager/输出重定向） |

## GPUI 桌面客户端

`crates/linkrs-gpui`（独立 workspace，原生桌面应用）复用 `linkrs-client` 传输层，
以 16 个 Dock 面板覆盖全部 11 个前端路由：Console、Schema、DataBrowser、Graph、Monitoring、
Transactions、Sync、Functions、Config、Batch、Transfer、Users、Sessions、Migration、
Vector、Fulltext。向量、全文、函数、配置在前端只是内嵌页签，在 GPUI 是独立面板，
能力持平或反超。事务面板另有显式 begin/commit/rollback 与保存点 UI，超出前端页面。

| 功能模块 | GPUI 支持 |
|---|---|
| 健康检查与指标 | ✅ 监控面板轮询 overview/system/query/database，加 QPS 与延迟面积图、慢查询表、画像查看、阈值告警样式；后台心跳，失活回登录 |
| 认证与用户管理 | ✅ 登录、最近连接、记住密码走系统凭据存储；用户列表/创建/启停/按空间授权回收/删除/自己改密；能力门控（用户与会话管理、批量仅 admin，命令面板同步过滤） |
| 会话管理 | ✅ 列表/详情/终止、自动刷新 |
| 会话变量（`$name`） | ✅ 控制台参数与变量透传执行，会话面板支持按会话查看/设置/删除 |
| 查询执行 | ✅ 多语句多结果 tab、参数与变量透传、CSV/JSON 文件导出、结果一键送图谱（自动切面板） |
| 预编译语句 | — 与服务端一致，无 UI（合理，带参查询已覆盖） |
| 查询管理（运行中查询） | ✅ 列表与终止，入口在控制台工具栏 |
| 流式查询 | ✅ 偏好三选（materialized/stream/auto，持久化）：auto 下多语句走流式、单语句先校验按预估行数与阈值路由，带绑定走 materialized |
| 结果游标 | ✅ 控制台游标分页器 |
| 查询校验/解释 | ✅ Validate/Explain/画像，均接线到控制台 |
| 事务控制 | ✅ 列表/指标/kill/outbox 重试，另有显式 begin/commit/rollback（超出前端页面） |
| 保存点 | ✅ 命名保存点创建与回滚/释放（超出前端页面） |
| Schema 管理 | ✅ 空间/标签/边类型创建删除、属性 ALTER、索引列表/创建/删除/重建、版本查看 |
| Schema 版本与迁移 | ✅ 版本历史/变更/破坏性检查只读，迁移计划/执行/流式进度 |
| 批量操作 | ✅ 创建/条目添加（含文件载入）/执行 |
| 向量索引 | ✅ 列表/创建/删除/搜索/计数/重建轮询/scroll/payload 合并删除/清空，空间自动填充 |
| 全文索引 | ✅ 列表/搜索/重建/状态/不一致清单/清空 |
| 统计信息 | ✅ 监控面板全量区块加 retrieval 聚合快照 |
| 配置管理 | ✅ 整配置 JSON 树、按 section/key 查询、更新与重置；无批量更新入口 |
| 函数管理 | ✅ 列表/详情/注册/注销 |
| 同步/Outbox | ✅ 状态快照、死信过滤与重入列、降级区间清除、retention 单次执行 |
| 导入/导出 | ✅ 服务端导入（本地文件读取上传）/导出（查询结果写本地文件）加状态轮询 |
| Web 管理 | — 非该层职责，原生端不挂载 Web 后端 |
| C API（FFI） | — 非该层职责 |
| 输出/脚本/分页 | ⚠️ 历史与收藏本地持久化、CSV/JSON 文件导出、游标分页；无 CLI 式脚本与输出重定向 |

与前端的差异详见 `docs/plan/frontend-gpui-gap-analysis.md`，复核结论如下：无离线 mock 模式、
会话标识只驻内存、管理页用内联表单加二次确认代替弹窗为有意为之，不再立项——会话标识
只驻内存是安全取向（与口令不落盘一致，且服务端会话闲置超时约 1 小时，持久化收益低，
重启后一次登录即可恢复）。四项缺口（索引管理、能力门控、自动路由偏好、
结果一键送图谱）已按 `docs/plan/gpui-gap-remediation-plan.md` 补齐；监控暂停开关复核为
已交付。另有三项已交付：控制台语句级执行（选区＞光标语句＞全文，校验/解释/游标作用于目标语句）
与本地格式化（见 `docs/plan/gpui-console-statement-execution-design.md`）、
控制台按 Linkrs 方言高亮与语句补全（见 `docs/plan/gpui-highlight-completion-design.md`，
Schema 名专属配色与子句感知作用域留待二期）、
Schema 级 ER 只读视图（见 `docs/plan/gpui-schema-er-design.md`）、中英文双语基础设施与
登录/主框架/控制台工具栏迁移（见 `docs/plan/gpui-i18n-design.md`，其余面板分批跟进）。

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
- C API（`crates/linkrs-api/src/embedded/c_api/`，`c_api` 特性门控并隐含 `embedded`，172 个导出函数）
  是嵌入式 Rust API 的 FFI 投影，按模块盘点如下：
  库生命周期 `open/open_memory/open_with_config/close/libversion/free[_string]`；
  会话 `session_create/close`、`use_space/current_space`、`set/get_autocommit`、
  `busy_timeout[_get]`、`trace/commit_hook/rollback_hook/update_hook`、
  `connection_interrupt/clear_interrupt/is_interrupted`；
  查询 `execute/execute_params`（位置绑定 `@param_{i}`）、`execute_named_params`（命名绑定）、
  `execute_with_variables`（参数加会话变量联合执行）、`validate_query`、`explain_query`；
  结果 `result_free`、`column_count/row_count/column_name/column_type`、
  `get_int/string/blob/bool/float` 与 `get_*_by_index`（含 `bool/float`）、`execution_time_ms/rows_scanned`、
  `row_to_json/result_to_json`（复杂类型走引擎 `Value` serde 编码）；
  事务 `txn_begin[_readonly]/txn_begin_with_config`（JSON 选项：只读、超时、持久化、隔离级别）、
  `txn_execute[_params/_named_params/_with_variables]`、`txn_info`（单事务 JSON，字段同 `list` 条目）、
  `txn_savepoint/release_savepoint/rollback_to_savepoint/list_savepoints/find_savepoint`、
  `commit/rollback/free`、
  `kill_transaction/transaction_count/transaction_list/transaction_metrics`（`list`/`metrics` 返回堆上 JSON）；
  预编译 `prepare/stmt_bind/stmt_bind_many/stmt_clear_bindings/stmt_reset/stmt_execute/stmt_execute_update/stmt_query/stmt_free`；
  会话变量 `set/remove/get/variables_snapshot`；批量 `batch_inserter_create/free/add_vertex/add_edge` +
  `add_vertices/add_edges`（多条，遇错即停）/`execute` + `vertices/edges_inserted/buffered_*` 计数 +
  `result_json`（插入数、缓冲数、错误数与错误信息，堆上 JSON）；UDF `create_function/create_aggregate/delete_function` +
  `context_*`（`set_result/result_type/set_error/get_arg/arg_count`）+ `load/unload/reload/list/install_extension`；
  配置构造器 `config_new/file/memory/free` + 类型化 setter（`cache_size/timeout/read_only/create_if_missing/enable_wal/sync_mode` +
  向量 `vector_enabled/local_dir` 与特性门控的 `vector_qdrant`）+ `is_memory/get_path` 只读查询 +
  运行期 `config_get/update/reset/bulk_update_value`（`bulk` 以 JSON 对象传 `section.key->value`）；
  迁移 `migration_plan_json/execute/rollback/report_free`（在线编排，阻塞式）；
  统计 `session_changes/total_changes/last_insert_vertex/edge_id/get_statistics` +
  `database_overview`（堆上 JSON：空间数/事务指标/`StatsManager` 计数器子集）；
  错误 `errcode/errmsg/errstr/error_offset/extended_errcode`；`busy_handler_*` 为调用方自驱动的重试记账
  （引擎不自动调用），`busy_timeout[_get]` 为建议式会话超时（引擎不据此强制锁等待，取消走中断与事务终止）。
  新增：结构化 Schema（`create/drop/list/get` 九组，`get_*` 不存在返回空而非报错；属性定义为 JSON 数组，
  无属性 tag 按引擎主键规则被拒绝并透出存储错误）、版本历史读（`label_version_history/applied_versions`，
  未知标签返回空）、预编译参数元信息（`param_count/param_name`）与批量执行（`execute_repeated`，按行参数集合 JSON，
  值用引擎 `Value` serde 编码，遇错即停）、事务内带参执行（与会话侧同为位置绑定）、向量（索引创建/删除/相似搜索，
  `threshold` 为负表示无截断）/全文/混合搜索（请求与结果走 JSON，阻塞式桥接，特性未编译时报未实现错误）、
  `cell_is_null` 空值判断（结果集仍整体物化，不引入游标）、结果句柄直接导出、`checkpoint_on_close` 类型化 setter。
  有意不覆盖用户管理、同步运维、流式/游标、Web 管理。补充设计见 `docs/plan/c_api_supplement_design.md`。
  分模块文档见 `docs/api/c_api/`，多语言绑定见 `docs/api/bindings/`（Python/JS/Go/Java/R 五端，
  覆盖库/会话/事务/结果集/模式/查询执行/参数化/校验解释/会话统计，首版排除 UDF、钩子、预编译、批量、向量/全文、迁移、会话变量、逐格访问器）。
