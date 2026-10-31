# deploy/

Linkrs 服务端的部署资源目录：systemd 服务单元、DEB/RPM 打包配置、logrotate 配置、默认配置模板，以及安装/卸载/状态检查脚本。

## 目录结构

```
deploy/
├── systemd/linkrs.service      # systemd 服务单元
├── config/linkrs.toml          # 默认配置模板（安装为 /etc/linkrs/config.toml）
├── packaging/                  # 系统包（deb/rpm）打包配置
│   ├── debian/control          # DEB 包元数据（版本由 gen_metadata.sh 同步）
│   ├── debian/conffiles        # DEB 配置文件清单
│   ├── rpm/linkrs.spec         # RPM spec 文件（含 systemd scriptlet）
│   ├── gen_metadata.sh         # 从 workspace 版本生成包版本号
│   ├── postinst                # DEB 安装后钩子
│   ├── prerm                   # DEB 卸载前钩子
│   └── postrm                  # DEB 卸载后钩子
├── logrotate/linkrs            # 日志轮转配置
├── windows/                    # Windows 部署（NSSM 服务包装）
│   ├── config/linkrs.toml      # Windows 默认配置模板
│   └── scripts/
│       ├── install-service.ps1
│       ├── uninstall-service.ps1
│       └── service-status.ps1
└── scripts/
    ├── install-service.sh      # 一键安装服务（含用户/目录/配置/单元）
    ├── uninstall-service.sh    # 卸载服务
    └── service-status.sh       # 查看服务状态
```

## systemd 服务单元（systemd/linkrs.service）

- **Type=exec**：只验证 `exec()` 成功。服务端尚未实现 sd_notify；待服务端上报 `READY=1` 后可改回 `Type=notify`。
- 以专用系统用户 `linkrs:linkrs` 运行，工作目录 `/var/lib/linkrs`。
- 通过环境变量 `LINKRS_CONFIG` 与 `--config` 双重指定配置文件 `/etc/linkrs/config.toml`。
- `After/Wants=network-online.target`；`Restart=on-failure`，间隔 5s。
- **没有 `ExecReload`**：服务端不处理 SIGHUP，配置 reload 语义未实现；请使用 `systemctl restart linkrs`。待服务端实现 SIGHUP 热重载后再添加。
- 二进制路径 `/usr/bin/linkrs-server`（与 RPM `%{_bindir}` 及脚本默认 `PREFIX=/usr` 一致）。
- `LimitNOFILE=65536`，日志输出到 journald（SyslogIdentifier=linkrs）。
- 安装目标 `multi-user.target`。

## 默认配置模板（config/linkrs.toml）

面向系统部署的最小配置：`storage_path=/var/lib/linkrs/data`、`log.dir=/var/log/linkrs`、HTTP/gRPC 绑定回环。其余配置项保持内置默认值（完整注释参考见仓库根 `config.toml`）。

由 `install-service.sh` 在 `/etc/linkrs/config.toml` 不存在时安装；DEB/RPM 打包时作为 `/etc/linkrs/config.toml`（conffile / `%config(noreplace)`）。

## 打包（packaging/）

**版本单一来源**：包版本号来自根 `Cargo.toml` 的 `[workspace.package].version`，由 `gen_metadata.sh` 同步写入 `debian/control` 与 `linkrs.spec`，不要手工编辑包版本：

```shell
./deploy/packaging/gen_metadata.sh
```

**DEB（debian/）**
- 包名 `linkrs`，Section `database`，依赖 `libc6`、`systemd`。
- `/etc/linkrs/config.toml` 声明为 conffile，升级时不覆盖用户修改。

**维护脚本**
- `postinst`：安装后执行 `daemon-reload`；无 `/run/systemd/system`（chroot/构建环境）时跳过；否则按发行版 preset 决定是否自启（`systemctl preset`）。
- `prerm`：卸载前停止并 disable 服务。
- `postrm`：卸载后 `daemon-reload`；仅在 `purge` 动作时删除 `/var/lib/linkrs`、`/var/log/linkrs`、`/etc/linkrs`（remove 保留数据）。

**RPM（rpm/linkrs.spec）**
- License Apache-2.0，x86_64，`Requires: systemd`、`BuildRequires: systemd-rpm-macros`。
- `%pre` 创建 `linkrs` 用户/组与目录；`%post/%preun/%postun` 使用 systemd 宏（与 DEB 行为对齐：安装后 preset、升级/卸载前停服务）。
- 路径使用 RPM 宏：`%{_bindir}`（`/usr/bin`）、`%{_unitdir}`。
- 打包文件：`/etc/linkrs/config.toml`（`%config(noreplace)`）、`linkrs-server`、`linkrs-cli`、unit 文件及数据/日志目录。

## 日志轮转（logrotate/linkrs）

对 `/var/log/linkrs/*.log`：每日轮转、保留 7 份、压缩（delaycompress）、`copytruncate`（服务端持有文件句柄，截断后无需信号即可继续写新文件）、文件不存在不报错、空文件不轮转；新文件权限 `0640 linkrs:linkrs`。

## 脚本

### install-service.sh（root 运行）

安装流程：

