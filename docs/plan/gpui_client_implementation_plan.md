# GPUI 桌面客户端分阶段实现方案

> 依据《[GPUI 桌面客户端设计方案](gpui_client_design.md)》制定的可执行实施计划。
> 方案基线：子模块已递归拉取完成，关键结论来自对 `crates/gpui-kit`、`crates/linkrs-client`、`frontend/src` 的实际调研，非估算。

## 0. 前置状态确认

子模块已全部递归拉取（`git submodule update --init --recursive`），锁定提交如下：

| 子模块 | 分支 | 提交 | 说明 |
|---|---|---|---|
| `crates/gpui-kit` | feat/compograph | `0f43ef2` | UI 框架 umbrella，内嵌 4 个二级子模块 |
| └ `crates/autostart` | main | `b6260e3` | 开机自启 |
| └ `crates/compograph` | main | `7c80d10` | 原生图渲染库（app/engine/foundation/render 四个子 crate，约 130 个源文件，开发中） |
| └ `crates/global-hotkey` | main | `f6f6a2a` | 全局热键 |
| └ `crates/tray` | main | `47717ad` | 系统托盘 |
| `crates/llm-suite` | main | `1f54f2a` | LLM 集成（本方案暂不使用） |
| `crates/simvec` | main | `f6e360c` | 向量库（本方案暂不使用） |
| `crates/tantivy` | feat/add-configurable-k1-b | `2709ca8` | 全文索引 fork（服务端已用） |

## 1. 调研结论与关键约束

### 1.1 gpui-kit 现状

- umbrella crate 为 `crates/gpui-kit/crates/kit`（`gpui-kit = 0.8.0`），GPUI 通过 `gpui-pre =0.3.9` 精确 pin（`script/check-gpui-pin.ts` 在 CI 强制精确版本）。
- **工具链约束**：`gpui-pre 0.3.9` 使用 `std::hint::cold_path`。该特性在 stable 1.96.0 已稳定（实测 1.96.0 编译通过；1.94.1 报 E0658），因此构建本客户端要求 rustc ≥ 1.96 的 stable 工具链。`crates/gpui-kit` 自身 `rust-toolchain.toml`（beta）仅在其独立 workspace 内生效；根 workspace 构建本客户端时以调用方工具链为准，无需切换 beta。
- **依赖接入方式（实测结论）**：cargo 会把 path 依赖清单里的 `{ workspace = true }` 继承解析到"消费方 workspace"的 `[workspace.dependencies]`（多轮最小实验证实，`exclude` 亦不改变该行为），而 gpui-kit 的私有依赖表（`gpui = gpui-pre =0.3.9`、`reqwest → gpui-pre-reqwest` 等）无法也不应镜像进根 workspace（`reqwest` 键与根 workspace 的真实 reqwest 冲突）。因此 `linkrs-gpui` 对 gpui-kit 采用 **git 依赖**（指向 fork `kkkqkx123/gpui-kit` 的 `feat/compograph` 分支）：git 依赖的清单继承由其仓库自身 workspace root 解析，与根 workspace 互不干扰。`crates/gpui-kit` submodule 仍保留，用于本地阅读源码与开发参考，两者锁定同一提交。
- 组件清单覆盖本方案所需绝大部分：`Table/DataTable`、`Tree`、`Dock`（支持布局序列化/恢复）、`Sidebar`、`Input/TextArea`（Rope 内核、tree-sitter 高亮）、`Form`、`Dialog/Sheet`、`Notification`、`Chart`（line/bar/area/pie/radar/sankey）、`Pagination`、`Progress`、`Select/Combobox`、`Command`（命令面板）、`TitleBar`、`StatusBar`、`Empty`、`Skeleton`、`Spinner`、`VirtualList`、`Theme`。
- `gpui-webview`（`crates/gpui-kit/crates/webview`）基于 wry，平台实现仅有 macOS/Linux 源文件，无 Windows 实现。本方案已选定 compograph 原生路线，webview 仅作兜底备选，不进入默认依赖。
- `compograph` 为原生图渲染库（app/engine/foundation/render 四个子 crate，约 130 个源文件），已选定为本方案图可视化的实现（见决策三）。
- 可参考示例：`examples/tig`（main/app/repository 三层组织）、`examples/system_monitor`（图表轮询刷新）、`examples/dock`、`examples/editor`、`examples/webview`。

### 1.2 linkrs-client 端点覆盖

