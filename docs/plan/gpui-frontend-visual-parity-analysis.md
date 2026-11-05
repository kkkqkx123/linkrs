# GPUI 桌面客户端与 Web 前端视觉效果差异分析

> 分析范围：`frontend/`（SvelteKit 3 + Svelte 5 + Tailwind CSS 4）与 `crates/linkrs-gpui/`（GPUI + gpui-kit，独立 workspace）。
> 分析目标：逐项比对视觉表现，判断现有设计是否合理，并给出可落地的改进方向。
> 本文只做静态源码分析，不含运行期截图比对；结论均附源码位置。
> 第一至六节记录改动前的差异基线，当前代码状态与实施结果见第七节。

## 一、总体结论

两端的**品牌层已经一致**：主色绿、语义色、页面底色、卡片底色、恒定深色导航轨、遮罩透明度在 `frontend/src/app.css` 与 `crates/linkrs-gpui/src/theme.rs` 中基本逐值对应。GPUI 侧也明确把 Web 端当作视觉基准（`src/theme.rs:1-6`、`src/metrics.rs:1-4`，单测名 `light_palette_matches_frontend_tokens`）。

但**「品牌一致」不等于「视觉一致」**。实际差异集中在三个层次：

1. **框架层**：GPUI 是 `TitleBar + Sidebar + 顶栏 + Dock/Tab + 面板头 + StatusBar` 的六层桌面外壳，Web 是 `Sidebar + 顶栏 + 内容区` 的三层。GPUI 顶部连续出现两条 52px 带底边框的横条，且状态栏与顶栏健康胶囊重复展示连接状态与延迟。
2. **表面层**：Web 卡片普遍带 `shadow-sm`、边框用较浅的 `#e8e8e8`；GPUI 卡片只有 1px 边框、用较深的 `#dddddd`、几乎无阴影。再叠加 Web 的 0.15–0.4s 过渡与 8 个关键帧、GPUI 无自写动效，第一眼观感是「Web 柔和有层次」对「GPUI 扁平硬朗」。
3. **排版与组件层**：Web 有 `14px/22px` 基准字号行高、三级文本色、按 TDesign 分级的 `2/3/6/12px` 圆角；GPUI 沿用 gpui-kit 的 `16px` 基准、无行高设置、靠散落的 `text_size(px(11..20))` 手调，且缺少 `text-placeholder` 一级与全部 `*-light` / `*-text` 语义浅底色。

根因不是「某个数值写错了」，而是**当前并不存在真正的共享设计令牌体系**：`metrics.rs` 自称镜像 `app.css`，实际是人工抄写常量；`theme.rs` 的 Palette 是 CSS 变量的第二份手写副本，且只有 5 个浅色值的断言测试。已经出现语义错配与漂移（详见第三节）。

最有业务风险的单项差异是**监控图表配色语义不一致**：同一个 QPS 指标在 Web 是 `info` 蓝、在 GPUI 是 `primary` 绿；同一个延迟指标在 Web 是 `primary` 绿、在 GPUI 是 `warning` 橙。这属于跨端读图会误判的可用性问题，而非单纯美观问题。

## 二、视觉差异逐项对照

### 2.1 色彩：一致的部分

| 语义 | Web `app.css` | GPUI `theme.rs` | 结论 |
| --- | --- | --- | --- |
| 主色（浅/深） | `#299c68` / `#34b37a` | `0x299c68` / `0x34b37a` | 一致 |
| 主色 hover | `#238757` / `#4cc08d` | `0x238757` / `0x4cc08d` | 一致 |
| 主色浅底 | `#e8f5ee` / `#1d3a2c` | 同值 | 一致 |
| info / success / warning / error / alt | `#0052d9` / `#2ba471` / `#e37318` / `#d54941` / `#7c3aed`，深色 `#6b93f0` / `#3cba85` / `#ef8c3f` / `#e06059` / `#a78bfa` | 同值 | 一致 |
| 页面底 / 卡片面 / 次级面 | `#f4f6f8` / `#ffffff` / `#f3f3f3`，深色 `#0b0f17` / `#1c2333` / `#232b3f` | 同值 | 一致 |
| 边框 / 浅边框 | `#ddd` / `#e8e8e8`，深色 `#454f63` / `#2d3748` | 同值 | 一致（但用量不同，见 2.4） |
| 导航轨（浅深共用） | `#26343d` | `nav() = 0x26343d` | 一致 |
| 遮罩 | `rgba(0,0,0,.2)` / `.5` | `0x00000033` / `0x00000080` | 一致 |
| 圆角与 mono 字号 | `6px` 卡片 / `12px` 大圆角 | `radius=6` / `radius_lg=12` / `mono=13` | 一致 |

### 2.2 色彩：偏差与缺失

