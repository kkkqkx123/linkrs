# GraphDB 前端目录结构设计

**文档版本**: v3.0  
**创建日期**: 2026-03-29  
**最后更新**: 2026-10-05

> 本文档描述前端**实际采用**的目录结构。v1.0 曾按 React 生态规划（`index.tsx` + `index.module.less` 组件目录、`hooks/` 自定义 Hook、`locales/` 与 `styles/` 目录），v2.0 记录了 Svelte 5 + Vite + `svelte-routing` 的实现；v3.0 反映迁移到 SvelteKit 3 与 Paraglide JS 后的真实代码。

---

## 1. 设计原则

### 1.1 核心原则

1. **模块化**: 按功能模块组织代码，高内聚低耦合
2. **可维护性**: 清晰的目录结构，便于定位和修改代码
3. **可扩展性**: 预留扩展空间，支持新功能快速添加
4. **一致性**: 单一文件即单一职责，Svelte 组件按 `.svelte` 单位组织

### 1.2 与规划版本的差异

| 方面 | v1.0 规划（React） | 实现（Svelte） |
|------|-------------------|----------------|
| 组件组织 | `ComponentName/index.tsx` 目录 | 单文件 `ComponentName.svelte` |
| 页面组织 | 路由配置集中声明 | SvelteKit 文件路由（`src/routes/**/+page.svelte`） |
| 样式 | `index.module.less` | Tailwind 原子类 + `src/app.css` |
| 状态管理 | `zustand` store | Svelte `writable` store（`$stores/*`） |
| 自定义 Hook | `src/hooks/useXxx.ts` | 无独立目录，逻辑内聚于组件或 `$utils/*` |
| 国际化资源 | `src/locales/{zh-CN,en-US}/translation.json` | `messages/{en,zh}.json`（项目根，构建期编译） |
| 全局样式 | `src/styles/*.less` | 单一 `src/app.css` + Tailwind 主题变量 |
| 路径别名 | `@/*` 系列 | `$lib` / `$types` / `$utils` / `$services` / `$stores` / `$config` / `$i18n` / `$components` / `$paraglide` |

---

## 2. 目录结构总览

```
frontend/                           # 前端项目根目录
├── messages/                      # 词条资源（Paraglide 输入）
│   ├── en.json                    # 英文词条（参考语言）
│   └── zh.json                    # 中文词条
├── project.inlang/                # inlang 项目配置
│   └── settings.json              # 语言列表与词条路径规则
├── public/                        # 静态资源（不经过构建）
├── scripts/                       # 工程脚本（i18n 词条校验）
├── src/
│   ├── lib/                       # 应用主体
│   │   ├── api/                   # API 客户端（openapi-fetch + mock 代理）
│   │   │   ├── client.ts
│   │   │   └── schema.d.ts        # 由 openapi.json 生成的契约类型
│   │   ├── components/            # 公共组件
│   │   │   ├── common/            # 通用基础组件
│   │   │   ├── business/          # 业务组件
│   │   │   └── layout/            # 布局组件
│   │   ├── config/                # 常量与主题配置
│   │   ├── i18n/                  # 词条取用入口（转出 Paraglide 生成物）
│   │   │   └── index.ts
│   │   ├── mock/                  # mock 层（USE_MOCK 时接管请求）
│   │   │   ├── index.ts           # 路由表
│   │   │   ├── fixtures.ts
│   │   │   ├── scenario.ts        # slow / error / empty 场景
│   │   │   └── handlers/          # 各业务域处理器
│   │   ├── services/              # API 服务
│   │   ├── stores/                # 状态管理（Svelte Store）
│   │   ├── types/                 # TypeScript 类型定义
│   │   ├── utils/                 # 工具函数（含单元测试）
│   │   └── paraglide/             # Paraglide 生成物（不入库）
│   ├── routes/                    # SvelteKit 文件路由
│   │   ├── +layout.ts             # ssr = false 等页面选项
│   │   ├── +layout.svelte         # 根布局（标题、主题、Toast）
│   │   ├── +error.svelte          # 未匹配路由与渲染错误
│   │   ├── login/                 # 登录页
│   │   └── (app)/                 # 受鉴权守卫保护的路由组
│   │       ├── +layout.svelte     # 守卫 + 侧边栏 + 顶栏
│   │       ├── +page.svelte       # 首页
│   │       ├── console/
│   │       ├── data-browser/
│   │       ├── graph/
│   │       ├── monitoring/
│   │       └── schema/[tab=schemaTab]/  # Schema 各子页
│   ├── params.ts                  # 路由参数匹配器（schema tab 取值校验）
│   ├── env.ts                     # 浏览器可见环境变量声明
│   ├── app.html                   # HTML 模板
│   ├── app.d.ts                   # App 类型 augment
│   └── app.css                    # 全局样式（Tailwind 入口）
├── .env                           # 默认环境变量
├── .env.mock                      # mock 模式环境变量
├── package.json
├── tsconfig.json                  # TypeScript 配置（extends $app/tsconfig）
├── vite.config.ts                 # Vite / SvelteKit / Paraglide 配置
└── README.md
```
---

