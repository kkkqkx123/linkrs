# GraphDB 前端技术栈

**文档版本**: v1.0  
**创建日期**: 2026-03-29  
**最后更新**: 2026-03-29

---

## 1. 技术选型概述

基于项目需求分析和 nebula-studio-3.10.0 的参考实现，GraphDB 前端采用以下技术栈：

| 类别 | 技术选择 | 版本 | 说明 |
|------|---------|------|------|
| **前端框架** | React | 18.x | 现代化组件化开发 |
| **开发语言** | TypeScript | 5.x | 类型安全，提升开发体验 |
| **构建工具** | Vite | 5.x | 快速冷启动，HMR 支持 |
| **UI 组件库** | Ant Design | 5.x | 企业级 UI 设计 |
| **状态管理** | Zustand | 4.x | 轻量级状态管理 |
| **路由** | React Router | 6.x | 官方推荐路由方案 |
| **HTTP 客户端** | Axios | 1.x | 成熟的 HTTP 请求库 |
| **代码编辑器** | Ant Design Input.TextArea | 5.x | 基础文本编辑（简化实现） |
| **图可视化** | Cytoscape.js | 3.x | 高性能图可视化（阶段 6 实现） |
| **样式方案** | CSS Modules + Less | - | 组件级样式隔离 |
| **国际化** | react-i18next | 13.x | 多语言支持 |

---

## 2. 核心技术详解

### 2.1 前端框架: React 18

**选型理由**:
- 成熟稳定的生态系统
- 组件化开发模式
- 优秀的性能表现
- 与 nebula-studio 技术栈一致，便于参考实现

**关键特性**:
- Concurrent Features (并发特性)
- Automatic Batching (自动批处理)
- Suspense 改进
- React Server Components (可选)

### 2.2 开发语言: TypeScript

**选型理由**:
- 静态类型检查，减少运行时错误
- 优秀的 IDE 支持（自动补全、重构）
- 更好的代码可维护性
- 与 nebula-studio 保持一致

**配置要求**:
- 启用严格模式 (`strict: true`)
- 配置路径别名 (`@/*`)
- 启用装饰器支持（如需要）

### 2.3 构建工具: Vite

**选型理由**:
- 极速冷启动（基于 ES Modules）
- 快速热更新（HMR）
- 开箱即用的 TypeScript 支持
- 优化的生产构建

**对比 Create React App**:
| 特性 | Vite | CRA |
|------|------|-----|
| 冷启动 | < 300ms | ~10s |
| HMR | 即时 | 较慢 |
| 配置复杂度 | 低 | 高 |
| 构建速度 | 快 | 一般 |

### 2.4 UI 组件库: Ant Design 5.x

**选型理由**:
- 企业级设计规范
- 丰富的组件生态
- 完善的 TypeScript 支持
- 与 nebula-studio 完全一致

**主题定制**:
```typescript
// theme.config.ts
export const themeConfig = {
  token: {
    colorPrimary: '#345EDA',
    colorSuccess: '#52C41A',
    colorWarning: '#FAAD14',
    colorError: '#F5222D',
    borderRadius: 4,
  },
};
```

### 2.5 状态管理: Zustand

**选型理由**:
- 轻量级（~1KB）
- 简单易用的 API
- 支持 TypeScript
- 无需 Provider 包裹
- 相比 MobX 更简洁

**对比 MobX**:
| 特性 | Zustand | MobX |
|------|---------|------|
| 学习曲线 | 低 | 中 |
| 包大小 | ~1KB | ~20KB |
| 样板代码 | 极少 | 较多 |
| 调试工具 | 良好 | 优秀 |

**Store 结构示例**:
```typescript
// stores/connection.ts
import { create } from 'zustand';

interface ConnectionState {
  isConnected: boolean;
  host: string;
  username: string;
  connect: (host: string, username: string) => void;
  disconnect: () => void;
}

export const useConnectionStore = create<ConnectionState>((set) => ({
  isConnected: false,
  host: '',
  username: '',
  connect: (host, username) => set({ isConnected: true, host, username }),
  disconnect: () => set({ isConnected: false, host: '', username: '' }),
}));
```