| 项 | Web | GPUI | 影响 |
| --- | --- | --- | --- |
| 三级文本 | `--color-text-placeholder`：亮 `rgba(0,0,0,.4)` / 暗 `#6b7280`，对应 `text-faint` | 无对应 token，侧栏用 `opacity(0.7)` / `opacity(0.6)` 现场模拟 | 占位符、脚注、禁用态的层级在 GPUI 不可控 |
| 文本色模型 | 亮色用 alpha `rgba(0,0,0,.9/.6/.4)`，可与任意底色融合 | `0x1a1a1a` / `0x666666` 不透明固定值 | 叠在非白表面时对比度不同 |
| 语义浅底 | 12 个精确 token：`info-light #ecf2fe`、`success-light #eaf7f0`、`warning-light #fff4e3`、`error-light #fff0f0`、`alt-light #f5f0ff`，深色各有独立值如 `error-light #3d1d1b` | 仅 `primary_light`；其余靠 `tone_color(...).opacity(0.12)` 现场计算（`src/components/ui/pill.rs:46-50`） | 徽章、告警、超阈值行的底色两端不同；深色模式偏差尤其大（12% 危险色叠 `#1c2333` 与 `#3d1d1b` 相差明显） |
| 语义深字 | `warning-text #9b5a0c`、`error-text #b13838`，深色 `#f0b26a` / `#ef9d98` | 直接用 `warning` / `danger` 主色作文字色 | 亮色底上文字对比度不足，语义色偏亮 |
| 导航辅助色 | `nav-muted #94a5ae`、`nav-card-border #47606b`、`nav-brand #fff` | 只覆写 7 个 sidebar token（`src/theme.rs:173-179`），缺 3 个 | 分组标题、卡片边框靠透明度替代 |
| 焦点环 | 明确 `box-shadow: 0 0 0 2px rgba(41,156,104,.2)` + 边框转主色（`app.css:228,306`） | 仅 `theme.colors.ring = primary`，由 gpui-kit 的 `focus_ring` 自行绘制 | 焦点反馈的宽度与浓度不一致 |

### 2.3 布局框架与垂直层级

| 项 | Web | GPUI | 差异说明 |
| --- | --- | --- | --- |
| 外壳 | `Sidebar + Header + main`（`(app)/+layout.svelte:45-59`） | `TitleBar + Sidebar + 顶栏 + DockArea + StatusBar`（`views/shell.rs:92-98`、`views/main_window.rs:1293-1356`） | GPUI 多 3 层桌面 chrome |
| 顶栏高度 | `58px`（`--comp-height-top`，`Header.svelte:33`） | `52px`（`metrics.rs:19`、`main_window.rs:1081`） | 差 6px；52 实际对应 Web 的 card-head 高度，属语义错配 |
| 顶栏内边距 | `px-7` = 28px | `px(20.)` = 20px | 差 8px |
| 内容区 | `py-8 px-7` = 32/28px，最大宽 1500px 居中 | `page_body` 同为 `28/32px`，`document_column` 最大宽 1500px | 主内容一致；但 Web 文档页 `.ui-page-doc` 另限 `72rem`(1152px)（`app.css:710-717`），GPUI 文档页用 1500px |
| 侧栏宽度 | 默认 **176px**，可拖拽 `76..300`（`Sidebar.svelte:30-32,244`） | 默认 **208px**，可拖拽 `180..300`（`metrics.rs:21-25`） | 默认差 32px，最小宽度差 104px（Web 可折叠成图标轨） |
| 侧栏声明令牌 | `--nav-width: 220px`（`app.css:108`） | `SIDEBAR_W: 208` | Web 的 220px 令牌全库无引用，是死令牌；三个数值互不相同 |
| 侧栏分组 | 3 组 11 项（explore / data / ops） | 4 组 16 项（query / data / observability / management） | 导航密度与首屏视觉节奏不同 |
| 导航选中态 | 3px 左边框 `--nav-accent #5bc28e` + `bg-nav-hover #3a4b57` + 白字（`app.css:915-922`） | `border_l_2()` + `theme.primary #299c68`（`main_window.rs:752-763`） | 宽度 2 vs 3，颜色 `#299c68` vs `#5bc28e`，背景由组件库决定 |
| 状态栏 | 无；连接状态放顶栏 `HealthMonitor` 圆点 + popover | 有 `StatusBar`：状态圆点 + 连接文案 + space + 延迟 + 版本（`main_window.rs:1231-1289`） | 桌面适配，但**与顶栏健康胶囊重复**（胶囊已含状态点 + 文案 + 延迟，`main_window.rs:1104-1129`） |
| 页面标题 | 内容区内的无边框大标题：24px/36px/600（`.ui-page-title`，`app.css:728-734`） | 面板头：52px 高、16px/600、`border_b_1()`、`px(28)`（`components/ui/page.rs:10-45`） | 两种标题语言；GPUI 在顶栏 52px 之下再来一条 52px 带边框横条 |
| 首页 | `/` 有 5 张入口卡 + 欢迎卡，`hover:scale-[1.02]` + `animate-fade-in`（`(app)/+page.svelte:7-90`） | 无首页，16 个 Dock 面板直接构成工作区 | 第一屏观感完全不同 |

### 2.4 卡片、边框、阴影、圆角

- Web `.ui-card`：`1px solid #e8e8e8`（浅边框）+ `6px` 圆角 + 无内建 padding，业务处普遍叠加 `shadow-sm`（`app.css:254-258`、`OverviewCards.svelte:33`、`+page.svelte:11`）。
- GPUI `card()`：`1px solid theme.border`（即较深的 `#dddddd`）+ `6px` 圆角，**无阴影**（`components/ui/card.rs:9-16`）；`theme.shadow = true` 已开启，但 `.shadow_lg()` 只用在登录卡（`connection/login_view.rs:415`）。
- 卡头 padding：Web `16px 20px` / `min-height 52px`（`app.css:262-273`）；GPUI `px(24)` / 固定 `h(52)`（`card.rs:29-49`）。
- 圆角分级：Web 控件 `3px`、卡片 `6px`、大容器 `12px`、`badge` 全圆；GPUI 以 `theme.radius=6` 为主轴，控件圆角由 gpui-kit 的 `radius_tokens` 派生（`radius.sm = radius/2 = 3`），但业务代码大量直接使用 `rounded(cx.theme().radius)` 或 `rounded(px(CARD_RADIUS))`，未形成与 Web 相同的分级语义。

