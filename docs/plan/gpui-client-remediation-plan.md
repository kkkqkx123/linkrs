# GPUI 客户端改进方案

> 依据分析：`docs/analysis/gpui-client-analysis.md`
> 基线提交：`a656b459`（`update frontend res requirement`）
> 目标：将分析中列出的全部问题纳入改进项，分阶段完成代码修改，每阶段以 `cargo check` 验证。
> 约束（AGENTS.md）：文档中文、代码/注释/日志英文、注释不得引用方案标识符、禁止 `unwrap`（测试可用 `expect`）。

## 0. 范围与验证能力

- 涉及 crate：`linkrs-client`（网络层）、`linkrs-gpui`（桌面 UI 层，独立 workspace）。
- **验证能力已确认**：`cargo check -p linkrs-client` 与 `cargo check --manifest-path crates/linkrs-gpui/Cargo.toml` 均可在本地通过（基线 1m42s）。因此每个阶段都做完整编译验证。
- grammar 的 `parser.c` 已提交，构建只需 `cc`，不依赖 tree-sitter CLI / 网络。

## 1. 问题清单（全部纳入改进项）

编号沿用分析文档，新增项以 G 前缀。

| 编号 | 严重度 | 问题 | 位置 | 归属阶段 |
| --- | --- | --- | --- | --- |
| G1 | 高 | 全局 `Mutex<SessionManager>` 持锁跨 HTTP await，132 处请求被串行化；2 个 tokio worker 放大排队 | `session_store.rs:10-14`、各 `services/*.rs`、`app.rs:62` | 阶段 A–C |
| G2 | 中 | SSE 全量缓冲非真流式（查询/迁移流均 `response.text().await`） | `http_client.rs:1027,1204` | 阶段 D |
| G3 | 中 | 30s 全局超时作用于整段 SSE 读取，长流被截断 | `http_client.rs:112`、`config.rs:31` | 阶段 D |
| G4 | 中 | `runtime::spawn` 为 abandon 语义，UI Task 取消后底层请求继续占锁 | `runtime.rs:19-20,21-45` | 阶段 D |
| G5 | 低 | 元数据（spaces/tags/schema）无 single-flight，重复请求 | 各 `services/*`、`views/schema.rs` | 阶段 E |
| G6 | 决策 | 心跳失败仅置红状态栏，不自动重连/重登 | `connection_service.rs:111-125` | 阶段 E（保守：保持现状 + 记录决策） |
| G7 | 决策 | gRPC transport 与 HTTP 双份维护成本 | `lib.rs:9`、`grpc_client/*` | 不在本轮改动，列入开放点 |

> 对照结论：前端 F1–F9 在 GPUI 侧不存在或形态不同，无需移植修复；本方案只处理 GPUI 特有问题 G1–G7。

## 2. 总体架构改造思路

### 2.1 去锁的核心：HttpClient 会话状态共享化 + Clone

当前瓶颈根因：`HttpClient` 的请求方法都是 `&self`（只读），但会话状态 `session_info`/`connected` 是内联字段，导致整个 `SessionManager` 必须放进 `Mutex` 才能可变，进而所有请求被迫持锁。

改造：

1. 将 `HttpClient` 的 `session_info: Option<SessionInfo>` 与 `connected: bool` 抽到 `Arc<RwLock<SessionState>>`；`HttpClient` 派生 `Clone`（克隆共享同一会话状态）。
2. `HttpClient::connect/disconnect` 从 `&mut self` 改为 `&self`（通过内部可变性更新会话状态）。
3. `SessionManager` 暴露 `client(&self) -> HttpClient`（返回 clone 句柄）。
4. gpui `SessionStore` 暴露无锁 `client()`；`services/*.rs` 的 72 处 `guard.client().xxx()` 改为 `client.xxx()`（不再持锁）。
5. 少数依赖 session 状态的方法（`execute_query`/`require_space`/`get_vertex` 等）通过 clone 句柄 + `current_space()` 读取，避免持锁。

改造后：请求路径完全无锁，仅 `connect/disconnect/switch_space` 等低频状态变更仍持 `Mutex`。