已覆盖（46 个公开方法）：连接/断开/健康检查、`execute_query(_raw/_batch/_stream)`、space 列表/创建/删除/切换、tag/edge_type 列表与创建、事务 begin/commit/rollback、批量任务全生命周期（create/add/execute/status/cancel/delete）、统计（session/query/database/system/overview）、配置读写重置、游标 open/fetch_page/close、函数列表。SSE 解析（`sse.rs`）与会话管理（`SessionManager`、变量替换）齐备。

缺口（需补强类型方法，或过渡期走 `HttpClient::inner()` + `base_url()`）：

| 功能域 | 缺失端点 | 使用阶段 |
|---|---|---|
| 数据浏览器 | 顶点/边分页扫描、单条记录详情、按谓词过滤 | 阶段二 |
| 图可视化 | N 跳邻接查询、节点度数统计 | 阶段二 |
| 用户管理 | 用户列表/创建/删除/改密/角色分配 | 阶段三 |
| 全文管理 | 索引列表/创建/删除/重建 | 阶段三 |
| 向量管理 | 集合列表/创建/删除/统计 | 阶段三 |
| 迁移 | 任务列表/发起/进度/取消 | 阶段三 |
| 会话管理 | 会话列表/终止 | 阶段三 |
| 导入导出 | 文件上传/下载进度 | 阶段二 |

### 1.3 Web Studio 功能域（对齐目标）

路由 13 个：console、schema、graph、data-browser、monitoring、users、batch、transfer、migration、vector、fulltext、sessions、login；服务层 20 个文件（`frontend/src/lib/services/*`），是各视图的数据契约参考。

### 1.4 关键决策

**决策一：工具链与 workspace 归属。**

实测 stable 1.96.0 已支持 `cold_path`（1.94.1 不支持），无需 beta 工具链。`linkrs-gpui` 直接加入根 workspace `members`，与设计文档第 4 节一致；包级 `rust-version = "1.96"` 显式声明工具链下限，开发者与 CI 使用 ≥1.96 的 stable。gpui-kit 以 git 依赖接入（原因见 1.1"依赖接入方式"），submodule 只作本地开发参考。仅当后续 gpui-kit 快照再次引入 stable 未稳定的特性时，才回退到"独立 workspace + 自带 beta 工具链"的备份方案。

**决策二：新端点只进 `linkrs-client`。** GPUI 层禁止直接拼 URL；`inner()` 逃生舱只允许出现在 `services/` 内并标注 TODO。

**决策三：图可视化直接使用 compograph。** 图视图基于 `crates/gpui-kit/crates/compograph` 原生渲染，不走 WebView 嵌 Cytoscape 的路线；这同时消除了 `gpui-webview` 缺 Windows 实现的平台限制。compograph API 未合 gpui-kit 主线，因此图视图模块用清晰接口边界隔离，compograph 升级只影响 `graph_host.rs` 与 `graph_service.rs` 两个文件。若 compograph 短期无法满足基础交互（点选/展开邻域），WebView 仅作兜底预案，不提前投入。

**决策四：不引入 mock 模式。** 初期直接连真实服务端；services 层以 trait 边界隔离，便于后续按需加 mock。

**决策五：子模块只读原则。** gpui-kit 以 submodule 引入的目的，是避免向主仓库引入大量 UI 框架代码造成与上游冲突。因此将其视为只读依赖：不在 submodule 内做本地修改；需要的能力变更先推到 gpui-kit 仓库（kkkqkx123/gpui-kit），再在主仓库更新 submodule 指针。

## 2. 阶段总览

| 阶段 | 主题 | 交付物 | 依赖 |
|---|---|---|---|
| 阶段 0 | 工程准备与骨架 | 可编译运行的空壳应用（TitleBar + Sidebar + Dock + StatusBar + 主题 + 设置持久化） | 子模块拉取（已完成） |
| 阶段 1 | MVP：连接与查询控制台 | 登录、主框架、控制台、基础 Schema 浏览 | 阶段 0 |
| 阶段 2 | 数据与可视化 | 数据浏览器、图（compograph 原生）、监控、批量、导入导出 | 阶段 1 + linkrs-client 端点补齐 |
| 阶段 3 | 管理与高级能力 | 用户、迁移、向量、全文、会话管理页；流式查询、游标、变量 | 阶段 2 + 端点补齐 |
| 阶段 4 | 打磨 | 快捷键/命令面板、打包、性能、compograph 交互深化 | 阶段 3 |

每阶段结束时仓库处于可构建、可演示状态；不存在跨阶段的半成品分支。

## 3. 阶段 0：工程准备与骨架

### 3.1 工具链验证（先行）

