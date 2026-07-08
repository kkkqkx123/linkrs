# GraphDB 前端目录结构设计

**文档版本**: v1.0  
**创建日期**: 2026-03-29  
**最后更新**: 2026-03-29

---

## 1. 设计原则

### 1.1 核心原则

1. **模块化**: 按功能模块组织代码，高内聚低耦合
2. **可维护性**: 清晰的目录结构，便于定位和修改代码
3. **可扩展性**: 预留扩展空间，支持新功能快速添加
4. **一致性**: 遵循 nebula-studio 的目录组织方式，便于参考复用

### 1.2 参考 nebula-studio

本目录结构主要参考 `ref/nebula-studio-3.10.0/app` 的组织方式，同时根据 GraphDB 的实际需求进行简化：

- 移除与数据导入相关的目录（Import, Datasource）
- 移除与 LLM 相关的目录（LLMBot）
- 移除可视化建模目录（SketchModeling）
- 使用 Zustand 替代 MobX 的 Store 组织方式

---

## 2. 目录结构总览

```
frontend/                           # 前端项目根目录
├── public/                         # 静态资源（不经过构建）
│   ├── favicon.ico
│   └── logo.svg
├── src/                            # 源代码目录
│   ├── assets/                     # 静态资源（经过构建）
│   │   ├── images/                 # 图片资源
│   │   ├── fonts/                  # 字体文件
│   │   └── icons/                  # SVG 图标
│   ├── components/                 # 公共组件
│   │   ├── common/                 # 通用基础组件
│   │   ├── business/               # 业务组件
│   │   └── layout/                 # 布局组件
│   ├── pages/                      # 页面组件
│   │   ├── Login/                  # 登录页面
│   │   ├── MainPage/               # 主布局页面
│   │   ├── Console/                # 查询控制台
│   │   ├── Schema/                 # Schema 管理
│   │   ├── GraphVisualization/     # 图可视化
│   │   └── DataBrowser/            # 数据浏览
│   ├── stores/                     # 状态管理（Zustand）
│   │   ├── connection.ts           # 连接状态
│   │   ├── console.ts              # 控制台状态
│   │   ├── schema.ts               # Schema 状态
│   │   ├── graph.ts                # 图可视化状态
│   │   └── index.ts                # Store 导出
│   ├── services/                   # API 服务
│   │   ├── api.ts                  # API 接口定义
│   │   ├── connection.ts           # 连接相关 API
│   │   ├── query.ts                # 查询相关 API
│   │   ├── schema.ts               # Schema 相关 API
│   │   └── index.ts                # 服务导出
│   ├── utils/                      # 工具函数
│   │   ├── http.ts                 # HTTP 请求封装
│   │   ├── function.ts             # 通用工具函数
│   │   ├── gql.ts                  # Cypher 查询生成
│   │   ├── constant.ts             # 常量定义
│   │   ├── parseData.ts            # 数据解析
│   │   └── storage.ts              # 本地存储封装
│   ├── hooks/                      # 自定义 Hooks
│   │   ├── useConnection.ts        # 连接管理 Hook
│   │   ├── useQuery.ts             # 查询 Hook
│   │   └── useLocalStorage.ts      # 本地存储 Hook
│   ├── types/                      # TypeScript 类型定义
│   │   ├── global.d.ts             # 全局类型
│   │   ├── api.ts                  # API 类型
│   │   ├── schema.ts               # Schema 类型
│   │   └── graph.ts                # 图数据类型
│   ├── config/                     # 配置文件
│   │   ├── constants.ts            # 应用常量
│   │   ├── routes.tsx              # 路由配置
│   │   ├── theme.ts                # 主题配置
│   │   └── i18n.ts                 # 国际化配置
│   ├── locales/                    # 国际化资源
│   │   ├── zh-CN/                  # 中文
│   │   │   └── translation.json
│   │   └── en-US/                  # 英文
│   │       └── translation.json
│   ├── styles/                     # 全局样式
│   │   ├── global.less             # 全局样式
│   │   ├── variables.less          # 变量定义
│   │   └── mixins.less             # Less 混合
│   ├── App.tsx                     # 应用入口组件
│   ├── main.tsx                    # 应用入口文件
│   └── vite-env.d.ts               # Vite 类型声明
├── tests/                          # 测试文件
│   ├── unit/                       # 单元测试
│   ├── integration/                # 集成测试
│   └── e2e/                        # E2E 测试
├── index.html                      # HTML 入口
├── package.json                    # 项目依赖
├── tsconfig.json                   # TypeScript 配置
├── vite.config.ts                  # Vite 配置
├── eslint.config.js                # ESLint 配置
├── prettier.config.js              # Prettier 配置
└── README.md                       # 项目说明
```