### 2.2 会话状态共享的数据结构

```rust
#[derive(Debug, Default)]
struct SessionState {
    info: Option<SessionInfo>,
    connected: bool,
}

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    base_url: String,
    config: ClientConfig,
    session: Arc<RwLock<SessionState>>,   // 请求路径只读，connect/disconnect 写
}
```

- `session_id()` 读 `session.read().info`；`authed()` 复用之。
- `connect()`：`login()` 后写 `session.write()` 设置 info + connected。
- `disconnect()`：写 `session.write()` 清空。
- 用 `std::sync::RwLock` 即可（临界区极短，不跨 await），避免 tokio RwLock 的调度开销；锁绝不跨越 `.await`。

### 2.3 真流式与取消

- `execute_query_stream` 改用 `response.bytes_stream()` + `SseParser::feed_line` 增量解析；通过 `futures::channel::mpsc` 把增量事件推给 UI，UI 侧用 `QueryTableData::extend_from_result` 承接增量渲染。
- 为流式请求使用独立的 `reqwest::Client`（长/无总超时 + 空闲超时），避免共用 30s 全局超时截断。
- `runtime::spawn_cancellable`：返回 `(Task, CancellationToken)`；generation 失效时取消 tokio 侧 future（reqwest future 被 drop 即取消 HTTP 请求）。

## 3. 分阶段方案与代码 diff

### 阶段 A：HttpClient 会话状态 Arc 化并实现 Clone

**文件**：`crates/linkrs-client/src/http_client.rs`

**A-1** 引入 `SessionState` 并改造结构体与构造：

```diff
+use std::sync::{Arc, RwLock};
+
+/// Mutable session fields shared across cloned request handles.
+#[derive(Debug, Default)]
+struct SessionState {
+    info: Option<SessionInfo>,
+    connected: bool,
+}
+
-#[derive(...)] // HttpClient 原本无 Clone
 pub struct HttpClient {
     inner: reqwest::Client,
     base_url: String,
     config: ClientConfig,
-    connected: bool,
-    session_info: Option<SessionInfo>,
+    session: Arc<RwLock<SessionState>>,
 }
```

> 注：`SessionInfo` 需派生 `Clone`（当前已 `Clone`，见 config.rs:4）。

**A-2** with_config 初始化：

```diff
         Ok(Self {
             inner,
             base_url,
             config,
-            connected: false,
-            session_info: None,
+            session: Arc::new(RwLock::new(SessionState::default())),
         })
```

**A-3** 只读访问点（line 137/147/667/737）：

```diff
     pub fn is_connected(&self) -> bool {
-        self.connected
+        self.session.read().unwrap_or_else(|e| e.into_inner()).connected
     }
     pub fn session_id(&self) -> Option<i64> {
-        self.session_info.as_ref().map(|s| s.session_id)
+        self.session.read().unwrap_or_else(|e| e.into_inner()).info.as_ref().map(|s| s.session_id)
     }
     // commit_transaction / rollback_transaction 中的 session_id 读取同理
-        let session_id = self.session_info.as_ref().map(|s| s.session_id)...
+        let session_id = self.session.read().unwrap_or_else(|e| e.into_inner()).info.as_ref().map(|s| s.session_id)...
```

**A-4** connect/disconnect 改 `&self`：

```diff
-    pub async fn connect(&mut self) -> Result<SessionInfo> {
+    pub async fn connect(&self) -> Result<SessionInfo> {
         ...
-        self.session_info = Some(session_info.clone());
-        self.connected = true;
+        {
+            let mut state = self.session.write().unwrap_or_else(|e| e.into_inner());
+            state.info = Some(session_info.clone());
+            state.connected = true;
+        }
         Ok(session_info)
     }
-    pub async fn disconnect(&mut self) -> Result<()> {
-        if let Some(ref session_info) = self.session_info {
+    pub async fn disconnect(&self) -> Result<()> {
+        let session_snapshot = self.session.read().unwrap_or_else(|e| e.into_inner()).info.clone();
+        if let Some(ref session_info) = session_snapshot {
             ... logout ...
         }
-        self.connected = false;
-        self.session_info = None;
+        let mut state = self.session.write().unwrap_or_else(|e| e.into_inner());
+        state.connected = false;
+        state.info = None;
         Ok(())
     }
```

