# Linkrs Server 操作清单

本文档整理 `linkrs-server` 二进制（根 crate，feature `server`）当前支持的操作。

## 1. 启动服务

```shell
linkrs-server serve [OPTIONS]
```

| 选项 | 说明 |
|------|------|
| `--config <path>` | 配置文件路径 |
| `--data-dir <path>` | 数据目录（覆盖 `common.database.storage_path`） |
| `--http-port <port>` | HTTP 端口（覆盖 `server.http.port`） |
| `--grpc-port <port>` | gRPC 端口（覆盖 `server.grpc.port`） |
| `--log-level <level>` | 日志级别（覆盖 `common.log.level`） |

行为：
- 加载配置（命令行指定或用户默认配置）。
- 校验有效配置；无效时退出。
- 初始化日志并启动 Tokio 运行时。
- 调用 `start_service_with_config_path` 启动服务。

## 2. 直接执行查询

```shell
linkrs-server query --query <gql>
# 或
linkrs-server query -q <gql>
```

| 选项 | 说明 |
|------|------|
| `--query` / `-q` | 要执行的 GQL 语句 |

行为：使用默认配置初始化日志，执行查询并输出结果。

## 3. 生命周期管理能力

当前服务进程支持：
- `SIGTERM`/`SIGINT` 触发优雅停机。
- gRPC 与 HTTP 服务随停机信号关闭。
- systemd `Type=notify` 就绪通知（`READY=1`/`STOPPING=1`）。
- 优雅停机超时可通过 `server.shutdown_timeout_secs` 配置，超时后强制退出。
- HTTP 健康检查端点 `/v1/health`、`/v1/health/live`、`/v1/health/ready`。

CLI 提供顶层 `service start|stop|restart|status` 管理本地进程；systemd 部署使用 `deploy/systemd/linkrs.service` 与 `deploy/scripts/*`。

## 4. 补充建议（Gap Analysis）

若需要“不依赖系统服务”的本地管理能力，建议按以下优先级推进：

1. **文档与脚本优先（低成本）**  
   提供启动/停止脚本与 systemd unit 示例，覆盖前台运行、端口/数据目录参数与日志位置。

2. **服务端增加最小生命周期命令（中成本）**  
   在 `linkrs-server` 中增加 `start`（可选后台化）、`stop`、`status`、`restart`，配合 PID 文件与健康检查端点。  
   - `start`：校验端口空闲、写入 PID、启动服务、等待就绪。  
   - `stop`：读取 PID、发送终止信号、等待退出、清理 PID。  
   - `status`：检查进程与健康端点。  
   - 不建议自动重启或隐式数据目录迁移。

3. **CLI 侧仅做调用，不做进程管理（低耦合）**  
   `linkrs-cli` 可增加顶层子命令 `server start|stop|status`，但内部应调用 `linkrs-server` 或管理其进程，避免在 REPL 中嵌入进程控制。

4. **嵌入式模式（长期方向）**  
   对“本地单用户、无服务”场景，优先考虑基于 `linkrs_api::embedded::GraphDatabase` 的嵌入式 CLI 模式，而非拉起 HTTP 服务进程。

结论：当前 `linkrs-server` 仅支持 `serve` 与 `query`；若要补充本地生命周期管理，建议先以服务端最小命令 + 文档/脚本落地，再决定是否在 CLI 暴露顶层子命令。