---

## 3. 详细目录说明

### 3.1 public/ - 静态资源

存放不经过构建处理的静态资源，直接复制到输出目录。

```
public/
├── favicon.ico           # 网站图标
└── logo.svg              # Logo 图片
```

### 3.2 src/assets/ - 构建资源

存放需要经过构建处理的资源文件。

```
assets/
├── images/               # 图片资源
│   ├── logo.png
│   ├── empty.png
│   └── error.png
├── fonts/                # 字体文件
│   └── Roboto-Regular.ttf
└── icons/                # SVG 图标（可作为组件使用）
    ├── database.svg
    ├── graph.svg
    └── table.svg
```

### 3.3 src/components/ - 公共组件

按功能层级组织组件，分为通用组件、业务组件和布局组件。

```
components/
├── common/                          # 通用基础组件
│   ├── Button/                      # 按钮组件
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── Icon/                        # 图标组件
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── EmptyTableTip/               # 空表格提示
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── Avatar/                      # 头像组件
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── Breadcrumb/                  # 面包屑导航
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── ColorPicker/                 # 颜色选择器（阶段 6 使用）
│   │   ├── index.tsx
│   │   └── index.module.less
│   └── ErrorBoundary/               # 错误边界
│       ├── index.tsx
│       └── index.module.less
├── business/                        # 业务组件
│   ├── ConnectionStatus/            # 连接状态显示
│   │   ├── index.tsx
│   │   └── index.module.less
│   ├── QueryResult/                 # 查询结果展示
│   │   ├── index.tsx
│   │   ├── TableView.tsx
│   │   ├── JsonView.tsx
│   │   ├── GraphView.tsx
│   │   └── index.module.less
│   └── SchemaForm/                  # Schema 表单
│       ├── index.tsx
│       └── index.module.less
└── layout/                          # 布局组件
    ├── Header/                      # 页面头部
    │   ├── index.tsx
    │   └── index.module.less
    ├── Sidebar/                     # 侧边栏
    │   ├── index.tsx
    │   └── index.module.less
    └── MainLayout/                  # 主布局
        ├── index.tsx
        └── index.module.less
```

**组件命名规范**:
- 目录名使用 PascalCase（如 `MonacoEditor/`）
- 入口文件统一为 `index.tsx`
- 样式文件为 `index.module.less`
- 组件名与目录名一致

### 3.4 src/pages/ - 页面组件

按功能模块组织页面，每个页面是一个独立目录。

```
pages/
├── Login/                           # 登录页面
│   ├── index.tsx                    # 页面入口
│   ├── index.module.less            # 页面样式
│   └── components/                  # 页面私有组件
│       └── LanguageSelect/
│           ├── index.tsx
│           └── index.module.less
├── MainPage/                        # 主布局页面
│   ├── index.tsx
│   ├── index.less
│   ├── routes.tsx                   # 子路由配置
│   ├── Header/                      # 头部组件
│   │   ├── index.tsx
│   │   ├── index.module.less
│   │   └── HelpMenu/                # 帮助菜单
│   │       ├── index.tsx
│   │       └── index.module.less
│   └── Sidebar/                     # 侧边栏
│       ├── index.tsx
│       └── index.module.less
├── Console/                         # 查询控制台
│   ├── index.tsx
│   ├── index.module.less
│   ├── components/                  # 控制台相关组件
│   │   ├── QueryEditor/             # 查询编辑器（简化版 TextArea）
│   │   ├── OutputBox/               # 输出面板
│   │   ├── HistoryBtn/              # 历史按钮
│   │   ├── FavoriteBtn/             # 收藏按钮
│   │   └── ExportModal/             # 导出弹窗
│   └── hooks/                       # 页面私有 Hooks
│       └── useQueryExecution.ts
├── Schema/                          # Schema 管理
│   ├── index.tsx
│   ├── index.module.less
│   ├── SpaceCreate/                 # 创建 Space
│   │   ├── index.tsx
│   │   └── CreateForm.tsx
│   ├── SchemaConfig/                # Schema 配置
│   │   ├── index.tsx
│   │   ├── List/                    # 列表视图
│   │   │   ├── Tag/                 # Tag 列表
│   │   │   ├── Edge/                # Edge 列表
│   │   │   ├── Index/               # 索引列表
│   │   │   └── SchemaVisualization/ # Schema 可视化
│   │   ├── Create/                  # 创建操作
│   │   │   ├── TagCreate/
│   │   │   ├── EdgeCreate/
│   │   │   └── IndexCreate/
│   │   └── Edit/                    # 编辑操作
│   │       ├── TagEdit/
│   │       └── EdgeEdit/
│   └── hooks/
│       └── useSchemaOperations.ts
├── GraphVisualization/              # 图可视化
│   ├── index.tsx
│   ├── index.module.less
│   ├── components/
│   │   ├── ForceGraph/              # 力导向图
│   │   ├── GraphControls/           # 图控制面板
│   │   ├── NodeTooltip/             # 节点提示
│   │   └── EdgeTooltip/             # 边提示
│   └── hooks/
│       └── useGraphLayout.ts
└── DataBrowser/                     # 数据浏览
    ├── index.tsx
    ├── index.module.less
    ├── VertexBrowser/               # 顶点浏览
    └── EdgeBrowser/                 # 边浏览
```