## 3. 详细目录说明

### 3.2 src/lib/components/ - 公共组件

按功能层级组织组件，分为通用组件、业务组件和布局组件。**每个组件为独立 `.svelte` 单文件**。

```
components/
├── common/                          # 通用基础组件
│   ├── CypherEditor.svelte          # Monaco 查询编辑器
│   ├── CytoscapeCanvas.svelte       # 图可视化画布
│   ├── HealthMonitor.svelte         # 服务健康监控
│   ├── LanguageSwitcher.svelte      # 语言切换
│   ├── LoadingScreen.svelte         # 全屏加载
│   ├── PageSkeleton.svelte          # 页面骨架
│   ├── Skeleton.svelte              # 骨架基元
│   ├── ThemeToggle.svelte           # 主题切换
│   ├── Toast.svelte                 # 轻提示
│   └── VirtualTable.svelte          # 虚拟滚动表格
├── business/                        # 业务组件
│   ├── CursorResult.svelte          # 游标结果展示
│   ├── FilterPanel.svelte           # 条件过滤面板
│   ├── GraphPreviewControls.svelte  # 图预览操作条
│   ├── SchemaErGraph.svelte         # ER 关系图视图
│   ├── SpaceSelector.svelte         # 空间选择器
│   └── StreamingResult.svelte       # 流式结果展示
└── layout/                          # 布局组件
    ├── Header.svelte                # 页面头部
    └── Sidebar.svelte               # 侧边栏
```

> 主布局与鉴权守卫是路由组布局 `(app)/+layout.svelte`，不再是独立组件——它们只在路由树里有意义。

**组件命名规范**:
- 文件名使用 PascalCase（如 `CypherEditor.svelte`）
- 组件名与文件名一致
- 样式优先使用 Tailwind 原子类；组件私有样式写在 `<style>` 块内

### 3.3 src/routes/ - 页面与路由

页面即路由：目录结构决定 URL，`+page.svelte` 是页面组件，`+layout.svelte` 是嵌套布局。

```
routes/
├── +layout.ts                       # ssr = false 等页面选项
├── +layout.svelte                   # 根布局：标题、主题 class、Toast
├── +error.svelte                    # 未匹配路由与渲染错误
├── login/
│   └── +page.svelte                 # /login
└── (app)/                           # 路由组：不进入 URL，只承载鉴权守卫与主布局
    ├── +layout.svelte               # 守卫 + 侧边栏 + 顶栏
    ├── +page.svelte                 # /
    ├── console/+page.svelte         # /console
    ├── data-browser/+page.svelte    # /data-browser
    ├── graph/+page.svelte           # /graph
    ├── monitoring/+page.svelte      # /monitoring
    └── schema/[tab=schemaTab]/      # /schema/{spaces,tags,edges,indexes,visualization}
        └── +page.svelte
```

**页面组织约定**:
- 一个路由一个目录，页面组件固定命名 `+page.svelte`
- 可被多处复用的子组件上移到 `$components/business`，页面私有逻辑留在 `+page.svelte`
- tab 属于路由而非组件状态，用 `src/params.ts` 的匹配器约束取值
- 需要共享的外壳写成 `+layout.svelte`，而不是在页面里判断

### 3.4 src/lib/stores/ - 状态管理

使用 Svelte 内置 `writable` store，按功能模块拆分。

```
stores/
├── connection.ts                    # 连接状态管理
├── console.ts                       # 控制台状态管理
├── dataBrowser.ts                   # 数据浏览状态管理
├── graph.ts                         # 图可视化状态管理
├── notification.ts                  # 通知状态管理
├── schema.ts                        # Schema 状态管理
└── theme.ts                         # 主题（明/暗）状态管理
```

**Store 文件示例**:
```typescript
// stores/graph.ts
import { writable } from 'svelte/store';
import type { GraphData, LayoutType } from '$types/graph';

interface GraphState {
  graphData: GraphData;
  layout: LayoutType;
}

function createGraphStore() {
  const { subscribe, set, update } = writable<GraphState>({
    graphData: { nodes: [], edges: [] },
    layout: 'cose',
  });

  return {
    subscribe,
    setGraphData: (data: GraphData) => update((s) => ({ ...s, graphData: data })),
    setLayout: (layout: LayoutType) => update((s) => ({ ...s, layout })),
  };
}

export const graphStore = createGraphStore();
```