**A-5** `clear_credentials` 改 `&self`：config 也在共享状态里才能改。将 `config` 一并纳入 `SessionState`：

```diff
 struct SessionState {
     info: Option<SessionInfo>,
     connected: bool,
+    config: ClientConfig,
 }
 pub struct HttpClient {
     inner: reqwest::Client,
     base_url: String,
-    config: ClientConfig,
     session: Arc<RwLock<SessionState>>,
 }
-    pub fn clear_credentials(&mut self) { self.config.password.clear(); }
+    pub fn clear_credentials(&self) {
+        self.session.write().unwrap_or_else(|e| e.into_inner()).config.password.clear();
+    }
```

> `config` 的读取仅在 connect（构造 login 用 username/password）与 clear_credentials。connect 内部改为从共享状态读凭证。

**验证**：`cargo check -p linkrs-client` + `cargo test -p linkrs-client`。

### 阶段 B：SessionManager 与 gpui 会话层适配

**文件**：`linkrs-client/src/session/manager.rs`、`gpui/src/connection/session_store.rs`、`gpui/src/services/connection_service.rs`

**B-1** SessionManager 适配（manager.rs）：

- `connect`（:139-159）：`self.config` 写入改为更新 client 共享 config；`self.client = HttpClient::with_config(...)` 重建逻辑调整为更新共享状态后调 `client.connect()`。由于 connect 需要换 host/port，采用「重建 client + 保留 Arc 引用」策略：`HttpClient::with_config` 仍重建，但 SessionManager 把新句柄暴露出去。
- `disconnect`（:175-185）：`self.client.disconnect()` 现为 `&self`；移除对 `self.session_info` 的依赖（已在 HttpClient 内）。
- `client()`（:285）：签名保持 `-> &HttpClient` 兼容旧调用，另加 `client_clone() -> HttpClient` 供无锁路径使用。
- `session()`（:263）：保留（connection_service 用），但改为读取共享 state 的 info 派生；`Session` 的 `variable_store` 单独保留在 SessionManager 内联字段。

**B-2** SessionStore 暴露无锁句柄（session_store.rs）：

```diff
     pub fn manager(cx: &App) -> Arc<tokio::sync::Mutex<SessionManager>> {
         cx.global::<SessionStore>().manager.clone()
     }
+
+    /// Clone of the shared HTTP client for lock-free request paths.
+    pub fn client(cx: &App) -> HttpClient {
+        cx.global::<SessionStore>().manager
+            .blocking_lock()   // 仅在 UI 线程取 clone，临界区极短
+            .client_clone()
+    }
```

> 备选：`client()` 用 `try_lock()` + 已缓存的 client clone，避免 blocking_lock。实现时优先用「SessionStore 内缓存 Arc<HttpClient>」方案（见 B-3）。

**B-3**（推荐替代 B-2）SessionStore 直接持有 client clone：

```diff
 pub struct SessionStore {
     manager: Arc<tokio::sync::Mutex<SessionManager>>,
+    client: HttpClient,
     _heartbeat: Option<Task<()>>,
 }
```

- `new()` 时从 manager 取 client clone 存入；connect 换 host 后需刷新该 clone——因 HttpClient 共享 session 状态，且 base_url 在 connect_with_host 时改变，需在 `connect` service 里同步更新 `SessionStore.client`。

**验证**：`cargo check` 两个 crate。

### 阶段 C：service 层去锁（132 处）

**文件**：`crates/linkrs-gpui/src/services/*.rs`

统一模式改造（以 monitoring_service.rs 为例）：

```diff
 pub fn overview(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
-    let manager = SessionStore::manager(cx);
+    let client = SessionStore::client(cx);
     runtime::spawn(cx, async move {
-        let guard = manager.lock().await;
-        guard.client().get_overview().await
+        client.get_overview().await
     })
 }
```

