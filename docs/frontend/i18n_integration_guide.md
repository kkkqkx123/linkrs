# 前端 i18n 集成指南

**文档版本**: v2.0  
**最后更新**: 2026-04-15

> 本文档描述前端**实际采用**的国际化方案（`svelte-i18n`）。v1.0 曾按 React 生态规划（`i18next` + `react-i18next`），工程实现阶段改为 Svelte 生态；本版已按 `frontend/src/lib/i18n/**` 与真实组件代码全面修订。

---

## 1. 当前实现状态

### 1.1 采用方案

| 项目 | 实现 |
|------|------|
| i18n 库 | `svelte-i18n` (^4.0.1) |
| 初始化入口 | `src/lib/i18n/index.ts`（在 `src/main.ts` 中 `import './lib/i18n'`） |
| 词条资源 | `src/lib/i18n/locales/en.json` / `zh.json`（**扁平 key**，各 146 条） |
| 语言切换 | `$components/common/LanguageSwitcher.svelte` |
| 取词方式 | 组件内 `import { t } from 'svelte-i18n'`，模板中用 `$t('key')` |
| 语言持久化 | `localStorage['graphdb_language']` |

### 1.2 初始化代码

**文件路径**: `src/lib/i18n/index.ts`

```typescript
import { register, init, getLocaleFromNavigator } from 'svelte-i18n';

register('en', () => import('./locales/en.json'));
register('zh', () => import('./locales/zh.json'));

init({
  fallbackLocale: 'en',
  initialLocale: localStorage.getItem('graphdb_language') || getLocaleFromNavigator() || 'en',
});
```

**入口接入**: `src/main.ts`

```typescript
import './lib/i18n';
```

---

## 2. 词条资源

### 2.1 结构规范

词条采用**扁平 key**（非嵌套 JSON），以 `.` 分隔命名空间：

```
# en.json / zh.json 节选
{
  "app.title": "GraphDB Studio",
  "common.login": "Login",
  "common.logout": "Logout",
  "common.username": "Username",
  "common.password": "Password",
  "common.rememberMe": "Remember me",
  "common.connected": "Connected",
  "common.disconnected": "Disconnected"
}
```

> v1.0 曾采用嵌套结构（`{ "common": { "login": "Login" } }`）；现已改为扁平 key，`$t('common.login')` 取值方式不变。

### 2.2 命名空间约定

| 前缀 | 用途 | 示例 |
|------|------|------|
| `app.*` | 应用级标题 | `app.title` |
| `common.*` | 通用词汇 | `common.login`、`common.cancel` |
| `login.*` | 登录页 | `login.usernamePlaceholder` |
| `header.*` | 头部组件 | `header.title` |
| `navigation.*` | 导航菜单 | `navigation.console` |
| `console.*` | 查询控制台 | `console.executing` |
| `schema.*` | Schema 页（含 ER 图） | `schema.erGraph`、`schema.erNoData` |
| `graph.*` | 图可视化 | `graph.layout` |
| `dataBrowser.*` | 数据浏览（含过滤面板） | `dataBrowser.filterPanel.title` |

### 2.3 维护要求（重要）

> **新增词条必须同时写入 `en.json` 与 `zh.json`**，两个文件 key 集合必须完全一致，否则会出现运行时缺词。

---

## 3. 组件中使用

### 3.1 取词

```svelte
<script lang="ts">
  import { t } from 'svelte-i18n';
</script>

<h1>{$t('console.title')}</h1>
<button>{$t('common.refresh')}</button>
```

> Svelte 中使用前缀 `$` 订阅 store：`$t`。带插值时用 `$t('key', { values: { name } })`。

### 3.2 示例：Header 组件

**文件路径**: `src/lib/components/layout/Header.svelte`（节选）

```svelte
<script lang="ts">
  import { t } from 'svelte-i18n';
  import LanguageSwitcher from '$components/common/LanguageSwitcher.svelte';
</script>

<header class="...">
  <span>{$t('header.title')}</span>
  <LanguageSwitcher />
</header>
```

### 3.3 示例：Login 页面

**文件路径**: `src/lib/pages/Login/Login.svelte`（节选）