### 3.5 src/lib/services/ - API 服务

按功能模块组织 API 服务，与后端 API 结构对应。所有请求经 `$utils/http.ts` 发出。

```
services/
├── api.ts                           # API 接口聚合
├── connection.ts                    # 连接相关 API
├── data.ts                          # 通用数据 API
├── dataBrowser.ts                   # 数据浏览相关 API
├── graph.ts                         # 图数据相关 API
├── query.ts                         # 查询相关 API
├── queryHistory.ts                  # 查询历史 API
├── schema.ts                        # Schema 相关 API
└── transaction.ts                   # 事务相关 API
```

**服务文件示例**:
```typescript
// services/connection.ts
import { get, post } from '$utils/http';

export interface ConnectParams {
  host: string;
  port: number;
  username: string;
  password: string;
}

export const connectionService = {
  connect: (params: ConnectParams) => post<ConnectResult>('/v1/connect', params),
  disconnect: () => post('/v1/disconnect'),
  health: () => get('/v1/health'),
};
```

### 3.6 src/lib/utils/ - 工具函数

存放通用的工具函数、配置与常量。

```
utils/
├── cytoscapeConfig.ts               # Cytoscape 样式与交互配置
├── export.ts                        # 导出（PNG / JSON）
├── filterExpression.ts              # 过滤条件 → WHERE 表达式编译
├── function.ts                      # 通用工具函数
├── gql.ts                           # Cypher 查询生成
├── graphLayout.ts                   # 图布局算法封装
├── http.ts                          # HTTP 请求封装（Axios）
├── monacoCypher.ts                  # Monaco Cypher 语言与补全注册
├── monacoSetup.ts                   # Monaco 按需引入入口
├── parseData.ts                     # 图数据解析
├── schemaGraph.ts                   # Schema → ER 图数据构建
└── storage.ts                       # 本地存储封装
```

**工具函数说明**:

| 文件 | 用途 |
|------|------|
| `http.ts` | Axios 封装、请求/响应拦截、BigInt 解析 |
| `function.ts` | 通用工具函数 |
| `gql.ts` | Cypher 查询语句生成 |
| `parseData.ts` | 图数据解析与归一化 |
| `cytoscapeConfig.ts` / `graphLayout.ts` | Cytoscape 样式、布局与交互配置 |
| `monacoCypher.ts` / `monacoSetup.ts` | Monaco 查询编辑器语言服务与按需引入 |
| `schemaGraph.ts` | 由 Tag / EdgeType 元数据构建 ER 关系图 |
| `filterExpression.ts` | 将结构化过滤条件编译为后端 WHERE 片段 |
| `export.ts` | 图数据导出为 PNG / JSON |
| `storage.ts` | 本地存储封装 |

### 3.7 src/lib/types/ - 类型定义

存放全局 TypeScript 类型定义。

```
types/
├── api.ts                           # API 请求/响应类型
├── data.ts                          # 通用数据类型
├── dataBrowser.ts                   # 数据浏览类型（含 FilterGroup）
├── graph.ts                         # 图数据类型（含 NeighborInfo）
├── query.ts                         # 查询相关类型
├── schema.ts                        # Schema 相关类型
└── schema.gen.d.ts                  # 由后端生成的 Schema 类型声明
```

### 3.8 src/lib/config/ - 配置文件

存放应用配置。

```
config/
├── constants.ts                     # 应用常量（分页、图规模上限等）
└── theme.ts                         # 主题常量
```

### 3.9 词条资源与取词入口

词条本身不在 `src/lib` 下，而是由 Paraglide 在构建期编译成 `src/lib/paraglide` 下的类型化消息函数（生成物，不入库）。

```
frontend/
├── messages/                        # 词条源（单层点号 key）
│   ├── en.json                      # 英文词条（参考语言）
│   └── zh.json                      # 中文词条
├── project.inlang/settings.json     # baseLocale、语言列表、词条路径规则
└── src/lib/i18n/index.ts            # 取词入口：t() / message() / MessageKey / setLocale()
```

> 词条采用单层点号 key（如 `common.login`），取词写作 `t('common.login')`，key 在编译期校验。数据驱动场景用 `message('common.login')` 取得消息函数而非字符串 key。详见 `docs/frontend/i18n_integration_guide.md`。
> 新增词条必须**同时**写入 `en.json` 与 `zh.json`，保持一致；`npm run check` 会校验两份词条库一致且所有引用都有定义。

---

## 4. 文件命名规范

### 4.1 通用规范