### 2.5 组件级差异

| 组件 | Web | GPUI | 差异 |
| --- | --- | --- | --- |
| 表格（舒适密度） | `th/td` padding `16/24px`、14px/22px、表头字重 400、次级面底，行高约 54px（`app.css:483-503`） | 表头固定 `36px`、单元格 `px(12)`、13px、表头 12px MEDIUM、行高固定 `40px`（`ui/table.rs:189-238`） | GPUI 明显更紧凑、更像数据网格；表头字重与字号相反 |
| 表格（密集） | CSS 变量把 padding 改为 `4/8px`，另有 `.ui-table-dense` 12px 字（`app.css:120-124,516-522`） | `ROW_H_DENSE = 30px`（`metrics.rs:29`），仅切换行高与卡片 padding | 方向一致但作用范围不同：Web 全局 CSS 变量 + 按路由自动（`stores/ui.ts:64-68`），GPUI 只在共享 `SimpleTable` / `card` 生效，gpui-kit 原生 `DataTable`/`Tree` 不受控 |
| 按钮 | `min-height 32px`、`4/16px` padding、14px、`3px` 圆角、9 种语义变体、hover `bg-hover`、disabled `opacity .45`（`app.css:330-451`） | gpui-kit `Button`，`.primary()/.ghost()/.small()/.xsmall()`，`.selected()` 代替开关与分段控件 | 缺少 Web 的 `warn`/`danger`/`soft` 等浅底变体；`.selected()` 用主色，与 Web `.ui-seg` 的 `info` 蓝激活态不同 |
| 输入框 | 32px 高、`8/12px` padding、`3px` 圆角、绿色 2px 焦点环、placeholder 用 tertiary 色 | `Input::new()` + `.placeholder()` / `.masked()` / `.validate()` | 焦点环与 placeholder 层级不一致 |
| 徽章 | `.ui-badge`：9999px 圆角、1px 边框、14px 字、精确 `*-light` 底 + 语义字色（`app.css:454-462`） | `pill()`：全圆、`px(8) py(2)`、12px MEDIUM、`opacity(0.12)` 底、无边框（`ui/pill.rs:54-70`） | 字号、边框、底色算法三项不同 |
| 告警 | `.ui-alert` 6 种语义，浅底 + `color-mix` 40% 边 + 专用文字色（`app.css:544-581`） | `Alert::error(...).small()` + `AlertDialog`，无自定义浅底体系 | 语义覆盖范围不同 |
| 标签页 | `.ui-tab-active`：主色绿字 + 2px 绿色下划线（`app.css:652-676`） | gpui-kit `TabBar`：`tab_bar=muted` / `tab_active=surface` 的选中卡片式（`theme.rs:168-171`） | 两套 tab 视觉语言 |
| Toast | 右上角 `top-4 right-4`、`max-w-sm`、`rounded-lg shadow-lg p-4`、4 语义浅底 + 图标、`slide-in-right .3s`（`Toast.svelte:21-86`） | gpui-kit `Notification::success/error/info`，未做与 Web 对齐的定制 | 位置、形状、动效、语义色均未对齐 |
| 弹层 | Modal `w-96 p-6`、Drawer `24rem`，`shadow-lg` + `bg-mask` | `AlertDialog` + `Sheet .size(384px)`，命令面板 `w(560) mt(96)` | 宽度 384 一致；命令面板为桌面特有 |
| 空态 / 加载 | `.ui-empty` `min-height 12rem` + `.ui-spinner` 1.5rem 蓝色旋转 + Skeleton shimmer | `Inbox` 图标 + `min_h(120)` + 20px 主色 Spinner + 固定宽度骨架条 | 视觉重量不同，Spinner 颜色蓝 vs 绿 |
| 图标 | `@lucide/svelte`：`Keyboard / Table / Database / Link2 / LineChart / Package / Repeat / Users / PlugZap / Activity / RefreshCw / Route` | `IconName`（Lucide 风格）：`SquareTerminal / HardDrive / FolderOpen / Network / Cpu / Inbox / ExternalLink / User / Globe / Repeat / ArrowLeftRight / FileText` | 风格接近但**同一功能用了不同图标**：Console、Schema、Graph、Monitoring、DataBrowser、Transfer、Sessions 全部不同 |

### 2.6 数据可视化与编辑器