1. 确认构建机 stable ≥ 1.96（`std::hint::cold_path` 自 1.96 起可用，1.94.x 报 E0658）。
2. 以 git 依赖方式解析并编译 gpui-kit（`cargo metadata` / `cargo check -p linkrs-gpui`），确认 `gpui-pre 0.3.9` 在 stable 下编译通过。

### 3.2 crate 骨架

```text
crates/linkrs-gpui/
  Cargo.toml            # 根 workspace 成员，workspace 继承
  src/
    main.rs             # 日志初始化、加载设置、进入应用
    app.rs              # Action/快捷键注册、窗口创建、主题应用
    state.rs            # AppState 实体：连接状态、当前 space
    settings.rs         # ~/.config/linkrs/gpui.toml 读写：主题、最近连接、窗口几何
    views/
      main_window.rs    # TitleBar + Sidebar + Dock + StatusBar 主框架与欢迎面板
```

`Cargo.toml`：

```toml
[package]
name = "linkrs-gpui"
version.workspace = true
edition.workspace = true
rust-version = "1.96"
description = "Linkrs native desktop client built on GPUI"
license.workspace = true
publish = false

[dependencies]
gpui-kit = { git = "https://github.com/kkkqkx123/gpui-kit", branch = "feat/compograph", default-features = false, features = ["component", "assets"] }
linkrs-client = { path = "../linkrs-client" }
linkrs-wire = { path = "../linkrs-wire" }
serde.workspace = true
serde_json.workspace = true
toml = "0.9"
directories = "6"
log.workspace = true
env_logger = "0.11"
anyhow.workspace = true
```

Workspace `members` 增加 `crates/linkrs-gpui`；`linkrs-client`/`linkrs-wire` 在阶段 0 一并纳入依赖，提前验证客户端依赖链可编译链接。

> gpui-kit 以 submodule 只读引入（决策五）：不在其中做本地修改，能力变更推上游后再更新指针；构建侧通过 git 依赖消费同一提交（见 1.1"依赖接入方式"）。包级 `rust-version = "1.96"` 覆盖根 workspace 的 1.94 下限。

初始化顺序（遵循 gpui-kit AGENTS.md）：

```rust
gpui_kit::application().run(|cx| {
    gpui_kit::init(cx);          // 必须在开窗前
    gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|_| MainView)
    })
});
```

### 3.3 主框架占位

- `TitleBar::window_options()` 自定义标题栏；最小尺寸约束。
- 左侧 `Sidebar`：导航分组（查询、数据、监控、管理），未实现项渲染为禁用态。
- 主区 `Dock`：欢迎面板 + 空标签容器；`DockEvent::LayoutChanged` 防抖序列化到配置目录 `dock_layout.json`（带版本号，版本不匹配时重置默认布局），启动时恢复。
- 底部 `StatusBar`：连接状态点、当前 space、版本号、主题切换按钮。
- 主题：启动按设置应用（system/light/dark，system 取当前窗口外观），按钮即时切换并持久化。

### 3.4 验收

- 根 workspace 下 `cargo build -p linkrs-gpui` / `cargo run -p linkrs-gpui` 打开窗口，Linux + macOS 实机验证。
- 主题切换即时生效；重启后主题、窗口几何与 Dock 布局恢复。
- 其余 crate 在同一 stable 工具链下构建不受影响。

## 4. 阶段 1：MVP——连接与查询控制台

### 4.1 新增模块

```text
src/
  connection/
    login_view.rs        # 登录表单
    session_store.rs     # 最近连接列表（不含密码）
  services/
    connection_service.rs
    query_service.rs
    schema_service.rs
  views/
    main_window.rs       # 登录成功后的主框架
    console.rs
    schema.rs
  components/
    result_table.rs
    empty_state.rs
```

### 4.2 任务分解

**连接（connection_service + login_view）**

- 包装 `Arc<Mutex<SessionManager>>`：`connect`、`disconnect`、`switch_space`、`health_check`（后台定时心跳，失败置状态栏红色）。
- 登录表单：host/port/user/password + 连接按钮；`SessionManager` 完成认证，session id 仅驻内存。
- 最近连接下拉（来自 settings，不含密码）；连接失败错误信封转用户可读通知。
- 会话失效（任何 service 返回 401 类错误）：清 session id，全局切回登录视图。

**查询控制台（query_service + console + result_table）**