### 2.6 路由: React Router v6

**选型理由**:
- React 官方推荐
- 声明式路由配置
- 支持嵌套路由
- 与 nebula-studio 保持一致

**路由结构**:
```typescript
// router/index.tsx
const router = createBrowserRouter([
  {
    path: '/login',
    element: <LoginPage />,
  },
  {
    path: '/',
    element: <MainLayout />,
    children: [
      { path: 'console', element: <ConsolePage /> },
      { path: 'schema', element: <SchemaPage /> },
      { path: 'graph', element: <GraphPage /> },
    ],
  },
]);
```

### 2.7 HTTP 客户端: Axios

**选型理由**:
- 成熟的错误处理机制
- 请求/响应拦截器
- 支持请求取消
- 与 nebula-studio 保持一致

**封装设计**:
```typescript
// utils/http.ts
import axios from 'axios';
import JSONBigint from 'json-bigint';

const service = axios.create({
  baseURL: import.meta.env.VITE_API_BASE_URL,
  timeout: 30000,
  transformResponse: [
    (data) => {
      try {
        return JSONBigint.parse(data);
      } catch {
        return data;
      }
    },
  ],
});

// 请求拦截器
service.interceptors.request.use((config) => {
  const sessionId = localStorage.getItem('sessionId');
  if (sessionId) {
    config.headers['X-Session-ID'] = sessionId;
  }
  return config;
});

// 响应拦截器
service.interceptors.response.use(
  (response) => response.data,
  (error) => {
    if (error.response?.status === 401) {
      // 处理未授权
      window.location.href = '/login';
    }
    return Promise.reject(error);
  }
);
```

### 2.8 查询编辑器: Ant Design TextArea（简化实现）

**选型理由**:
- 优先保证基础功能快速实现
- 减少复杂依赖（Monaco Editor ~3MB）
- 满足核心需求：文本输入、查询执行、结果显示
- 后续可平滑升级到 Monaco Editor

**阶段 1 实现功能**:
- 基础多行文本输入
- 支持 Tab 键缩进
- 查询执行（Ctrl/Cmd + Enter 快捷键）
- 执行结果显示（表格/JSON/错误信息）

**后续升级路径**:
- 阶段 3+ 可替换为 Monaco Editor
- 添加 Cypher 语法高亮
- 添加关键字自动补全
- 添加语法错误提示

### 2.9 图可视化: Cytoscape.js

**选型理由**:
- 高性能图渲染
- 丰富的布局算法
- 支持大规模图数据
- 灵活的样式定制

**对比其他方案**:
| 库 | 性能 | 功能 | 学习曲线 |
|----|------|------|----------|
| Cytoscape.js | 优秀 | 丰富 | 中 |
| D3.js | 良好 | 极丰富 | 高 |
| React Flow | 良好 | 中等 | 低 |
| Force Graph | 良好 | 基础 | 低 |

### 2.10 样式方案: CSS Modules + Less

**选型理由**:
- CSS Modules 提供组件级样式隔离
- Less 提供变量、嵌套等增强功能
- 与 nebula-studio 保持一致

**文件命名规范**:
```
ComponentName/
  ├── index.tsx
  └── index.module.less
```

---

## 3. 开发工具链

### 3.1 代码质量工具

| 工具 | 用途 | 配置 |
|------|------|------|
| ESLint | 代码规范检查 | 使用 @typescript-eslint |
| Prettier | 代码格式化 | 统一代码风格 |
| Stylelint | CSS 规范检查 | 检查 Less 文件 |
| Husky | Git 钩子 | 提交前检查 |
| lint-staged | 暂存区检查 | 仅检查修改的文件 |

### 3.2 测试工具