1. 创建系统用户/组 `linkrs`（shell 为 nologin，home 在数据目录）。
2. 创建目录 `/etc/linkrs`、`/var/lib/linkrs`、`/var/log/linkrs`，属主 `linkrs:linkrs`，权限 0750；umask 027。
3. 安装二进制：按 `./bin/linkrs-server` → `./linkrs-server` → `${PREFIX}/bin/linkrs-server` 顺序查找；均不存在则仅警告（可手动安装）。**`START_SERVICE=yes` 时二进制缺失会直接报错退出**。
4. 安装默认配置：`/etc/linkrs/config.toml` 不存在时安装 `deploy/config/linkrs.toml` 模板（回退到当前目录的 `config.toml`），权限 0640。
5. 安装 systemd 单元到 `/etc/systemd/system/linkrs.service` 并 `daemon-reload`。
6. 根据环境变量决定 enable/start。

可通过环境变量覆盖默认行为：

| 变量 | 默认 | 说明 |
|---|---|---|
| `PREFIX` | `/usr` | 二进制安装前缀（单元中的路径为 `/usr/bin/linkrs-server`，改 PREFIX 需同步改单元） |
| `CONFIG_DIR` | `/etc/linkrs` | 配置目录 |
| `DATA_DIR` | `/var/lib/linkrs` | 数据目录 |
| `LOG_DIR` | `/var/log/linkrs` | 日志目录 |
| `CONFIG_TEMPLATE` | `deploy/config/linkrs.toml` | 默认配置模板来源 |
| `SERVICE_USER` / `SERVICE_GROUP` | `linkrs` | 服务运行账户 |
| `ENABLE_SERVICE` | `no` | `yes` 强制开机自启；`no` 强制 disable |
| `START_SERVICE` | `no` | `yes` 时安装后启动 |

示例：

```shell
sudo ENABLE_SERVICE=yes START_SERVICE=yes ./scripts/install-service.sh
```

### uninstall-service.sh（root 运行）

单元文件存在时停止并 disable 服务、删除单元文件并 `daemon-reload`。默认保留数据与配置，按需清理：

| 变量 | 默认 | 说明 |
|---|---|---|
| `PURGE_CONFIG` | `no` | `yes` 时删除 `/etc/linkrs` |
| `PURGE_DATA` | `no` | `yes` 时删除 `/var/lib/linkrs` 与 `/var/log/linkrs` |

二进制不会被删除，需手动移除 `/usr/bin/linkrs-server`。

### service-status.sh

输出 systemd 单元状态（`systemctl status`）、enable 状态与 active 状态，用于快速诊断。

## 快速开始

```shell
# 构建
cargo build --release

# 安装服务（二进制放在当前目录或 ./bin 下）
sudo ./deploy/scripts/install-service.sh

# 管理
sudo systemctl start linkrs
sudo systemctl status linkrs
sudo systemctl restart linkrs     # 配置变更后重启（SIGHUP 热重载暂未实现）

# 卸载
sudo ./deploy/scripts/uninstall-service.sh
```

## Windows（NSSM 方案）

Windows 下通过 [NSSM](https://nssm.cc/)（Non-Sucking Service Manager）把控制台程序包装为 Windows 服务：NSSM 负责与 SCM（服务控制管理器）交互并转发 start/stop 控制到进程，`linkrs-server` 无需内建 SCM 代码即可服务化。需要管理员权限的 PowerShell。

### 前置条件

- `nssm.exe`：放在脚本目录、`scripts\bin\`、`C:\Program Files\linkrs\`、PATH 之一，或用 `-NssmPath` 指定。
- `linkrs-server.exe`（release 构建）：查找顺序同上，或用 `-ServerPath` 指定。

### 目录约定（可用参数覆盖）

| 内容 | 默认路径 |
|---|---|
| 配置 | `C:\ProgramData\linkrs\config.toml`（由 `windows\config\linkrs.toml` 模板安装） |
| 数据 | `C:\ProgramData\linkrs\data` |
| 日志 | `C:\ProgramData\linkrs\logs` |
| 程序 | `C:\Program Files\linkrs` |

### 安装（deploy/windows/scripts/install-service.ps1）

```powershell
# 仅安装（手动启动）
.\install-service.ps1

# 安装 + 开机自启 + 立即启动
.\install-service.ps1 -EnableService -StartService
```

服务配置（由 NSSM 设置）：`AppDirectory` 为数据目录；stdout/stderr 重定向到 `logs\service-out.log` / `service-err.log`，并启用 NSSM 内建轮转（单文件 10MB，保留 5 份）；异常退出自动重启，间隔 5 秒；启动类型默认 Manual，`-EnableService` 时为 Automatic。重复执行会先卸载旧服务再重装；已存在 `config.toml` 时不覆盖。

### 卸载（uninstall-service.ps1）

```powershell
.\uninstall-service.ps1                # 仅移除服务，保留配置/数据/日志
.\uninstall-service.ps1 -PurgeData     # 另删 data 与 logs
.\uninstall-service.ps1 -PurgeConfig   # 另删整个 ProgramData\linkrs
```

二进制不会被删除，需手动移除。

### 状态诊断（service-status.ps1）

输出服务状态、NSSM 参数 dump 与最近的日志文件列表。

### 局限与后续

NSSM 是外部包装器，SCM 交互在 NSSM 进程内完成；服务端内建 Windows Service dispatcher（`windows-service` crate，SCM stop 接入现有优雅停机路径、`--service` 参数安装/卸载）的完整方案见 `docs/plan/deployment_design_review.md` 第四节，待产品确认支持 Windows 服务后实施。