- 编辑器：gpui-kit `Input/TextArea` + `tree-sitter-sql` 语法高亮；多语句分号切分。
- 执行路径：`execute_query` / `execute_query_raw`；`Ctrl+Enter` 运行、`Esc` 取消（取消为放弃结果读取，服务端游标类查询走 `close_cursor`）。
- 结果表：`DataTable` + 列排序 + 单元格复制 + 右键菜单；导出 CSV/JSON（写用户选择的路径）；行数上限内联提示。
- 多结果集 tab；错误信息面板（服务端错误码 + 消息）。
- 历史侧栏：优先服务端历史接口，暂缺则本地环形缓存（内存 + settings 截断持久化）。

**Schema 浏览（schema_service + schema）**

- `list_spaces` / `list_tags` / `list_edge_types` 渲染为 `Tree`；节点展开显示属性列。
- space 切换：`switch_space` 后全局刷新状态栏与控制台。
- 只读浏览即可（创建/删除走阶段三统一管理页），但 space 创建/删除可直接复用已有端点提前交付。

### 4.3 验收

与设计文档第 14 节一致：`cargo build` 通过；登录；执行 GQL 并展示表格结果与错误；Schema 列出 space/tag/edge；断开后回到登录视图。补充：状态栏实时反映连接健康；导出 CSV 内容与表格一致。

## 5. 阶段 2：数据与可视化

### 5.1 前置：linkrs-client 端点补齐（本阶段第一项工作）

在 `crates/linkrs-client/src/http_client.rs` 新增强类型方法（DTO 放 `linkrs-wire`）：

- 数据浏览器：顶点分页扫描（按 tag）、边分页扫描（按 edge type）、单条记录属性读取、谓词过滤参数。
- 图邻接：N 跳邻居查询、邻居度数。
- 导入导出：文件上传/下载端点（若服务端为分块接口，封装进度回调）。

每补一批端点，`linkrs-client` 单测同步补充（参照 `tests/command_integration_tests.rs` 的组织方式）。

### 5.2 新增模块

```text
src/
  services/
    data_browser_service.rs
    graph_service.rs
    monitoring_service.rs
    transfer_service.rs
  views/
    data_browser.rs
    graph.rs
    monitoring.rs
    batch.rs
    transfer.rs
  components/
    graph_host.rs        # gpui-webview 封装
    property_panel.rs
    filter_panel.rs
```

### 5.3 任务分解

**数据浏览器**：tag/edge type 选择器 + `FilterPanel`（属性等值/范围条件）+ 分页 `DataTable` + 行点击 `Sheet` 属性预览。游标类大表直接走 `open_cursor`/`fetch_cursor_page`（端点已存在）。

**图可视化（compograph 原生路线）**

- `graph_host.rs`：封装 compograph 图容器（同属 gpui 渲染管线，主题与事件体系天然一致），提供节点/边数据模型接入与视口控制。
- 交互：节点搜索定位、点选高亮、N 跳邻域展开（数据来自图邻接端点）、缩放/平移。
- 属性面板联动：点选节点/边后右侧展示原始记录。
- compograph 未合 gpui-kit 主线：阶段 2 起以 git 依赖指向其独立仓库（`kkkqkx123/compograph`，与 submodule 同源同提交），接口收敛在 `graph_host.rs` 与 `graph_service.rs`，升级只影响这两个文件（决策三、五）。

**监控**：`get_overview`/`get_query_statistics`/`get_system_statistics` 轮询（间隔可配，默认 5s）；`Chart` 折线（QPS、P99 延迟）+ 柱状（错误数）；慢查询列表 `DataTable`。参照 `examples/system_monitor` 的轮询-刷新模式。

**批量任务**：列表 + 详情（`get_batch_status` 轮询进度条 `Progress`）+ 新建向导（粘贴多语句或上传文件）+ 取消/删除。端点已齐，纯 UI 工作。

**导入导出（transfer）**：文件选择、目标 space/format 表单、进度条、结果通知。

### 5.4 验收

浏览顶点/边并分页过滤；图页可从搜索节点扩展 1–2 跳邻域并点选查看属性；监控页图表随负载变化；批量任务可创建、观察进度、取消；导入导出完成一次往返。

## 6. 阶段 3：管理与高级能力

### 6.1 前置：端点补齐第二批

用户 CRUD、全文索引管理、向量集合管理、迁移任务与进度、会话列表/终止、查询收藏（若服务端无收藏接口则本地实现）。

### 6.2 新增模块

```text
src/
  services/
    management_service.rs     # 用户/会话
    migration_service.rs
    vector_service.rs
    fulltext_service.rs
  views/
    users.rs
    sessions.rs
    migration.rs
    vector.rs
    fulltext.rs
```

### 6.3 任务分解

