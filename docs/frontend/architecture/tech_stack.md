# GraphDB 前端技术栈

**文档版本**: v3.0  
**创建日期**: 2026-03-29  
**最后更新**: 2026-10-05

> 本文档描述前端**实际采用**的技术栈。v1.0 曾按 React 生态规划（React 18 / Ant Design / Zustand / React Router），v2.0 记录了 Svelte 5 + Vite + `svelte-routing` 的实现；v3.0 反映迁移到 SvelteKit 3 与 Paraglide JS 后的真实代码。

---

## 1. 技术选型概述

| 类别 | 技术选择 | 版本 | 说明 |
|------|---------|------|------|
| **应用框架** | SvelteKit | ^3.0.0 | 文件路由、`load`、`adapter-static`；SPA 模式（`ssr = false`） |
| **组件模型** | Svelte | ^5.57.1 | 基于 Runes（`$state`/`$props`/`$effect`）的响应式模型 |
| **开发语言** | TypeScript | ~6.0.2 | 类型安全，`svelte-check` + `tsc` 校验 |
| **构建工具** | Vite | ^8.2.0 | rolldown 内核，含 HMR 与生产构建优化 |
| **样式方案** | Tailwind CSS | ^4.3.3 | 通过 `@tailwindcss/vite` 插件接入，原子化类名 |
| **状态管理** | Svelte Store | 内置 | `writable` store（`$stores/*`），无需额外依赖 |
| **HTTP 客户端** | openapi-fetch | ^0.17.0 | 按 OpenAPI 契约做类型校验，封装于 `$lib/api/client`，含 BigInt 解析与 `X-Session-ID` 注入 |
| **代码编辑器** | Monaco Editor | ^0.57.0 | Cypher 语法高亮 + 关键字/Schema 自动补全 |
| **图可视化** | Cytoscape.js | ^3.34.0 | 力导向/环形/网格/层级布局，样式与交互定制 |
| **国际化** | Paraglide JS | ^2.25.4 | 编译期生成消息函数，`en` / `zh` 两套词条，`t()` 取词 |
| **大整数 JSON** | json-bigint | ^1.0.0 | 保留超过 JS 安全整数范围的大整数 |

---

## 2. 核心技术详解

### 2.1 前端框架: Svelte 5（Runes）

**选型理由**:
- 编译期生成高效更新代码，运行时体量小
- Runes 提供细粒度响应式，无需手动依赖追踪
- 组件即文件（`.svelte`），模板与脚本同处

**关键用法**:
- `$state` 声明响应式局部状态
- `$props` 声明组件入参，`$bindable` 支持双向绑定
- `$effect` 处理副作用（订阅、DOM 操作、主题切换）
- `$derived` / `$derived.by` 声明派生值

**Store 结构示例**（本仓库 `$stores/*`）:
```typescript
// stores/graph.ts
import { writable } from 'svelte/store';

function createGraphStore() {
  const { subscribe, set, update } = writable<GraphState>({ /* ... */ });
  return {
    subscribe,
    setGraphData: (data: GraphData) => update(s => ({ ...s, graphData: data })),
    setLayout: (layout: LayoutType) => update(s => ({ ...s, layout })),
  };
}

export const graphStore = createGraphStore();
```

### 2.2 开发语言: TypeScript

**配置**: `tsconfig.app.json` 继承 `@tsconfig/svelte`，开启 `noEmit`、`allowJs`、`checkJs`，并配置路径别名（`$lib` / `$types` / `$utils` / `$services` / `$stores` / `$config` / `$components` / `$pages`）。

**校验命令**:
```shell
npm run check   # svelte-check + tsc
```

### 2.3 构建工具: Vite 8（rolldown）

**选型理由**:
- 极速冷启动（基于 ESM）
- 由 `@sveltejs/kit` 插件接管 SvelteKit 的构建与路由
- 词条由 `@inlang/paraglide-js` 插件编译，产物按路由自动分包

**开发代理**: `/v1` 与 `/api` 转发至 `http://localhost:9758`（mock 模式下无后端，全部请求在浏览器内被拦截）。

### 2.4 样式方案: Tailwind CSS 4

**选型理由**:
- 原子化类名，减少手写 CSS
- 原生支持暗色模式（`dark:` 前缀）
- 通过 `@tailwindcss/vite` 与构建流程集成

**暗色模式**: 由 `$stores/theme.ts` 的 `theme` 控制，切换 `document.documentElement` 的 `dark` class。

### 2.5 状态管理: Svelte Store

**选型理由**:
- 框架内置，零额外依赖
- `svelte/store` 在 Svelte 5 中仍是受支持 API；组件侧用 `fromStore()` 把 store 桥接进 runes

**现有 store**: `connection` / `console` / `schema` / `graph` / `dataBrowser` / `monitoring` / `theme` / `notification`。

### 2.6 路由: SvelteKit 文件路由

**渲染模式**: SPA。根 `+layout.ts` 导出 `ssr = false`，产物由 `adapter-static` 输出为单页应用（`fallback: index.html`）。会话凭据在 `localStorage`、SSE 与 Monaco / Cytoscape 均浏览器专属，服务端渲染只能产出加载骨架。

**路由结构**（`src/routes/`）:
- `/login` —— 登录页
- `(app)` —— 受鉴权守卫保护的主布局（`(app)/+layout.svelte`）
  - `/` —— 首页
  - `/console` —— 查询控制台
  - `/schema/spaces|edges|indexes|visualization` —— Schema 各子页，tab 即路由，深链接与刷新均可恢复
  - `/graph` —— 图可视化
  - `/data-browser` —— 数据浏览
  - `/monitoring` —— 监控指标
