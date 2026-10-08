# 前端 i18n 集成指南

**文档版本**: v4.0
**最后更新**: 2026-10-05

> 本文档描述前端实际采用的国际化方案（Paraglide JS）。词条结构、类型约束与校验流程均与 `frontend/messages/**` 与 `frontend/src/lib/i18n/index.ts` 保持一致。

---

## 1. 当前实现状态

### 1.1 采用方案

| 项目 | 实现 |
|------|------|
| i18n 库 | Paraglide JS（`@inlang/paraglide-js`），构建期经 `paraglideVitePlugin` 编译 |
| 词条资源 | `frontend/messages/en.json` / `zh.json`（单层点号 key，各 297 条） |
| 生成产物 | `frontend/src/lib/paraglide/`（不入库，由构建生成） |
| 取词入口 | `frontend/src/lib/i18n/index.ts`，导出 `t()` / `message()` / `MessageKey` / `setLocale()` |
| 类型约束 | key 由生成的类型推导，`t('key')` 在编译期校验 |
| 语言检测与持久化 | Paraglide `localStorage` 策略，键名 `linkrs_language`；回退顺序为浏览器语言 → 基准语言 `en` |
| 语言切换 | `LanguageSwitcher` 调用 `setLocale()`，整档重载文档 |
| 一致性校验 | `frontend/scripts/check-i18n.mjs`，`npm run check` 首步执行 |

### 1.2 编译接入

`vite.config.ts` 中挂载插件：

```typescript
paraglideVitePlugin({
  project: './project.inlang',
  outdir: './src/lib/paraglide',
  emitTsDeclarations: true,
  strategy: ['localStorage', 'preferredLanguage', 'baseLocale'],
  localStorageKey: 'linkrs_language',
})
```

策略顺序的含义：用户显式选择优先于浏览器语言，最后回退到基准语言。应用是纯客户端渲染（SPA），服务端读不到 `localStorage`，因此不引入 cookie 策略——若将来开启 SSR，再在数组前补 `cookie`。

`project.inlang/settings.json` 声明基准语言、语言列表与词条路径规则；`pathPattern` 相对项目根目录解析，因此词条放在 `frontend/messages/` 而非 `project.inlang/` 内。

---

## 2. 词条资源

### 2.1 结构规范

词条为**单层 JSON**，key 是点号路径：

```jsonc
// messages/en.json（节选）
{
  "app.title": "Linkrs Studio",
  "common.cancel": "Cancel",
  "common.confirmDelete": "Delete \"{name}\"? This cannot be undone."
}
```

取词即 `t('app.title')`、`t('common.confirmDelete', { name })`。

> key 名沿用迁移前的嵌套路径（`common.login` 而非 `common_login`），以保持既有标识稳定。Paraglide 官方建议迁移时不为换风格而重命名既有 key。

### 2.2 命名空间约定

| 前缀 | 用途 | 条目数 |
|------|------|--------|
| `app.*` | 应用级标题（含 `document.title`） | 1 |
| `common.*` | 跨模块通用词汇、操作、无障碍标签 | 50 |
| `errors.*` | 数据层（store / service / utils）的错误兜底文案 | 32 |
| `notification.*` | 轻提示与页面级加载失败文案 | 9 |
| `login.*` | 登录页 | 3 |
| `sidebar.*` | 侧边栏导航项 | 11 |
| `console.*` | 查询控制台 | 59 |
| `schema.*` | Schema 页 | 15 |
| `dataBrowser.*` | 数据浏览（含过滤面板与操作符） | 24 |
| `graph.*` | 图可视化 | 12 |
| `graphPreview.*` | 结果页图预览操作条（三处页面共用） | 6 |
| `mainPage.*` | 首页模块说明 | 7 |
| `monitoring.*` | 监控指标 | 65 |
| `theme.*` | 主题切换按钮 | 3 |

命名规则：

- 同一概念只保留一处词条。
- 数据驱动场景（导航项、布局下拉、过滤算子）返回**消息函数**而非 key 字符串。
- **产品名 / 品牌名 / 技术标识不翻译**：`Linkrs`、`CSV`、`JSONL`，以及 `DATA_TYPE_LABELS` 中的类型名（`Fixed String`、`Geography LineString` 等）——它们是后端类型标识，翻译会与查询语法脱节。

### 2.3 插值

使用 ICU MessageFormat 语法，值直接作为第二参数传入：

```jsonc
"console.historyReceivedTotal": "received {received} of {total}"
```

```svelte
{t('console.historyReceivedTotal', { received: item.receivedCount, total: item.reportedTotal })}
```

数字与时长等格式化统一走 `Intl`（`$utils/metricsFormat`），跟随当前语言，不在词条里硬编码 locale。

---

## 3. 组件中使用

### 3.1 取词

```svelte
<script lang="ts">
  import { t } from '$i18n';
</script>

<h1>{t('console.title')}</h1>
<button>{t('common.refresh')}</button>
```

`t()` 是普通函数，在模板、事件回调、`confirm()` 等任何位置都直接可调用，不需要 store 前缀。

**不要**从 `$paraglide` 或 `$lib/i18n` 之外的路径导入消息。经 `$i18n` 导出的 `t` 携带 `MessageKey` 类型，可对 key 做编译期校验。

### 3.2 数据驱动的消息

导航项、下拉选项等持有消息函数，而不是 key 字符串：

```typescript
// $utils/graphLayout.ts
import { message } from '$i18n';

export function getLayoutOptions(): { label: () => string; value: LayoutType }[] {
  return [
    { label: message('graph.force'), value: 'force' },
    { label: message('graph.circle'), value: 'circle' },
  ];
}
```

