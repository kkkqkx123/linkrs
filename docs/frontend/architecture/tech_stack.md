# GraphDB 前端技术栈

**文档版本**: v2.0  
**创建日期**: 2026-03-29  
**最后更新**: 2026-04-15

> 本文档描述前端**实际采用**的技术栈。v1.0 曾按 React 生态规划（React 18 / Ant Design / Zustand / React Router），但工程实现阶段改为 Svelte 生态；本版已按 `frontend/package.json` 与 `frontend/src/**` 的真实代码全面修订，消除文档与代码的偏差。

---

## 1. 技术选型概述

| 类别 | 技术选择 | 版本 | 说明 |
|------|---------|------|------|
| **前端框架** | Svelte | ^5.56.8 | 基于 Runes（`$state`/`$props`/`$effect`）的响应式组件模型 |
| **开发语言** | TypeScript | ~6.0.2 | 类型安全，`svelte-check` + `tsc` 校验 |
| **构建工具** | Vite | ^8.2.0 | rolldown 内核，含 HMR 与生产构建优化 |
| **样式方案** | Tailwind CSS | ^4.3.3 | 通过 `@tailwindcss/vite` 插件接入，原子化类名 |
| **状态管理** | Svelte Store | 内置 | `writable` store（`$stores/*`），无需额外依赖 |
| **路由** | svelte-routing | ^2.13.0 | 声明式 `<Router>` / `<Route>` |
| **HTTP 客户端** | Axios | ^1.19.0 | 统一封装于 `$utils/http.ts`，含拦截器与 BigInt 解析 |
| **代码编辑器** | Monaco Editor | ^0.57.0 | Cypher 语法高亮 + 关键字/Schema 自动补全 |
| **图可视化** | Cytoscape.js | ^3.34.0 | 力导向/环形/网格/层级布局，样式与交互定制 |
| **国际化** | svelte-i18n | ^4.0.1 | `en` / `zh` 两套词条，`$t()` 取词 |
| **工具库** | clsx / lodash-es / dayjs / json-bigint | 见依赖清单 | 类名组合、工具函数、时间处理、大整数 JSON |

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
- 与 `@sveltejs/vite-plugin-svelte` 深度集成
- 生产构建支持 `manualChunks` 分块

**分块策略**（`vite.config.ts`）:
- `monaco-vendor` —— Monaco Editor（懒加载）
- `cytoscape-vendor` —— Cytoscape.js
- `utils-vendor` —— axios / lodash / dayjs / json-bigint

**开发代理**: `/v1` 与 `/api` 转发至 `http://localhost:9758`。

### 2.4 样式方案: Tailwind CSS 4

**选型理由**:
- 原子化类名，减少手写 CSS
- 原生支持暗色模式（`dark:` 前缀）
- 通过 `@tailwindcss/vite` 与构建流程集成

**暗色模式**: 由 `$stores/theme.ts` 的 `theme` 控制，切换 `document.documentElement` 的 `dark` class。

### 2.5 状态管理: Svelte Store

**选型理由**:
- 框架内置，零额外依赖
- 与组件订阅天然契合（`onMount` 内 `store.subscribe`）
- 类型友好

**现有 store**: `connection` / `console` / `schema` / `graph` / `dataBrowser` / `theme` / `notification`。

### 2.6 路由: svelte-routing

**路由结构**（`App.svelte`）:
- `/login` —— 登录页
- `/` —— 主布局（受 `ProtectedRoute` 保护）
  - `/console` —— 查询控制台
  - `/schema` —— Schema 管理（含 ER 关系图）
  - `/graph` —— 图可视化
  - `/data-browser` —— 数据浏览

### 2.7 HTTP 客户端: Axios

**封装**（`$utils/http.ts`）: 统一 `get`/`post`/`put`/`_delete`，请求拦截注入 `X-Session-ID`，响应拦截统一取 `data`，`transformResponse` 使用 `json-bigint` 处理大整数，401 触发登出跳转。

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

### 2.10 国际化: svelte-i18n

词条位于 `src/lib/i18n/locales/{en,zh}.json`（扁平 key），组件中通过 `$t('...')` 取词，`LanguageSwitcher` 组件切换语言。

---

## 3. 开发工具链

| 工具 | 用途 |
|------|------|
| svelte-check | Svelte + TypeScript 类型检查 |
| tsc | TypeScript 编译校验（`tsconfig.node.json`） |
| Vite | 开发服务器与生产构建 |

---

## 4. 依赖清单

### 4.1 生产依赖

```json
{
  "dependencies": {
    "axios": "^1.19.0",
    "clsx": "^2.1.1",
    "cytoscape": "^3.34.0",
    "dayjs": "^1.11.21",
    "json-bigint": "^1.0.0",
    "lodash-es": "^4.18.1",
    "monaco-editor": "^0.57.0",
    "svelte-i18n": "^4.0.1",
    "svelte-routing": "^2.13.0"
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
| 路由 | React Router v6 | svelte-routing | 与 Svelte 生态一致 |
| 国际化 | react-i18next | svelte-i18n | 与 Svelte 生态一致 |
| 查询编辑器 | Ant Design TextArea | Monaco Editor | 满足 Cypher 高亮与补全的完整体验 |

---

## 7. 参考文档

- [Svelte 官方文档](https://svelte.dev/)
- [TypeScript 官方文档](https://www.typescriptlang.org/)
- [Vite 官方文档](https://vite.dev/)
- [Tailwind CSS 文档](https://tailwindcss.com/)
- [svelte-routing 文档](https://github.com/EmilTholin/svelte-routing)
- [svelte-i18n 文档](https://github.com/kaisermann/svelte-i18n)
- [Monaco Editor 文档](https://microsoft.github.io/monaco-editor/)
- [Cytoscape.js 文档](https://js.cytoscape.org/)

---

**文档结束**
