# linkrs-studio 前端体验改进方案（检索测试场 / 命令面板 / i18n / 首页仪表盘 / 图谱搜索 / a11y）

> 依据 2026-11-12 对 `frontend/`（linkrs-studio）全量代码走查结论编写。
> **各阶段已全部落地（见第 5-10 章"状态"标记与 11.4 落地记录）**；
> 第 2 章行号为改造前基线（`git diff HEAD` 可对照改动），落地后的新增文件清单见 11.4。

## 0. 文档目的与范围

对 linkrs-studio 前端做"最后一公里"体验改造，使其对齐 RAG / 代码分析类前端（Neo4j Browser、Nebula Studio、Dify、Kibana 等）的通行做法。范围仅限 `frontend/src`，不触碰后端与 OpenAPI 契约（`frontend/openapi.json`）。

**不在本期范围**（列为开放点，见第 11 章）：WebGL 大图渲染、示例数据集一键载入、组件级 e2e 测试、检索面板升格为独立 `/search` 一级页面。

## 1. 结论摘要

架构层（类型安全、mock 层、流式/游标/虚拟化、设计令牌、i18n 编译期检查）已达通行水准之上。本次改造聚焦 6 个产品化短板，全部可在纯前端完成：

| 阶段 | 改造点 | 涉及文件 |
|---|---|---|
| 1 | 检索测试场：space 下拉化 + 索引列表结构化 + score 格式化 | `VectorSearchPanel` / `FulltextSearchPanel` + 新增公共组件 |
| 2 | 命令面板 Ctrl+K | 新增 `CommandPalette.svelte` + 导航配置抽离 |
| 3 | i18n 硬编码修复 | `GraphWorkbench.svelte` + `messages/{en,zh}.json` |
| 4 | 首页导航卡片 → 真实指标仪表盘 | `routes/(app)/+page.svelte` |
| 5 | 图谱搜索命中清单 + 点击定位 | `GraphWorkbench.svelte` |
| 6 | a11y 修补 + 新增工具函数单测 | 导出菜单键盘导航、chip aria-pressed、`*.test.ts` |

## 2. 现状事实（file:line 引述）

2.1 `frontend/src/lib/components/business/VectorSearchPanel.svelte`
- L7 仅有 `currentSpace: string | null` props，索引管理区 L22 `let spaceId = $state(1)` 为**手填数字**，用户需自行猜测 space_id；
- L44 索引管理结果 `manageResultText = stringify(await fn())`，L334-336 直接以 `<pre>` 输出原始 JSON；
- L186-205 检索结果表格对所有列一视同仁，score 列无格式化、无排序提示；
- 已具备 text 模式（L82 `WITH text=`），此前"缺文本查询模式"的判断不成立，仅缺体验打磨。

2.2 `frontend/src/lib/components/business/FulltextSearchPanel.svelte`
- L20 `let spaceId = $state(1)` 同样手填数字；L26 `resultText` + L230-232 `<pre>` 原始 JSON。

2.3 空间数据源已可用：`services/schema.ts` L83-88 `spaces.list()`，`toSpace()` L30 返回 `{id, name, vid_type}`；`stores/schema.ts` L27 `spaces: Space[]` 已随连接加载。**前端不需要任何后端改动即可把手填 id 换成下拉。**

2.4 `openapi.json` `/v1/vector/indexes` GET 响应 `ListVectorIndexesResponse.indexes` 声明为 `string[]`，但 mock（`mock/handlers/management.ts` L146-161）返回对象数组（space_id/tag_name/field_name/vector_size/distance/status/point_count），且服务层注释自述"shapes vary by backend"（`services/vector.ts` L8-11）。→ 结构化渲染必须做**运行时形状分类**而非绑定单一 schema。

2.5 `frontend/src/lib/components/business/workbench/GraphWorkbench.svelte`
- L370-374 `{t('graph.title')}` 后硬编码 `Expanding…`；
- L732-735 标题拼接 `{detailType === 'node' ? t('graph.selectNode') : t('graph.selectEdge')} Detail`，"Detail" 为硬编码英文；
- L790 `Retry Expand` 硬编码；
- L422-446 导出下拉菜单无键盘导航（无 ArrowUp/Down、无 Escape 关闭、打开后焦点不移入菜单）；
- L502-509 / L517-523 过滤 chip 按钮用作开关但无 `aria-pressed`；
- L127-144 搜索命中只有 match/dimmed 两态，无命中清单与定位入口。

