# GPUI 剩余开放问题：设计决策与修改方案

> 依据分析：`docs/analysis/gpui-client-analysis.md`
> 依据方案：`docs/plan/gpui-client-remediation-plan.md`（第 5 节开放点）
> 基线提交：`a656b459`（`update frontend res requirement`）
> 约束（AGENTS.md）：文档中文、代码/注释/日志英文、注释不得引用方案标识符、禁止 `unwrap`（测试可用 `expect`）。
> 本轮目标：对上一轮遗留的 5 个开放点逐条拍板并完成实现；随后追加三项收尾——重登指数退避、未提交事务提示、消除 `is_cancelled` 死代码告警。

## 0. 结论速览

| 开放点 | 决策 | 本轮动作 |
| --- | --- | --- |
| G6 自动重登 | **实现静默重登**：401/AuthError 触发、OS keychain 凭据重放、single-flight、3 次失败预算、space 恢复、失败降级登录页 | 已实现（阶段 R） |
| G7 gRPC 去留 | **不移除 SDK，也不接入 GPUI**：gRPC 定位为「服务间调用」可选能力，保持 feature-gated；GPUI 维持纯 HTTP | 澄清边界 + 编译期证据 + 文档定位（阶段 R-文档） |
| 重登退避间隔 | **指数退避冷却门**：1s→2s→4s→8s 封顶，失败清零；被动触发（后续 401 驱动） | 已实现（阶段 R2） |
| 未提交事务 | **重登成功后 toast 提示核对**：401 时旧会话事务已失效且无法查询，客户端不跟踪 txn_id | 已实现（阶段 R3） |
| 流式 UI 增量渲染 | **暂缓**：网络层已真流式，UI 增量渲染收益低且需实机调参 | 不改，保留开放点 |
| G5 元数据 single-flight | **不实施**：`ClientError` 不可 Clone，view 层已有 generation guard | 不改，记录取舍 |
| 阻塞诊断（取 clone 瞬间争用） | **不预判**：保持现状，压测后再定 | 不改，保留开放点 |
| `is_cancelled` 告警 | **删除未调用的查询方法** | 已修复 |

---

## 1. 决策一：自动重登（G6）——实现

### 1.1 为什么现在实现

上一轮记录了「是否自动重新登录涉及凭据重放与用户意图，属产品决策」。本轮用户明确要求实现，并参考其他数据库 GUI 客户端。调研结论：主流 GUI 客户端（DBeaver、DataGrip、Navicat、Compass）普遍支持「记住密码 + 连接失效时自动重连重登」，但遵循三条边界：

1. **仅在用户已授权保存凭据时静默重放**（否则必须弹出登录框）；
2. **失败有上限与退避**，不可无限重试（DBeaver 默认 5–10 次、间隔 5–10s）；
3. **重连后恢复会话上下文**（当前库/space、身份），失败则清晰降级到登录页。

本项目已具备全部前提：

- 凭据已在 OS keychain：`credentials.rs:26` `load` / `:50` `save`，按 `host:port:user` 隔离（`credentials.rs:19-21`）；登录成功后按"记住密码"勾选写入（`login_view.rs:174-186`）。
- 最后连接信息（host/port/user，无密码）已持久化：`settings.rs:20-24` `LastConnection`、`:300` `record_connection`。
- 重连入口现成：`SessionManager::connect_with_host`（`manager.rs:162-172`）→ `HttpClient::reconfigure`（`http_client.rs:158-165`）原地换配置并保留 `Arc` 共享会话，已发出的 client clone 不失效。
- 失效信号统一：各 view 检测到 401/AuthError 后调用同一个 `invalidate_session`（`connection_service.rs:107-111` 定义 `is_session_expired`；调用点遍布 `views/*.rs`，如 `console.rs:520`、`data_browser.rs:171`、`schema.rs:309`）。

因此改动可收敛到单一入口 `invalidate_session`，无需触碰 60+ 调用点。

### 1.2 触发条件与边界