| 类型 | 命名方式 | 示例 |
|------|----------|------|
| Svelte 组件 | PascalCase + `.svelte` | `CypherEditor.svelte` |
| 页面组件 | PascalCase + `.svelte` | `Schema.svelte` |
| 工具文件 | camelCase | `http.ts` |
| Store 文件 | camelCase | `graph.ts` |
| 服务文件 | camelCase | `dataBrowser.ts` |
| 类型文件 | camelCase | `graph.ts` |
| 常量文件 | camelCase（导出用 UPPER_SNAKE_CASE） | `constants.ts` |

### 4.2 组件文件结构

Svelte 组件为**单文件**，不采用 `index.tsx` + `index.module.less` 的目录形式：

```
ComponentName.svelte
├── <script lang="ts">      # 逻辑与 props
├── 模板（markup）           # 结构与绑定
└── <style>                 # 可选：组件私有样式（优先用 Tailwind 类）
```

### 4.3 页面文件结构

```
PageName/
├── PageName.svelte          # 页面入口
├── SubComponent.svelte      # 页面私有组件（可选）
└── ...                      # 页面私有工具/类型（可选）
```

---

## 5. 导入路径规范

### 5.1 路径别名配置

别名在 `tsconfig.json` 与 `vite.config.ts` 中**保持一致**：

```json
// tsconfig.json（节选）
{
  "extends": "$app/tsconfig",
  "compilerOptions": {
    "paths": {
      "$lib/*": ["./src/lib/*"],
      "$components/*": ["./src/lib/components/*"],
      "$stores/*": ["./src/lib/stores/*"],
      "$services/*": ["./src/lib/services/*"],
      "$utils/*": ["./src/lib/utils/*"],
      "$types/*": ["./src/lib/types/*"],
      "$config/*": ["./src/lib/config/*"],
      "$paraglide/*": ["./src/lib/paraglide/*"],
      "$i18n": ["./src/lib/i18n/index.ts"]
    }
  }
}
```

SvelteKit 自带的 `$app/*`（`navigation`、`state`、`env/public` 等）由框架解析，不需要在此声明。

### 5.2 导入顺序规范

```typescript
// 1. Svelte 与 SvelteKit
import { onMount } from 'svelte';
import { writable } from 'svelte/store';
import { goto } from '$app/navigation';
import { page } from '$app/state';

// 2. 第三方库
import cytoscape from 'cytoscape';

// 3. 路径别名导入
import { t } from '$i18n';
import CypherEditor from '$components/common/CypherEditor.svelte';
import { consoleStore } from '$stores/console';
import { queryService } from '$services/query';
import type { GraphData } from '$types/graph';

// 4. 相对路径导入（仅同目录）
import GraphPreviewControls from '$components/business/GraphPreviewControls.svelte';
```

> 业务组件之间也走 `$components` 别名，不使用相对路径，避免组件在各页面间移动时批量改导入。

---

## 6. 与 nebula-studio 的目录对比

| nebula-studio | GraphDB（现状） | 说明 |
|---------------|----------------|------|
| `app/components/` | `src/lib/components/` | 结构一致，组件改为 `.svelte` 单文件 |
| `app/pages/` | `src/routes/` | 改为文件路由，页面文件名统一为 `+page.svelte` |
| `app/stores/` | `src/lib/stores/` | 结构一致，改用 Svelte Store |
| `app/config/service.ts` | `src/lib/services/` | 拆分为独立目录 |
| `app/utils/` | `src/lib/utils/` | 一致 |
| `app/interfaces/` | `src/lib/types/` | 重命名 |
| `app/config/locale/` | `messages/` | 词条移出 `src`，由 Paraglide 编译 |
| `app/static/` | `public/` | 只保留不经过构建的静态资源 |
| `app/pages/Import/` | ❌ 移除 | 不需要数据导入 |
| `app/pages/LLMBot/` | ❌ 移除 | 不需要 LLM |
| `app/pages/SketchModeling/` | ❌ 移除 | 不需要可视化建模 |
| `app/stores/datasource.ts` | ❌ 移除 | 不需要多数据源 |
| `app/stores/import.ts` | ❌ 移除 | 不需要导入管理 |
| `app/stores/llm.ts` | ❌ 移除 | 不需要 LLM |
| `app/components/MonacoEditor/` | ✅ 已实现 | 见 `$components/common/CypherEditor.svelte` |

---

## 7. 参考文档

- [前端技术栈](./tech_stack.md)
- [前端功能清单](../feature_checklist.md)
- [前端 i18n 集成指南](../i18n_integration_guide.md)
- [Web API 概览](../../api/web/web_api_overview.md)

---

**文档结束**