2.6 `frontend/src/routes/(app)/+page.svelte`：L1-69 全部为静态导航卡片，其中 L18-40 两张卡（console/graph）均 `goto('/console')`，未消费任何统计接口；而后端统计能力完备（`services/statistics.ts` L60-67 `database()` 返回 spaces/storage/performance/search 四组真实指标）。

2.7 `frontend/src/lib/components/layout/Sidebar.svelte` L130-210：菜单项定义内嵌于组件，命令面板无法复用；L26-44 侧栏宽度逻辑与之耦合，不受影响。

2.8 全局快捷键现状：`utils/keymap.ts` 仅 console 页内生效（Ctrl+Enter/Ctrl+L/Ctrl+H + 单键 1/2/3），`routes/(app)/+layout.svelte` 无全局键盘层；全库无 Ctrl+K / palette 实现。

2.9 测试现状：`src/lib/utils` 下仅 7 个 `*.test.ts`（autoRoute/cypherTemplates/migrationEvents/parse/sseParser/virtualWindow），无组件测试；新增的纯函数需按 `autoRoute.test.ts` L1-2 的 `node:test` 模式补测。

2.10 项目约定（`AGENTS.md`）：代码/注释一律英文、文档一律中文；代码注释禁止引用阶段编号。→ 所有新增代码不含中文、不含 P1/phase 等标识。

## 3. 差距分析（对照通行做法）

| 通行做法 | 现状 | 差距等级 |
|---|---|---|
| 检索试查页：space 用结构化选择器、索引列表表格化、score 数值格式化 | 手填数字 id + 原始 JSON pre + 无格式化 | 高 |
| 全局命令面板（Ctrl+K）快速跳页/执行动作 | 无 | 中 |
| i18n 零硬编码（自家 README 承诺缺 key fail check） | 3 处硬编码英文 | 低（但违背自身规范） |
| 首页 = 真实数据仪表盘（顶点/边数、延迟、引擎健康） | 纯静态导航卡片 | 中 |
| 图内搜索给出命中清单 + 点击定位 fit | 仅高亮/变暗 | 中 |
| 自绘交互组件键盘可达 | 导出菜单不可键盘操作 | 中 |

## 4. 方案总览

- **阶段 1**：新增 `lib/components/common/StructuredPayload.svelte`（对 unknown 载荷做 表格 / chip 列表 / JSON 三态分类渲染）与 `lib/components/business/retrieval/SpaceIdSelect.svelte`（schemaStore.spaces 驱动的 id 下拉，回填当前 space）；两个检索面板接入，score 列经 `formatScore()`（新增于 `utils/metricsFormat.ts`）格式化。
- **阶段 2**：从 `Sidebar.svelte` 抽离 `lib/config/navigation.ts`（menuItems 单一来源）；新增 `utils/commandPalette.ts`（纯函数过滤）+ `components/common/CommandPalette.svelte`（dialog + listbox 键盘导航）；`stores/ui.ts` 增加 `commandPalette` 开关；`(app)/+layout.svelte` 挂全局 Ctrl+K；`Header.svelte` 加触发按钮。
- **阶段 3**：`GraphWorkbench.svelte` 三处硬编码改 `t()`，`messages/en.json`/`zh.json` 同步补 key。
- **阶段 4**：首页 onMount 拉取 `statisticsService.database()`（失败静默降级回导航卡片形态），渲染 spaces/storage/performance/search 四张真实指标卡 + 保留快捷入口。
- **阶段 5**：`GraphWorkbench.svelte` 搜索态下左侧浮动命中清单（上限 50 条），点击条目 → `cy.getElementById(id)` 选中并 fit 该节点。
- **阶段 6**：导出菜单补 role/键盘导航/焦点管理；chip 补 `aria-pressed`；为 `commandPalette.ts`、`structuredPayload` 分类函数、`formatScore` 补 node:test 单测。
- 所有阶段共享：代码英文注释、i18n 双语 key、不引用阶段编号。

## 5. 阶段 1：检索测试场升级

