# GPUI 客户端剩余工作细化方案

> 依据前端实现（`frontend/src`）与 GPUI 工作树实测状态制定。设计基线见《[GPUI 桌面客户端设计方案](gpui_client_design.md)》，阶段 0–3 进度见《[实施计划](gpui_client_implementation_plan.md)》§12。
> **交付状态（2026-11-02 已核验）**：阶段 A–G 全部完成。`cargo check` 通过（`dead_code` 约 62 条降至 31 条），`cargo test` 7 通过；Dock 布局版本升至 4；16 面板全部注册。

## 0. 范围界定：什么不做

- 历史记录走服务端接口：前端历史同样是本地 `localStorage`（`stores/history.ts`），服务端无历史接口，GPUI 保持本地持久化，只需对齐前端的数据模型。
- 文件传输分块进度：服务端为单请求体端点，无分块能力，不做客户端假进度。
- OS keychain：无可用的离线依赖（`keyring` 不在本地 cargo 缓存），继续不存密码，会话 id 只驻内存；待网络恢复后单独引入。
- 管理页坚持内联表单 + 二次确认，不引入 `Sheet`/`Dialog`。

## 1. 对照结论：前端有、GPUI 缺的

| 缺口 | 前端参照 | GPUI 现状 |
|---|---|---|
| 历史记录模型 | `history.ts`：富条目（耗时/行数/成功/路径/错误码/trace id）+ 收藏（命名 + 首选路径）+ 清空，上限 50/30 | `settings.query_history: Vec<String>` 纯文本，无收藏 |
| 历史/收藏侧栏交互 | `HistorySidebar`/`FavoritesSidebar`：载入编辑器 + 重跑；`SaveQueryModal` 命名收藏 | 侧栏只载入编辑器，无重跑、无收藏 |
| Validate / Explain | `query.ts`：`validate`、`explain` 单语句端点；控制台按钮触发 | `query_service` 已有 `validate`/`explain`/`list_running`/`kill_running`，控制台未接线 |
| 查询画像 | `PortraitDrawer`：按 trace id 拉 `queryProfile` 详情 | 控制台展示 trace id 但无详情入口；`linkrs-client` 已有 `get_query_profile_detail` |
| 事务页 | `transactions/+page.svelte`：overview + `transactionMetrics` + 活跃事务表 + kill（含确认）+ outbox 重试 | `transaction_service` 全备但无视图 |
| 同步页 | `sync/+page.svelte` + `SyncPanel`：overview + `syncStatus` + 死信/重入列 + 降级区间/清除 + retention | `sync_service` 全备但无视图 |
| 函数管理 | schema 页函数区：列表 + 详情 + 注册（名/实现）+ 注销 | `function_service` 全备但无视图 |
| 配置管理 | `OpsPanel`：整配置读取 + 按 section/key 查改删；`retrieval.ts` 按 section 解析 | `config_service` 全备但无视图 |
| 监控扩展 | `monitoring.ts` store：search、freeze（+触发）、session、migration 统计，`retrieval.snapshot` 聚合 | 监控视图只有 overview/system/query/database 四块；`stats_service` 全备但无调用 |
| 命令面板 | —（前端无，桌面端新增） | 未实现；`gpui-component::command` 可直接用 |
| 打包分发 | — | 未启动，只需文档与清单配置 |

## 2. 分阶段交付（2026-11-02：全部已交付并核验）

每阶段结束时 `cargo check` 与 `cargo test` 通过；阶段间无半成品分支。

### 阶段 A：控制台对等（历史/收藏/校验/画像）——已交付

- 设置模型升级：`query_history` 改为富条目（语句、耗时、行数、成功、执行路径、错误码、trace id、时间戳），新增 `favorites`（名、语句、首选路径、创建时间），上限与前端一致，去重规则与前端一致（历史按语句去重置顶，收藏按名大小写不敏感判重），清空历史入口。旧纯文本文件解析失败时整体回退默认（开发期规则，不做迁移）。
- 控制台每次执行后按语句记录富条目（含流式路径的已收行数与状态）；历史侧栏展示耗时/行数/状态并支持重跑；收藏侧栏支持命名新增、载入、按首选路径重跑、删除。
- 控制台新增 Validate 与 Explain 按钮：Validate 结果渲染 JSON 块；Explain 结果作为独立只读结果 tab（复用表格渲染）。运行中查询列表与终止走 `list_running`/`kill_running`，入口放在控制台工具栏。
- 控制台结果 tab 若有 trace id，提供画像按钮，调用新增的 `query_service::profile` 拉详情并渲染 JSON 块。

### 阶段 B：事务视图（`views/transactions.rs`）——已交付

