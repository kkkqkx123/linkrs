# UDF 远程下载设计方案（未来参考，不在本期实现）

本文分析 `INSTALL EXTENSION ... FROM 'https://...'` 远程下载应如何实现，
并给出可落地的设计方案，供未来参考。
现状依据：`docs/plan/udf_extension_refined.md`（细化方案）、
`docs/function/udf_dynamic_loading_design.md`（详设），
以及当前代码 `UdfLoader::install_from_source` 预留入口
（远程源一律返回 `UdfError::RepoDownloadUnsupported`）。

## 1. 现状与约束

### 1.1 已有链路

```text
SQL: INSTALL EXTENSION <name> FROM '<source>'
  -> parser/parsing/stmt_parser.rs: parse_extension_statement
  -> ExtensionStmt { action: Install, name, source }
  -> session.rs: parse_command 关键字门识别
  -> session.rs: execute_extension -> install_extension(source)
  -> registry.rs: install_dynamic_udf(source)
  -> loader.rs: UdfLoader::install_from_source(source)
       本地路径 -> UdfLoader::load -> 校验 -> libloading 加载 -> 注册
       http(s)  -> RepoDownloadUnsupported（显式错误，保持行为）
```

`ExtensionStmt` 目前只有 `name + source`，没有校验和、版本、
签名等选项字段。`LoadedExtension` 只记录本地路径、mtime、插件句柄，
没有来源 URL、哈希、版本等溯源信息。

### 1.2 硬约束

1. **依赖 DAG 不可破坏**：`graphdb-query` 位于底层，
   `query -> api -> server`。`reqwest` 目前只在
   `graphdb-server / vector-client / graphdb-cli / graphdb-embedding` 中使用，
   `graphdb-query` 没有网络依赖。把重型 HTTP 客户端直接塞进
   `graphdb-query` 会污染核心查询路径的编译体积与攻击面，应避免。
2. **Session 执行是同步的**：`execute_extension` 返回
   `CoreResult<QueryResult>`，跑在调用线程上。下载是阻塞 IO，
   必须有超时与大小上限，不能无界阻塞。
3. **当前实现持有写锁做加载**：`load_extension / install_extension`
   在持有 `function_registry.write()` 的临界区内完成加载。
   本地文件很快所以可接受；远程下载若沿用此模式，
   会在网络等待期间阻塞所有函数查询，必须拆分为
   “锁外抓取 + 锁内提交”两阶段。
4. **细化方案明确不启动后台线程**：自动热重载线程、沙箱进程隔离
   均为远期选项。远程下载设计同样不应引入常驻后台线程；
   用调用线程同步执行 + 超时控制即可。
5. **平台与 ABI 强约束**：扩展名按平台校验（so / dylib / dll），
   插件可选导出 `udf_abi_version` 并与 `UDF_ABI_VERSION` 比对。
   远程产物必须携带目标平台三元组与 ABI 版本信息，否则下载下来也无法加载。

### 1.3 可复用的已有能力

- 工作区已有 `reqwest 0.12`、`url`、`sha2`、`tempfile`（根依赖与 dev 依赖）、
  `tokio`（含 `time`，可做超时）、`log`。
  `graphdb-migration/src/plan.rs` 已有 `sha2::Sha256` 使用先例。
- `UdfError` 为 `thiserror` 枚举，扩展变体成本低；
  `to_expression_error()` 与 `CoreError::InvalidParameter` 映射链路已通。

## 2. 目标与非目标

### 2.1 目标

- `INSTALL EXTENSION <name> FROM 'https://...'` 端到端可用：
  解析策略检查 → 下载 → 大小/哈希/签名校验 → 落盘到扩展目录 →
  复用现有 `UdfLoader::load` 加载注册。
- 默认安全：域名白名单、HTTPS 优先、大小与超时上限、
  哈希校验、原子落盘、审计日志。
- 失败语义明确：网络、校验、策略、清单错误各自有独立错误变体，
  不与现有 `RepoDownloadUnsupported` 混淆（该变体保留给“未配置远程能力”场景）。

### 2.2 非目标

- 不做通用包管理器（依赖 transitive 解析、版本 SAT 求解）。
- 不做自动更新/后台轮询热重载（仍手动 `reload`，沿用 mtime 对比）。
- 不做进程级沙箱（仍以 `catch_unwind` 为执行期边界）。
- 首期不强制代码签名验证，可做可选能力（见 8.4）。

## 3. 总体架构