**状态：已落地**（详见各文件）

5.1 新增 `frontend/src/lib/utils/structuredPayload.ts`

```ts
export type PayloadShape = 'records' | 'strings' | 'primitive' | 'unknown';

/** Classify an opaque API payload for display: arrays of objects render as
 * tables, arrays of scalars render as chips, anything else falls back to
 * formatted JSON. Keeps index-management UIs readable without binding to
 * one backend response shape. */
export function classifyPayload(payload: unknown): {
	shape: PayloadShape;
	records: Record<string, unknown>[];
	values: string[];
}
```

5.2 新增 `frontend/src/lib/components/common/StructuredPayload.svelte`：按 classify 结果渲染 `<table class="ui-table">` / chip 列表 / `<pre class="ui-pre">`，附复制按钮。

5.3 新增 `frontend/src/lib/components/business/retrieval/SpaceIdSelect.svelte`：props `{ spaceId: number }`（bind），从 `schemaStore` 读取 `spaces`，选项 `name (id)`，默认选中 `currentSpace` 对应项。

5.4 `VectorSearchPanel.svelte` diff 要点：
- 删除 L22 `let spaceId = $state(1)` 手填 `<input type="number">`（L225-232），改挂 `<SpaceIdSelect bind:spaceId />`；
- L30 `manageResultText` 改为 `managePayload: unknown`，L44 `manageResultText = stringify(...)` 改 `managePayload = await fn()`，L334-336 `<pre>` 改 `<StructuredPayload payload={managePayload} />`；
- 结果表格 score 列（列名含 `score`/`similarity`/`distance`，不区分大小写）用 `formatScore()` 渲染，其余列不变。

5.5 `FulltextSearchPanel.svelte` 同样接入 SpaceIdSelect + StructuredPayload（L20、L26、L230-232 对应位置）。

5.6 `utils/metricsFormat.ts` 追加：

```ts
/** Score-like similarity values: fixed 4 decimals, locale aware. */
export function formatScore(value: unknown): string
```

## 6. 阶段 2：命令面板 Ctrl+K

**状态：已落地**

6.1 新增 `frontend/src/lib/config/navigation.ts`：把 `Sidebar.svelte` L111-210 的 `MenuItem[]` + `GROUP_ORDER` + `groupLabels` 原样迁出（含 `requiresUsers` 过滤交由消费方，从 `stores/connection` 读 `canManageUsers`）。`Sidebar.svelte` 改 import，渲染逻辑不变。

6.2 新增 `frontend/src/lib/utils/commandPalette.ts` 纯函数：

```ts
export interface PaletteCommand {
	id: string;
	label: () => string; // resolved i18n text at call time
	keywords?: () => string;
	run: () => void;
}
export function filterCommands(commands: PaletteCommand[], query: string): PaletteCommand[]
```
匹配策略：大小写不敏感子串 + 词首字母连拼（"db" 匹配 "Data Browser"）。

6.3 新增 `frontend/src/lib/components/common/CommandPalette.svelte`：
- `stores/ui.ts` 增加 `commandPaletteOpen` writable + `openCommandPalette()` / `closeCommandPalette()`；
- 命令源：全部导航路由（navigation.ts 派生）+ 主题切换（`toggleTheme`）+ 密度切换（`toggleDensity`）；
- 键盘：↑/↓ 移动 activeIndex、Enter 执行并关闭、Esc 关闭；打开时 focus 输入框；`role="dialog"` + `aria-modal` + 列表项 `role="option"` + `aria-selected`；
- 执行命令后 `goto(route)` 或调用动作并关闭面板。

6.4 `routes/(app)/+layout.svelte`：`onMount` 挂全局 keydown（Ctrl/Cmd+K → preventDefault → openCommandPalette），渲染 `<CommandPalette />`。

6.5 `Header.svelte`：在 LanguageSwitcher 左侧加触发按钮（搜索图标 + `Ctrl K` kbd 提示，title/aria-label 走 i18n）。

## 7. 阶段 3：i18n 硬编码修复

**状态：已落地**

`GraphWorkbench.svelte`：
- L371-373 `Expanding…` → `<span class="text-xs font-normal text-info">{t('graph.expanding')}</span>`
- L733-735 标题 → `{t(detailType === 'node' ? 'graph.nodeDetail' : 'graph.edgeDetail')}`（去掉拼接的 " Detail"）
- L790 `Retry Expand` → `{t('graph.retryExpand')}`