- 顶部指标卡：overview 关键数 + `transaction_metrics` 的 outcomes/resources 键值。
- 活跃事务表：id、状态、owner、耗时、staged/undo 字节、阻塞原因；kill 需二次确认（内联确认行），阻塞行额外提供 outbox 重试。
- 刷新按钮；错误统一走会话失效判定。注册进 Dock 与侧栏管理组。

### 阶段 C：同步视图（`views/sync.rs`）——已交付

- 状态快照卡：`status` + `diagnostics` JSON 块。
- 死信表：target/index/generation/limit/offset 过滤 + 列表 + 按过滤重入列 + 全局重试。
- 降级区间表：同条件过滤 + 按行清除（二次确认）。
- retention 状态查询与单次执行（可选距离/时间参数）。
- 注册进 Dock 与侧栏管理组。

### 阶段 D：函数视图（`views/functions.rs`）——已交付

- 函数名列表 + 选中查看详情 JSON；注册表单（名 + 实现路径）；注销二次确认；刷新。注册进 Dock 与侧栏管理组。

### 阶段 E：配置视图（`views/config.rs`）——已交付

- 整配置 JSON 树展示；section/key 查询行；值编辑（JSON 文本，解析失败按字符串提交，与会话变量编辑一致）+ 更新；重置二次确认。注册进 Dock 与侧栏管理组，Dock 布局版本升至 4。

### 阶段 F：监控扩展（无新面板）——已交付

- 监控视图新增 search、freeze、当前会话、migration 四块 JSON/键值区；freeze 区附带单次触发按钮（二次确认）。
- retrieval 聚合（config + vector + fulltext）按前端 `retrieval.ts` 做 best-effort 快照，失败分项标注 unknown，不阻塞其它区块。

### 阶段 G：命令面板与收尾——已交付（打包为文档说明，keyring 仍延期）

- `Ctrl+K` 全局面板（`gpui-component::command`）：分组为面板跳转（全部已注册面板）、控制台执行、space 切换；面板跳转复用主框架激活逻辑。
  已交付：面板跳转（16 面板）、运行编辑器查询、流式开关、主题切换（Light/Dark/System，复用全局 Action 自动分发）；`Esc` 经 `on_cancel` 关闭，打开时自动聚焦搜索框。space 切换未纳入（需先拉取 space 列表的异步选择器，单独任务）。
- 打包说明文档化（Linux AppImage/deb、macOS bundle、Windows）：清单与构建命令，不含二进制产物。
  已交付为构建说明（见下 §4）；产物本身不在仓库内。
- 性能：大表已虚拟化，复查各视图轮询并发度；`keyring` 引入条件记录（需恢复网络后单独任务）。
- 实施计划 §12 更新为本方案交付记录。
  已交付：见《[实施计划](gpui_client_implementation_plan.md)》§12.6。

## 3. 风险（2026-11-02：均已处置或接受）

| 风险 | 缓解 |
|---|---|
| 设置模型变更导致旧 `gpui.toml` 回退默认 | 开发期可接受；回退逻辑已存在（`unwrap_or_default`） | 已接受：类型变更后旧文件解析失败整体回退默认 |
| 画像/运行中查询等端点需管理员权限 | 按区块 best-effort 展示，错误只进错误条，不阻塞整页 | 已落实：监控扩展区块独立失败不影响核心四块 |
| 新面板增多导致 Dock 旧布局漂移 | 布局版本升至 4，旧布局自动重置 | 已落实：`MAIN_DOCK_VERSION = 4` |
| 命令面板与面板注册耦合 | 跳转只调主框架已有激活函数，不碰 Dock 内部状态 | 已落实：`confirm_palette` 只调 `activate_panel` 与控制台方法 |

## 4. 打包说明（阶段 G 产物）

构建命令（独立 workspace，根 workspace 不覆盖 GUI）：

```shell
git submodule update --init crates/gpui-kit   # 构建前置
cargo build --manifest-path crates/linkrs-gpui/Cargo.toml --release
```

产物为单一可执行文件 `target/release/linkrs-gpui`（workspace 名 `linkrs-gpui`，包未发布）。各平台分发形态：

- Linux：`target/release/linkrs-gpui` + 桌面入口与图标即可组成 tarball；AppImage/deb 可后续用 `cargo-deb` / appimagetool 打包，不改代码。
- macOS：按 `Linkrs.app/Contents/MacOS/linkrs-gpui` 布局组装 bundle，签名与公证位单独任务。
- Windows：单 exe 分发；原生窗口边框由 gpui-kit 处理。

`keyring`（OS 凭据存储）引入条件：恢复外部网络后单独任务，当前继续不存密码、会话 id 只驻内存。
