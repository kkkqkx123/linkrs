# llm-suite 子仓库升级与 provider 新字段改造分析

## 背景

llm-suite 子仓库从 `fcd8551` 更新至 `97e68df`（origin/main，共 12 个提交，101 个文件变更，
+8542/-1564）。核心变更是 provider/profile 定义新增 4 个可选字段：

```rust
// llm-suite-types/src/llm/provider_definition.rs 与 profile.rs 各自新增
pub compat: Option<super::ProviderCompat>,      // 兼容性意见（developer role、strict mode、max_tokens 字段名等）
pub identity: Option<super::LlmIdentity>,       // 客户端身份头（User-Agent / HTTP-Referer / X-Title）
pub transport: Option<super::LlmHttpTransport>, // 连接级 HTTP 调优（connect timeout、跳过 TLS 校验）
pub env: Option<HashMap<String, String>>,       // auth 解析期的环境变量覆盖层
```

同时新增/变更的关联面：

| 变更 | 位置 | 说明 |
| --- | --- | --- |
| 新增 `ProviderCompat` | `llm-suite-types/src/llm/provider_compat.rs` | 含 `detect()` 本地网关启发式（localhost/vLLM/Ollama 默认 `max_tokens` 旧字段） |
| 新增 `LlmHttpTransport` | `llm-suite-types/src/llm/transport.rs` | connect_timeout_secs、danger_accept_invalid_certs |
| `apply_provider_defaults` 扩展 | `llm-suite-config/src/provider.rs` | 定义→profile 逐字段合并（profile 显式值优先），无 provider_id 的独立 profile 也走 compat 自动检测 |
| gateway client cache key 纳入 transport | `llm-suite-gateway/src/gateway/client_cache.rs` | transport 变化触发 HTTP client 重建 |
| 新增 `build_http_client_with_options` | `llm-suite-proxy/src/lib.rs` | 兼容旧 `build_http_client` 签名 |
| `LlmRequest` 新增 `max_retries`/`retry_delay_ms`/`extra_headers` | `llm-suite-types/src/llm/request.rs` | 请求级覆盖，纯增量 |
| `TokenUsageStats` 新增 `provider_total` | `llm-suite-types/src/llm/usage.rs` | `Option` 字段，纯增量 |
| `SYMBOL_FACTOR` 1.0 → 0.5 | `llm-suite-token/src/estimation.rs` | **行为变更**：符号类字符估算减半 |
| `ModelCatalog` 迁移 | `llm-suite-config/catalog.rs` → `llm-suite-discovery` | 网络发现逻辑拆出；config 侧只剩纯函数 |
| 删除 `llm-suite-chat-basic` | 整 crate 移除 | 保留 codec 层 |
| 新增 crates | decision / discovery / images | linkrs 未引用 |

所有新字段均带 `#[serde(default, skip_serializing_if = "Option::is_none")]`，
结构体无 `deny_unknown_fields` —— **serde 层面向后兼容**：旧配置文件反序列化不报错，
缺省即 `None`（用户提到的 `compat: None, identity: None, transport: None, env: None`
正是 llm-suite 自身测试/调用点补齐结构体字面量时的形态）。

## 当前项目的 llm 集成面

通过全仓 grep 确认，linkrs 主工作区对 llm-suite 的**实际消费仅 3 个 crate**：

| linkrs crate | 依赖（rename） | 使用位置 |
| --- | --- | --- |
| linkrs-config | `llm-embedding`、`llm-rerank`（feature 门控） | `vector.rs` 直接内嵌 `EmbeddingConfig`/`RerankConfig` 作为 TOML 配置结构 |
| linkrs-sync | `llm-embedding`、`llm-rerank`、`llm-token` | `vector_sync.rs`：`EmbeddingProvider`/`RerankProvider` trait、`llm_token::estimate_tokens` 分批 |
| linkrs-server | `llm-embedding`、`llm-rerank` | `startup.rs` 组装 `CohereRerankProvider` |
| vector-client | `llm-embedding` | `EmbeddingService`/`OpenAICompatibleProvider` 封装 |

根 `Cargo.toml` 仅额外声明 `llm-suite-common`/`llm-suite-proxy`/`llm-suite-types` 三条
workspace 依赖，用于满足嵌套 path 依赖的 `workspace = true` 继承解析（见根 Cargo.toml
注释），**主工作区源码不直接 use 它们**。

**关键结论：linkrs 全仓（含 src/、crates/linkrs-*、tests/、frontend、gpui 客户端）
没有任何代码引用 `LlmProfile`、`LlmProviderDefinition`、`ProviderCompat`、`LlmIdentity`、
`LlmHttpTransport`、`ModelCatalog`、`llm-suite-gateway`、`llm-suite-config`、
`llm-suite-client`。** 新 4 字段落在 linkrs 未触达的类型上。

## check 检查结果

执行环境：rustc 1.92.0（另有 1.90/1.91/1.94 可选，无 1.97）。

```shell
# llm-suite 子工作区（自身 MSRV 1.88）
cargo check --workspace --all-targets        # ✅ 通过，0 error

# linkrs 主工作区消费方（--ignore-rust-version 为绕过下述 MSRV 门槛，与本次升级无关）
cargo check --ignore-rust-version -p linkrs-config --features embedding,rerank   # ✅
cargo check --ignore-rust-version -p linkrs-sync   --features embedding,rerank --all-targets  # ✅
cargo check --ignore-rust-version -p linkrs-server --features embedding,rerank   # ✅
cargo check --ignore-rust-version -p vector-client                              # ✅

# 单元测试
cargo test -p llm-suite-types -p llm-suite-config -p llm-suite-proxy -p llm-suite-token  # ✅ 71 passed
cargo test --ignore-rust-version -p linkrs-sync --features embedding,rerank vector_sync  # ✅ 17 passed
```