| 维度 | 决策 | 依据 |
| --- | --- | --- |
| 触发信号 | 仅 401/AuthError（会话确实失效），由 `is_session_expired` 判定 | 心跳 `Unhealthy`（`connection_service.rs:118-141`）只代表探测失败，不等于会话失效；用心跳触发重登会在网络抖动时制造无谓登录 |
| 凭据门槛 | 仅当 keychain 中存在该 endpoint 的密码才静默重登；否则立即降级登录页 | 用户意图边界；未勾选"记住密码"时不可凭空重放 |
| 并发 | single-flight：并发 401 只启动一次重登 | 一个失效会话会让多个在途请求同时 401；避免登录风暴 |
| 失败预算 | 连续失败 3 次后降级登录页，成功即清零 | 防密码已改/服务不可用时无限重试 |
| 状态恢复 | 重登成功后恢复失效前的 space 与身份 | 新登录默认无 space（`manager.rs:149-157`），需显式 `switch_space` 还原 |
| 用户提示 | 静默：成功无感；失败回登录页，由现有登录视图承接 | 不新增 i18n 文案，改动最小 |

### 1.3 状态机设计

新增 `ReconnectState`（`session_store.rs:15-47`），以 `AtomicBool`（单飞）+ `AtomicU32`（失败计数）实现，无锁、可跨 await `Arc` 共享：

```rust
pub struct ReconnectState {
    in_flight: AtomicBool,
    failures: AtomicU32,
}
```

| 方法 | 行号 | 语义 |
| --- | --- | --- |
| `try_start` | `session_store.rs:23-27` | CAS 抢占单飞槽；失败表示已有重登在进行 |
| `reset` | `:30-33` | 成功后清零失败计数并释放槽 |
| `abort` | `:37-39` | 无凭据时释放槽，不计失败 |
| `finish_failure` | `:43-47` | 失败计数 +1 并释放槽；达到 `MAX_RECONNECT_ATTEMPTS=3`（`:9`）时返回 `true` 触发降级 |

### 1.4 执行流程（`invalidate_session`，`connection_service.rs:151-233`）

```
401/AuthError → invalidate_session
  ├─ try_start 失败 → 直接返回（已有重登在途）
  ├─ 无 last_connection → abort + drop_to_login
  └─ 有 last_connection
       ├─ 快照 space（cx 线程），后台线程读 keychain 密码
       ├─ 无密码 → abort + drop_to_login
       └─ 有密码 → connect(host,port,user,password)
            ├─ 成功 → switch_space(还原 space) + reset + set_connected
            └─ 失败 → finish_failure
                 ├─ 未达 3 次 → 保持会话（后续请求可再次触发）
                 └─ 达 3 次 → drop_to_login
```

关键实现点：

1. keychain 读取放到 `background_executor`（`connection_service.rs:170-173`），避免阻塞 UI 线程；为此将 `credentials` 模块从私有改为 `pub(crate)`（`connection.rs:1`）。
2. 重登复用 `connect()`（`:28`）而非自建登录逻辑，确保 `auth_me` 刷新角色、密码不出错路径。
3. space 还原在 `cx.update` 中完成（`:190`），成功后覆盖 profile.space 再 `set_connected`（`:195-205`）。
4. `drop_to_login`（`:221-230`）抽出为共享原语，best-effort 服务端 logout 后 `set_disconnected`。

### 1.5 与前端的一致性

前端在 401 时是 `clearSessionId()` 后退回（`client.real.ts:35,120`、`streamQuery.ts:65-67`），不做重登。GPUI 侧实现静默重登是桌面端增强（有 OS keychain 持久化凭据，前端只有 sessionId 无密码，无法重放登录）。两者不冲突：GPUI 重登成功即恢复，失败退化为与前端一致的"未连接"状态。

---

## 2. 决策二：gRPC 去留（G7）——不移除 SDK，也不接入 GPUI

### 2.1 项目事实（file:line）

1. gRPC 客户端代码全部集中在 `linkrs-client`，且受 feature 门控：`lib.rs:8-9` `#[cfg(feature = "grpc")] pub mod grpc_client;`、`:22-23` 条件 re-export。
2. feature 定义：`linkrs-client/Cargo.toml:31` `grpc = ["dep:tonic", "dep:prost", "dep:tonic-build", "dep:rustls"]`。
3. **无任何消费方启用**：GPUI 依赖为 `linkrs-client = { path = "../linkrs-client" }`（`gpui/Cargo.toml:67`），CLI 同样不带 features（`cli/Cargo.toml:29`）；全仓 grep 无 `linkrs-client` 的 `features = ["grpc"]`。
4. GPUI 源码 0 处 gRPC 调用（grep `grpc` 于 `linkrs-gpui/src` 无结果）。
5. 根 `Cargo.toml:254` 的 `grpc = ["linkrs-server/grpc"]` 只启用**服务端** gRPC，与客户端 crate 无关。
6. 服务端 gRPC 是独立端口：`config.rs:170-172` 强制 http.port 与 grpc.port 不同，默认 gRPC 端口 9669（`:815`）。

