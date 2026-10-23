# GPUI 桌面客户端设计方案

## 1. 背景与目标

现有客户端形态有三类：

- Web Studio：`frontend/`，SvelteKit SPA，覆盖查询控制台、Schema 浏览、图可视化、数据浏览器、监控、用户管理、批量任务、迁移、向量、全文等完整管理功能。
- CLI：`crates/linkrs-cli`，基于 `crates/linkrs-client`，面向终端交互与脚本。
- 嵌入/库：`crates/linkrs-client` 提供 HTTP 客户端、会话状态、SSE 解析与 wire DTO 复用。

新增 GPUI 桌面客户端的目标是提供一个原生桌面管理端，复用 `linkrs-client` 的传输层与 `linkrs-wire` 的契约，功能对齐 Web Studio 的核心交互路径，而不是重写服务端协议或替代 Web/CLI。

GPUI 来源直接使用当前项目中的 `crates/gpui-kit`，不额外引入其它 UI 框架。

## 2. 现有模块分析

### 2.1 `linkrs-client`

- `HttpClient`：封装 reqwest、认证头、所有 CRUD/查询/事务/批量/配置/统计/游标/流式查询入口。
- `SessionManager`：连接生命周期、当前 space、变量替换、健康检查。
- `SseParser`：流式查询事件解析。
- `linkrs-wire`：查询、Schema、批量、全文、向量、迁移、元数据 DTO。
- 缺口：用户管理、向量管理、全文管理、迁移、数据浏览器、图邻接、收藏/历史等部分端点没有专用方法，但可通过 `HttpClient::inner()` + `base_url()` 直接访问，或逐步补到 `linkrs-client`。

### 2.2 Web Studio

- 服务层位于 `frontend/src/lib/services`，统一通过 `client.ts` 发起请求，自动附加 `X-Session-ID`，处理 JSON 大整数、错误信封与会话失效。
- 功能域由路由划分：console、schema、graph、data-browser、users、monitoring、batch、transfer、migration、vector、fulltext、sessions。
- 状态使用 Svelte store + `localStorage`，支持 mock 模式用于离线演示。

### 2.3 `gpui-kit`

- 提供 GPUI umbrella、主题、窗口、TitleBar、Sidebar、Dock、Table、Tree、Resizable、Dialog/Sheet、Notification、Chart、WebView、Assets 等组件。
- `examples/tig`、`examples/system_monitor` 给出了原生桌面应用的组织方式：命令定义、异步任务、多面板布局、主题切换。

## 3. 总体架构

新增 crate：`crates/linkrs-gpui`，作为 workspace member。它不修改服务端，只作为客户端壳层。

分层：

- Bootstrap：命令行参数、日志、gpui 初始化、主窗口创建。
- App State：全局连接状态、当前 space、主题、布局、打开的面板。
- Services：包装 `linkrs-client`，把异步结果投递到 GPUI 实体状态。
- Views：登录、主框架、查询控制台、Schema、图、数据浏览器、监控、管理页。
- Components：结果表格、属性面板、图容器、表单、空状态。
- Persistence：本地设置、最近连接、窗口几何。

数据流：

View 事件 -> Service 方法 -> `SessionManager` / `HttpClient` -> 服务端 -> 异步 Task -> 更新 GPUI Entity -> 重新渲染。

不引入第二套 HTTP 契约；新增端点优先补到 `linkrs-client`，GPUI 层只消费其类型。

## 4. Crate 与依赖

`crates/linkrs-gpui/Cargo.toml` 建议：

```toml
[package]
name = "linkrs-gpui"
version.workspace = true
edition.workspace = true

[dependencies]
gpui-kit = { path = "../gpui-kit/crates/kit", features = ["component", "assets", "tree-sitter-sql"] }
linkrs-client = { path = "../linkrs-client" }
linkrs-wire = { path = "../linkrs-wire" }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
anyhow.workspace = true
directories = "5"
```

GPUI 直接从当前项目 `crates/gpui-kit` 获取，不使用外部版本。可选特性 `gpui-fast` 与 `gpui-kit/gpui-fast` 保持一致开启。

Workspace `members` 增加 `crates/linkrs-gpui`。

## 5. 模块设计

```text
crates/linkrs-gpui/src/
  main.rs
  app.rs
  state.rs
  settings.rs
  commands.rs
  connection/
    login_view.rs
    session_store.rs
  services/
    connection_service.rs
    query_service.rs
    schema_service.rs
    graph_service.rs
    data_browser_service.rs
    monitoring_service.rs
    management_service.rs
  views/
    main_window.rs
    console.rs
    schema.rs
    graph.rs
    data_browser.rs
    monitoring.rs
    users.rs
    batch.rs
    transfer.rs
    migration.rs
    vector.rs
    fulltext.rs
  components/
    result_table.rs
    property_panel.rs
    graph_host.rs
    loading.rs
  tasks.rs
```

- `app.rs`：应用入口动作、全局快捷键、主题初始化。
- `state.rs`：`AppState` 实体，持有连接、当前 space、打开的标签页、通知队列。
- `services/*`：把 `linkrs-client` 的同步/异步调用转换为 GPUI 可消费的 `Task`。
- `views/*`：每个功能域一个视图实体，内部使用 GPUI 组件组合。
- `tasks.rs`：统一封装 `cx.spawn`、错误转通知、主线程状态更新。

## 6. 功能映射