- 72 处 `guard.client().xxx()` 直接改 `client.xxx()`。
- 依赖 session 状态的调用：`guard.execute_query(&sql)` → 需变量替换，改为 `SessionStore::manager(cx)` 取一次快照（变量替换是纯 CPU，持锁瞬间完成）后释放锁，再用 client 发请求；或把 variable_store 也纳入共享状态后直接 `client` 调用。实现时采用「快照后释放锁」：
  ```rust
  let (client, sql) = {
      let guard = manager.lock().await;
      (guard.client_clone(), guard.substitute_variables(&sql)?)
  };
  client.execute_query(&sql, session_id).await
  ```
- `guard.require_space()` → `client.current_space()`（新增只读方法）。

**验证**：`cargo check --manifest-path crates/linkrs-gpui/Cargo.toml`，确认 0 处残留 `guard.client()`。

### 阶段 D：SSE 真流式 + 取消 + 独立超时

**文件**：`linkrs-client/src/http_client.rs`、`sse.rs`、`gpui/src/runtime.rs`、`services/query_service.rs`、`views/console.rs`

**D-1** 流式客户端分离：

```diff
+// A dedicated client with no total timeout for long-lived SSE responses.
+fn stream_client(&self) -> reqwest::Client {
+    reqwest::Client::builder()
+        .timeout(Duration::ZERO)          // disable total timeout
+        .build()
+        .unwrap_or_else(|_| self.inner.clone())
+}
```

- `execute_query_stream` 用 `bytes_stream()` 增量读取，逐 `data:`/`event:` 行喂 `SseParser::feed_line`，通过 mpsc channel 推送增量 `StreamUpdate`。
- 保留现有 `execute_query_stream` 作为「收集完整结果」的兼容 API，新增 `stream_query_updates` 返回增量流，供 UI 渐进渲染。

**D-2** 取消机制（runtime.rs）：

```diff
+pub fn spawn_cancellable<R, F>(cx: &App, future: F)
+    -> (Task<Result<R, ClientError>>, tokio_util::sync::CancellationToken)
+{
+    let token = tokio_util::sync::CancellationToken::new();
+    let token_for_task = token.clone();
+    // tokio 侧 select! { _ = token.cancelled() => Err(...), result = future => result }
+    ...
+}
```

- `CancellationToken` 由调用方（view 的 generation guard）持有；generation 递增时 `cancel()`。
- 需为 gpui 增加 `tokio-util` 依赖（或用 oneshot 自实现取消信号，避免新依赖）——优先用 oneshot 自实现（workspace 已有 futures）。

**D-3** UI 增量渲染（console.rs）：`QueryTableData::extend_from_result` 已支持 append，把增量结果逐批 append 并 `cx.notify()`。

**验证**：两个 crate `cargo check` + linkrs-client 单测（sse.rs 测试保持通过）。

### 阶段 E：元数据 single-flight 与心跳策略

**文件**：`gpui/src/services/schema_service.rs`、`connection_service.rs`

**E-1** single-flight：在 gpui 侧引入一个轻量 `MetaFlight` 全局（key → 共享 `Task` 结果），对 spaces/tags/schema 的读请求去重；latest-wins 由各 view 已有的 generation guard 保证，无需重复实现。变更操作后显式 invalidate 对应 key。

**E-2** 心跳策略（G6）：保持「失败置红 + 不自动重连」现状，理由：自动重连涉及凭据重放与用户意图，属产品决策。在方案开放点记录，不做代码改动。

**验证**：gpui `cargo check`。

## 4. 落地状态跟踪