- `+error.svelte` —— 兜住未匹配路由（`goto()` 对未解析 URL 会 reject）

### 2.7 HTTP 客户端: openapi-fetch

**封装**（`$lib/api/client.ts`）: 路径、方法、参数与请求体按生成的 OpenAPI 契约（`schema.d.ts`）做类型校验；自定义 fetch 用 `json-bigint` 解析响应以保留大整数，请求拦截注入 `X-Session-ID`，401 清理会话并跳转登录页。

设置 `USE_MOCK` 时，`$lib/mock` 提供的同签名代理接管全部请求，无需后端即可运行完整界面。

### 2.8 查询编辑器: Monaco Editor

**实现**:
- 自定义 Cypher 语言（`$utils/monacoCypher.ts`）：Monarch tokenizer 提供注释/字符串/数字/关键字/函数/标点高亮。
- 自动补全：关键字、内置函数、当前 Space 的 Tag / EdgeType 名称与属性。
- 快捷键：`Ctrl/Cmd + Enter` 执行查询。
- 主题：跟随 `$stores/theme`，在 `vs` / `vs-dark` 间切换。
- **按需引入**（`$utils/monacoSetup.ts`）：仅引入 editor API 与必要 contribution，避免打包 json/css/html/typescript 语言服务及其 worker；Monaco 包在用户首次进入控制台时**懒加载**。

**封装组件**: `$components/common/CypherEditor.svelte`。

### 2.9 图可视化: Cytoscape.js

**能力**:
- 布局：力导向（cose）/ 环形 / 网格 / 层级
- 节点/边样式自定义（颜色、尺寸、标签字段）
- 交互：缩放、拖拽、选择、详情面板
- 导出：PNG / JSON
- 展开邻居、暗色模式样式
- 大数据保护：`MAX_GRAPH_NODES` / `MAX_GRAPH_EDGES` 截断统计

**封装组件**: `$components/common/CytoscapeCanvas.svelte`，配置与解析在 `$utils/cytoscapeConfig.ts`、`$utils/graphLayout.ts`。

### 2.10 国际化: Paraglide JS

词条位于 `frontend/messages/{en,zh}.json`，采用单层点号 key（如 `common.login`），由 Paraglide 编译为 `src/lib/paraglide` 下的类型化消息函数（生成物不入库）。组件从 `$i18n` 导入 `t()` 取词，key 在编译期校验；数据驱动场景用 `message()` 取得消息函数。语言检测与持久化由 Paraglide 的 `localStorage` 策略负责，`LanguageSwitcher` 调用 `setLocale()` 切换（切换会重载文档）。

---

## 3. 开发工具链

| 工具 | 用途 |
|------|------|
| svelte-check | Svelte + TypeScript 类型检查（`tsconfig.json`） |
| check-i18n | 校验 `en` / `zh` 词条 key 集合一致且非空 |
| node:test | `src/lib/utils` 下的纯函数单元测试 |
| Vite | 开发服务器与生产构建 |

---

## 4. 依赖清单

### 4.1 生产依赖

```json
{
  "dependencies": {
    "cytoscape": "^3.34.0",
    "json-bigint": "^1.0.0",
    "monaco-editor": "^0.57.0",
    "openapi-fetch": "^0.17.0"
  }
}
```

### 4.2 开发依赖

```json
{
  "devDependencies": {
    "@dvaji/vite-plugin-monaco-editor": "^2.0.0",
    "@sveltejs/vite-plugin-svelte": "^7.2.0",
    "@tailwindcss/vite": "^4.3.3",
    "@tsconfig/svelte": "^5.0.8",
    "@types/json-bigint": "^1.0.4",
    "@types/node": "^24.13.3",
    "svelte": "^5.56.8",
    "svelte-check": "^4.7.3",
    "tailwindcss": "^4.3.3",
    "typescript": "~6.0.2",
    "vite": "^8.2.0"
  }
}
```

---

## 5. 环境配置

### 5.1 环境变量

```bash
# .env.development
VITE_API_BASE_URL=http://localhost:9758
```

### 5.2 TypeScript 路径别名

`tsconfig.app.json` 与 `vite.config.ts` 保持一致的别名映射：`$lib` / `$types` / `$utils` / `$services` / `$stores` / `$config` / `$components` / `$pages`。

---

## 6. 架构决策记录

| 方面 | 规划（v1.0） | 实现（现状） | 决策原因 |
|------|-------------|-------------|----------|
| 框架 | React 18 | Svelte 5 | 更小的运行时、编译期优化、Runes 细粒度响应式 |
| UI 组件库 | Ant Design 5 | Tailwind CSS 4 | 原子化样式，避免重型组件库依赖 |
| 状态管理 | Zustand | Svelte Store | 框架内置，零额外依赖 |
| 路由 | React Router v6 | SvelteKit 文件路由 | 官方方案，支持 `load` 与 `hooks` |
| 国际化 | react-i18next | Paraglide JS | 编译期生成，无运行时 store |
| 查询编辑器 | Ant Design TextArea | Monaco Editor | 满足 Cypher 高亮与补全的完整体验 |

---

## 7. 参考文档

- [Svelte 官方文档](https://svelte.dev/)
- [TypeScript 官方文档](https://www.typescriptlang.org/)
- [Vite 官方文档](https://vite.dev/)
- [Tailwind CSS 文档](https://tailwindcss.com/)
- [SvelteKit 文档](https://svelte.dev/docs/kit)
- [Paraglide JS 文档](https://paraglidejs.com)
- [Monaco Editor 文档](https://microsoft.github.io/monaco-editor/)
- [Cytoscape.js 文档](https://js.cytoscape.org/)

---

**文档结束**
