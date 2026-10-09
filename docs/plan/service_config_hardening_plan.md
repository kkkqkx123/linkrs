# 服务配置分析与改进方案

对 `config.toml` 及 `linkrs-config` 配置体系的整体评估、改进方案，以及日志文件名与滚动（rotation）的专项分析。

## 一、现状评估

### 1.1 符合数据库项目一般做法的部分

- 分节设计 + 每节独立 `validate()`，错误信息具体，`Config::validate()` 统一汇总。
- 默认值深度合并（`merge_toml_values`），用户配置只写差量即可。
- 路径解析友好：相对路径按配置文件所在目录解析、支持 `~` 展开、`LINKRS_CONFIG_DIR` 环境变量覆盖配置目录。
- 安全默认：默认绑定 loopback（`127.0.0.1`）、bcrypt cost 可配、口令策略下限 8、首次登录强制改密、CORS 白名单格式校验。
- 各类超时齐备（事务、gRPC keepalive/连接/请求、会话空闲）。

### 1.2 需要改进的问题（按优先级）

**高 — 安全**

1. 启动日志明文泄露凭证：`linkrs-server/src/startup.rs:56` 用 `{:?}` 打印整个 `Config`，派生 Debug 会把 `auth.default_password`、qdrant/embedding 的 `api_key` 明文写入日志文件。配置对象应自定义 Debug 脱敏，或删除该日志。
2. 默认口令 `root/root` 明文写死在配置文件中：主流数据库（PostgreSQL/MySQL）不在配置里放默认口令，而是首次启动生成随机口令写入仅属主可读的文件，或由环境变量注入。`force_change_default_password` 只是缓解，误关即裸奔；配置文件本身也无 0600 权限检查。
3. gRPC 无 TLS：`GrpcConfig` 没有任何证书/密钥字段；`[security.ssl]`、`[http].https_*` 虽在代码中存在，但默认关闭且未出现在用户配置模板中。生产环境管理面（HTTP + gRPC）不应默认明文。

**中 — 一致性与误用抗性**

4. `[http].port` / `[http].bind_address` 是死配置：HTTP 实际绑定 `database.host:database.port`（`linkrs-server/src/http_server.rs:56,112`），改 `[http].port` 完全无效。监听语义应统一归 `[http]` 节所有，`[database].host/port` 不再兼任监听地址。
5. 同名不同义的并发上限：`[database].max_connections`（会话数，`graph_service/factory.rs:144`）、`[grpc].max_connections`、`[connection_pool].max_size` 三者名称相近、语义各异，且 `connection_pool` 未出现在用户配置中。
6. 无严格反序列化：所有配置结构均未启用 `deny_unknown_fields`，配置键拼错被静默忽略并回退默认值。数据库配置应 fail-fast，至少启动时对未知键告警。
7. 缺跨节校验：端口冲突（HTTP/gRPC 配成同端口要等 bind 失败才暴露）、宽松 CORS + 非 loopback 绑定、`single_user_mode` + 对外监听等危险组合均无启动期拦截。
8. 零值语义未定义/未校验：`transaction.default_timeout=0`、`session_idle_timeout_secs=0`、`monitoring.slow_query_threshold_ms=0`（全部查询被记为慢查询）都没有校验或文档说明。

**低 — 完备性与运维面**

9. 用户可见配置缺失生产关键节：`[storage]`（内存预算、checkpoint、压缩）、`[query_resource]`（并发查询数、单查询内存、超时）、`[fulltext]`、`[connection_pool]`、`[security.audit]` 均未出现在 `config.toml` 模板中。参照 postgresql.conf 的做法，模板应包含全部重要旋钮并以注释给出默认值。
10. 审计日志默认关闭且未在模板暴露（数据库合规基本项）。
11. 无可观测性导出端点：只有业务统计 API，没有 Prometheus 风格 `/metrics`，无法接入通用监控栈。
12. 日志形态单一：仅文件输出，无 stdout 选项（容器部署常用）、无 JSON 结构化格式、无模块级过滤。
13. 缺备份恢复、WAL 持久性（fsync/group commit）等数据库标配运维旋钮。

## 二、改进方案

### 阶段一（安全修复，改动小、收益大）

1. `Config`/`AuthConfig` 等含凭证结构实现自定义 `Debug`，敏感字段显示为 `***`；或移除 `startup.rs:56` 的全量打印。
2. 默认口令机制改造：配置中不再提供 `default_password` 字面值；首启用随机口令并落盘（仅属主可读），支持 `LINKRS_PASSWORD` 环境变量注入；`enable_authorize=true` 且口令仍为默认值时启动告警。
3. gRPC 增加 TLS 配置（cert/key/ca，复用 `SslConfig`），并在模板中给出注释示例；HTTP 侧已有字段写入模板。