发现的两个问题（均**非**本次升级引入）：

1. **MSRV 门槛冲突（预存在）**
   根 `Cargo.toml [workspace.package] rust-version = "1.97"`，而 llm-suite 成员写
   `rust-version.workspace = true`。由于嵌套 path 依赖的 workspace 继承被解析到**主
   工作区根**（根 Cargo.toml 第 162–166 行注释已记录该机制），llm-suite 各 crate 在
   主工作区内被判定要求 rustc 1.97，本地 1.92 直接被 cargo 拒绝。
   **验证**：检出旧提交 `fcd8551` 后 `cargo metadata` 同样解析出 1.97 —— 冲突在升级前
   就存在，与新字段无关。
2. **`cargo check --workspace --all-targets` 全量时报 `linkrs-query/tests/lifecycle.rs`
   编译错误（预存在，与 llm 无关）**：`PhysicalPlan` 已重构为 `diagnostics` 字段，
   测试仍构造 `parallel_fallback_reason`/`cbo_notes`（E0560/E0609）。该测试不依赖
   llm-suite，主工作区 diff 仅有 `M crates/llm-suite`，可确认为遗留破损。

## 改造评估：哪些代码需要更新

### 必须更新：无

- `EmbeddingConfig`、`RerankConfig` 两个被 linkrs 直接内嵌的配置结构在本次升级中
  **零变更**（`git diff` 为空），`[vector.embedding]`/`[vector.rerank]` TOML 面不受影响。
- 新 4 字段所在类型（`LlmProfile`/`LlmProviderDefinition`）linkrs 不构造、不反序列化，
  不存在需要补 `compat: None` 之类的字面量。
- `llm-suite-chat-basic` 被删除、`ModelCatalog` 迁移至 discovery —— linkrs 均未引用，
  无迁改点。
- `TokenUsageStats.provider_total`、`LlmRequest` 三个新字段均为 `Option` 增量，
  linkrs 不构造这些结构。
- `Cargo.lock` 无变化（path 依赖版本号仍为 0.1.0，新 crates 未被引用）。

### 需要关注的行为变更（1 处）

**`llm-token` 估算系数 `SYMBOL_FACTOR` 1.0 → 0.5**：linkrs-sync 的
`split_for_token_budget`（`vector_sync.rs` 第 1020 行起）依赖 `estimate_tokens` 切分
embedding 批次、并以 `EMBEDDING_MAX_TOKENS_PER_CALL = 8192` 拒绝超长文本。系数下调后
同一批文本估算值变小 → 单批可能装入更多文本，且"保守超计保护"（防止 provider 400）
减弱。现有 17 个 vector_sync 测试全部通过，无需改码；但若目标 embedding 服务对
8192 预算敏感，建议在真实端点上做一次批量压测确认。这是唯一需要**验证**而非修改的点。

### 配置体系是否需要调整：不需要

- linkrs 自己的 `[vector.embedding]` / `[vector.rerank]` 配置面绑定的类型未变，
  `deny_unknown_fields` 结构无新增字段，用户现有 `config.toml` 完全有效。
- 新 4 字段属于 llm-suite 的 provider/profile 配置体系（供 gateway 消费），
  linkrs 未接入该体系，不存在需要暴露到 TOML 的字段。
- llm-suite 侧 `LlmProviderDefinition`/`LlmProfile` 自身不带 `deny_unknown_fields`，
  且新字段全部 `serde(default)`——即便未来 linkrs 接入 provider 定义，旧配置文件也
  天然兼容，无需迁移脚本。

### 可选/后续事项

1. **MSRV 对齐（建议单独立项）**：三选一 —— 主工作区 `rust-version` 降级到与实际
   工具链一致、升级 CI 工具链到 1.97、或为 llm-suite 路径依赖显式豁免。当前所有
   check 需带 `--ignore-rust-version`，应消除这一噪音。
2. **修复 `linkrs-query/tests/lifecycle.rs`**：按 `PhysicalPlan::diagnostics` 新结构
   改写构造，恢复全工作区 `--all-targets` 绿灯（与 llm 无关，另行处理）。
3. **能力预留**：若未来要在 linkrs 中接入 LLM chat/provider 定义，本次升级已备齐
   `compat`（本地网关 max_tokens 兼容）、`transport`（内网自签证书）、`env`、`identity`
   全套机制，届时配置体系需要新增对应 TOML 段 —— 属新功能，不在本次范围。

## 结论

本次 llm-suite 升级对 linkrs 是**纯增量、无破坏**的：被消费的 embedding/rerank 两个
crate 零变更，新增 4 字段落在未使用的类型上，serde 向后兼容，全部相关包 check/test
通过。**代码与配置体系均无需改造**；唯一需要留意的是 `SYMBOL_FACTOR` 系数变更对
embedding 批切分的行为影响，以及两个与本次升级无关的预存在问题（MSRV 门槛、
lifecycle 测试破损）。

升级本身只需提交子仓库指针变更（`git add crates/llm-suite`）。