| 项 | Web | GPUI | 判断 |
| --- | --- | --- | --- |
| 监控趋势图 | 自绘 SVG 折线，600×120、`stroke-width 2`、无面积填充、无坐标轴/网格、虚线十字准线、legend 点击开关、各系列独立归一化（`TrendChart.svelte`） | gpui-kit `AreaChart`，渐变面积填充（`opacity .4 → .1`）、`tick_margin 15`、tooltip、卡头显示 18px 等宽当前值（`views/monitoring.rs:320-379`） | 两套图表语言 |
| 趋势图配色 | QPS `info` 蓝、错误 `error` 红、延迟 `primary` 绿（`TrendPanel.svelte:14-42`） | QPS `chart_1 = primary` 绿、延迟 `chart_3 = warning` 橙（`monitoring.rs:750,760`） | **同一指标颜色语义冲突**，跨端读图易误判，优先级最高 |
| KPI 卡 | 4 列 `gap 12px`、`padding 16px`、label 12/20、数值 **24px/36px/600** 且 `margin-top 16px`（`app.css:866-890`） | `min_w(160)`、`p(16)`、`gap_1()`、label 12px、数值 **20px SEMIBOLD**、hint 12px（`ui/kpi.rs:38-78`） | 数字视觉重量与卡间距不同 |
| 关系图 | Cytoscape.js：节点 30/40/50px、边 1/2/4px、选中 `#1890ff`、搜索匹配 `#faad14`、边类型 `#8b5cf6`、图例用 `info`/`alt` 两色 | compograph 原生画布：节点标签 12px、边标签 11px、8 色标签板（与 Web 一致）、5 种边类型色、选中节点品牌绿、左下 220px 半透明图例（`components/graph_host.rs`、`views/graph.rs:315-379`） | 标签色板一致，但选中色与边色不一致 |
| 查询编辑器 | Monaco 0.57，自定义 `linkrs-light/dark` token 色板（keyword 紫、edge 绿、function 青等）、行号、minimap | gpui-kit `Input/TextArea` + tree-sitter SQL 语法注册 | 平台能力差异，可接受；但高亮色板未对齐 |
| 图表种类 | 仅折线 + 进度条 + 密集表 | `AreaChart` + `Progress` + 阈值告警 + 骨架 | GPUI 可视表达略丰富 |

### 2.7 动效与反馈

- Web 有明确动效体系：`body` 背景/文字 `0.3s`、控件 `0.15s`、chip `0.2s`、主题按钮 `duration-300` 旋转缩放、首页卡片 `hover:scale-[1.02]`，以及 `fade-in / slide-up / slide-down / scale-in / shimmer / pulse-scale / ui-spin / slide-in-right` 八个关键帧（`app.css:949-1068`）；滚动条统一 6px。
- GPUI **无自写动画**（全 crate 无 `with_animation` / `transition` 调用），仅继承 gpui-kit 的 Spinner、Progress、滚动条淡入（idle 2s / enter 300ms / exit 500ms / 滑块 3→6→8px）。半透明浮层用 `popover.opacity(0.92)` 等模拟层次，无模糊与毛玻璃；渐变仅出现在 AreaChart 填充。
- 主题切换：Web 有 `.3s` 过渡，GPUI 是立即重绘（且切换后必须重新 `theme::apply(cx)`，见 `theme.rs:88-90`）。

### 2.8 登录页与文案

| 项 | Web | GPUI |
| --- | --- | --- |
| 卡片 | `380px` / `p-8`(32px) / `shadow-lg` / 无边框强调（`login/+page.svelte:67`） | `420px` / `p_6`(24px) / `gap_4` / `border_1` / `shadow_lg`（`login_view.rs:392-527`） |
| 标题 | 20px bold 居中 + 12px 副标题 | `text_xl()` SEMIBOLD + `text_sm()` 副标题 |
| 附加内容 | 页脚分隔线 | 卡内语言切换 `EN / 中文`、最近连接、硬编码 `Linkrs Desktop v{}` |
| i18n | Paraglide 编译期校验，en/zh 各 611 条 | `en.json`/`zh.json` + `t/tf`，运行时回退；存在硬编码英文（`Graph Database`、`No rows`、`NULL`、`ASC/DESC`、`qps`/`ms`）与未译条目 `common.desktopClient` |

中文译文长度普遍短于英文，GPUI 侧栏宽度固定 208px 而按钮文案宽度随语言变化，切换语言时存在挤压风险；Web 用流式布局吸收得更好。

### 2.9 差异归因分类

| 类别 | 典型差异 | 是否应当消除 |
| --- | --- | --- |
| 平台合理适配 | TitleBar、Dock/Tab、StatusBar、命令面板、原生图渲染、`Sheet` 抽屉、窗口最小尺寸、布局持久化 | 保留，但需去冗余并做语义映射 |
| 引擎能力限制 | Monaco vs tree-sitter Input、Cytoscape vs compograph、CSS keyframes vs GPUI 动画模型 | 不要求 1:1，只对齐可对齐的部分（色板、圆角、反馈时机） |
| 实现遗漏 | 卡片阴影、语义浅底 token、三级文本色、焦点环、Toast 定制、动效、图标映射、顶栏高度 | 应当消除 |
| 设计不一致 | 图表指标配色语义、表格密度、页面标题层级、导航选中色、tab 语言、`ui-seg` 强调色 | 应当消除或写入明确的差异白名单 |

## 三、现有设计合理性评估

### 3.1 合理的部分