```text
Session::install_extension(source, options)
  │
  ├─ 1. Source 分类（loader 层，graphdb-query，无网络依赖）
  │     本地路径 -> 直接 UdfLoader::load
  │     http(s)  -> 走 Repository 链路
  │
  ├─ 2. Repository 链路（锁外执行，不持有 registry 写锁）
  │     ExtensionPolicy::check(url)          # 开关 / 白名单 / scheme
  │     Fetcher::fetch(url) -> TempFile      # HTTP 下载，超时 + 限大小 + 跟随有限重定向
  │     Verifier::verify(bytes, expectations) # sha256 必选/可选，签名可选
  │     Installer::stage(bytes) -> 最终路径   # 扩展目录 + 原子 rename + fsync
  │
  └─ 3. 提交（锁内执行，复用现有路径）
        UdfLoader::load(最终路径) -> register_loaded_plugin
        LoadedExtension 追加溯源字段（origin_url / sha256 / version / installed_at）
```

### 3.1 依赖划分（关键决策）

| 层次 | 放置内容 | 依赖 |
|------|----------|------|
| `graphdb-query`（udf 模块） | `Source` 分类、`ExtensionPolicy`（纯逻辑）、`Verifier`（sha256）、`Manifest` 解析、`UdfError` 新变体、溯源字段 | 新增 `url` + `sha2`（轻量，无网络）。**不引入 `reqwest`** |
| `graphdb-query`（udf 模块） | `RepositoryFetcher` trait（`fetch(url) -> Vec<u8>/TempFile`） | 仅 trait，无实现 |
| `graphdb-api` | `HttpRepositoryFetcher`（reqwest 实现）、`install_extension` 编排（锁外抓取 + 锁内提交） | `reqwest`（工作区已有） |
| `graphdb-config` | `[extension]` 配置节（见第 6 节） | 既有 config 机制 |
| `graphdb-server`（可选后置） | 异步卸载、审计日志落盘、metrics 暴露 | 既有 server 能力 |

这样 `graphdb-query` 保持可独立测试（用内存假 Fetcher），
真正的网络实现只活在 `graphdb-api` 及以上层次，DAG 与编译隔离都成立。

被否决的备选：直接在 `graphdb-query` 加 `reqwest` 依赖。
否决理由：查询核心路径引入重型异步 HTTP 栈，编译时间、特性蔓延、
安全审计面都不划算；且嵌入式用户可能根本不需要远程能力。

## 4. SQL 语法演进

当前语法保持可用：

```sql
INSTALL EXTENSION my_udf FROM '/local/path/to/lib.so';
INSTALL EXTENSION my_udf FROM 'https://example.com/udf/libmy_udf.so';
```

未来扩展（向后兼容的可选子句，首期可只实现 `CHECKSUM`）：

```sql
INSTALL EXTENSION my_udf FROM 'https://example.com/udf/libmy_udf.so'
WITH (CHECKSUM 'sha256:<hex>', VERSION '1.2.0', FORCE);
```

- `CHECKSUM`：期望哈希；若配置要求必选而语句未给，则拒绝并提示。
- `VERSION`：期望版本；与清单/插件元数据交叉核对，不一致拒绝。
- `FORCE`：覆盖同名已加载扩展（默认行为保持“已加载则拒绝”，见现行
  `AlreadyLoaded` 语义；`FORCE` 先卸载再安装）。
- AST 变更：`ExtensionStmt` 新增 `options: ExtensionInstallOptions`
 （全字段可选，默认为空，现有两字段构造不受影响）；
  parser 在 `FROM '<source>'` 后尝试解析可选 `WITH (...)`。

另支持清单模式（见第 5 节）：`source` 指向清单 URL 时，
按清单中的目标平台条目选择产物。清单与直链用内容类型/扩展名区分，
不在 SQL 层新增关键字。

## 5. 产物与清单格式

### 5.1 直链模式

URL 直接指向动态库文件。调用方通过 `WITH (CHECKSUM ...)` 或配置
提供期望哈希。文件名必须符合平台扩展名约束（沿用现有
`expected_extension` 逻辑，下载后同样校验）。

### 5.2 清单模式（推荐用于多平台）

清单为 JSON 文件，例如 `https://example.com/udf/my_udf.json`：

```json
{
  "name": "my_udf",
  "version": "1.2.0",
  "abi_version": 1,
  "artifacts": [
    {
      "target": "x86_64-unknown-linux-gnu",
      "url": "https://example.com/udf/my_udf-1.2.0-x86_64-linux.so",
      "sha256": "<hex>",
      "signature": "<base64, 可选>"
    }
  ]
}
```

