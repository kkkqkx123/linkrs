# Linkrs CLI 功能完整性分析 — 与 server 能力对比及补全设计

## 0. 文档信息

| 项 | 内容 |
|---|---|
| 分析对象 | `crates/linkrs-cli`（CLI）与 `crates/linkrs-server` / `crates/linkrs-client` / `linkrs-wire` 的功能覆盖差 |
| 代码基线 | main 分支，`linkrs-cli` 12,028 行 / `linkrs-server` 33,482 行（`.rs` 文件统计） |
| 关联文档 | `docs/client/design/feature_comparison.md`（psql 对比，含设计承诺）、`docs/client/design/cli_design.md` |
| 结论速览 | CLI 面向"交互式 GQL 客户端"定位基本完整；但 server 暴露的**管理面能力（用户/会话/全文/向量/迁移/统计扩展）在 client 与 CLI 层完全未接入**，且早期设计承诺的部分命令未落地。 |

## 1. 背景与目标

1. 对比 CLI 与 server 的功能面，回答"CLI 功能实现是否完整"。
2. 给出需补充功能的分层设计方案（client 层 API 封装 → CLI 元命令），含行号锚点与代码 diff 草案。
3. 留出待拍板开放点，供开发板验证前决策。

## 2. 现状事实（架构与代码定位）

### 2.1 数据通路

```
linkrs-cli ──> linkrs-client (HTTP, reqwest) ──> linkrs-server HTTP (axum /v1/*)
                    │                                └─ gRPC (tonic, proto/linkrs.proto, 102 rpcs)
                    └─ linkrs-wire (共享 DTO)
```

- CLI 通过 `linkrs-cli/src/lib.rs:3` `pub use linkrs_client as client;` 与 `lib.rs:12` `pub use linkrs_client::session;` 复用 client crate。
- CLI **只有 HTTP 一种传输**：`crates/linkrs-cli/src/cli.rs:13-17` 仅有 `--host/--port`，无协议选择、无 TLS。
- client 的 HTTP 封装集中在 `crates/linkrs-client/src/http_client.rs`（822 行），全部端点为字符串拼接（如 `http_client.rs:45` `format!("http://{}:{}/v1", ...)`）。

### 2.2 server 侧能力面（HTTP 路由，`crates/linkrs-server/src/http/router.rs`）

