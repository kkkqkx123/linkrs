# 前端 i18n 集成指南

**文档版本**: v3.0
**最后更新**: 2026-10-05

> 本文档描述前端实际采用的国际化方案（`svelte-i18n`）。词条结构、类型约束与校验流程均与 `frontend/src/lib/i18n/**` 保持一致。

---

## 1. 当前实现状态

### 1.1 采用方案

| 项目 | 实现 |
|------|------|
| i18n 库 | `svelte-i18n` (^4.0.1) |
| 初始化入口 | `src/lib/i18n/index.ts`（`src/main.ts` 中 `import { ready } from './lib/i18n'` 并 `await ready`） |
| 词条资源 | `src/lib/i18n/locales/en.json` / `zh.json`（**嵌套 JSON**，各 242 条） |
| 类型约束 | `MessageKey` 由 `en.json` 推导，`$t()` 的字面量 key 在编译期校验 |
| 语言切换 | `$components/common/LanguageSwitcher.svelte` → `setLocale()` |
| 取词方式 | `import { t } from '$i18n'`，模板中 `$t('key')` |
| 语言持久化 | `localStorage['graphdb_language']`（由 `setLocale()` 统一写入） |
| 一致性校验 | `frontend/scripts/check-i18n.mjs`，`npm run check` 首步执行 |

### 1.2 初始化代码

**文件路径**: `src/lib/i18n/index.ts`（节选，完整实现见源码）

```typescript
const LOADERS = {
  en: () => import('./locales/en.json'),
  zh: () => import('./locales/zh.json'),
};

export type Locale = keyof typeof LOADERS;
export const SUPPORTED_LOCALES = Object.keys(LOADERS) as Locale[];

for (const code of SUPPORTED_LOCALES) {
  register(code, LOADERS[code]);
}

export const ready: Promise<void> = Promise.resolve(
  init({
    fallbackLocale: FALLBACK_LOCALE,
    initialLocale: resolveInitialLocale(),
  }),
);
```

**入口接入**: `src/main.ts`

```typescript
import { ready } from './lib/i18n';

await ready;

mount(App, { target: document.getElementById('app')! });
```

> `register()` 使用动态 `import()`，语言包在构建产物中各自独立 chunk（`en-*.js` / `zh-*.js`）。
> `main.ts` 必须 `await ready` 后再 `mount()`：否则首屏会在词条加载完成前渲染原始 key 路径。

**路径别名**: `tsconfig.app.json` 中 `"$i18n": ["./src/lib/i18n/index.ts"]`，同时开启 `resolveJsonModule` 以支持 `import type ... from './locales/en.json'`。

---

## 2. 词条资源

### 2.1 结构规范

词条采用**嵌套 JSON**，按命名空间分层。取词路径与嵌套层级一一对应：

```jsonc
// en.json / zh.json 节选
{
  "app": { "title": "GraphDB Studio" },
  "common": {
    "login": "Login",
    "cancel": "Cancel",
    "close": "Close",
    "confirmDelete": "Delete \"{name}\"? This cannot be undone."
  },
  "login": { "usernamePlaceholder": "Enter username" }
}
```

对应的取词写法即 `$t('app.title')`、`$t('common.login')`、`$t('common.confirmDelete', { values: { name } })`。

> 早期版本使用「扁平点号 key」，已废弃。嵌套结构是 `svelte-i18n` 的通用做法，且能直接通过 `keyof` 推导 `MessageKey`。

### 2.2 命名空间约定

| 前缀 | 用途 | 条目数 |
|------|------|--------|
| `app.*` | 应用级标题（含 `document.title`） | 1 |
| `common.*` | 跨模块通用词汇与操作 | 44 |
| `login.*` | 登录页 | 3 |
| `sidebar.*` | 侧边栏导航项与各 Schema 资源名称 | 11 |
| `console.*` | 查询控制台 | 60 |
| `schema.*` | Schema 页（含 ER 图） | 15 |
| `dataBrowser.*` | 数据浏览（含过滤面板与操作符） | 24 |
| `graph.*` | 图可视化 | 12 |
| `mainPage.*` | 首页模块说明 | 7 |
| `monitoring.*` | 监控指标 | 65 |