`messages/en.json` / `messages/zh.json` 同步新增（按字母序插入 `graph.*` 段）：

| key | en | zh |
|---|---|---|
| `graph.expanding` | Expanding… | 展开中… |
| `graph.nodeDetail` | Node Detail | 节点详情 |
| `graph.edgeDetail` | Edge Detail | 边详情 |
| `graph.retryExpand` | Retry Expand | 重试展开 |
| `graph.matchCount` | {count} matches | {count} 个匹配 |
| `graph.matchList` | Search matches | 搜索命中 |
| `graph.jumpToMatch` | Jump to match | 定位到该节点 |

（后三键为阶段 5 预置，避免两次改 messages 文件。）

## 8. 阶段 4：首页真实指标仪表盘

**状态：已落地**

`routes/(app)/+page.svelte` 重写：
- onMount 调 `statisticsService.database()`（`services/statistics.ts` L66-67），try/catch 容错；失败时**静默降级**：显示一行 `home.unavailable` 提示、保留快捷入口（不弹错误 toast——首页属浏览场景，请求失败由 HealthMonitor 与监控页呈现）；
- 加载中显示骨架卡（animate-pulse），不闪空白；
- 指标卡（复用 `formatCount`/`formatBytes`/`formatLatencyMs`/`formatQps`，`utils/metricsFormat.ts` L42-110）：数据规模（spaces.count / total_vertices / total_edges）、存储（total_size_bytes / index_size_bytes）、查询（total_queries / avg_latency_ms / queries_per_second）、检索（search.total_queries / total_errors）；
- 页头右侧"查看监控"按钮 goto('/monitoring')；原 5 张导航卡降级为底部"快捷入口"卡。

## 9. 阶段 5：图谱搜索命中清单

**状态：已落地**

`GraphWorkbench.svelte`：
- 把画布高亮 effect 里的匹配谓词抽为组件函数 `matchesSearch(node, query)`；`GraphNode` 无 label 字段（`types/graph.ts` L41-45），谓词取 id + 全部 property 值的小写子串匹配（画布 label 即 labelProperty 指向的属性值，两者等价）；
- `$derived` 计算 `searchMatches = { total, items }`：total 为全量命中数，items 截断 50 条（`SEARCH_MATCH_LIMIT`），条目含 `{id, tag}`；
- 画布左下角浮层（`absolute bottom-2 left-2`，可滚动 `max-h-64`，`aria-label` 用 `graph.matchList`）：标题 `graph.matchCount` 显示全量数；条目按钮显示 id + tag，点击 → `cy.getElementById(id)` 选中 + `cy.center(ele)` + 放大至 `Math.max(cy.zoom(), 1.5)`。

## 10. 阶段 6：a11y 修补 + 单测

**状态：已落地**

10.1 导出菜单（GraphWorkbench L410-446）：
- 容器 `role="menu"`、按钮 `role="menuitem"`；
- 打开时 focus 首个 menuitem；`onkeydown` 处理 ArrowDown/ArrowUp 在选项间循环、Escape 关闭并把焦点还给触发按钮；
- 触发按钮补 `aria-haspopup="menu"` + `aria-expanded`。

10.2 chip 开关补 `aria-pressed`：GraphWorkbench 过滤 chip（L502-509、L517-523）与 VectorSearchPanel 模式 chip（L133-144）。

10.3 新增单测（node:test，模式同 `autoRoute.test.ts`）：
- `utils/commandPalette.test.ts`：子串匹配、词首连拼匹配、关键词匹配、空查询全量返回、顺序保持（6 用例）；
- `utils/structuredPayload.test.ts`：records/strings/primitive/unknown 四分类 + 信封解包 + recordColumns 键序（8 用例）。
- ~~`utils/metricsFormat.test.ts`~~ **取消**：`metricsFormat.ts` 顶层 `import { currentLocale } from '$i18n'`，`$` 别名在 `node --test` 下不可解析（存量测试均只覆盖无别名依赖的纯模块），formatScore 由 svelte-check 覆盖类型正确性。详见 11.4 落地记录 D5。