1. **单向视觉基准**：`theme.rs` 头注释明确「Web Studio 是视觉参考」，`metrics.rs` 头注释说明尺寸镜像 `app.css`。双端各自定义会更快漂移，指定单一基准是正确决策。
2. **覆写语义 token 而非逐组件硬编码**：`Theme::update` 一次性改写 gpui-kit 的 `colors.*`（背景、表、列表、tab、sidebar、chart、selection 等约 60 个 token），使第三方组件库自动继承品牌色，改动成本低、覆盖面广。这是 GPUI 生态下的正确做法。
3. **共享尺寸常量集中化**：`metrics.rs` 把 `PAGE_PAD_X/Y`、`CARD_PAD`、`HEADER_H`、`SIDEBAR_W`、`ROW_H_*`、`SECTION_GAP` 收拢，避免了魔法数字散落在 16 个面板里；`components/ui/` 的 8 个展示原语（card / table / kpi / pill / page / overlay / loading / data_view）让面板间保持一致。
4. **恒定深色导航轨**：`nav()` 独立于浅深 Palette，与 Web 的「`.dark` 不覆盖 nav 系列」策略一致，品牌识别稳定。
5. **双主题 + 跟随系统**：`System/Light/Dark` 三态、持久化 `gpui.toml`、快捷键切换，与 Web 的 `matchMedia` + `html.dark` 语义等价。
6. **Tone 语义映射**：`Tone::from_status()` 把服务端状态词归一到 7 种色调（`ui/pill.rs:16-30`），避免了各面板自行判断颜色，方向正确。
7. **密度模式**：两端都提供舒适/密集两档，且 GPUI 持久化偏好，符合运维类界面诉求。
8. **桌面工作流增强**：Dock 布局版本化持久化、可拖拽侧栏、`Ctrl+K` 命令面板、状态栏连接上下文，都是 Web 端没有但桌端确实需要的能力。

### 3.2 不合理或有风险的部分

1. **「令牌镜像」实为手工抄写，且已发生语义错配**。`metrics.rs` 声明镜像 `app.css`，但：
   - `HEADER_H = 52` 对应的是 Web `.ui-card-head` 的 `min-height`，而非顶栏 `--comp-height-top: 58px`；
   - `SIDEBAR_W = 208` 既不等于 Web 实际默认 `176`，也不等于声明令牌 `220`；
   - `CONTENT_MAX_W = 1500` 对应 Web 的 `main` 上限，却被 GPUI 用在文档页列宽，而 Web 文档页是 `1152px`。
   
   同时 Web 自身的 `--nav-width: 220px` 全库无引用，是死令牌。**两端都不存在可靠的唯一真相源**，任何一侧调整都需要人工同步另一侧，且没有机制阻止漂移。

2. **一致性测试覆盖严重不足**。`theme.rs:202-228` 只有 3 个单测：浅色断言 5 个值（primary/page/surface/danger/info）、深色断言 3 个、nav 断言 1 个。`warning`、`success`、`alt`、`border`、全部 `text`、全部 `*-light`、全部深色语义色、全部 nav 辅助色均无断言。测试名 `light_palette_matches_frontend_tokens` 给人的保证远大于它实际提供的保证。

3. **语义色模型结构性缺失**。Web 的告警/徽章/超阈值行依赖 12 个 `*-light` 与 4 个 `*-text` 精确值；GPUI Palette 只有 `primary_light`，其余用 `tone.opacity(0.12)` 在组件层现场计算。这不仅是数值偏差：`.opacity()` 叠在 `popover` 上，深色模式下 `danger@12%` 叠 `#1c2333` 与 Web 的 `#3d1d1b` 是两种完全不同的颜色，且各组件可能各自选不同 alpha，无法保证一致。

4. **主题覆写依赖隐含调用契约**。`Theme::change` 会重新加载注册主题并丢弃所有覆写，因此每次切换后必须再调 `crate::theme::apply(cx)`（`theme.rs:88-90`、`app.rs:103-107`、`main_window.rs:692-694`）。目前靠注释和开发者记忆在 3 处维护，新增切换入口极易遗漏，且症状是全盘配色回退到组件库默认，不易定位。

5. **桌面 chrome 冗余且层级含混**。顶栏健康胶囊已含状态点 + 状态文案 + 延迟（`main_window.rs:1104-1129`），状态栏又含状态点 + 连接文案 + space + 延迟 + 版本（`main_window.rs:1231-1289`），状态与延迟重复两次；顶栏 52px 与面板头 52px 都是白底 + 底边框横条，连续堆叠既浪费垂直空间，又让「全局栏」与「面板栏」难以区分。Web 端没有这个问题（顶栏 58px + 内容区内无边框大标题）。

6. **排版没有基准轴**。GPUI 沿用 gpui-kit `font_size = 16px`、`.SystemUIFont`，全 crate 无 `line_height` 设置，靠 `text_size(px(11/12/13/14/16/18/20))` 和 `text_xs/sm/xl` 混用撑起全部文本。后果：同一句 14px 文案在两端物理尺寸与行距不同；中文回退字体链不受控；没有 `faint` 一级导致三级层次靠 `opacity` 拼凑。这是「视觉对齐」最难靠改常量补救的部分。

7. **图表指标配色语义冲突**，见 2.6。这是最应优先修复的功能性视觉问题。

8. **密度模式覆盖不全**。Web 靠 `:root[data-density='dense']` 全局改写 CSS 变量并自动按路由应用；GPUI 只在自写的 `SimpleTable` / `card` 上分支，gpui-kit 的 `DataTable`、`Tree`、`TabBar` 仍按组件库默认渲染，用户切换密度后界面呈现「一半变紧、一半不变」。

9. **反馈层未定制**。Toast 直接使用 gpui-kit `Notification` 默认样式，与 Web 右上角 4 语义浅底 + 滑入的 Toast 不同；主题切换无过渡；按钮/输入 hover 无过渡。桌面端不必复刻全部 CSS keyframes，但「状态变化没有反馈」会显得生硬。

10. **品牌资产自相矛盾**。`frontend/public/favicon.svg` 是紫色闪电 `#863bff`，与两端 UI 的品牌绿 `#299c68` 冲突；GPUI 侧栏用 `GalleryVerticalEnd` 绿色图标方块 + 硬编码副标题 `Graph Database`，Web 侧栏只有文字 `Linkrs` + 本地化副标题，品牌呈现方式不统一。