| Web Studio 路由 | GPUI 视图 | 主要组件 |
|---|---|---|
| login | `connection/login_view` | Form、Input、Button |
| console | `views/console` | 编辑器、Toolbar、Table、Sidebar |
| schema | `views/schema` | Tree、Resizable、Dialog |
| graph | `views/graph` | WebView(Cytoscape) 或 Canvas |
| data-browser | `views/data_browser` | Table、Pagination、FilterPanel |
| monitoring | `views/monitoring` | Chart、Table |
| users | `views/users` | Table、Sheet |
| batch | `views/batch` | Table、Progress |
| transfer | `views/transfer` | File picker、Progress |
| migration | `views/migration` | Form、Progress list |
| vector | `views/vector` | Table、Form |
| fulltext | `views/fulltext` | Table、Button |
| sessions | `views/sessions` | Table |

## 7. 服务层适配

- `ConnectionService` 持有 `Arc<Mutex<SessionManager>>`，提供 `connect`、`disconnect`、`switch_space`、`health_check`。
- `QueryService` 提供 `execute`、`execute_batch`、`explain`、`validate`、`open_cursor`、流式查询。
- 对 `linkrs-client` 未覆盖的端点，优先在 `linkrs-client::HttpClient` 增加强类型方法；GPUI 层不直接拼 URL。如短期无法补齐，可使用 `HttpClient::inner()` + `base_url()`，但必须在服务层封装，避免视图层触碰 HTTP 细节。
- 错误统一映射为 `ClientError`，视图层只处理用户可读消息与通知。
- 会话失效时清理本地 session id，并回到登录视图。

## 8. UI 与交互设计

- 主窗口使用 `TitleBar::window_options()` 与自定义 TitleBar，支持最小尺寸。
- 左侧导航使用 Sidebar，主区域使用 Dock/Tab 容器，支持多标签与面板拆分。
- 底部状态栏显示连接状态、当前 space、最近查询耗时。
- 主题跟随系统，支持手动 light/dark，使用 gpui-kit Theme。
- 查询控制台：
  - SQL/GQL 编辑器使用 gpui-kit Input/TextArea 或 tree-sitter 高亮；
  - 结果使用 Table，支持复制、导出 CSV/JSON；
  - 历史与收藏使用侧栏列表；
  - 流式结果逐步追加行。
- 图可视化：
  - 第一阶段使用 WebView 嵌入现有 Cytoscape 页面或独立 HTML；
  - 后续可实现原生 Canvas 布局。
- 数据浏览器：
  - 分页表格、条件过滤、编辑预览弹窗。
- 监控：
  - Chart 展示 QPS/延迟/资源，Table 展示慢查询。

## 9. 异步与流式

- 所有网络调用通过 gpui 的 `cx.spawn` 或后台 Task 执行，避免阻塞 UI。
- 流式查询使用 `HttpClient::execute_query_stream`，在后台任务中循环读取 SSE，将行追加到结果实体；UI 通过状态更新刷新。
- 迁移流式进度同理。
- 长任务显示 Spinner/Progress，并支持取消。

## 10. 本地持久化

- 设置文件放在系统配置目录，例如 `~/.config/linkrs/gpui.toml`，保存最近 host/port/username、主题、窗口几何、编辑器偏好。
- 不保存密码；会话 id 仅保存在内存，断开或过期后重新登录。
- 查询历史优先使用服务端接口，本地只缓存 UI 状态。
- 后续可接入 OS keychain 保存凭据。

## 11. 构建与运行

```shell
cargo build -p linkrs-gpui
cargo run -p linkrs-gpui
cargo run -p linkrs-gpui -- --host 127.0.0.1 --port 9758 --user root
```

平台说明：

- Linux：X11/Wayland 均由 gpui-kit 支持，Wayland 下注意 IME。
- macOS：使用原生 TitleBar 与菜单。
- Windows：使用原生窗口边框。

## 12. 实施路线

### 阶段一：MVP

- Bootstrap、登录、连接状态、主框架、查询控制台、基础 Schema 浏览、设置。
- 验收：能连接服务端、执行 GQL、查看表格结果与错误。

### 阶段二：数据与可视化

- 数据浏览器、图可视化(WebView)、监控图表、批量任务、导入导出。
- 验收：能浏览顶点/边、分页过滤、查看监控。

### 阶段三：管理与高级能力

- 用户管理、迁移、向量、全文、会话管理、流式查询、游标、变量。
- 验收：管理页可完成用户/索引/迁移操作。

### 阶段四：打磨

- 快捷键、主题、打包、崩溃恢复、性能优化。

## 13. 风险与取舍

- `gpui-kit` 绑定特定 GPUI pre-release snapshot，升级需要整体验证。
- Web Studio 功能多，完全对齐成本高，按阶段交付。
- 图可视化若完全原生实现成本高，第一阶段使用 WebView 降低风险。
- `linkrs-client` 端点覆盖不完整，需要持续补齐，避免 GPUI 层散落 HTTP 调用。
- 桌面端缺乏 mock 模式，初期直接依赖真实服务端。

## 14. 验收标准

- `cargo build -p linkrs-gpui` 通过。
- 应用可启动并登录。
- 查询控制台可执行查询并展示结果。
- Schema 浏览可列出 space/tag/edge。
- 监控页可拉取概览数据。
- 连接断开后 UI 正确回到登录状态。