- **用户管理**：`DataTable` + `Sheet` 表单（创建/改密/角色）；操作前 `Dialog` 确认；错误码映射（权限不足等）。
- **会话管理**：列表 + 终止按钮 + 自动刷新开关。
- **迁移**：表单（源/目标/模式）→ 发起；`migrationStream` 对应的 SSE 进度复用 `linkrs-client::SseParser` 在后台任务中解析，进度列表逐项更新；支持取消。
- **向量/全文管理**：索引/集合列表、创建表单、删除确认、重建进度。
- **流式查询**：控制台新增"流式执行"模式——`execute_query_stream` + 后台循环读 SSE，行追加到结果实体，底部显示已收行数与速率，完成后可导出。
- **变量**：会话变量编辑面板（`SessionManager` 变量替换能力已存在），控制台提示未定义变量。
- **游标**：控制台结果超阈值自动转游标分页加载。

### 6.4 验收

管理页完成用户/索引/迁移操作（对齐设计文档阶段三验收）；流式查询持续输出 10 万行级结果 UI 不卡顿（虚拟滚动）；游标翻页正确关闭。

## 7. 阶段 4：打磨

1. **快捷键与命令面板**：全局 `Action` 注册表整理；`Command` 组件实现 `Ctrl+K` 面板（切换面板、执行查询、切换 space）。
2. **主题**：引入 gpui-kit `themes/*.json` 中的 2–3 套预设；高亮主题同步切换。
3. **打包分发**：Linux（AppImage/deb）、macOS（app bundle + 签名占位）、Windows；图标、版本号注入。
4. **性能**：大表 `VirtualList`/`DataTable` 虚拟化核查；轮询任务统一调度避免多页并发拉取；`profiler` feature 采样一轮。
5. **凭据与恢复**：评估 OS keychain（macOS Keychain / libsecret）存凭据，可选交付；Dock 布局崩溃恢复（布局序列化已有）。
6. **compograph 交互深化**：跟踪 gpui-kit 上游 compograph 进展，按需补齐布局算法选项、节点聚合与性能上限测试；compograph 依赖只出现在 `graph_host.rs`/`graph_service.rs`，不扩散到其他模块。

## 8. 测试策略

| 层 | 手段 |
|---|---|
| linkrs-client 新端点 | 单测 + 对真实 server 的集成测试（沿用现有 `tests/` 组织） |
| services 层 | 逻辑单测（参数组装、错误映射、SSE 事件折叠） |
| 视图状态机 | gpui-kit `test-support` UI 测试（`crates/gpui-kit/crates/kit/tests/` 提供了 harness 范式）：登录流转、会话失效回登录、结果追加 |
| 每阶段人工验收 | 各阶段验收清单逐项过，Linux 优先，macOS 次之 |

## 9. 风险与缓解

| 风险 | 等级 | 缓解 |
|---|---|---|
| `cold_path` 要求 rustc ≥ 1.96（更早 stable 报 E0658） | 低 | 已实测 1.96.0 可用；包级 `rust-version = "1.96"` 显式声明下限；若未来 gpui-kit 快照再用 unstable 特性，回退独立 workspace + beta 方案 |
| gpui-pre 快照精确 pin，升级需整体验证 | 中 | 锁定当前 0.8.0 + 0.3.9；升级作为独立任务走完整回归 |
| compograph 未合 gpui-kit 主线、API 不稳定 | 中 | git 依赖固定分支（Cargo.lock 锁定提交），submodule 只读参考；接口收敛在 `graph_host.rs`/`graph_service.rs`；能力变更走 gpui-kit 上游 |
| linkrs-client 端点缺口多 | 低 | 按阶段补齐，先于使用该端点的视图开发；禁止 GPUI 层拼 URL |
| Linux Wayland IME | 低 | 记录已知问题；X11 为默认推荐会话 |
| 大结果集渲染 | 低 | 阶段 3 起强制游标/流式 + 虚拟滚动，不做全量渲染 |

## 10. 里程碑验收总表

| 阶段 | 硬性验收 |
|---|---|
| 阶段 0 | 空壳应用可编译运行；主题/窗口几何/Dock 布局持久化生效 |
| 阶段 1 | 登录 → 执行 GQL → 表格结果/错误 → Schema 树 → 断线回登录，全链路可用 |
| 阶段 2 | 数据浏览器分页过滤；图原生渲染邻域扩展与点选；监控图表；批量任务全流程 |
| 阶段 3 | 用户/会话/迁移/向量/全文管理页可用；流式与游标大结果稳定 |
| 阶段 4 | 打包产物可安装启动；命令面板与快捷键齐备；性能采样无明显回归 |