11. **首页策略未定义**。Web 有门户首页，GPUI 无对应物。这大概率是有意的桌面端取舍，但没有任何文档说明，后续评审会反复被当作「缺失」。

## 四、改进建议

按投入产出比排序。每条都给出可核对的落点，不引入新的重型依赖。

### 高优先级

1. **建立可机器校验的共享令牌层**
   - 把 `frontend/src/app.css` 的 `@theme` / `:root` / `.dark` 令牌抽成仓库级机器可读文件（如 `design/tokens.json`），Web 侧由构建脚本生成 CSS 变量，GPUI 侧用 `include_str!` 在编译期读取并生成 `Palette`。
   - 若暂不改造构建流程，至少把 `theme.rs` 的单测升级为「解析 `app.css`，逐项比对全部浅色/深色/nav/语义浅底 token」的一致性测试，并把 `metrics.rs` 的常量与 `app.css` 的 `:root` 常量做同样的比对测试。
   - 同步清理 Web 的死令牌 `--nav-width`，确定侧栏默认宽度的唯一真相（`176` 或 `220`），再让 GPUI 的 `SIDEBAR_W` 对齐。
   - 明确一份**平台差异白名单**（顶栏高度、状态栏、Dock/Tab、原生图渲染等允许不同，并注明理由），让「是适配还是 bug」可判定。

2. **补齐 GPUI 语义色与文本层级**
   - `Palette` 增加 `text_placeholder`、`info_light / success_light / warning_light / danger_light / alt_light`、`warning_text / error_text`、`nav_muted / nav_card_border / nav_brand`，取 Web 的精确值；在 `theme.rs` 的 `Theme::update` 中映射到对应 gpui-kit token（无对应项时新增应用级 helper）。
   - `ui/pill.rs:46-50` 的 `tone_background` 改为查精确 token，弃用 `.opacity(0.12)`；`Tone` 增加 `text_color` 的语义深字分支。
   - 修正 `text` / `text_muted` 的 alpha 语义，或显式记录在白名单中。

3. **统一卡片表面**
   - `ui/card.rs:13-14` 边框色改用 `border_light`（浅 `#e8e8e8`）；叠加与 `--shadow-sm` 等价的阴影（`theme.shadow` 已开启，登录卡已验证 `.shadow_*()` 可用）。
   - `card_head` 的 `px(CARD_PAD)` 改为 `px(20)` / `py(16)` 与 Web `.ui-card-head` 对齐，或统一改为两端一致的新值。

4. **建立 GPUI 排版基准**
   - 覆写 `theme.font_size = px(14.)` 并引入统一的 `text_body()` helper 表达 `14/22`，把散落的 `text_size(px(11..20))` 收敛为有限几档（caption 12/20、body 14/22、title 16/24、headline 24/36）。
   - 设置与 Web 等价的 CJK 回退字体链；补齐 `faint` 第三级文本色并替换侧栏/表格中的 `opacity()` 用法。
   - 若 gpui-kit 不支持显式行高，则在 helper 中用容器高度与 `items_center` 模拟，并在白名单中记录差异。

5. **统一监控图表配色语义**
   - 以 Web 为准：`QPS = info`、`错误 = error`、`延迟 = primary`；`views/monitoring.rs` 不再使用 `chart_1/chart_3` 作为指标色，改为显式语义色常量。
   - 面积填充、坐标轴、十字准线、当前值展示作为「引擎差异」写入白名单；若希望双端观感更近，可让 Web 的 `TrendChart` 增加淡面积填充。

6. **消除桌面 chrome 冗余**
   - 顶栏与状态栏去重：建议顶栏保留健康胶囊（状态 + 延迟），状态栏只保留 space、版本与当前面板上下文；或反过来移除胶囊，把健康信息收进状态栏。
   - 顶栏高度对齐 `--comp-height-top: 58px`（或两端统一到同一新值）；面板头去掉底边框、改为内容区内的 24px 标题行（`ui-page-title` 语义），避免两条等高的带边框横条。
   - 导航选中态改为 `3px + #5bc28e + bg-nav-hover #3a4b57`。

### 中优先级

7. **对齐表格密度**：确定一种基准（建议以 Web 的 `16/24px padding + 14px` 为准，把 GPUI `ROW_H_COMFORTABLE` 从 40 提到约 54；或若产品调性偏紧凑，则把 Web 表格改成固定 40px 行高），同时把表头字号/字重/水平 padding 统一。
8. **扩展密度模式覆盖面**：让 GPUI 的 `Density` 通过 Theme token 影响 gpui-kit `DataTable` / `Tree` / `TabBar`，而不只是自写组件；并像 Web 一样按面板类型给出默认密度。
9. **图标语义映射表**：为 16 个面板建立「功能 → Lucide 图标名」对照，优先统一 Console、Schema、Graph、Monitoring、DataBrowser、Transfer、Sessions 这 7 个当前不同的入口；对 gpui-kit 缺失的图标补充资源。
10. **统一控件状态语言**：按钮/输入 `3px` 圆角、绿色 `2px rgba(41,156,104,.2)` 焦点环、disabled 透明度；`.ui-seg` / `.ui-tab` 的强调色统一为主色绿；Pill 补齐 1px 边框与 14px 字号以贴近 `.ui-badge`；补 Web 的 `warn` / `danger` / `soft` 浅底按钮变体。
11. **补关键反馈动效**：主题切换过渡、面板进入淡入、Toast 右上角滑入、按钮/输入 hover 过渡。不必复刻全部 8 个 keyframes；桌面端克制为宜。
12. **定制 GPUI Toast**：`Notification` 使用 4 语义浅底 + 边框 + Lucide 图标 + 右上角定位，与 `Toast.svelte` 对齐。
13. **统一弹层与登录页**：Modal 384px 已一致；登录卡统一为 `380px / 32px padding / 居中 20px 标题`，语言切换与版本脚注的位置与 Web 对齐（或明确列白名单）。
14. **统一图可视化色板**：选中节点色、边类型色、图例色在 `components/graph_host.rs` 与 `frontend/src/lib/utils/schemaGraph.ts` / `cytoscapeConfig.ts` 中取同一组值；保留各自渲染引擎。
15. **封装主题切换入口**：把 `Theme::change` + `theme::apply` 收进单一函数，删除 3 处重复调用；补一个「切换后 token 仍为品牌色」的回归测试，消除隐含契约。