**结论先行**：「移除 GPUI 客户端对 gRPC 的支持」是伪命题——GPUI 从未依赖、从未调用 gRPC。真正要决策的是 `linkrs-client` 中那份 feature-gated 的 gRPC SDK 是否保留。

### 2.2 其他数据库客户端的做法（调研）

| 产品 | 客户端↔服务端协议 | gRPC 的角色 |
| --- | --- | --- |
| DBeaver / DataGrip / Navicat | JDBC/ODBC 厂商驱动 | 不用于 GUI 连接 |
| pgAdmin 4 | Web HTTP → 后端 libpq | 不用于 GUI 连接 |
| RedisInsight | Electron → REST + WebSocket → 原生 Redis 协议 | 不用于 GUI 连接 |
| Neo4j | Bolt 7687（驱动）、HTTP 7474（Browser） | 集群内网端口非客户端主通道 |
| CockroachDB | SQL 26257（客户端）、HTTP 8080（Console）、gRPC 26258（**仅节点间**） | 服务间 |
| TiDB | MySQL 4000（应用/DBA 工具）、HTTP 10080、PD gRPC 2379 | 服务间 |

要点：

- gRPC 的公认定位是 **service-to-service**；浏览器无法原生发 gRPC，需 gRPC-Web + 代理，且不支持 client streaming / 全双工。
- 二进制帧不便抓包调试，HTTP/2 trailer 常被代理/防火墙阻断；SSE 推送通常另开 HTTP 通道。
- 分布式库普遍采用 **「外部 HTTP/原生协议给客户端，内部 gRPC 给节点间」** 的双通道隔离（CockroachDB、TiDB）。

### 2.3 决策

**保留 `linkrs-client` 的 gRPC SDK，但明确其为「服务间调用」可选能力；GPUI 维持纯 HTTP。** 理由：

1. 该 SDK 与服务端 gRPC 端口（9669）成对，正是"其他服务调用 Linkrs"的通道，符合 CockroachDB/TiDB 的内部 gRPC 模式；删除会损害真实的服务间调用场景。
2. 它已是 feature-gated（`Cargo.toml:31`），不启用时零编译/零链接成本，**保留没有维护税负**——`services/*.rs` 与 GPUI 完全不感知它。
3. GPUI 不接入 gRPC，与 Web 前端共用 HTTP 传输，符合"桌面客户端走 HTTP"的业界惯例，也避免两套传输语义分叉（上一轮 F1–F9 已证明 HTTP 侧鉴权/流式处理是正确路径）。

**不采取的动作**（及原因）：

- 不从 `linkrs-client` 删除 gRPC 模块 / 依赖：会让服务间调用方失去 SDK，且与"保留服务端 gRPC"自相矛盾。
- 不在 GPUI 引入 gRPC：无收益，徒增双重维护与调试成本。

**边界固化建议**（写入文档，不强制代码）：在 `docs/api/server/grpc_api.md` 或 SDK README 注明「gRPC 面向服务间调用；GUI/CLI 客户端使用 HTTP」。本轮不改该文件，列为文档开放点（见第 5 节）。

---

## 3. 决策三：流式 UI 增量渲染——暂缓

- 事实：网络层 `execute_query_stream` 已改 `bytes_stream()` 增量读取（`http_client.rs:1085`、`:1279`），消除全量缓冲与 30s 截断；但 UI 侧仍"等完整 `QueryResult` 再一次渲染"（上一轮方案 §4.1）。
- 决策：**暂缓**。网络层的正确性/超时问题已解决，增量渲染属体验优化；批次粒度与 `cx.notify()` 频率需实机调参，无法在当前无 GUI 运行环境验证。贸然改 UI 渲染路径有回归风险。
- 触发再评估的条件：实机出现"大结果集首屏等待明显"的用户反馈，或可做目标平台性能基线时。

