# gRPC 客户端补全方案

> 配套现状分析见 `docs/api/feature_matrix.md` 的 gRPC 客户端说明。本文档是本次
> 改动的唯一任务编号来源，代码注释中不得引用这些编号。

## 背景

`crates/linkrs-client` 的 `GrpcClient` 是 tonic 生成类型的薄封装，目前只覆盖
119 个 RPC 中的 64 个，且存在以下实质问题：

- 服务端 gRPC 没有认证中间件，受保护 RPC 从请求体读取 `session_id`，而客户端
  只注入 `x-session-id` metadata，登录态与后续调用没有打通；
- 自述覆盖的统计、迁移、Schema 版本、批量等分组仍有 RPC 缺失；
- 非 OK 状态统一压平成字符串错误，丢失 `tonic::Code`；
- 端点固定为 `http://`，没有消息大小上限，与默认 10 MB 的服务端契约不匹配；
- 没有针对真实服务端或逐 RPC 的测试，仓库内也没有使用方。

## 目标

把 `GrpcClient` 从“核心链路薄封装”补全为“与服务端 proto 契约一致的完整客户端”，
并修复会话传递、错误语义和传输配置的关键缺陷。本次不引入新的服务端行为，不改
proto 契约。

## 范围

本次包含：

1. 补齐缺失的 55 个 RPC 封装，达到 119/119；
2. 会话自动注入、`create_session` 结果保存、显式 `set_session`；
3. 结构化 gRPC 错误（状态码 + 消息），区分建连失败与调用失败；
4. 传输配置增强：完整 endpoint URI、客户端消息大小上限；
5. 单元测试与编译/测试验证。

本次不包含（记录为后续工作）：

- 服务端增加 metadata 认证拦截器（属于服务端契约演进）；
- CLI / GPUI 切换到 gRPC 远程通道（涉及各自传输抽象重构）；
- tonic TLS（rustls）依赖接入；服务端当前明确要求网关终止 TLS，客户端保留
  `https` scheme 透传能力，TLS 配置待后续单独立项。

## 任务

### G1 子仓库同步与基线验证

- `git submodule update --init --recursive` 初始化 `tantivy`、`simvec`、
  `llm-suite`、`gpui-kit` 及其嵌套子仓库。
- 验收：`git submodule status` 不再出现 `-` 前缀；`cargo check -p linkrs-client`
  可进入依赖解析阶段。

### G2 RPC 全集封装

在 `grpc_client.rs` 补齐缺失分组：

- 用户管理 8 个（List/Create/Drop/ResetPassword/Enable/Disable/GrantRole/RevokeRole）；
- 自定义函数 4 个（Register/Unregister/List/GetInfo）；
- 向量 14 个（索引 CRUD/Search、Scroll/Count/Rebuild/Status/Clear、Point/Payload）；
- 全文 5 个（Rebuild/Status/Clear/Inconsistent/Search）；
- 同步 9 个（Status/Retry/Diagnostics/DeadLetters/Requeue/Retention/Degraded）；
- 统计扩展 6 个（Overview/Search/Migration/Freeze/Trigger/Profile）；
- 迁移扩展 3 个（DryRun/History/Status）；
- Schema 版本 2 个（Changes/BreakingChanges）；
- 查询扩展 1 个（ExecuteBatchQuery）；
- 批量删除 1 个（DeleteBatch）；
- 事务扩展 2 个（ListSavepoints/RetryTransactionOutbox）。

统一约定：无字段请求使用无参方法，其余方法直接接收生成的请求类型并返回生成
的响应类型；流式 RPC 继续返回 `tonic::Streaming`。

- 验收：客户端 `pub async fn` 与 proto `rpc` 一一对应（119 个，不含 `connect`）。

### G3 会话传递

- 新增内部辅助：可选 `session_id` 缺省时填入已存会话；必填字段为空时填入，
  无会话则返回 `AuthError`；可选 `owner` 同理。
- 对带 `session_id` 的请求方法统一调用辅助，调用方显式传值保持不变。
- `create_session` 成功后将返回的会话 id 写入客户端状态。
- 新增 `set_session_id`，便于外部恢复会话；`login_in_space` 支持登录时指定空间。
- 澄清 `authed` 的 metadata 只用于兼容未来服务端拦截器，当前服务端不读取。

- 验收：`login` 后直接调用 `execute_query`（不手动填 `session_id`）可被服务端
  识别；无会话时必填方法返回 `AuthError` 而非发到服务端。

### G4 结构化错误

- 增加 `GrpcErrorCode` 枚举（Unauthenticated / PermissionDenied / NotFound /
  InvalidArgument / AlreadyExists / Unavailable / DeadlineExceeded /
  ResourceExhausted / Unimplemented / Internal / Cancelled / Unknown / Other）。
- `ClientError::GrpcError` 改为携带 `code` 与 `message`；`status_error` 映射
  `tonic::Code`。
- 建连失败保持 `ConnectionError`，调用失败映射为结构化 `GrpcError`，并修正模块
  文档中“调用期传输失败也是 ConnectionError”的描述。

- 验收：测试可断言错误码；`ClientError` 显示包含错误码与消息。

### G5 传输配置

- 新增 `from_endpoint` / `connect_endpoint`，接受完整 URI（`http://` 或
  `https://`），保留 `new` / `connect` 的 host+port 形式。
- 新增 `with_message_size_limit`，在每次调用时同时设置解码/编码上限，默认沿用
  tonic 默认值。
- 文档说明 `ClientConfig` 的默认端口面向 HTTP，gRPC 默认端口为 9669。

- 验收：完整 URI 构造可用；设置上限后生成的 client 携带该配置。

### G6 测试与验证

- 补充单元测试：会话填充（可选/必填/owner/int64）、`create_session` 保存、
  错误码映射、endpoint 构造、消息大小配置。
- 验证命令：`cargo check -p linkrs-client --features grpc`、
  `cargo test -p linkrs-client --features grpc`。

- 验收：上述命令通过，格式符合 `cargo fmt`。

### G7 生成 patch

- 基于 `git diff` 生成 `grpc-client-completion.patch`，供本地 `git apply`。
- 验收：patch 可用 `git apply --check` 校验。

## 执行顺序

G1 → G2 → G3 → G4 → G5 → G6 → G7。
