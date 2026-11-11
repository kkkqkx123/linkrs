# linkrs-studio 前端改进设计方案

> 依据 2026-11 对 linkrs 仓库的前端评审结论（对照 Neo4j Browser、NebulaGraph Studio、pgAdmin 等数据库管理前端的通行做法），本方案把评审中的改进建议落为可执行的分阶段实施计划。本文是各阶段的总纲；实施过程中的事实修正一并列出。

## 0. 评审结论摘要（背景）

linkrs-studio（SvelteKit + Svelte 5 + TS + Tailwind 4，位于 `frontend/`）整体架构符合数据库管理前端的一般做法，工程化水准较高（OpenAPI 类型化客户端、mock 场景层、编译期 i18n 校验、Svelte 5 runes 全量使用）。评审识别出的主要差距：

1. 监控页仅有 10 秒轮询，无服务端推送；趋势面板无时间范围选择。
2. 无 E2E 测试；单元测试仅覆盖纯函数。
3. 执行计划（EXPLAIN）已有服务与按钮，但计划文本被塞进截断的表格单元格，无结构化视图，阶段耗时（`QueryStageTimings`）与 `plan_node_count` 元数据被丢弃；另发现 EditorPanel 存在重复的历史/收藏按钮渲染缺陷。
4. 图工作台的增量探索（点击展开邻居 + 撤销栈）**已存在**（`GraphWorkbench.svelte` 的 `expandNode`/`exploreStack`），此前评审误判为缺失；实际差距是若干硬编码英文字符串未走 i18n。
5. vite 无显式 vendor 分包，Monaco/Cytoscape 等重依赖仅靠隐式动态 import 分割。
6. 会话安全取舍（localStorage 存 session id）未文档化；`package.json` 中 `dompurify` override 的意图（传递依赖安全钉版）易被误解为 XSS 防护在用。
7. 多语句批量执行与多结果集**已存在**（`executeBatch` + `StatementResultEntry[]` + 流式 `statement_begin/end` 事件），此前评审误判为缺失；趋势图交互缺口由本方案阶段 1 的时间范围选择器补齐。

## 1. 后端能力边界（实施前核实的事实）

- SSE 先例：`GET /v1/migration/stream/{space}/{label}`（`crates/linkrs-server/src/http/handlers/migration_progress.rs`）使用 `axum::response::sse::Sse` + `KeepAlive` + `BroadcastStream`，utoipa 以 `content_type = "text/event-stream"` 注解。
- 统计路由：`crates/linkrs-server/src/http/router.rs` 的 `/statistics/*` 段；`overview.rs` 已聚合 system/database/latency/timeseries（`stats_manager.query_timeseries(300)`）。
- OpenAPI 契约由 utoipa 宏生成，`frontend/openapi.json` 为快照；`http/openapi.rs` 内含路由-注解一致性测试，`LINKRS_REFRESH_OPENAPI=1` 可刷新快照。前端类型经 `npm run gen:api` 由 `tools/openapi-codegen` 生成。
- `GET /v1/statistics/queries` 已支持 `from/to` 时间范围参数。
- 邻居查询 `GET /api/v1/graph/vertices/{vid}/neighbors`、执行计划 `POST /v1/query/explain`、画像 `GET /v1/statistics/queries/{trace_id}` 均已存在。

## 2. 阶段划分与验收标准

### 阶段 1 —— 监控实时推送与时间范围（跨栈，本方案核心）

**后端**：新增 `GET /v1/statistics/stream?interval_ms=2000` SSE 端点（`handlers/statistics/stream.rs`），复用 `overview.rs` 的聚合逻辑（抽公共 `collect_overview`），以 mpsc + ReceiverStream 定时推送 `event: snapshot`（data 为 `OverviewResponse` JSON）；`interval_ms` 夹紧到 [500, 30000]；同步 router 注册、utoipa 注解与 `frontend/openapi.json` 契约、前端生成类型。

**前端**：
- 新增通用 SSE 帧解析 `utils/sseFrames.ts`（`event:`/`data:` 帧切分，含 node:test 单测）与 `services/monitoringStream.ts`（raw fetch + `X-Session-ID` + Accept 头；mock 模式合成快照；AbortError 语义与 migrationStream 一致）。
- `stores/monitoring.ts` 增加 `streamStatus: off/connecting/live/unavailable`、`startStreaming/stopStreaming`；SSE 推 `overview`（含 system/database/timeseries），既有 10 秒轮询保留为兜底与其余面板的刷新通道；流连接失败自动降级标记 `unavailable`。
- 监控页头部增加实时徽标（live/unavailable）与时间范围选择（Live/5m/15m/1h）：`Live` 跟随流式推送，预设档位对 `overview.timeseries` 做客户端窗口切片（`utils/timeWindow.ts` 纯函数 + 单测），并把 `from/to` 传给 `statisticsService.queries()`。
- i18n：`monitoring.streamLive/streamUnavailable/streamConnecting/range*` 等键，en/zh 双语。