| 阶段 | 问题 | 状态 | 落地文件 | 验证 |
| --- | --- | --- | --- | --- |
| A | G1（基础） | 已完成 | `linkrs-client/http_client.rs`：`SessionState` 抽为 `Arc<RwLock>`、`HttpClient: Clone`、`reconfigure()` 原地换配置 | cargo check + 14 单测通过 |
| B | G1（会话层） | 已完成 | `session/manager.rs`：`client_clone()` 暴露、connect 改用 reconfigure（共享不中断） | 两 crate cargo check 通过 |
| C | G1（请求路径） | 已完成 | `gpui/services/*.rs`：112 处 `guard.client()` → `manager.lock().await.client_clone()`（瞬间持锁，HTTP 无锁） | 0 处业务路径残留 guard.client() |
| D | G2,G3,G4 | 已完成 | `http_client.rs`：两 SSE 端点改 `bytes_stream()` 增量读取 + 无总超时 `stream_inner`；`runtime.rs`：`CancellationToken`/`spawn_cancellable`；`query_service.rs`+`console.rs`：流式查询取消接入 | 两 crate 编译 + 单测通过 |
| E | G5,G6 | 部分完成 | `connection_service.rs`：心跳改探测 Unhealthy 态（修复永久卡红 bug）+ 3 次连续失败阈值；G5 不改（见开放点） | gpui cargo check 通过 |

### 4.1 实施说明与偏差

- **阶段 C 的实现与方案 diff 的差异**：方案写的是 `SessionStore::client(cx)` 无锁句柄；实际采用 `manager.lock().await.client_clone()`。锁只在 clone 的瞬间持有（纳秒级、不跨 await），HTTP 请求在无锁 clone 上进行，同样消除串行化；且这保证 connect_with_host 后总能取到最新 base_url 的 client（避免缓存 clone 的配置过期问题）。
- **阶段 D 的 UI 增量渲染未做**：网络层已改为真流式增量读取（消除全量缓冲与 30s 截断），但 UI 侧仍是「等完整 QueryResult 再一次渲染」。`execute_query_stream` 签名保持不变，UI 无回归；增量渲染列为开放点。
- **阶段 E 的 G5 未实施**：`ClientError` 含 `reqwest::Error`（不 `Clone`），无法用 `futures::Shared` 做结果级 single-flight；若转成 `String` 会破坏 `is_session_expired` 的 401 匹配。各 view 已有 generation guard，重复请求是浪费而非正确性问题，故不引入高风险的类型改造。
- **阶段 E 的 G6 发现并修复一个真实 bug**：原心跳循环仅在 `status == Connected` 时探测，一旦进入 `Unhealthy` 便持续 `continue`，永久无法自动恢复。现改为 Connected/Unhealthy 均探测，并加连续失败阈值。

## 5. 开放点（待拍板）

1. **G6 心跳自动重连/重登**：本轮修复了「Unhealthy 后无法恢复」的 bug 并加失败阈值；是否进一步自动重新登录（涉及凭据重放与用户意图）仍需产品决策。
2. **G7 gRPC transport 去留**：与 HTTP 双份维护，是否收敛为单一 transport。本轮不改。
3. **流式 UI 增量渲染**：网络层已真流式；是否改 UI 为逐批 append 渲染（批次粒度、notify 频率需实机调参）。
4. **G5 元数据 single-flight**：需先让 `ClientError` 可 Clone 或设计不依赖结果缓存的 in-flight 去重（如 oneshot 广播 + Arc 错误包装），收益为低，暂缓。
5. **阻塞诊断**：`manager.lock().await` 的瞬间争用仍存在（取 clone 时），压测确认是否需要在 SessionStore 层缓存句柄。

## 6. 验证清单

- [x] 阶段 A–C：`cargo check` 两 crate 通过；业务路径 0 处 `guard.client()`（仅 connect 流程 1 处合理保留）
- [x] 阶段 D：`cargo test -p linkrs-client` 14 passed；sse 测试保持绿；两 crate 编译通过
- [x] 阶段 E：gpui `cargo check` 通过，无警告
- [x] 全程：`cargo fmt --check`（两 workspace）通过
- [x] 会话注入在启用鉴权下仍正确（`authed()` 保持不变，不回归 F1/F2 正面状态）
- [ ] 实机验证：监控页 11 路请求的并发度提升、长流式查询不再 30s 截断、stop 按钮取消 HTTP、心跳恢复——需在开发机/目标平台验证