命名规则：

- 同一概念只保留一处词条。历史上散落在 `schema.*` / `sidebar.*` / `monitoring.*` 的同名词条已合并到唯一 key。
- 若两个命名空间下的同一概念在中文里措辞不同（例如「空间」与「空间数」），则各自保留独立 key，不做合并。
- 数据驱动场景（导航菜单、图布局下拉）返回 `MessageKey` 而非文案本身。

### 2.3 插值

使用 ICU MessageFormat 语法，值经 `values` 传入：

```jsonc
"console": {
  "historyReceivedTotal": "received {received} of {total}",
  "autoDecisionStream": "Auto: estimated {estimated} rows (> {threshold}) routed to stream"
}
```

```svelte
{$t('console.historyReceivedTotal', { values: { received: item.receivedCount, total: item.reportedTotal } })}
```

数字与时长等格式化统一走 `Intl`（见 `$utils/metricsFormat`），跟随当前语言，不在词条里硬编码 locale。

---

## 3. 组件中使用

### 3.1 取词

```svelte
<script lang="ts">
  import { t } from '$i18n';
</script>

<h1>{$t('console.title')}</h1>
<button>{$t('common.refresh')}</button>
```

Svelte 中使用前缀 `$` 订阅 store：`$t`。带插值时用 `$t('key', { values: { ... } })`。

**不要**从 `svelte-i18n` 直接导入。经 `$i18n` 导出的 `t` 携带 `MessageKey` 类型，可对 key 做编译期校验。

### 3.2 非响应式上下文

在事件回调、`confirm()` 等非模板上下文中，用 `get` 读取 store 当前值：

```svelte
import { get } from 'svelte/store';
import { t } from '$i18n';

if (confirm(get(t)('common.confirmDelete', { values: { name } }))) {
  // ...
}
```

### 3.3 数据驱动的 key

导航菜单、下拉选项等把 key 交给 `$t()`，并用 `MessageKey` 约束类型：

```typescript
// $utils/graphLayout.ts
import type { MessageKey } from '$i18n';

export function getLayoutOptions(): { labelKey: MessageKey; value: LayoutType }[] {
  return [
    { labelKey: 'graph.force', value: 'force' },
    { labelKey: 'graph.circle', value: 'circle' },
  ];
}
```

```svelte
{#each layoutOptions as opt (opt.value)}
  <option value={opt.value}>{$t(opt.labelKey)}</option>
{/each}
```

动态拼接 key 时，必须把类型标注为 `MessageKey` 并保证拼接结果落在词条集合内：

```typescript
function cardStatusKey(status: StreamCardState['status']): MessageKey {
  if (status === 'pending') return 'console.streamCardPending';
  // ...
}
```

---

## 4. 语言切换组件

**文件路径**: `src/lib/components/common/LanguageSwitcher.svelte`

```svelte
<script lang="ts">
  import { locale, setLocale, SUPPORTED_LOCALES, type Locale } from '$i18n';

  const LABELS: Record<Locale, string> = { en: 'EN', zh: '中文' };
</script>

{#each SUPPORTED_LOCALES as code (code)}
  <button onclick={() => setLocale(code)}>{LABELS[code]}</button>
{/each}
```

要点：

- 语言列表来自 `$i18n` 的 `SUPPORTED_LOCALES`，新增语言只需在 `LOADERS` 中登记。
- 持久化由 `setLocale()` 负责，组件内不再直接写 `localStorage`。
- 切换后 `svelte-i18n` 自动同步 `document.documentElement.lang`；`document.title` 由 `src/App.svelte` 中的 `$effect` 跟随 `$t('app.title')` 更新。

---

## 5. 类型约束

`src/lib/i18n/index.ts` 从 `en.json` 推导 key 全集：

