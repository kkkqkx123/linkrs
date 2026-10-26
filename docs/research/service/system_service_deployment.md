# 系统服务实现参考（PostgreSQL）

## 1. 概述

PostgreSQL 通过 `pg_ctl` 管理数据库进程生命周期，并结合 systemd/Windows 服务管理器实现开机自启与服务控制。本文件仅描述参考项目的实现，不包含 Linkrs 的改造方案。

## 2. pg_ctl 进程生命周期管理

`pg_ctl` 是 PostgreSQL 的进程生命周期封装工具，支持：
- `start`：启动服务并等待可接受连接；需要通过 `PGDATA` 或 `-D` 指定数据目录。
- `stop`：停止服务，默认使用默认关闭模式；可通过 `-m` 控制关闭模式。
- `restart`：重启服务，默认复用先前运行实例的命令行参数。
- `status`：显示服务是否运行，包括 PID 与启动命令。
- `register`（Windows）：将 PostgreSQL 注册为系统服务。

辅助能力：
- 启动时可重定向日志输出并正确脱离终端与进程组。
- 受控关闭支持不同关闭模式。

## 3. 传统 Unix 自启动方式

- Linux：可将命令加入 `/etc/rc.d/rc.local` 或 `/etc/rc.local`，例如使用 `pg_ctl start -l logfile -D /usr/local/pgsql/data`。
- OpenBSD：在 `/etc/rc.local` 中检查二进制存在后，以 `postgres` 用户执行 `pg_ctl start` 并记录日志。
- Red Hat Linux：使用 init script `/etc/rc.d/init.d/postgresql stop` 停止开机启动的服务。

## 4. Windows 服务注册

`pg_ctl register` 将 PostgreSQL 注册为 Windows 系统服务。关键选项：
- `-N servicename`：服务名称与显示名称，默认 `PostgreSQL`。
- `-S start-type`：服务启动类型，`auto`（系统启动时自动启动）或 `demand`（按需启动），默认 `auto`。
- `-e source`：pg_ctl 作为 Windows 服务运行时写入事件日志的事件源名称，默认 `PostgreSQL`。

## 5. systemd 服务单元

PostgreSQL 官方提供的 systemd 单元示例：

```ini
[Unit]
Description=PostgreSQL database server
Documentation=man:postgres(1)
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
User=postgres
ExecStart=/usr/local/pgsql/bin/postgres -D /usr/local/pgsql/data
ExecReload=/bin/kill -HUP $MAINPID
KillMode=mixed
KillSignal=SIGINT
TimeoutSec=infinity

[Install]
WantedBy=multi-user.target
```

关键点：
- `Type=notify` 要求服务二进制支持 systemd 就绪通知，且需要在构建时启用 systemd 支持。
- 服务以专用用户运行。
- `ExecReload` 通过向主进程发送 `HUP` 信号实现重载。
- `KillMode=mixed` 与 `KillSignal=SIGINT` 控制停止行为。
- `TimeoutSec=infinity` 避免长时间关闭被超时终止。
- `[Install]` 中的 `WantedBy=multi-user.target` 决定启用后的启动目标。

## 6. systemd 服务启用与禁用管理

systemd 通过 `/etc/systemd/system/*.wants/` 中的符号链接管理启用状态：
- `systemctl enable <service>`：创建开机启动链接。
- `systemctl disable <service>`：移除链接。
- `systemctl is-enabled <service>`：查询启用状态，可能为 `enabled`、`disabled`、`masked`、`static` 等。
- `systemctl mask <service>`：将服务链接到 `/dev/null`，彻底禁止启动。
- `systemctl preset <service>`：依据 vendor preset 策略启用或禁用。
- `systemctl reenable <service>`：重新创建链接，用于单元变更后刷新。

包管理器通常在安装脚本中执行 `systemctl daemon-reload`，并依据发行版策略决定是否自动启用或启动服务。在 chroot、容器构建或非运行环境安装时，应避免实际启动服务。