| 域 | 端点 | router.rs 行号 |
|---|---|---|
| 认证 | `/auth/login`、`/auth/logout`、`/auth/me` | 78, 89-90 |
| **用户管理** | `/users`（list/create）、`/users/{name}`（drop）、`/users/{name}/password|enable|disable|grant|revoke` | 92-106 |
| **会话管理** | `GET /sessions`、`GET/DELETE /sessions/{id}` | 107-108 |
| 查询 | `/query`、`/query/validate`、`/query/explain`、`/query/batch` | 109-112 |
| 事务 | begin/list/commit/rollback/**kill**/**outbox retry**/**savepoints CRUD** | 114-139 |
| 批量 | `/batch*` | 141-145 |
| **服务端导入导出** | `/import`、`/import/{id}`、`/export` | 147-149 |
| 统计 | sessions/queries/**queries/{trace_id}**/overview/database/system/**search**/**freeze**(GET+POST)/**migration** | 151-162 |
| 配置 | `/config`、`/config/{section}/{key}`（GET/PUT/DELETE） | 164-168 |
| 函数 | `/functions`（register/list/info/unregister） | 170-171 |
| 流式/游标 | `/query/stream`、`/query/cursor/{open,fetch,close}` | 173-186 |
| 同步 | `/sync/*`（status/retry/diagnostics/dead_letters/requeue/degraded/retention） | 188-196 |
| Schema | spaces/tags/edge-types CRUD | 198-213 |
| **Schema 版本化** | `/schema/versions/{space}/{label}`、`/schema/changes/...`、`/schema/breaking-changes/...` | 215-226 |
| **迁移 REST** | `/migration/{plan,execute,rollback,dry-run,history,status,stream}` | 228-246 |
| **全文（feature）** | `/fulltext/{indexes,rebuilds,search,clear,inconsistent}` | 317-338, 374 |
| **向量（feature）** | `/vector/{indexes,search,scroll,payload,rebuilds,clear,count}` 等 12 条 | 407-443 |
| 运维 | `/health`、`/metrics`（Prometheus） | 76-77 |
| Web 管理面 | `/api/v1/{queries,schema,data,graph}`（web UI 专用，`web.rs:76-79`） | — |

### 2.3 CLI 侧已实现功能

- 启动参数：`cli.rs:12-69`（host/port/user/password/database/-c/-f/output/format/quiet/single-transaction/force/variables）。
- REPL：`main.rs:145-203`（多行续行 `is_statement_complete`、`exit/quit/\q`）。
- 元命令分发全集：`crates/linkrs-cli/src/command/executor.rs:199-406`，共 **63 个 MetaCommand 变体**（`parser/types.rs:13-180`），覆盖：
  连接（login/passwd/whoami/connect/disconnect/conninfo）、对象查看（show_spaces/show_tags/show_edges/show_functions/describe/describe_edge）、输出（format/pager/timing）、变量、脚本（\i/\ir/\o/\!）、缓冲区（\e/\p/\r/\w）、历史、事务（begin/commit/rollback/autocommit/isolation/savepoint/rollback_to/release/txstatus）、条件执行（\if/\elif/\else/\endif）、EXPLAIN/PROFILE、导入导出（\import/\export/\copy/\export-space/\export-schema/\import-schema）、流式/游标（\stream/\cursor）、统计（\statistics 5 个 target/\status）、配置（\config show|get|set|reset）、批量（\batch 6 个子命令）、同步（\sync 8 个子命令）。

## 3. 对比矩阵（server 能力 × CLI 覆盖）

图例：✅ 已接入；🟡 部分接入/等价实现；❌ server 有但 CLI 链路完全缺失。

| 能力域 | server 端点 | client 封装 (http_client.rs) | CLI 命令 | 结论 |
|---|---|---|---|---|
| 登录/登出 | /auth/login, /auth/logout | `login` :808, `disconnect` :137 | \login, \disconnect | ✅ |
| 当前身份 | /auth/me | 无 | \whoami 用**本地缓存**（connection.rs:42-59，不回源） | 🟡 |
| 改密 | CHANGE PASSWORD GQL（grammar 支持，bind_dcl.rs:4） | 走 /query | \passwd（connection.rs:85-114） | ✅ |
| **用户管理** | /users×8 | 无 | 无 \du/\show_users/\create user | ❌ |
| **会话管理** | GET/DELETE /sessions | 无 | 无 \sessions | ❌ |
| 查询/批量查询 | /query, /query/batch | :164, :192 | 直接输入, \batch | ✅ |
| 语法预检 | /query/validate | 无 | 无 \validate | ❌ |
| 执行计划 | /query/explain | 无（CLI 发 `EXPLAIN <q>` GQL，analyze.rs:16-25） | \explain（ANALYZE 明确未实现 analyze.rs:14） | 🟡 |
| **查询画像** | /statistics/queries/{trace_id} | 无 | \profile 仅本地计时（analyze.rs:31-40） | ❌ |
| 事务基本 | begin/commit/rollback | :388, :415, :438 | \begin/\commit/\rollback | ✅ |
| **事务管理面** | list/metrics/kill/outbox retry | 无 | 无 \tx kill/list | ❌ |
| 保存点 | REST /transactions/{id}/savepoints；GQL SAVEPOINT 也支持（bind_ddl.rs:181） | 无 REST（走 GQL） | \savepoint/\rollback to/\release | 🟡（GQL 通路可用，REST 未接） |
| 批量任务 | /batch×6 | :463-549 | \batch | ✅ |
| 客户端导入导出 | （CLI 本地文件→分批语句） | — | \import/\export/\copy/\export-space | ✅ |
| **服务端导入导出** | /import, /import/{id}, /export | 无 | 无 | ❌ |
| 统计 5 项 | overview/database/system/queries/session | :554, :565, :576, :642, :653 | \statistics 5 target | ✅ |
| **统计扩展** | search/freeze/migration/queries/{trace_id} | 无 | \statistics 不支持这 4 个 target | ❌ |
| 配置热更 | /config×2 | :589-639 | \config show/get/set/reset | ✅ |
| 函数 | /functions×4 | 仅 `list_functions` :781 | \show_functions 仅列表 | 🟡（缺 register/info/unregister） |
| 流式查询 | /query/stream | :666 | \stream | ✅ |
| 游标 | /query/cursor×3 | :697-775 | \cursor | ✅ |
| 同步/outbox | /sync×9 | outbox.rs:7-162 | \sync 8 子命令 | ✅ |
| **Schema 版本化** | /schema/versions、/changes、/breaking-changes | 无 | 无 | ❌ |
| **迁移** | /migration×7 | 无（独立 `linkrs-migration-cli` 嵌入式工具，直连 db_path，不走向网络） | 无 | ❌ |
| **全文检索（feature）** | /fulltext×6 | 无 | 无（设计承诺 `\show_fulltext_indexes`，feature_comparison.md:96） | ❌ |
| **向量（feature）** | /vector×12 | 无 | 无（设计承诺 `\show_vector_indexes`，feature_comparison.md:97） | ❌ |
| gRPC | 102 rpcs | 仅 HTTP | 仅 HTTP | 🟡（定位问题，见开放点 3） |
| Prometheus | /metrics | 无 | 无 | ❌（运维向，低优） |
| Web 管理面 | /api/v1/* | 不适用 | 不适用 | —（web UI 专属，CLI 不必覆盖） |

## 4. 差距分析

### 4.1 定性结论

1. **"交互式查询客户端"主链路完整**：GQL 执行、schema 查看、事务、游标/流式、导入导出、配置、批量、同步均已闭环，质量与 psql 对标设计基本一致。
2. **"管理面"整体缺失**：server 把用户/会话/全文/向量/迁移/版本化做成了一等公民（含鉴权与 DTO），但 `linkrs-client` 没有封装、CLI 没有命令。这是最大的一块系统性缺口，而非零散遗漏。
3. **设计承诺未兑现清单**（`docs/client/design/feature_comparison.md`）：`\show_users`（:22）、`\show_indexes`（:19）、`\show_fulltext_indexes`（:96）、`\show_vector_indexes`（:97）、`\ef`（:58）、EXPLAIN ANALYZE 等价物（:76 的 PROFILE 实际只是本地计时）。
4. **半成品两处**：`\explain ... analyze`（analyze.rs:14 打印 "not yet implemented"）；`\profile`（analyze.rs:31-40 只做本地 `QueryTimer`，未消费 server 的 trace 级画像 `/statistics/queries/{trace_id}`）。

### 4.2 缺口分级

| 级别 | 缺口 | 理由 |
|---|---|---|
| P0 | 用户管理、会话管理、\show_users、EXPLAIN ANALYZE、\profile 接 trace 画像 | 管理面核心；wire DTO 已就绪（见 5.1），补齐成本低 |
| P1 | 全文/向量元命令、\statistics 扩展 target、\validate、函数 register/unregister/info、schema 版本化查看 | feature 开关依赖，价值高但可分批 |
| P2 | 服务端 import/export 任务模式、迁移 REST 接入、gRPC 传输、TLS/超时参数、/metrics | 场景较窄或与现有工具定位重叠 |

## 5. 补充功能设计

### 5.1 P0：管理面接入（client → CLI 三层改动）

**依据**：`linkrs-wire/src/meta.rs` 已有全部 DTO——`UserListResponse`(:61)、`CreateUserRequest`(:74)、`ResetPasswordRequest`(:81)、`GrantRoleRequest`(:87)、`RevokeRoleRequest`(:94)、`AuthMeResponse`(:37)。client 层只需新增方法，CLI 层新增元命令。

#### 改动 1：`crates/linkrs-client/src/http_client.rs` 追加 user/session 管理（插在 `:803` `// ── Auth ──` 区块之前）

```rust
// ── User management ──

/// List users via `GET /v1/users`.
pub async fn list_users(&self) -> Result<linkrs_wire::meta::UserListResponse> {
    let url = format!("{}/users", self.base_url);
    let response = Self::check_response(self.authed(self.inner.get(&url)).send().await?, "Failed to list users").await?;
    Ok(response.json().await?)
}

/// Create a user via `POST /v1/users`.
pub async fn create_user(&self, username: &str, password: &str) -> Result<()> {
    let url = format!("{}/users", self.base_url);
    let request = linkrs_wire::meta::CreateUserRequest {
        username: username.to_string(),
        password: password.to_string(),
    };
    let response = Self::check_response(self.authed(self.inner.post(&url)).json(&request).send().await?, "Failed to create user").await?;
    Ok(())
}

/// Drop / enable / disable / grant / revoke / reset_password 同构，分别对应
/// DELETE /v1/users/{name}、POST /v1/users/{name}/{enable,disable,grant,revoke,password}。
// （实现略，模式同上；DTO 复用 GrantRoleRequest / RevokeRoleRequest / ResetPasswordRequest）

// ── Session management ──

/// List active sessions via `GET /v1/sessions`.
pub async fn list_sessions(&self) -> Result<Vec<linkrs_wire::meta::SessionResponse>> { /* GET /sessions */ }