## 4. 决策四：G5 元数据 single-flight——不实施

- 事实：`ClientError` 含 `reqwest::Error`（`utils/error.rs`），不实现 `Clone`；转 `String` 会破坏 `is_session_expired`（`connection_service.rs:107-111`）对 401 的结构化匹配。各 view 已有 generation guard（如 `console.rs` 的 `stream_cancel`/generation），重复请求是浪费而非正确性问题。
- 决策：**不实施结果级 single-flight**。收益低、改造面大（需重构错误类型）。若未来要做，优先方案是"oneshot 广播 + `Arc<ClientError>` 错误包装"，而非直接依赖 `futures::Shared`。

## 5. 决策五：阻塞诊断——保持现状

- 事实：阶段 C 采用 `manager.lock().await.client_clone()`，锁只在 clone 瞬间持有（纳秒级、不跨 await），HTTP 请求在无锁 clone 上进行（上一轮方案 §4.1）。
- 决策：**不预判、不改**。真正验证需在目标平台压测监控页 11 路并发请求的吞吐；若确认 clone 争用成为瓶颈，再在 `SessionStore` 层缓存句柄。避免在无数据支撑时增加缓存一致性复杂度。

---

## 6. 落地状态与验证

| 阶段 | 决策 | 状态 | 落地文件 | 验证 |
| --- | --- | --- | --- | --- |
| R | 自动重登 | 已完成 | `connection/session_store.rs`（`ReconnectState` + 6 单测）、`services/connection_service.rs`（`invalidate_session` 重写 + `drop_to_login`）、`connection.rs`（credentials 可见性） | `cargo check` / `rustfmt --check` 通过；6 单测 + 全量 72 测试通过（见 §6.2） |
| R2 | 重登指数退避 | 已完成 | `connection/session_store.rs`（`backoff_ms` + `cooldown_until_ms` + 3 新单测） | 同上；退避序列 1s/2s/4s/8s 封顶 |
| R3 | 未提交事务提示 | 已完成 | `state.rs`（notice drop-box）、`services/connection_service.rs`（重登成功后发布提示）、`views/shell.rs`（消费并弹 toast）、i18n 中英文案 | `cargo check` 通过；i18n JSON 校验通过 |
| — | gRPC 文档定位 | 已完成 | `docs/api/server/grpc_api.md`（「Intended audience」小节） | 见 §2 |
| — | 流式 UI / G5 / 阻塞诊断 | 决策完成（不改） | 本文 §3–5 | — |
| — | `is_cancelled` 死代码告警 | 已修复 | `runtime.rs`（删除未使用的查询方法） | `cargo check` 无 dead_code 告警 |

### 6.1 代码 diff 摘要

**`connection/session_store.rs`** — 协调状态 + 指数退避 + 6 单测：

```diff
 pub(crate) const MAX_RECONNECT_ATTEMPTS: u32 = 3;
+const RECONNECT_BACKOFF_BASE_MS: u64 = 1000;
+const RECONNECT_BACKOFF_CAP_MS: u64 = 8000;
+fn backoff_ms(failures: u32) -> u64 { /* 1s,2s,4s,8s capped */ }
+fn now_ms() -> u64 { /* SystemTime 毫秒 */ }
 #[derive(Default)]
 pub struct ReconnectState {
     in_flight: AtomicBool,
     failures: AtomicU32,
+    cooldown_until_ms: AtomicU64,
 }
 impl ReconnectState {
-    pub fn try_start(&self) -> bool { /* CAS false->true */ }
-    pub fn reset(&self) { /* failures=0, in_flight=false */ }
+    pub fn try_start(&self) -> bool { /* now>=cooldown 才 CAS false->true */ }
+    pub fn reset(&self) { /* failures=0, cooldown=0, in_flight=false */ }
     pub fn abort(&self) { /* in_flight=false */ }
-    pub fn finish_failure(&self) -> bool { /* +1; >=3 => true */ }
+    pub fn finish_failure(&self) -> bool { /* +1; cooldown=now+backoff(n); >=3 => true */ }
 }
```