**验收**：`npm run check`/`test` 通过；`cargo check -p linkrs-server` 通过；mock 模式下监控页可看到 live 徽标与快照刷新；断流时自动回到轮询。

### 阶段 2 —— Playwright E2E（基于 mock 层）

- 引入 `@playwright/test`（devDependency）+ `playwright.config.ts`（`webServer` 启 `npm run dev:mock`，端口 3011）。
- 冒烟用例：登录 → 控制台执行查询出结果 → 结果视图切换 → Schema 标签导航 → 数据浏览器渲染 → 监控页渲染 + live 徽标；`?scenario=error` 下错误态可见。
- `package.json` 增加 `test:e2e` 脚本；README 补运行说明。

**验收**：`npx playwright test` 全绿（chromium）。

### 阶段 3 —— 执行计划面板结构化

- 将 `EditorPanel.svelte` 内联的计划表格抽为独立 `PlanPanel.svelte`：
  - 结构化树视图：解析后端 ASCII 计划表（`| id | name | deps | profiling_data | operator_info | output_var |`）为算子节点（`utils/planParse.ts` + 单测），按 `deps` 构树，可折叠，`est_rows` 徽标展示；
  - 原文视图：计划文本以 `<pre>` 等宽完整呈现（替换现有 truncate 单元格渲染）；
  - 元数据条：`plan_node_count` 与 `QueryStageTimings`（parse/plan/validate/optimize/execute ms）——`handleExplain` 保留 `outcome.stages/planNodeCount`。
- 修复 EditorPanel 中游离的重复历史/收藏按钮（`EditorPanel.svelte` 卡片内 L290-301 与 QueryToolbar 重复）。
- i18n：`console.planTree/planRaw/planNodes/planStagesMs` 等键。

**验收**：`npm run check`/`test` 通过；mock 的 explain 数据可在树/原文两视图切换。

### 阶段 4 —— 图工作台 i18n 补全

- `GraphWorkbench.svelte` 中硬编码英文（`Expanding…`、`Retry Expand`、`Layout:/Sync:` 性能角标）改走 i18n；`Detail` 后缀并入既有标题键。
- 探索功能本体（expand/undo/reset）已存在，不改行为。

**验收**：`npm run check` 的 i18n 目录校验通过，中文界面无残留英文。

### 阶段 5 —— vite 显式 vendor 分包

`vite.config.ts` 增加 `build.rollupOptions.output.manualChunks`：monaco、cytoscape、web-tree-sitter/grammar wasm、paraglide 运行时各自成 chunk，保证缓存稳定性；`build:analyze` 验证 chunk 边界。

**验收**：`npm run build` 产物中上述依赖独立成 chunk 且构建无警告。

### 阶段 6 —— 会话安全决策文档化

- `frontend/README.md` 增设 Security 一节：localStorage 存 session id 的取舍（本地单机部署威胁模型）、`X-Session-ID` 头设计、dompurify override 为传递依赖安全钉版（源码零使用，XSS 防护依赖 Svelte 默认转义）、后续迁移 httpOnly cookie 的条件。
- 本阶段为文档交付，不改代码行为。

### 阶段 7 —— 趋势交互与多语句（收尾核对）

- 趋势时间范围交互已由阶段 1 覆盖（范围选择器 + 窗口切片）；本阶段仅核对多语句能力已由 `executeBatch`/流式 `statement_begin/end` 提供，并在 README 功能清单中显式记述，避免再次误判。

## 3. 实施顺序与工程约束

- 顺序：1 → 2 → 3 → 4 → 5 → 6 → 7；阶段 1 为跨栈改动先行落地，阶段 2/3 可并行。
- 全程遵守 `AGENTS.md`：代码/注释/日志只用英文，不得引用计划编号；文档用中文；生产代码禁 `unwrap`；前端新增文案必须同时进 `messages/en.json` 与 `messages/zh.json`（`npm run check` 强校验）。
- 每阶段完成后跑前端 `npm run check`/`test`/`lint`，涉后端跑 `cargo check -p linkrs-server`；全部完成后做一次全量回归（含 `npm run build` 与 E2E）。

## 4. 实施结果记录（2026-11-11）

各阶段实际落地情况与验收结论，含两处基于实施时核实事实的方案修正：