- 客户端按运行平台三元组 + `UDF_ABI_VERSION` 选择条目；
  无匹配条目则拒绝（明确错误，含可用 target 列表）。
- 清单本身建议同样校验：已知清单哈希（pin）或签名；
  否则攻击者替换清单即可指向恶意产物。
- 首期可只实现直链模式，清单解析器先行落地（纯逻辑、无网络），
  作为第二步接入。

## 6. 配置设计

在 `graphdb-config` 新增 `[extension]` 节（全部有安全默认值）：

```toml
[extension]
# 总开关：关闭则 http(s) 源一律返回 RepoDownloadUnsupported（即现状行为）
allow_remote = false
# 扩展落盘目录；缺省为 <storage_path>/extensions
extension_dir = "data/graphdb/extensions"
# 域名白名单；为空表示拒绝所有远程源
allowed_hosts = ["example.com"]
# 是否允许 http 明文（默认 false，只允许 https；内网测试可显式打开）
allow_http = false
# 下载超时（秒）与产物大小上限（字节）
fetch_timeout_secs = 30
max_download_bytes = 64 * 1024 * 1024
# 哈希策略：require 要求语句或清单必须提供期望 sha256
require_checksum = true
# 签名策略：off / optional / require（首期实现 off + optional，require 留待签名方案确定）
signature_policy = "off"
```

行为矩阵：

| 配置 | 行为 |
|------|------|
| `allow_remote = false` | 任何 http(s) 源返回 `RepoDownloadUnsupported`（现状） |
| 白名单为空或命中失败 | 返回策略拒绝错误，不发起网络请求 |
| `allow_http = false` + `http://` 源 | 拒绝，提示使用 https |
| 超时/超大 | 中止传输并返回对应错误，已写入的临时文件删除 |

## 7. 详细流程

### 7.1 阶段一：分类与策略（锁外，`graphdb-query`）

1. `install_from_source` 按 scheme 分类：
   - 本地路径 → 现有 `UdfLoader::load`。
   - `http/https` → 检查远程总开关；关闭则 `RepoDownloadUnsupported`。
   - 其他 scheme（`ftp:` 等）→ 策略拒绝错误。
2. URL 规范化与校验（`url` crate）：
   - 拒绝非 `http/https`、拒绝 embedded 凭据（`user:pass@host`）进入日志；
     日志中对 URL 脱敏（隐藏 query 中的 token 类参数，至少隐藏 password）。
   - 主机名大小写归一化后匹配 `allowed_hosts`（精确匹配；是否支持通配符
     由配置决定，默认不支持，避免 `*.example.com` 误放行）。
3. 语句名与产物名一致性：`INSTALL EXTENSION <name>` 的 `<name>`
   必须与加载后 `plugin.name()` 按 `to_uppercase()` 相等（沿用现有归一化），
   不一致则卸载已落盘文件并返回明确错误（防止“挂羊头卖狗肉”）。

### 7.2 阶段二：下载（锁外，`graphdb-api` 的 Fetcher 实现）

1. `reqwest::Client` 单例（复用连接池），配置：
   - `timeout(fetch_timeout_secs)`；
   - 重定向上限（如 5 次）且**跨 scheme 重定向 https→http 默认拒绝**；
   - 不自动跟随到白名单外的主机（每次重定向重新执行策略检查）。
2. 流式读取响应体，边读边计数，超过 `max_download_bytes` 立即中止；
   边读边喂 `Sha256`（避免二次遍历）。
3. 非 2xx 状态码、DNS/TLS/超时错误映射为独立错误变体（见第 9 节），
   错误信息包含状态码与主机（不含敏感 query）。
4. 扩展名校验前置：URL path 尾段扩展名不符合平台要求可提前拒绝，
   节省一次下载（清单模式下校验所选 artifact 的 URL）。

### 7.3 阶段三：校验（锁外）

1. 大小：已在下载时强制执行。
2. 哈希：期望值来自 `WITH (CHECKSUM)` 或清单条目；
   `require_checksum = true` 且无期望值 → 拒绝。
   比较时用常量时间比较（`subtle` 或等效实现，避免时序侧信道；
   若不愿引入新依赖，至少保证比较逻辑不提前泄露长度以外信息——
   但推荐直接用常量时间比较）。
3. 签名（可选能力）：`signature_policy != off` 且条目携带 signature 时验证；
    ...

[truncated 6551 chars]