### 低优先级

16. **品牌资产**：`favicon.svg` 改为品牌绿或明确说明紫色是独立产品标识；统一两端侧栏的品牌区（图标方块 + 标题 + 副标题）与副标题本地化。
17. **i18n 清理**：移除 GPUI 源码中的硬编码英文、补齐 `common.desktopClient` 等未译条目；增加与 Web `messages/*.json` 的 key 对齐检查，降低中英文切换时的布局挤压。
18. **明确首页策略**：在视觉规范中写明「桌面端以 Dock 工作区替代门户首页」，或提供等价的 Dock 欢迎面板，避免被反复当作缺失。
19. **建立视觉回归基线**：为两端各截取「浅色/深色 × 登录、Console、Monitoring、Schema、Data Browser、Graph」的对照截图，形成人工 diff 清单；令牌一致性测试与截图清单共同作为后续改动的验收依据。

## 五、验证方式

- 令牌层：`cargo test -p linkrs-gpui` 中的新增一致性测试通过；`cd frontend && npm run check` 通过。
- 实现层：逐项核对本文第二、四节的表格，每条改进给出改动文件与前后数值；桌面端手动检查浅/深 × 舒适/密集 × 中/英共 8 种组合，重点看侧栏挤压、表格行高、语义徽章底色、图表配色。
- 截图层：按上述视觉回归基线做双端浅深色对照，确认「品牌一致」且「差异均在白名单内」。

## 六、关键证据索引

| 主题 | 位置 |
| --- | --- |
| Web 设计令牌与组件类 | `frontend/src/app.css:9-58`（浅色）、`:76-118`（尺寸/阴影）、`:126-155`（深色）、`:162-241`（字体/控件）、`:247-947`（组件类）、`:949-1068`（动效） |
| Web 布局 | `frontend/src/routes/(app)/+layout.svelte:45-59`、`frontend/src/lib/components/layout/Header.svelte:31-88`、`Sidebar.svelte:30-32,244`、`PageHeader.svelte:11-23`、`(app)/+page.svelte:7-90` |
| Web 图表与图 | `frontend/src/lib/components/common/TrendChart.svelte`、`business/monitoring/TrendPanel.svelte:14-42`、`lib/utils/schemaGraph.ts:11-22`、`lib/utils/cytoscapeConfig.ts:156-218` |
| GPUI 色彩 | `crates/linkrs-gpui/src/theme.rs:1-6,37-85,101-199,202-228` |
| GPUI 尺寸 | `crates/linkrs-gpui/src/metrics.rs:6-35` |
| GPUI 框架 | `crates/linkrs-gpui/src/views/shell.rs:92-98`、`views/main_window.rs:752-763,1044-1130,1231-1289,1293-1360` |
| GPUI 组件原语 | `crates/linkrs-gpui/src/components/ui/card.rs:9-64`、`ui/page.rs:10-91`、`ui/table.rs:154-246`、`ui/pill.rs:16-70`、`ui/kpi.rs:38-78` |
| GPUI 图表与图 | `crates/linkrs-gpui/src/views/monitoring.rs:320-379`、`components/graph_host.rs:23-77`、`views/graph.rs:315-379` |
| GPUI 登录 | `crates/linkrs-gpui/src/connection/login_view.rs:392-527` |
| 既有对比文档 | `docs/plan/gpui_client_design.md:178-195`、`docs/plan/gpui_client_implementation_plan.md:148-152`、`docs/plan/gpui-frontend-parity-plan.md`（功能对等，非视觉） |

## 七、实施状态

> 第一至六节是改动前的分析基线，数值用于说明差异来源；当前代码状态以本节为准。

### 7.1 已完成的改动

**共享令牌与一致性校验**

- 新增 `design/tokens.json`，从 `app.css` 的 `@theme` / `@theme inline` / `:root` / `:root[data-density='dense']` / `.dark` 全量提取 104 个变量，作为机器可读的唯一真相源。
- 新增 `frontend/scripts/check-design-tokens.mjs` 与 `npm run check:tokens`，并纳入 `npm run check`；脚本按块比对 key 集合与值，任一侧新增或改动变量而未同步即失败。
- 删除 `app.css` 中无任何引用的 `--nav-width: 220px` 死令牌。
- 新增 `crates/linkrs-gpui/src/design_tokens.rs`：`include_str!` 读取同一 JSON，解析 hex / rgba / 像素度量 / `--shadow-sm` 双层阴影；新增 7 个单测覆盖浅色、深色、导航轨共享、alpha 语义、尺寸常量与阴影层。
- `theme.rs` 的 Palette 不再手写色值，改由令牌生成；`metrics.rs` 修正语义错配：`HEADER_H` 52→58、`SIDEBAR_W` 208→176、`ROW_H_COMFORTABLE` 40→54、`HEADER_ROW_H` 36→54，并由单测逐项比对。