**页面组件规范**:
- 页面组件作为路由入口
- 页面内部组件放在 `components/` 子目录
- 页面私有 Hooks 放在 `hooks/` 子目录
- 复杂页面可以拆分子目录

### 3.5 src/stores/ - 状态管理

使用 Zustand 进行状态管理，按功能模块拆分 Store。

```
stores/
├── index.ts                         # Store 导出和初始化
├── connection.ts                    # 连接状态管理
├── console.ts                       # 控制台状态管理
├── schema.ts                        # Schema 状态管理
├── graph.ts                         # 图可视化状态管理
└── types.d.ts                       # Store 类型定义
```

**Store 文件示例**:
```typescript
// stores/connection.ts
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface ConnectionState {
  isConnected: boolean;
  host: string;
  port: number;
  username: string;
  sessionId: string | null;
  connect: (host: string, port: number, username: string, password: string) => Promise<void>;
  disconnect: () => Promise<void>;
}

export const useConnectionStore = create<ConnectionState>()(
  persist(
    (set, get) => ({
      isConnected: false,
      host: 'localhost',
      port: 7001,
      username: '',
      sessionId: null,
      connect: async (host, port, username, password) => {
        // 实现连接逻辑
      },
      disconnect: async () => {
        // 实现断开逻辑
      },
    }),
    {
      name: 'connection-storage',
      partialize: (state) => ({ host: state.host, port: state.port }),
    }
  )
);
```

### 3.6 src/services/ - API 服务

按功能模块组织 API 服务，与后端 API 结构对应。

```
services/
├── index.ts                         # 服务导出
├── api.ts                           # API 接口定义（类型）
├── connection.ts                    # 连接相关 API
├── query.ts                         # 查询相关 API
├── schema.ts                        # Schema 相关 API
├── graph.ts                         # 图数据相关 API
└── dataBrowser.ts                   # 数据浏览相关 API
```

**服务文件示例**:
```typescript
// services/connection.ts
import { post } from '@/utils/http';

export interface ConnectParams {
  host: string;
  port: number;
  username: string;
  password: string;
}

export interface ConnectResult {
  sessionId: string;
  version: string;
}

export const connectionService = {
  connect: (params: ConnectParams) => 
    post<ConnectResult>('/api/connect')(params),
  
  disconnect: () => 
    post('/api/disconnect')(),
  
  health: () => 
    get('/api/health')(),
};
```

### 3.7 src/utils/ - 工具函数

存放通用的工具函数和常量。

```
utils/
├── http.ts                          # HTTP 请求封装（Axios）
├── function.ts                      # 通用工具函数
├── gql.ts                           # Cypher 查询生成
├── constant.ts                      # 常量定义
├── parseData.ts                     # 数据解析函数
├── storage.ts                       # 本地存储封装
└── validators.ts                    # 表单验证函数
```

**工具函数说明**:

| 文件 | 用途 | 参考来源 |
|------|------|----------|
| `http.ts` | Axios 封装、拦截器配置 | nebula-studio `utils/http.ts` |
| `function.ts` | 通用工具函数（handleKeyword、handleEscape 等） | nebula-studio `utils/function.ts` |
| `gql.ts` | Cypher 查询语句生成 | nebula-studio `utils/gql.ts`（需适配 Cypher） |
| `constant.ts` | 数据类型、操作符等常量 | nebula-studio `utils/constant.ts` |
| `parseData.ts` | 图数据解析 | nebula-studio `utils/parseData.ts` |

### 3.8 src/hooks/ - 自定义 Hooks

存放可复用的自定义 React Hooks。

```
hooks/
├── useConnection.ts                 # 连接管理 Hook
├── useQuery.ts                      # 查询执行 Hook
├── useSchema.ts                     # Schema 操作 Hook
├── useLocalStorage.ts               # 本地存储 Hook
├── useDebounce.ts                   # 防抖 Hook
└── usePrevious.ts                   # 获取前值 Hook
```

### 3.9 src/types/ - 类型定义

存放全局 TypeScript 类型定义。

```
types/
├── global.d.ts                      # 全局类型声明
├── api.ts                           # API 请求/响应类型
├── schema.ts                        # Schema 相关类型
├── graph.ts                         # 图数据类型
└── store.ts                         # Store 状态类型
```

### 3.10 src/config/ - 配置文件

存放应用配置文件。

```
config/
├── constants.ts                     # 应用常量
├── routes.tsx                       # 路由配置
├── theme.ts                         # Ant Design 主题配置
└── i18n.ts                          # 国际化配置
```

### 3.11 src/locales/ - 国际化资源

存放多语言翻译文件。

```
locales/
├── zh-CN/                           # 中文
│   └── translation.json
└── en-US/                           # 英文
    └── translation.json
```

### 3.12 src/styles/ - 全局样式

存放全局样式文件。

```
styles/
├── global.less                      # 全局样式
├── variables.less                   # Less 变量定义
└── mixins.less                      # Less 混合
```

---

## 4. 文件命名规范

### 4.1 通用规范

| 类型 | 命名方式 | 示例 |
|------|----------|------|
| 组件文件 | PascalCase | `MonacoEditor.tsx` |
| 组件目录 | PascalCase | `MonacoEditor/` |
| 工具文件 | camelCase | `http.ts` |
| 样式文件 | camelCase + .module.less | `index.module.less` |
| 常量文件 | UPPER_SNAKE_CASE（导出） | `MAX_COUNT` |
| 类型文件 | PascalCase | `api.ts` |
| Hook 文件 | camelCase + use 前缀 | `useConnection.ts` |

### 4.2 组件文件结构

```
ComponentName/
├── index.tsx              # 组件入口（必须）
├── index.module.less      # 组件样式（必须）
├── types.ts               # 组件私有类型（可选）
├── utils.ts               # 组件私有工具（可选）
└── __tests__/             # 组件测试（可选）
    └── index.test.tsx
```

### 4.3 页面文件结构

```
PageName/
├── index.tsx              # 页面入口
├── index.module.less      # 页面样式
├── components/            # 页面私有组件
│   └── ComponentName/
├── hooks/                 # 页面私有 Hooks
│   └── useHookName.ts
└── types.ts               # 页面私有类型
```

---

## 5. 导入路径规范

### 5.1 路径别名配置

```json
// tsconfig.json
{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"],
      "@components/*": ["src/components/*"],
      "@pages/*": ["src/pages/*"],
      "@stores/*": ["src/stores/*"],
      "@utils/*": ["src/utils/*"],
      "@services/*": ["src/services/*"],
      "@hooks/*": ["src/hooks/*"],
      "@types/*": ["src/types/*"],
      "@assets/*": ["src/assets/*"],
      "@config/*": ["src/config/*"],
      "@locales/*": ["src/locales/*"],
      "@styles/*": ["src/styles/*"]
    }
  }
}
```

### 5.2 导入顺序规范

```typescript
// 1. React 相关
import React, { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';

// 2. 第三方库
import { Button, Input } from 'antd';
import { create } from 'zustand';

// 3. 绝对路径导入（@/*）
import { useConnectionStore } from '@stores/connection';
import { connectionService } from '@services/connection';
import { formatDate } from '@utils/function';

// 4. 相对路径导入
import { LoginForm } from './components/LoginForm';
import styles from './index.module.less';
```