```typescript
import type en from './locales/en.json';

type MessageLeaves<T> = T extends string
  ? never
  : { [K in keyof T & string]: T[K] extends string ? K : `${K}.${MessageLeaves<T[K]>}` }[keyof T & string];

export type MessageKey = MessageLeaves<typeof en>;
```

`import type` 为纯类型导入，构建时会被完全擦除，因此不影响语言包的代码分包。

效果：任何拼错的 key 都是编译错误，而不是运行时在界面上显示原始 key 路径。

```text
Error: Argument of type '"common.loding"' is not assignable to parameter of type
'"app.title" | "common.search" | ... | "monitoring.queriesAvg"'. (ts)
```

---

## 6. 一致性校验

**文件路径**: `frontend/scripts/check-i18n.mjs`

```shell
npm run check:i18n     # 仅跑 i18n 校验
npm run check          # 先跑 i18n 校验，再跑 svelte-check 与 tsc
```

校验内容（任一不通过即退出码 1）：

1. **词条集合一致**：`zh.json` 与 `en.json` 的 key 集合必须完全相同，不允许单边多出或少掉词条。
2. **引用有定义**：代码中出现的每个字面量 key（`$t(...)`、`get(t)(...)`、`label:` / `labelKey:` / `messageKey:` 字段）都必须在词条库中定义。
3. **非空文案**：非参考语言的空词条视为遗漏翻译。
4. **动态前缀**：`$t(\`dataBrowser.op.${op}\`)` 这类动态拼接按前缀整体校验，并列出已解析的前缀。

扫描范围限定在 i18n 调用参数与 key 字段内，因此不会把 `link.download = 'graph.png'`、Monaco 的 `'number.float'` 等同名字符串误判为词条。

---

## 7. 维护要求

1. **新增词条必须同时写入 `en.json` 与 `zh.json`**；漏写会被 `npm run check` 直接拦下。
2. **引用词条时从 `$i18n` 导入 `t` / `locale`**，不要从 `svelte-i18n` 直接导入，否则失去 key 类型校验。
3. **数据驱动场景返回 `MessageKey`**，不要返回已拼好的文案字符串。
4. **不要在词条里硬编码 locale**，数字、百分比、字节、时长统一由 `$utils/metricsFormat` 的 `Intl` 格式化函数输出。
5. **产品名 / 品牌名不翻译**（如侧边栏的 `GraphDB` 字样）。
6. **修改后执行同步脚本**，让 `frontend-preview` 保持一致：

   ```shell
   bash scripts/sync-frontend-preview.sh
   ```

   `frontend-preview/package.json` 由 preview 自行维护，其中必须保留 `check:i18n` 脚本；同步脚本会校验这一项存在。

---

## 8. 与早期版本的差异

| 方面 | 早期实现 | 当前实现 |
|------|---------|---------|
| 词条结构 | 扁平点号 key | 嵌套 JSON |
| key 类型 | 无，`t` 直接来自 `svelte-i18n` | `MessageKey` 推导，编译期校验 |
| 词条一致性 | 文档口头约定 | `npm run check:i18n` 强制校验 |
| 首屏渲染 | `init()` 未等待 | `main.ts` 中 `await ready` 后再 `mount` |
| 语言列表 | `LanguageSwitcher` 内硬编码 | 由 `LOADERS` / `SUPPORTED_LOCALES` 推导 |
| 语言持久化 | 组件内直接写 `localStorage` | `setLocale()` 统一处理 |
| 页面标题 | `index.html` 硬编码 `frontend-svelte` | 跟随 `$t('app.title')` |
| 数字格式化 | `toLocaleString('en-US')` 硬编码 | `Intl.NumberFormat(locale)` |

---

## 9. 参考文档

- [svelte-i18n 官方文档](https://github.com/kaisermann/svelte-i18n)
- [前端技术栈](./architecture/tech_stack.md)
- [前端目录结构](./architecture/directory_structure.md)

---

**文档结束**