**色彩与表面**

- 补齐语义浅底与深字：`pill` 的状态底色改用精确 `*-light` / `*-text` token（含深色模式独立值），弃用 `.opacity(0.12)` 现场计算。
- 卡片边框改用 `--color-border-light`，叠加由 `--shadow-sm` 解析出的双层阴影（深色自动取更强 alpha）；KPI 卡复用同一表面。
- 卡片头水平 padding 24→20；控件圆角 6→3（卡片仍为 6）；通知宽度设为 384px。
- 排版基准：`font_size` 16→14、补齐 CJK 回退字体链、`tab_active_foreground` 设为主色。

**布局与桌面外壳**

- 顶栏高度 58px、左右 padding 28px；面板头去掉底边框、标题 16→24px，消除两条等高带边框横条。
- 状态栏去掉与顶栏健康胶囊重复的状态点、连接文案与延迟，只保留空间上下文与版本。
- 导航选中态由 2px `#299c68` 改为 3px `#5bc28e`；登录卡 420→380px、padding 24→32px、标题 20px 居中。

**表格、图表、图标与编辑器**

- 表格单元格 13→14px、表头常规字重、水平 padding 24/8px；`table_size` 改用 `Size::Size(px(...))`，使 gpui-kit 原生 `DataTable` 与自研表格共用 54/30 两档行高。
- KPI 数值 20→24px 并补 16px 上间距；监控 QPS 改 `info` 蓝、延迟改 `primary` 绿，与 Web 的 `TrendPanel` 一致。
- 控制台 `TabBar` 改用 `.underline()`（2px 主色底线）；图节点选中色改 `#1890ff`。
- 侧栏与命令面板的 16 个入口统一为 Web 的 Lucide 图标名（Keyboard / Database / Table / Link2 / ChartLine / Users / Package / Repeat / Route / PlugZap / Activity / RefreshCw），`app.rs` 注册 `AllAssets` 保证图标资源可加载。

**品牌与文案**

- `favicon.svg` 的紫/蓝渐变换为品牌绿系四色；`common.desktopClient` 补齐中文。
- 新增 `common.graphDatabase`、`login.desktopVersion`、`graph.nodeEdgeSummary` 三个词条，移除侧栏副标题、登录页版本、图统计三处硬编码英文。
- 主题切换收敛到 `theme::set_mode` 单一入口，消除「切换后必须手动 `apply`」的隐含契约。

### 7.2 平台差异白名单

以下差异经评估属于合理适配，不再列为待修复项：

| 差异 | 理由 |
| --- | --- |
| TitleBar / Dock Tab / StatusBar / Ctrl+K 命令面板 | 桌面多面板工作流所需，Web 端无对应概念 |
| Monaco vs tree-sitter `Input`、Cytoscape vs compograph | 渲染引擎能力差异，色板已尽量对齐 |
| 面积图填充 vs 自绘 SVG 折线、坐标轴与十字准线 | 图表引擎差异；指标配色语义已统一 |
| 侧栏展开态最小宽度 GPUI 180 / Web 76 | GPUI 另有专用图标折叠态，Web 以极窄宽度承担该职责 |
| 动效实现方式 | GPUI 不自写 CSS keyframes，使用组件库内置的 Notification 进出、Tab 文字淡入、TabBar spring 指示器与滚动条淡入 |
| 门户首页 | 桌面端以 Dock 工作区替代 Web 的入口卡首页，属于有意取舍 |

### 7.3 尚未实施及原因

- **Toast 语义浅底与自定义图标**：gpui-kit `Notification` 的 render 固定使用 `tokens.popover` 作背景，且 `with_type` 会覆盖自定义图标；完整对齐需替换 33 处调用或自建 toast 层。当前位置（TopRight）、宽度（384px）与阴影已与 Web 对齐。
- **按钮 9 种语义浅底变体、绿色焦点环浓度**：需逐处包 `ButtonCustomVariant`，且 Web 焦点环为 CSS `box-shadow`，GPUI 由组件库按 `ring` token 绘制；投入产出比低。
- **Web 侧按路由自动密度**：GPUI 仍由用户手动切换并持久化，两端行为刻意保持差异。
- **`zh.json` 剩余占位式机翻与硬编码英文**：约 100 条词条与 20 余处 `"No rows"` / `"NULL"` / `"ASC"` 等，涉及大量文案校订，建议单独处理。
- **双端浅/深色截图对照基线**：需在可运行 GUI 的环境采集，本次未执行。

### 7.4 验证结果

| 检查 | 结果 |
| --- | --- |
| `cargo check`（GPUI 独立 workspace） | 通过，无 warning |
| `cargo test` | 57 passed / 0 failed |
| `node scripts/check-design-tokens.mjs` | 通过，104 variables across 4 CSS blocks |
| `node scripts/check-i18n.mjs` | 通过，611 messages / 2 locales |
| `rustfmt`（本次修改的 16 个 Rust 文件） | 已格式化 |
| `npm run check` / `npm run build` | 未执行：`frontend/node_modules` 未安装 |
| 双端截图比对 | 未执行：需运行期环境与 GUI |
