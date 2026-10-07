# 权限与认证重构方案

## 背景

当前权限系统整体架构合理（RBAC 五层 + Space 级绑定 + God 全局角色），但在安全与纯本地场景适配方面存在若干缺陷。本方案分五个阶段修复问题并补齐缺口。

## 阶段 1：修复 `POST /v1/sessions` 绕过密码的安全漏洞

### 问题

`crates/graphdb-server/src/http/handlers/session.rs` 的 `create` handler 仅检查 `is_user_locked`，不校验密码：

```rust
let session = session_manager
    .create_session(request.username, request.client_ip)
    .await;
```

任何人只要猜一个存在的用户名就能拿到 session。

### 修改

1. `POST /v1/sessions` handler：当 `enable_authorize = true` 时必须校验密码，复用 `graph_service.authenticate()`。当 `single_user_mode = true` 时跳过密码直接创建（与 bootstrap 配置对齐）。
2. 路由上：把 `/sessions` 从 protected_routes 移到 public_routes 不合理——它本身变成了一个登录端点。改为让 protected_routes 里的 handler 都经过 auth_middleware，但 `/v1/sessions` 仍然放在 protected 区域，因为它需要 session 才能列出 sessions。保留 `POST /sessions` 的登录语义，但增加密码校验。

文件修改清单：
- `crates/graphdb-server/src/http/handlers/session.rs` — 增加密码校验
- `crates/graphdb-server/src/http/router.rs` — 无改动（路由本来就在 protected，保持）

## 阶段 2：修复 `/v1/auth/logout` 无鉴权问题

### 问题

`router.rs` 把 logout 放在 public_routes，但 logout handler 只从 body 读 session_id 直接 remove，没有验证调用者身份。任何人只要知道 session_id 就能登出别人。

### 修改

1. 把 logout 从 public_routes 移到 protected_routes，让 auth_middleware 先跑。
2. handler 里不再从 body 读 session_id，改为从 Extension 拿当前用户的 session_id。
3. 如需登出指定 session（admin 场景），走 `/v1/sessions/{id}` DELETE（已在 protected）。

文件修改清单：
- `crates/graphdb-server/src/http/router.rs` — logout 路由位置 + request body 类型
- `crates/graphdb-server/src/http/handlers/auth.rs` — logout handler 签名和逻辑
- `crates/graphdb-wire/src/meta.rs` — LogoutRequest 结构体（可能不再需要 body）

## 阶段 3：session_id 改为高熵随机值

### 问题

当前 session_id 由 `timestamp(48) + counter(16)` 拼接，可预测。公网部署下容易被 session hijacking。

### 修改

使用 `rand::RngCore` 生成 64 位随机数。因为 SessionManager 默认启用了 `rand`（或加 `rand = "0.8"` 到 Cargo.toml）。

文件修改清单：
- `crates/graphdb-server/src/session/session_manager.rs` — `generate_session_id` 方法

## 阶段 4：middleware 遵循 `enable_authorize` + `single_user_mode`

### 问题

- `AuthConfig.enable_authorize = false` 时，`auth_middleware` 和 `web_auth_middleware` 仍然强制校验 X-Session-ID
- `BootstrapConfig.single_user_mode = true` 是个空壳，没有任何代码消费

### 修改

1. `GraphService` 暴露 `auth_config()` 和 `bootstrap_config()` 读取，或直接暴露一个 `auth_disabled()` 方法
2. `auth_middleware`：如果 `auth_disabled()` 为 true，跳过 session 校验，把 `default_username` 的"匿名 session"注入 Extension；同时跳过 lock/password-change 检查
3. `web_auth_middleware`：同样逻辑
4. `GraphSessionManager::create_session` 在 auth_disabled 时允许任何人创建任何 username 的 session（session manager 本身不做密码校验，密码校验由 handler / authenticate 完成）

文件修改清单：
- `crates/graphdb-server/src/graph_service.rs` 或 `graph_service/session.rs` — 暴露配置
- `crates/graphdb-server/src/http/middleware/auth.rs` — 增加 enable_authorize 跳过逻辑
- `crates/graphdb-server/src/web/middleware.rs` — 同上
- `crates/graphdb-server/src/http/handlers/session.rs` — create_session 跳过密码
- `crates/graphdb-server/src/http/handlers/auth.rs` — login 跳过密码

## 阶段 5：gRPC 统一 auth 拦截器

### 问题

当前每个 gRPC handler 都要手动 `find_session`，容易遗漏。

### 修改

1. 在 `graphdb-server/src/grpc/` 下新增 `interceptor.rs`，实现 `tower::Service` 或 tower-grpc 的 intercept 逻辑：从 gRPC metadata 读 `x-session-id`（或 proto request 里的 session_id 字段），校验后注入 request extensions
2. `run_server` / `grpc::service` 把 interceptor 挂上去
3. 保留 handler 内部 find_session 的 fallback（向后兼容 proto request 里带 session_id 的调用方）

注意：tower-grpc 的 intercept 对 unary 和 streaming 有不同 API，本阶段先实现 unary 版本并覆盖主要 handler，streaming 做兜底。

### （降优先级：为了控制改动范围，可简化为"把 session 校验抽成公共函数，每个 handler 开头调用一次"，避免 tower 泛型复杂度）

## 不做的事情（本次范围内）

- PasswordPolicyConfig 的实际执行（复杂度高，建议单独迭代）
- session_id HMAC 签名（高熵随机 + HTTPS 已够用；签名需要额外 secret 管理）
- CORS 收紧（本地友好的开发默认，生产场景文档提示即可）

## 编译测试

所有 cargo 命令都加 `-j 2`：

```shell
cargo build -j 2
cargo test -j 2 --lib
```

## Patch 生成

```shell
git diff > auth-refactor.patch
# 额外 exclude 构建产物
git diff --stat
```

验证 patch：`git stash && git apply auth-refactor.patch && cargo build -j 2 && git stash pop`
