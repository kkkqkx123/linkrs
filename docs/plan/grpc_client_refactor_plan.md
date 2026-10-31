# gRPC 客户端第二阶段方案：模块拆分与 TLS

> 上一阶段的 RPC 补全、会话注入、结构化错误与传输配置已经落地（见
> `docs/plan/grpc_client_completion_plan.md`）。本文档描述剩余工作的设计与实施方式。

## 背景

`crates/linkrs-client/src/grpc_client.rs` 当前约 2000 行，单文件同时承担：

- 生成代码的挂载（`tonic::include_proto!`）；
- 传输层构造（endpoint 解析、超时、消息大小、TLS）；
- 会话状态与元数据注入；
- 119 个 RPC 方法；
- 状态码映射与单元测试。

职责叠加导致文件难以导航，新增一个分组的方法要跨越上千行查找插入点，评审时也无法
按领域聚焦。此外客户端仍然只能明文传输：`https` scheme 只是被 tonic 接受，没有真正
的 TLS 配置入口。

## 目标

1. 把 `grpc_client` 拆成“门面文件 + 同名子目录”的结构，按领域拆分 `impl` 块，
   对外导出路径不变；
2. 补齐 TLS 能力：可配置 CA 证书与校验域名，未提供 CA 时回落到系统根证书；
3. 在新基线上完成编译、测试与 RPC 覆盖核验，并重新生成 patch。

## 范围

本次包含：

1. 模块拆分（仅移动代码，不改变任何公开方法签名与行为）；
2. TLS 依赖与配置类型、构造入口；
3. 单元测试补充、编译/测试/clippy 验证；
4. patch 重新生成。

本次不包含（延后项及理由）：

- **服务端 metadata 认证拦截器**：服务端当前从请求体读取 `session_id`。增加拦截器
  会改变服务端契约，需要同时梳理所有 handler 的鉴权路径，属于独立的服务端改造。
  客户端已提前通过 `authed()` 注入 `x-session-id`，拦截器落地后无需再改客户端。
- **CLI / GPUI 切换到 gRPC 远程通道**：两者都通过 `SessionManager` 枚举封装
  `HttpClient` 与内嵌引擎，`client()` 返回 `&HttpClient`，被命令层大量使用。整体切换
  需要先抽象出传输无关的客户端接口，再改命令层，规模远超本次范围；gRPC 客户端继续
  作为可独立使用的库能力存在。
- **mTLS / 客户端证书**：当前服务端没有客户端证书校验需求，本次只做单向 TLS。

## 拆分设计

`grpc_client.rs` 保留为门面，只负责模块声明与导出：

```
src/grpc_client.rs          # 门面：模块声明、pub use
src/grpc_client/
├── proto.rs                # tonic::include_proto!("linkrs")
├── core.rs                 # 结构体、构造器、TLS、会话状态、endpoint 构造、核心测试
├── error.rs                # status_error / grpc_code 映射及其测试
├── auth.rs                 # 健康检查、登录登出、会话管理
├── users.rs                # 用户管理
├── functions.rs            # 自定义函数
├── query.rs                # 查询、游标、查询扩展
├── transaction.rs          # 事务、savepoint、事务扩展
├── schema.rs               # space/tag/edge type、Schema 版本
├── config.rs               # 配置读写
├── statistics.rs           # 统计与统计扩展
├── batch.rs                # 批量任务与批量扩展
├── migration.rs            # 迁移、Schema 版本历史、迁移扩展
├── transfer.rs             # 导入导出
├── vector.rs               # 向量索引、向量扩展、向量 payload
├── fulltext.rs             # 全文检索
└── sync.rs                 # 同步、死信、保留策略、降级区间
```

约束：

- 门面保持 `pub mod proto;`，`pub use core::{GrpcClient, GrpcTlsConfig}`，因此
  `linkrs_client::grpc_client::GrpcClient` 与 `linkrs_client::GrpcClient` 的引用路径不变；
- 需要跨文件复用的内部辅助（`client()`、`authed()`、会话填充等）在 `core.rs` 内声明
  为 `pub(super)`：父模块及其全部子模块可见，不扩大到 crate 之外；
- 各领域文件写 `impl GrpcClient { ... }`，只依赖 `super::proto::*`、`super::error`、
  会话辅助与 `crate::utils::error::Result`；
- 错误映射留在 `error.rs`，避免每个领域文件重复实现。

## TLS 设计

依赖：为 `tonic` 打开 `tls` 与 `tls-native-roots` feature，新增可选 `rustls` 依赖
（`ring` provider），仅随 `grpc` feature 启用。

```rust
#[derive(Debug, Clone, Default)]
pub struct GrpcTlsConfig {
    /// PEM 编码的 CA 证书；为 `None` 时使用系统根证书。
    pub ca_certificate_pem: Option<Vec<u8>>,
    /// 覆盖证书校验使用的域名；为 `None` 时取 endpoint 的 host。
    pub domain_name: Option<String>,
}
```

构造入口：

- `with_config_and_tls(config, tls)`：host + port 形式；
- `from_endpoint_with_tls(endpoint, timeout_seconds, tls)`：完整 URI，惰性建连；
- `connect_endpoint_with_tls(endpoint, timeout_seconds, tls)`：完整 URI，立即建连。

原有 `new` / `with_config` / `from_endpoint` / `connect` / `connect_endpoint` 保持签名，
内部以 `None` 委托，行为不变。

实现要点：

- `build_endpoint` 增加可选 TLS 参数，在 timeout/connect_timeout 之前应用
  `Endpoint::tls_config`；
- 未提供 CA 时调用 `ClientTlsConfig::with_native_roots()`，提供时使用
  `Certificate::from_pem`；
- 域名缺省时由 tonic 从 URI host 推导；
- `ClientTlsConfig` 构造错误与非法 endpoint 同样映射为 `ConfigError`；
- **crypto provider 冲突**：工作区中 `reqwest` 通过 rustls 启用了 `aws-lc-rs`，而
  tonic 的 `tokio-rustls` 启用 `ring`，两者同时启用时 rustls 无法自动选择默认
  provider，TLS 建连会 panic。因此建立 TLS 端点前，若进程尚未安装默认 provider，
  显式安装 `ring`。

TLS 不影响明文用法：未调用 `*_with_tls` 的路径不会触碰任何 TLS 代码。

## 验证

- `cargo fmt`；
- `cargo check -p linkrs-client --features grpc` 与不带 feature 的 `cargo check`；
- `cargo test -p linkrs-client --features grpc`（含新增 TLS 配置测试）；
- `cargo clippy -p linkrs-client --features grpc -- -D warnings`；
- RPC 覆盖核验：proto 中 `rpc` 数量与客户端 `pub async fn` 数量一致（119）。

## patch

基于当前 `origin/main` 基线，`git add -N` 新文件后 `git diff` 输出到
`grpc-client-refactor.patch`，并在临时 worktree 中 `git apply --check` 校验。