## 11. 验证方式与待拍板开放点

11.1 验证方式（本地可执行，开发板/用户环境执行）：
```shell
cd frontend
npm install
npm run check        # i18n 双语完备性 + design tokens + svelte-check + mock 覆盖
npm run lint
npm run test         # 含新增 commandPalette/structuredPayload/formatScore 用例
npm run dev:mock     # ?scenario=slow|error|empty 逐场景走查检索面板/首页/图谱搜索
npm run build
```
验收要点：Ctrl+K 开面板并跳页；检索面板 space 下拉默认选中当前 space；`GET /v1/vector/indexes` mock（对象数组）与 OpenAPI 声明（string[]）两种形状均正确渲染；首页断开 mock error 场景时无报错、只隐藏指标区。

11.2 待拍板开放点（本次未做，需决策）：
| # | 开放点 | 选项 | 影响 |
|---|---|---|---|
| A1 | `/v1/vector/indexes` OpenAPI 契约声明 `string[]` 但 mock/实际返回对象数组 | 改契约（`VectorIndexInfo` schema）或改后端统一形状 | 不拍板则前端必须长期保持运行时形状分类（本期已按此容错） |
| A2 | 检索测试场是否升格为独立 `/search` 一级页面 | A) 独立页 + 命中结果"在图中查看"直通 GraphWorkbench；B) 维持 data-browser 标签页 | A 更符合 RAG 前端惯例，但与"数据浏览"的信息架构重组冲突 |
| A3 | 大图渲染策略 | A) Cytoscape 头部替换为 WebGL（sigma.js）双引擎按节点数自动切换；B) 仅 LOD 优化 | A 工作量大（新依赖 + 事件体系迁移），需单独立项 |
| A4 | 首页轮询 | 当前一次性拉取；监控页已有 30s 轮询（monitoringStore） | 首页是否接 monitoringStore.startPolling 共享数据，避免双份请求 |
| A5 | 示例数据集一键载入 | 需后端提供 seed 接口 | 前端无法独立完成 |
| A6 | 组件级 e2e（Playwright） | mock 层已具备，缺测试框架接入 | CI 成本与维护责任待定 |

11.3 文档维护约定：各阶段落地后更新本章上方对应"状态"标记；若 diff 与实际实现有出入，以实际代码为准修订第 5-10 章，而非标注"过期"。

11.4 落地记录（2026-11-12）

新增文件（8 个）：

| 文件 | 用途 |
|---|---|
| `frontend/src/lib/config/navigation.ts` | 菜单项单一来源（Sidebar 与命令面板共用） |
| `frontend/src/lib/utils/structuredPayload.ts` / `.test.ts` | 载荷形状分类 + 8 用例 |
| `frontend/src/lib/utils/commandPalette.ts` / `.test.ts` | 命令过滤纯函数 + 6 用例 |
| `frontend/src/lib/components/common/StructuredPayload.svelte` | 表格/chip/JSON 三态渲染 + 复制按钮 |
| `frontend/src/lib/components/common/CommandPalette.svelte` | Ctrl+K 面板（dialog/listbox + 键盘导航） |
| `frontend/src/lib/components/business/retrieval/SpaceIdSelect.svelte` | schemaStore 驱动的 space id 下拉 |

修改文件（12 个）：`messages/en.json`、`messages/zh.json`（各 +30 键：common.copy、graph.expanding/edgeDetail/jumpToMatch/matchCount/matchList/nodeDetail、home.* 14 键、palette.* 8 键；`graph.retryExpand` 已存在直接复用）、`VectorSearchPanel.svelte`、`FulltextSearchPanel.svelte`、`GraphWorkbench.svelte`、`Sidebar.svelte`、`Header.svelte`、`stores/ui.ts`、`utils/metricsFormat.ts`、`routes/(app)/+layout.svelte`、`routes/(app)/+page.svelte`。

与原稿的实际偏差（已同步修订第 5-10 章）：