### 阶段二（监听与命名统一）

4. HTTP 监听改用 `[http].bind_address/port/enabled`，`[database].host/port` 收敛为纯客户端展示信息或删除；消除死配置。
5. 并发上限重命名：`[database].max_connections` → 语义化为会话上限（或并入 `[connection_pool]`），保留唯一事实来源。
6. 所有配置结构启用 `deny_unknown_fields`（或加载后 diff 未知键并报错），杜绝拼写错误静默生效。

### 阶段三（校验补强）

7. `Config::validate()` 增加跨节校验：监听端口互不相同；`cors_allowed_origins` 为空且绑定非 loopback 时拒绝启动（或要求显式 `allow_insecure` 开关）；`single_user_mode` + 非 loopback 拒绝。
8. 为超时/阈值字段定义 0 值语义（0 = 禁用 或 直接拒绝 0），并在模板注释中写明。

### 阶段四（模板与运维面）

9. `config.toml` 模板补齐 `[storage]`、`[query_resource]`、`[fulltext]`、`[connection_pool]`、`[security.audit]`、`[security.ssl]`、`[http].*` 各节，全部以注释形式给出默认值与单位。
10. 新增 `/metrics` Prometheus 端点（复用现有 StatsManager 数据）。
11. 日志增加 stdout 开关与 JSON 格式选项。

## 三、专项：日志文件名与日志滚动是否冲突

### 3.1 主日志本身不冲突，但字段语义误导

`config.toml` 中 `[log].file = "linkrs"`，`linkrs-config/src/logging.rs:51-65` 将其作为 `FileSpec::basename` 传入 flexi_logger：

- 实际当前文件是 `linkrs.log`（库自动补 `.log` 扩展名），滚动后产生 `linkrs_NN.log` 编号文件，与 `Naming::Numbers` + `Criterion::Size` + `KeepLogFiles(5)` 自洽，**不构成功能性冲突**。
- 但字段名为 `file` 而语义是 basename：写 `linkrs.log` 时扩展名会被规范化（结果仍是 `.log`），写 `linkrs.txt` 时 `.txt` 被吞掉。应改名为 `basename`（或在文档中明确“不含扩展名的文件主名”），避免用户按“完整文件名”理解。

### 3.2 真正的冲突：三路日志的目录与滚动配置各自为政

| 日志流 | 路径来源 | 相对路径基准 | 滚动大小单位 | 保留份数字段 |
|---|---|---|---|---|
| 主日志 | `[log].dir` + `[log].file` | `~` 展开 → `~/.linkrs/logs` | 字节（`max_file_size`） | `[log].max_files` |
| 慢查询日志 | `monitoring.slow_query_log.log_file_path`，默认 `logs/slow_query.log`（`monitoring.rs:107`） | 相对**配置文件目录**（`lib.rs:569`） | MB（`max_file_size_mb`） | 独立 `max_files` |
| 审计日志 | `security.audit.log_file`，默认 `logs/audit.log`（`security.rs:64`） | 同上 | MB | 独立 `max_files` |

由此产生三个实际问题：

1. **目录分裂**：用户把 `[log].dir` 配成 `~/.linkrs/logs`，主日志如期落此；但慢查询/审计日志的默认相对路径 `logs/...` 按配置文件所在目录解析（如 `~/.config/linkrs/logs/`），`[log].dir` 对它们完全无效——单实例日志散落两处。
2. **单位不一致**：主日志滚动阈值用字节（104857600），慢查询/审计用 MB（100）。同一个数字“100”在三处相差 6 个数量级，是典型误配陷阱。
3. **策略不可见**：慢查询/审计日志的滚动参数（阈值、路径、保留数）没有出现在 `config.toml` 模板中，用户既不知道有这些独立日志流，也无法统一调整。

### 3.3 修复方案（并入阶段四）

- 统一日志根目录：慢查询与审计日志的文件名默认从 `[log].dir` 派生（如 `<log_dir>/slow_query.log`、`<log_dir>/audit.log`）；其路径字段仅接受文件名或绝对路径，删除“相对配置文件目录”这一直觉外解析。
- 统一大小单位：三处滚动阈值统一为同一单位（建议字段名带 `_mb` 后缀，或统一用带单位的字符串如 `"100MB"`）。
- `[log].file` 更名为 `basename`，并在模板注释中说明扩展名与滚动编号由日志库追加。