| 工具 | 用途 |
|------|------|
| Vitest | 单元测试 |
| React Testing Library | 组件测试 |
| Playwright | E2E 测试 |

### 3.3 开发辅助工具

| 工具 | 用途 |
|------|------|
| Vite Plugin SVG | SVG 组件化 |
| Vite Plugin Checker | 类型检查 |
| vite-tsconfig-paths | 路径别名支持 |

---

## 4. 依赖清单

### 4.1 生产依赖

```json
{
  "dependencies": {
    "react": "^18.2.0",
    "react-dom": "^18.2.0",
    "react-router-dom": "^6.20.0",
    "antd": "^5.12.0",
    "zustand": "^4.4.7",
    "axios": "^1.6.2",
    "react-i18next": "^13.5.0",
    "i18next": "^23.7.6",
    "json-bigint": "^1.0.0",
    "dayjs": "^1.11.10",
    "lodash-es": "^4.17.21",
    "clsx": "^2.0.0"
  }
}
```

### 4.2 开发依赖

```json
{
  "devDependencies": {
    "@types/react": "^18.2.43",
    "@types/react-dom": "^18.2.17",
    "@types/node": "^20.10.4",
    "@types/json-bigint": "^1.0.4",
    "@types/lodash-es": "^4.17.12",
    "@typescript-eslint/eslint-plugin": "^6.14.0",
    "@typescript-eslint/parser": "^6.14.0",
    "eslint": "^8.55.0",
    "eslint-plugin-react": "^7.33.2",
    "eslint-plugin-react-hooks": "^4.6.0",
    "prettier": "^3.1.1",
    "stylelint": "^16.0.2",
    "stylelint-config-standard": "^36.0.0",
    "typescript": "^5.3.3",
    "vite": "^5.0.8",
    "@vitejs/plugin-react": "^4.2.1",
    "vite-tsconfig-paths": "^4.2.2",
    "vitest": "^1.0.4",
    "@testing-library/react": "^14.1.2",
    "@testing-library/jest-dom": "^6.1.5",
    "playwright": "^1.40.1",
    "husky": "^8.0.3",
    "lint-staged": "^15.2.0",
    "less": "^4.2.0"
  }
}
```

---

## 5. 环境配置

### 5.1 环境变量

```bash
# .env.development
VITE_API_BASE_URL=http://localhost:7001
VITE_APP_TITLE=GraphDB Studio
VITE_APP_VERSION=1.0.0

# .env.production
VITE_API_BASE_URL=/api
VITE_APP_TITLE=GraphDB Studio
VITE_APP_VERSION=1.0.0
```

### 5.2 TypeScript 配置

```json
// tsconfig.json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
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
      "@assets/*": ["src/assets/*"]
    }
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

---

## 6. 与 nebula-studio 的差异

| 方面 | nebula-studio | GraphDB |
|------|---------------|---------|
| 状态管理 | MobX | Zustand |
| 国际化 | @vesoft-inc/i18n | react-i18next |
| 查询语言 | nGQL | Cypher |
| 后端通信 | WebSocket + HTTP | HTTP |
| 查询编辑器 | Monaco Editor | Ant Design TextArea（简化） |
| 数据导入 | 完整功能 | 简化/暂不实现 |
| LLM 集成 | 有 | 无 |
| 图可视化 | 有 | 阶段 6 实现 |

---

## 7. 参考文档

- [React 官方文档](https://react.dev/)
- [TypeScript 官方文档](https://www.typescriptlang.org/)
- [Vite 官方文档](https://vitejs.dev/)
- [Ant Design 官方文档](https://ant.design/)
- [Zustand 文档](https://docs.pmnd.rs/zustand)
- [React Router 文档](https://reactrouter.com/)
- [Ant Design Input 文档](https://ant.design/components/input)
- [Monaco Editor 文档](https://microsoft.github.io/monaco-editor/)（后续升级参考）
- [Cytoscape.js 文档](https://js.cytoscape.org/)（阶段 6 参考）

---

**文档结束**
