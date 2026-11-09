# deploy/

Linkrs 服务端的部署资源目录：systemd 服务单元、launchd 服务描述、Windows 原生服务说明、DEB/RPM 打包配置、logrotate 配置、默认配置模板，以及安装/卸载/状态检查脚本。

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
├── launchd/com.linkrs.server.plist  # launchd 服务描述（macOS）
├── windows/                    # Windows 部署（原生服务 + 默认配置模板）
│   └── config/linkrs.toml      # Windows 默认配置模板
└── scripts/
    ├── install-service.sh      # 一键安装服务（含用户/目录/配置/单元）
    ├── uninstall-service.sh    # 卸载服务
    └── service-status.sh       # 查看服务状态
```

## systemd 服务单元（systemd/linkrs.service）

- **Type=notify**：systemd 等待服务端上报 `READY=1`（HTTP 监听绑定成功后发送，纯 gRPC 部署为地址解析成功后发送）；停机时上报 `STOPPING=1`。需配合 `NotifyAccess=main` 使用。
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

## Windows（原生服务）

`linkrs-server` 内建 Windows 服务支持（`windows-service`）：被 SCM（服务控制管理器）拉起时走服务入口，监听绑定成功后上报运行中，停止控制接入现有优雅停机路径；交互运行时自动回落到控制台模式。NSSM 包装方案已退役（长期不维护），存量实例先移除旧包装服务再按本节注册。需要管理员权限的 PowerShell。

### 目录约定

| 内容 | 默认路径 |
|---|---|
| 配置 | `C:\ProgramData\linkrs\config.toml`（由 `windows\config\linkrs.toml` 模板安装） |
| 数据 | `C:\ProgramData\linkrs\data` |
| 日志 | `C:\ProgramData\linkrs\logs` |
| 程序 | `C:\Program Files\linkrs\linkrs-server.exe` |

### 注册与管理

```powershell
# 注册（注意 binPath= 与 start= 后的空格是 sc.exe 语法要求）
sc.exe create linkrs binPath= '"C:\Program Files\linkrs\linkrs-server.exe" serve --config "C:\ProgramData\linkrs\config.toml"' start= auto
# 异常退出自动重启，间隔 5 秒
sc.exe failure linkrs reset= 86400 actions= restart/5000

sc.exe start linkrs
sc.exe query linkrs
sc.exe stop linkrs
sc.exe delete linkrs    # 仅移除服务，保留配置/数据/日志；二进制需手动删除
```

已存在 `config.toml` 时模板不覆盖；配置变更后 `sc.exe stop/start` 重启生效（热重载未实现）。

## macOS（launchd）

macOS 无需服务端代码改动：`launchd` 没有就绪协议，`SIGTERM` 即进入现有优雅停机路径。模板见 `deploy/launchd/com.linkrs.server.plist`，按实际安装路径调整后装载：

```shell
cp deploy/launchd/com.linkrs.server.plist /Library/LaunchDaemons/
launchctl load /Library/LaunchDaemons/com.linkrs.server.plist
launchctl unload /Library/LaunchDaemons/com.linkrs.server.plist  # 停止并卸载
```

`KeepAlive` 保证异常退出后重启，`RunAtLoad` 保证开机自启，日志落到模板配置的 `StandardOutPath` / `StandardErrorPath`。