```svelte
{#each layoutOptions as opt (opt.value)}
  <option value={opt.value}>{opt.label()}</option>
{/each}
```

同理，`Sidebar` 的菜单项、`FilterPanel` 的算子列表、`StreamingResult` 的状态文案都用消息函数：

```typescript
const OPERATORS = [
  { value: 'eq', label: message('dataBrowser.op.eq') },
  { value: 'ne', label: message('dataBrowser.op.ne') },
  // ...
];
```

这样拼错 key 仍是编译错误，同时避免了模板字符串动态拼 key（`t(\`dataBrowser.op.${op}\`)`）——那种写法既失去类型检查，也无法被 tree-shake 分析。

### 3.3 数据层的错误文案

store / service 层产出错误文案时直接取词，得到的就是当前语言的文本：

```typescript
// $lib/stores/console.ts
error: { code: 'EXECUTION_ERROR', message: t('errors.executeQuery') }
```

因为切换语言会重载文档，错误信息在抛出时翻译一次即可，无需把 key 存进状态再在渲染层解析。

`notificationStore` 是唯一的例外：它存 `MessageKey` + `values`，由 `Toast` 在渲染时取词，以便提示与当前语言始终一致：

```typescript
notificationStore.error('notification.loadNeighborsFailed', undefined, err.message);
```

---

## 4. 语言切换组件

**文件路径**: `src/lib/components/common/LanguageSwitcher.svelte`

要点：

- 语言列表来自 `$i18n` 的 `SUPPORTED_LOCALES`，新增语言只需在 `project.inlang/settings.json` 的 `locales` 中登记并补词条文件。
- 持久化由 Paraglide 的 `localStorage` 策略负责，组件内不直接写 `localStorage`。
- 语言名用**.endonym**（`English` / `中文`），本身不可翻译；按钮带 `lang` 属性与 `aria-pressed`，读屏软件才能正确发音并播报选中态。
- 切换调用 `setLocale()`，Paraglide 会重载文档——这是它保证 URL、`document` 语言与文档级状态一致的机制。代价是控制台页的查询结果与流式游标会丢失。

---

## 5. 类型约束

`src/lib/i18n/index.ts` 从生成的消息映射推导 key 全集：

```typescript
import { m } from '$paraglide/messages.js';

export type MessageKey = keyof typeof m;

export function t(key: MessageKey, values?: Record<string, string | number>): string {
  return (m[key] as (values?: Record<string, unknown>) => string)(values);
}
```

效果：任何拼错的 key 都是编译错误，而不是运行时在界面上显示原始 key 路径；未使用的消息不会进入产物。

---

## 6. 一致性校验

**文件路径**: `frontend/scripts/check-i18n.mjs`

```shell
npm run check:i18n     # 仅跑 i18n 校验
npm run check          # 先跑 i18n 校验，再跑 svelte-check
```

校验内容（任一不通过即退出码 1）：

1. **词条集合一致**：`zh.json` 与 `en.json` 的 key 集合必须完全相同，不允许单边多出或少掉词条。
2. **非空文案**：非参考语言的空词条视为遗漏翻译。
3. **词条为字符串**：词条库不接受数字、数组等非字符串值。

代码侧的 key 存在性由生成的消息类型保证，不再需要正则扫描源码。

---

## 7. 维护要求

1. **新增词条必须同时写入 `messages/en.json` 与 `messages/zh.json`**；漏写会被 `npm run check` 直接拦下。
2. **引用词条时从 `$i18n` 导入**（`t` / `message` / `SUPPORTED_LOCALES`），不要绕过门面直接引生成目录，否则失去统一的 key 类型来源。
3. **数据驱动场景用 `message()` 返回消息函数**，不要返回 key 字符串或拼好的文案。
4. **不要在词条里硬编码 locale**，数字、百分比、字节、时长统一由 `$utils/metricsFormat` 的 `Intl` 格式化函数输出。
5. **产品名 / 品牌名 / 技术标识不翻译**（详见 2.2）。
6. 修改词条后无需手动编译，`vite dev` / `vite build` 会自动重新生成 `src/lib/paraglide`。

---

## 8. 与早期版本的差异

| 方面 | 早期实现（svelte-i18n） | 当前实现（Paraglide JS） |
|------|-------------------------|--------------------------|
| 运行时 | 运行时 store，异步加载词包 | 编译期生成消息函数，无运行时加载 |
| 词条结构 | 嵌套 JSON | 单层点号 key（key 名不变） |
| key 类型 | 手写 `MessageLeaves<T>` 从 `en.json` 推导 | 由生成的消息映射推导 |
| 取词写法 | 模板内 `$t('key')`，非模板处 `get(t)('key')` | `t('key')`，任何位置一致 |
| 数据驱动 | 存 `MessageKey` 字符串，渲染时 `t(key)` | 存消息函数，渲染时 `label()` |
| 首屏 | `main.ts` 中 `await ready` 后再挂载 | 无异步等待，消息随产物内联 |
| 语言检测 | 自写 `localStorage` + navigator 逻辑 | Paraglide 策略链配置 |
| 一致性校验 | 校验词条对齐 + 正则扫描源码引用 | 仅校验词条对齐（key 存在性由类型保证） |
| 语言切换 | 原地响应式重渲染 | 整档重载（框架默认语义） |
| 页面标题 | `App.svelte` 中 `$effect` 写 `document.title` | 根布局 `<svelte:head>` |

---

## 9. 参考文档

- [Paraglide JS 官方文档](https://paraglidejs.com)
- [前端技术栈](./architecture/tech_stack.md)
- [前端目录结构](./architecture/directory_structure.md)

---

**文档结束**