- D1 阶段 4：`Promise.allSettled` 简化为 try/catch；失败显示一行 `home.unavailable` 提示（非完全隐藏），加载中显示骨架卡；"查看监控"为页头按钮而非每卡右上角。
- D2 阶段 5：`GraphNode` 无 label 字段，命中谓词取 id + property 值；清单显示 id + tag；`searchMatches` 返回 `{total, items}`（total 全量计数、items 截断 50），标题显示全量数。
- D3 阶段 2：全局 Ctrl+K 拆为独立 onMount 以保证解绑清理；面板键盘处理挂在 dialog 根节点（输入框与列表共享冒泡）。
- D4 阶段 1：`StructuredPayload` 对空数组渲染 EmptyState；分类器对信封对象（`indexes/items/results/points/data/list` 六个常见键）递归解包；结果表格首行加 `bg-primary-light` 高亮（top-1 命中）。
- D5 阶段 6：`metricsFormat.test.ts` 取消（见 10.3）；改为 `svelte-check` 覆盖类型正确性。
- D6 阶段 6：`role="menu"` / `role="dialog"` 容器补 `tabindex="-1"`（svelte-check 的 a11y 规则要求交互式 role 必须可聚焦）。

验证记录（沙箱执行，2026-11-12，`npm install` 及 `npm --prefix crates/linkrs-grammar install` 后）：

| 命令 | 结果 |
|---|---|
| `npm run check` | ✅ 0 errors / 0 warnings；i18n 双语键一致（各 639 keys）；design tokens、mock coverage 均 OK |
| `npm run test` | ✅ 59 用例全过（21 suites，含新增 14 用例） |
| `npm run lint` | ⚠️ 本次改动引入的错误已清零；仅剩**存量** `routes/(app)/batch/+page.svelte:418` 的 `svelte/require-each-key`（该文件未改动，属基线问题，见 11.5） |
| `npm run build` | ⚠️ SSR/客户端编译通过（`✓ built in 3.84s`），但预渲染阶段被**存量**工具链问题阻断：`src/params.ts` 报 `ERR_UNKNOWN_FILE_EXTENSION`（Node 无法原生加载路由参数匹配器 .ts）。已用 `git stash` 在未改动基线上复现同一失败，确认与本期改动无关（见 11.5） |

11.5 构建/校验的环境与存量说明

- **build 需要 tree-sitter CLI**：`frontend/vite/grammarWasm.ts` L27 优先取 `crates/linkrs-grammar/node_modules/.bin/tree-sitter`，缺失时 `spawn tree-sitter ENOENT` 导致构建失败。执行 `npm --prefix crates/linkrs-grammar install` 装好 pinned CLI 后，首次构建还会触发 `tree-sitter build --wasm` 联网从 GitHub 下载 binaryen 工具链（本沙箱网络下该下载挂起）。仓库既有 `isFresh()` 判定（L51-67）：当 `src/lib/assets/tree-sitter-linkrs.wasm` 比所有 grammar 源新时跳过重编译——本次验证以临时占位产物（该文件被 `frontend/.gitignore` L34 忽略、组件以 `?url` 引用不解析内容）跳过网络步骤，验证后已删除。**这是构建前置条件，与本期改动无关。**
- **build 的预渲染存量问题**：装好 CLI 并跳过 wasm 下载后，Vite/SvelteKit 的 SSR 与客户端产物均能编译成功（`✓ built in 3.84s`），但在 SPA 静态生成的预渲染阶段，Node 加载 `frontend/src/params.ts`（路由参数匹配器 `[tab=schemaTab]` 用）时抛 `ERR_UNKNOWN_FILE_EXTENSION: Unknown file extension ".ts"`。该文件为仓库既有文件、本期未改动；用 `git stash` 回退到未改动基线后重跑 `npm run build` 得到**同一个错误**（`BASELINE_EXIT=1`），确证为存量工具链/Node 版本问题，非本期引入。类型正确性由 `svelte-check`（0 errors）与完整编译产物覆盖。
- **lint 存量错误**：`routes/(app)/batch/+page.svelte:418` 的 `{#each}` 缺 key（`svelte/require-each-key`），在未改动的基线上已存在，本期不扩大改动范围去修无关文件，留待独立修复。
- **`sseParser.test.ts` 早期失败**：`sseParser.ts` L1 `import JSONBigint from 'json-bigint'`，未安装 node_modules 时 node:test 报 `ERR_MODULE_NOT_FOUND`；`npm install` 后 `npm run test` 全绿，与本次改动无关。