/// Kill a session via `DELETE /v1/sessions/{id}`.
pub async fn kill_session(&self, session_id: i64) -> Result<()> { /* DELETE /sessions/{id} */ }

/// Refresh current identity via `GET /v1/auth/me`.
pub async fn auth_me(&self) -> Result<linkrs_wire::meta::AuthMeResponse> { /* GET /auth/me */ }
```

#### 改动 2：`crates/linkrs-cli/src/command/parser/types.rs` 扩展枚举

```rust
// types.rs:37 之后（WhoAmI 变体后）插入：
    ShowUsers,                       // \show_users / \du
    Users { action: UsersAction },   // \users create|drop|enable|disable|grant|revoke|passwd
    Sessions { action: SessionsAction }, // \sessions list|kill <id>
    Validate { query: String },      // \validate <query>
```

配套新增 `UsersAction` / `SessionsAction` 枚举（模式对齐 `types.rs:209` `BatchAction`）。

#### 改动 3：`crates/linkrs-cli/src/command/executor.rs` 分发（`:213` WhoAmI 之后对齐插入）+

 新执行器文件 `command/executor/meta/users.rs`（参照 `meta/batch.rs` 结构）；帮助文本更新 `command/meta_commands.rs:20-44`（Connection 区块）。

#### 改动 4：EXPLAIN ANALYZE 与 PROFILE 落地

- `analyze.rs:14` 删除 "not yet implemented" 分支：server 已有 `POST /v1/query/explain`（router.rs:111），改为：`analyze=true` 时请求该端点（需在 client 增加 `explain_query`，请求体复用 `linkrs_wire::query::QueryRequest`）。
- `\profile`：查询后从 `QueryResponse` 提取 `trace_id`（`linkrs-wire/src/query.rs`，若响应未携带需在 wire/server 侧补字段——见开放点 5），再调 client 新增 `get_query_profile_detail(trace_id)`（GET `/v1/statistics/queries/{trace_id}`，router.rs:153），渲染 `ProfileResult`（analysis/profile.rs:105 已有报告格式化器，直接复用）。

### 5.2 P1：全文/向量与统计扩展

- client 增加 `fulltext_search`、`list_fulltext_indexes`、`rebuild_fulltext`、`clear_fulltext`、`fulltext_rebuild_status`（对应 router.rs:317-338, 374；DTO 用 `linkrs-wire/src/fulltext.rs`）。
- client 增加 vector 12 端点封装（router.rs:407-443；DTO 用 `linkrs-wire/src/vector.rs`）。
- CLI 新增：`\fulltext search <q> [limit]`、`\fulltext rebuild|clear|status`、`\show_fulltext_indexes`；`\vector create|drop|search|scroll|count|payload|rebuild`、`\show_vector_indexes`；`\statistics fulltext|vector|freeze|migration|profile <trace_id>` 扩展 target（stats.rs:33-56 的 match 增加分支）。
- `\validate`：client `validate_query`（POST `/v1/query/validate`，router.rs:110）。
- schema 版本化：`\schema history <space> <label>`、`\schema changes <space> <label> <from> <to>`、`\schema breaking <space> <label> <from> <to>`（router.rs:215-226）。

### 5.3 P2：可选增强

- 服务端 import/export：`\import-server` / `\export-server`（router.rs:147-149），大文件场景走任务态轮询 `/import/{id}`。
- 迁移 REST 接入：`\migration plan|dry-run|execute|rollback|status|history|watch`（router.rs:228-246；`watch` 用 SSE `/migration/stream/...`，SSE 解析器已具备 `linkrs-client/src/sse.rs`）。注意与 `linkrs-migration-cli`（嵌入式，migration-cli/src/main.rs:5-8 直连 `db_path`）的定位区分——REST 版面向远程管理，嵌入版面向本地运维，二者并存。
- `cli.rs` 增加 `--timeout <secs>`、`--tls`（ClientConfig 已有 `timeout_seconds` :21，reqwest 需换 `rustls` feature，workspace `reqwest` 已声明 rustls）。
- gRPC 传输：`cli.rs` 加 `--protocol http|grpc`，grpc feature 已在 workspace（tonic 0.12）；建议等 gpui 客户端稳定后再做。

## 6. 代码改动清单汇总

| 文件 | 改动 | 级别 |
|---|---|---|
| `crates/linkrs-client/src/http_client.rs` | +users/sessions/auth_me/explain/validate/profile/fulltext/vector 封装（约 +400 行） | P0/P1 |
| `crates/linkrs-cli/src/command/parser/types.rs` | MetaCommand +UsersAction/SessionsAction/Validate 等 | P0/P1 |
| `crates/linkrs-cli/src/command/parser/` | meta 解析分支（参照 parser/meta/batch.rs 模式） | P0/P1 |
| `crates/linkrs-cli/src/command/executor.rs` | 分发分支（:199-406 区域） | P0/P1 |
| `crates/linkrs-cli/src/command/executor/meta/users.rs`（新） | 用户/会话执行器 | P0 |
| `crates/linkrs-cli/src/command/executor/meta/analyze.rs` | EXPLAIN ANALYZE 接 /query/explain；\profile 接 trace 画像 | P0 |
| `crates/linkrs-cli/src/command/meta_commands.rs` | 帮助文本（:20-44, :216-224 区域） | P0/P1 |
| `crates/linkrs-cli/src/completion/completer.rs` | 新命令补全 | P1 |
| `docs/client/design/feature_comparison.md` | 兑现后更新承诺表 | P1 |

## 7. 验证方案

1. 单测：client 层每个新方法配 wire 序列化往返测试（模式同 `http_client.rs` 既有测试）；CLI 层 parser 测试追加到 `command/parser/tests.rs`。
2. 集成：`crates/linkrs-cli/tests/command_integration_tests.rs` 增加用户/会话管理用例（需 server 起本地实例）。
3. 开发板验证顺序：先 `cargo test -p linkrs-client -p linkrs-cli`，再起 `linkrs-server` 后以 `\users create` → `\show_users` → `\sessions list` → `\explain analyze` → `\profile` 全链路手工验证。
4. feature 矩阵：`--no-default-features`（P1 的 fulltext/vector 命令需优雅降级报错，不能编译失败）。

## 8. 风险与兼容性

| 风险 | 缓解 |
|---|---|
| 用户管理命令涉及鉴权（auth_middleware，router.rs:247-250），低权限用户执行会 4xx | CLI 统一渲染 server 错误体（check_response 已带 body，http_client.rs:93-108），并在 help 标注 admin-only |
| `\profile` 依赖响应携带 trace_id，wire 可能缺字段 | 见开放点 5，先核验 `QueryResponse` 字段再动工 |
| fulltext/vector 为 feature-gated，默认构建（default = server,vector，root Cargo.toml `[features]`） | 命令执行时按 `statistics` 返回的能力探测降级，或用编译期 feature 传递到 CLI crate |
| gRPC 与 HTTP 双协议长期维护成本 | P2 且默认单协议；项目处 no-backward-compatible 阶段（AGENTS.md:3-4），可大胆收敛 |

## 9. 与既有文档的一致性

- 本文档兑现 `docs/client/design/feature_comparison.md` 中"第三阶段"未完成项（:160-183 的事务保存点已通过 GQL 完成；性能分析、扩展功能两节仍缺）。
- `docs/client/design/implementation_roadmap.md` 如与本文冲突，以本文的行号核验为准，落地后同步回写。

## 10. 待拍板开放点

1. **用户管理命令的命名**：`\users create <name>`（子命令式）还是 `\create user <name>`（GQL DCL 式）？建议子命令式，与 \batch/\sync 一致。
2. **REST savepoints 是否切替**：CLI 现走 GQL SAVEPOINT（grammar 支持），server 另有 REST savepoints（router.rs:129-139，可操作存储层 staged boundary）。是否把 CLI 事务管理切到 REST 以获得更强的崩溃一致性？（涉及 transaction.rs:118-165 重写）
3. **gRPC 传输是否进 CLI**：102 个 RPC 已在 proto 就绪，但 CLI 场景 HTTP+SSE 已覆盖；建议推迟到 gpui 客户端阶段统一决策。
4. **Web 管理面（/api/v1/queries 历史与收藏）**是否值得给 CLI 加 `\favorites` 命令（复用 sqlite 存储），还是保持 web UI 专属？
5. **`\profile` 的前置依赖**：需核验 `QueryResponse`（linkrs-wire/src/query.rs）是否返回 trace_id；若无，改动面扩大到 wire + server，需单独评估。
6. **P1 全文/向量命令的 feature 降级策略**：编译期 feature 透传（构建产物分叉）vs 运行期能力探测（单一产物）。建议运行期探测。