**`services/connection_service.rs`** — 失效处理 + 成功后提示 + 登出清理：

```diff
 pub fn invalidate_session(cx: &mut App) {
     let reconnect = SessionStore::reconnect(cx);
     if !reconnect.try_start() { return; }   // 含冷却期拒绝
     // 快照 space → 后台读 keychain → connect → 成功恢复 space
+    //   成功分支：state.push_notice(t("status.reconnectedNotice"), cx)
     //   失败分支：finish_failure()（arm 退避）/ 预算耗尽降级
 }
 fn drop_to_login(cx: &mut App) {
+    SessionStore::reconnect(cx).reset();   // 清理失败计数与冷却
     /* best-effort logout + set_disconnected */
 }
```

**`state.rs`** — notice drop-box：`notice: Option<SharedString>` + `notice_generation: u64` + `push_notice()`，供后台任务向 Shell 传递一次性 toast。

**`views/shell.rs`** — `sync()` 观察 `notice_generation`，变化时 `window.push_notification(Notification::info(msg), cx)`。

**i18n**：`status.reconnectedNotice`（en/zh）提示用户旧会话的未提交事务已失效。

**`runtime.rs`**：删除未调用的 `CancellationToken::is_cancelled`（消除 dead_code 告警）。

**`docs/api/server/grpc_api.md`**：新增「Intended audience」小节，声明 gRPC 面向服务间调用、GUI/CLI 走 HTTP。

**`connection.rs`**：`mod credentials;` → `pub(crate) mod credentials;`

### 6.2 验证清单

- [x] `cargo check --manifest-path crates/linkrs-gpui/Cargo.toml` 通过（0 error、0 warning）
- [x] `rustfmt --edition 2021` 格式化 7 个改动文件后 `cargo check` 复验通过
- [x] 沙箱安装 `libxkbcommon-x11-0` / `-dev`（apt 去掉 `google-chrome.list` + `timeout 20s`），测试二进制可链接
- [x] `cargo test --manifest-path crates/linkrs-gpui/Cargo.toml session_store::tests` 6 单测通过（单飞、失败预算、abort、退避序列、冷却门、reset）
- [x] `cargo test --manifest-path crates/linkrs-gpui/Cargo.toml` 全量通过（75 passed，0 failed）
- [x] i18n `en.json` / `zh.json` JSON 校验通过
- [ ] 实机验证（待开发机）：记住密码后杀掉服务端 → 首个请求 401 → 静默重登成功且 space 保留、弹出事务提示；连续错误密码 3 次 → 回到登录页；并发请求只触发一次登录

> 已修复上一轮遗留告警 `method is_cancelled is never used`（`runtime.rs:39`，G4 引入）：该方法全 crate 无调用，删除后 `cargo check` 无 dead_code 告警。

## 7. 遗留开放点（本轮处理结论）

1. **gRPC 文档定位** — ✅ 已落 `docs/api/server/grpc_api.md`「Intended audience」小节。SDK README（`linkrs-client`）措辞未改：其 crate 文档已由 `lib.rs` 的 feature gate 说明，无需重复。
2. **自动重登的退避间隔** — ✅ 已实现指数退避冷却门（1s→2s→4s→8s 封顶 + 失败次数清零），见 §6.1。仍为被动触发（由后续 401 请求驱动重试），未做后台定时主动重试——避免无人操作时无限登录循环。
3. **未提交事务的处理** — ✅ 采用"重登成功后提示核对"方案。理由：401 时旧 session 已失效，服务端事务随会话消失、无法查询；客户端不跟踪 txn_id（由用户在面板手动输入），无法可靠检测未提交事务。故不采用 DBeaver 式"重连前拦截提示 commit/rollback"（需活跃会话，本项目场景不成立），改为 toast 提示用户检查并重跑。
4. **流式 UI 增量渲染 / G5 single-flight / 阻塞诊断** — 见 §3–5，均保留不改，触发条件已写明。
5. **未提交事务的精确跟踪（可选增强）** — 若产品要求"仅当确有未提交事务时才提示"，需在 `transaction_service::begin` 记录 txn_id、`commit/rollback/kill` 移除，并在重登前判断集合非空；因 txn_id 由文本框手动输入、覆盖不全，本轮不做，列为后续产品决策。