---

## 6. 与 nebula-studio 的目录对比

| nebula-studio | GraphDB | 说明 |
|---------------|---------|------|
| `app/components/` | `src/components/` | 一致 |
| `app/pages/` | `src/pages/` | 一致 |
| `app/stores/` | `src/stores/` | 一致，但使用 Zustand |
| `app/config/service.ts` | `src/services/` | 拆分为独立目录 |
| `app/utils/` | `src/utils/` | 一致 |
| `app/interfaces/` | `src/types/` | 重命名 |
| `app/config/locale/` | `src/locales/` | 重命名 |
| `app/static/` | `public/` + `src/assets/` | 拆分静态资源 |
| `app/pages/Import/` | ❌ 移除 | 不需要数据导入 |
| `app/pages/LLMBot/` | ❌ 移除 | 不需要 LLM |
| `app/pages/SketchModeling/` | ❌ 移除 | 不需要可视化建模 |
| `app/stores/datasource.ts` | ❌ 移除 | 不需要多数据源 |
| `app/stores/import.ts` | ❌ 移除 | 不需要导入管理 |
| `app/stores/llm.ts` | ❌ 移除 | 不需要 LLM |
| `app/components/MonacoEditor/` | ⏳ 延后 | 阶段 2 使用 TextArea 简化实现 |

---

## 7. 新增目录说明

### 7.1 src/hooks/

nebula-studio 中没有独立的 hooks 目录，GraphDB 新增此目录用于存放可复用的自定义 Hooks。

### 7.2 src/services/

将 nebula-studio 中的 `config/service.ts` 拆分为独立的 `services/` 目录，便于管理和扩展。

### 7.3 src/types/

将 nebula-studio 中的 `interfaces/` 重命名为 `types/`，更符合 TypeScript 社区惯例。

---

## 8. 阶段实施建议

### 8.1 阶段 1（基础框架）

需要创建的目录和文件：

```
src/
├── components/
│   ├── common/
│   │   ├── Button/
│   │   ├── Icon/
│   │   ├── EmptyTableTip/
│   │   ├── Avatar/
│   │   └── ErrorBoundary/
│   └── layout/
│       ├── Header/
│       ├── Sidebar/
│       └── MainLayout/
├── pages/
│   ├── Login/
│   └── MainPage/
├── stores/
│   ├── index.ts
│   └── connection.ts
├── services/
│   ├── index.ts
│   └── connection.ts
├── utils/
│   ├── http.ts
│   ├── function.ts
│   └── storage.ts
├── config/
│   ├── routes.tsx
│   └── theme.ts
└── App.tsx
```

### 8.2 阶段 2（查询控制台）

新增目录和文件：

```
src/
├── components/
│   └── business/
│       └── QueryResult/         # 查询结果展示组件
├── pages/
│   └── Console/
│       └── components/
│           └── QueryEditor/     # 简化版查询编辑器（TextArea）
├── stores/
│   └── console.ts
├── services/
│   └── query.ts
└── utils/
    └── gql.ts                   # Cypher 查询生成（简化版）
```

**说明**：
- 阶段 2 使用 Ant Design 的 `Input.TextArea` 作为查询编辑器
- 不引入 Monaco Editor，减少包体积和复杂度
- 后续阶段可平滑升级到 Monaco Editor

### 8.3 阶段 3-5（Schema 管理）

新增目录和文件：

```
src/
├── components/
│   └── business/
│       └── SchemaForm/
├── pages/
│   └── Schema/
├── stores/
│   └── schema.ts
├── services/
│   └── schema.ts
└── utils/
    └── constant.ts
```

### 8.4 阶段 6（图可视化）

新增目录和文件：

```
src/
├── components/
│   ├── common/
│   │   └── ColorPicker/
│   └── business/
│       └── GraphVisualization/
├── pages/
│   └── GraphVisualization/
├── stores/
│   └── graph.ts
├── services/
│   └── graph.ts
└── utils/
    └── parseData.ts
```

---

## 9. 参考文档

- [nebula-studio 目录结构](../component_reuse_analysis.md)
- [前端功能清单](../feature_checklist.md)
- [PRD 阶段 1](../prd_phase1.md)
- [Web API 概览](../../api/web/web_api_overview.md)

---

**文档结束**