| 阶段 | 状态 | 落地内容与验收 |
| --- | --- | --- |
| 1 监控实时推送 | ✅ 完成 | 后端新增 `GET /v1/statistics/stream` SSE 端点（`handlers/statistics/stream.rs`，复用抽出的 `collect_overview`，mpsc+ReceiverStream 定时推送，`interval_ms` 夹紧 500–30000ms，失败发 `error` 事件不断流）；路由/utoipa 注解/`frontend/openapi.json` 契约（`LINKRS_REFRESH_OPENAPI=1` 快照测试刷新）与 `schema.d.ts`（`npm run gen:api`）全链路同步；前端新增 `utils/sseFrames.ts` 通用 SSE 帧解析、`services/monitoringStream.ts`（真实流 + mock 合成快照 + error 场景拒绝连接）、monitoring store 增加 `streamStatus`/`startStreaming`/`stopStreaming`/`setTimeRange`（保留 10s 轮询兜底）；监控页新增实时徽标（live/connecting/unavailable）与时间范围选择器（Live/5m/15m/1h，`utils/timeWindow.ts` 客户端窗口切片 + `queries` 端点 `from/to`）。验收：`cargo +nightly check -p linkrs-server` 通过、`npm run check`/`test` 通过、E2E 断言 live 徽标与 error 场景下 "Live stream unavailable, polling" 回退均通过。 |
| 2 Playwright E2E | ✅ 完成 | `playwright.config.ts`（webServer 自启 `dev:mock` 于 :3011）+ `e2e/` 四个 spec（auth 三例、console 四例含结果视图切换与历史、schema/data-browser 两例、monitoring 三例含流徽标/范围/断流回退）共 11 例全绿（chromium，镜像 `PLAYWRIGHT_DOWNLOAD_HOST` 解决 CDN 不可达）；`package.json` 新增 `test:e2e`，README 补运行说明。注意：dev:mock 首次需 `npm --prefix crates/linkrs-grammar install`（tree-sitter CLI）。 |
| 3 执行计划面板 | ✅ 完成 | `utils/planParse.ts` 解析 ASCII 计划表（id/name/deps/profiling_data/operator_info/output_var）并按 deps 建树（9 个单测）；`PlanPanel.svelte` 提供 tree/raw 双视图（`<pre>` 完整呈现替代原截断单元格、可折叠算子树、解析失败回退提示）与元数据条（`plan_node_count` + 五阶段耗时）；`EditorPanel` 保留 explain 响应元数据并移除游离重复按钮缺陷。**修正**：`QueryStageTimings` 的值经核实已是毫秒（后端 `micros_to_millis`），非设计稿假设的纳秒，按 ms 直接展示。 |
| 4 图工作台 i18n | ✅ 完成 | 增补 `graph.expanding/detailNode/detailEdge/perfLayout/perfSync` 五键（en/zh），替换 `GraphWorkbench` 中硬编码英文；`Retry Expand` 复用既有键。增量探索本体（expand/undo/reset/重试）已存在，未改动行为。 |
| 5 vendor 分包 | ⚠️ 结论修正 | A/B 实测（增删 `manualChunks` 两次构建产物字节级一致）：本仓库使用 rolldown-vite，`manualChunks` 为**空操作**；monaco（3.3MB chunk）与 cytoscape 本就经动态 import 自然分割，SvelteKit immutable 目录的内容寻址哈希已保证缓存稳定。故**不引入无效配置**，原设计目标（缓存稳定性）经核实已由既有机制满足。 |
| 6 会话安全文档 | ✅ 完成 | `frontend/README.md` 新增 "Security notes"：localStorage 存 session id 的取舍与 httpOnly cookie 迁移条件、`X-Session-ID` 设计、dompurify 为传递依赖安全钉版（源码零使用、全库无 `{@html}`，XSS 依赖 Svelte 转义）、`filterExpression.ts` 的语法层注入中和。 |
| 7 趋势交互与多语句 | ✅ 完成 | 时间范围交互由阶段 1 覆盖；多语句批量执行与多结果集经核实**已存在**（`executeBatch`/`StatementResultEntry[]`/流式 `statement_begin/end` 事件），列入 README/本文档明确记述以防再次误判。 |

**全量回归**：`npm run check`（i18n 双语目录、design tokens、mock 覆盖 125/122 全过）、`npm run test` 69/69、`npm run test:e2e` 11/11、`npm run build` 通过；`cargo +nightly check -p linkrs-server` 通过；openapi 快照契约测试通过。存量问题（与本方案无关，未处理）：`batch/+page.svelte` 有一处先行存在的 lint 每块缺 key 警告。