```svelte
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { connectionStore } from '$stores/connection';

  let username = $state('');
  let password = $state('');

  async function handleSubmit() {
    // ...
  }
</script>

<form onsubmit={handleSubmit}>
  <input bind:value={username} placeholder={$t('login.usernamePlaceholder')} />
  <input type="password" bind:value={password} placeholder={$t('login.passwordPlaceholder')} />
  <button type="submit">{$t('common.login')}</button>
</form>
```

---

## 4. 语言切换组件

**文件路径**: `src/lib/components/common/LanguageSwitcher.svelte`

```svelte
<script lang="ts">
  import { locale } from 'svelte-i18n';

  const languages = [
    { code: 'en', label: 'EN' },
    { code: 'zh', label: '中文' },
  ];

  function switchLang(code: string) {
    $locale = code;
    localStorage.setItem('graphdb_language', code);
  }
</script>

<div class="flex items-center gap-1 text-sm">
  {#each languages as lang}
    <button
      class="px-2 py-0.5 rounded cursor-pointer transition-colors {$locale === lang.code ? 'bg-blue-100 text-blue-600 font-medium' : 'text-gray-500 hover:text-gray-700'}"
      onclick={() => switchLang(lang.code)}
    >
      {lang.label}
    </button>
  {/each}
</div>
```

> 直接对 `$locale` 赋值即可触发全应用语言切换；同步写入 `localStorage` 保证刷新后保持。
> `LanguageSwitcher` 已在 `Header` 组件中集成。

---

## 5. 与 v1.0 规划的差异

| 方面 | v1.0 规划（React） | 实现（Svelte） |
|------|-------------------|----------------|
| 核心库 | `i18next` | `svelte-i18n` |
| 框架集成 | `react-i18next` + `initReactI18next` | `register` / `init` + `$t` |
| 资源结构 | 嵌套 JSON（`common: { login }`） | 扁平 key（`common.login`） |
| 词条路径 | `src/i18n/locales/*.json` | `src/lib/i18n/locales/*.json` |
| 切换方式 | `i18n.changeLanguage()` | `$locale = code` |
| 语言检测 | `i18next-browser-languagedetector` | `getLocaleFromNavigator()` |

---

## 6. 迁移状态

规划中列出的待迁移文件**均已迁移完成**（当前组件已全量使用 `$t()`）。下列为已接入 i18n 的核心组件（真实 Svelte 路径）：

- [x] `src/lib/pages/Login/Login.svelte`
- [x] `src/lib/components/layout/Header.svelte`
- [x] `src/lib/components/layout/Sidebar.svelte`
- [x] `src/lib/pages/Console/Console.svelte`
- [x] `src/lib/pages/Schema/Schema.svelte`
- [x] `src/lib/pages/Schema/SchemaErGraph.svelte`
- [x] `src/lib/pages/Graph/Graph.svelte`
- [x] `src/lib/pages/DataBrowser/DataBrowser.svelte`
- [x] `src/lib/pages/DataBrowser/FilterPanel.svelte`
- [x] `src/lib/components/business/SpaceSelector.svelte`
- [x] `src/lib/components/common/LoadingFallback.svelte`
- [x] `src/lib/components/common/LoadingScreen.svelte`
- [x] `src/lib/components/common/HealthMonitor.svelte`
- [x] `src/lib/components/common/LanguageSwitcher.svelte`

---

## 7. 最佳实践

1. **命名规范**：使用 `namespace.key` 格式（如 `login.title`、`common.submit`）
2. **分类组织**：按功能模块划分前缀（`common` / `console` / `schema` / `dataBrowser` 等）
3. **双语同步**：新增 key 时同步更新 `en.json` 与 `zh.json`
4. **插值支持**：`$t('key', { values: { name } })`
5. **复数支持**：`$t('key', { values: { count: 5 } })`
6. **默认回退**：`fallbackLocale` 设为 `en`，缺失词条时回退英文

---

## 8. 参考文档

- [svelte-i18n 官方文档](https://github.com/kaisermann/svelte-i18n)
- [前端技术栈](./architecture/tech_stack.md)
- [前端目录结构](./architecture/directory_structure.md)

---

**文档结束**
