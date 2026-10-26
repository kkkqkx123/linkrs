# 前端约束与设计决策

本文档整理 `frontend`（linkrs-studio，SvelteKit + Svelte 5 + Vite）当前的构建约束与架构设计决策，供后续开发对照，避免无意破坏既有取舍。现状体积数据见 `docs/plan/frontend-bundle-analysis.md`。

## 一、构建与工具链约束

### 静态站点适配（adapter-static）

- 使用 `@sveltejs/adapter-static`，`fallback: 'index.html'`，产物写入 `frontend/build`。
- 应用为纯客户端渲染（CSR），SSR 环境仅用于构建期校验；新增页面不得依赖服务端数据加载（`+page.server.ts`）。

### 模式（mode）与 API 客户端别名

- `preview` 与 `mock` 模式下，`$lib/api/client` 被别名替换为 `client.preview.ts`；真实模式使用生成的 openapi-fetch 客户端。
- 新增 API 调用必须经由 `$lib/api/client`，不得绕过别名机制直接 import。

### 语法 wasm（grammarWasm 插件）

- `vite/grammarWasm.ts` 定制插件把 `crates/linkrs-grammar` 的 wasm 与高亮 query 接入构建；dev server `fs.allow` 需包含 `../crates/linkrs-grammar`。
- 该插件的 `buildStart` 是构建耗时大头（约占 8s），属于已知成本。

### bundle 报告门控

- `rollup-plugin-visualizer` 仅在 `VITE_BUNDLE_REPORT=1` 时启用（`template: 'markdown'`、`emitFile: true`、gzip + brotli）。普通构建不生成 `bundle-report.md`，也不随站点发布。
- 报告产出位置：`.svelte-kit/output/client/bundle-report.md`（及 `build/bundle-report.md`）。

### 体积告警阈值

- `build.chunkSizeWarningLimit: 600`（KB）。monaco 合并 chunk（约 3.2 MB）必然触发告警，属已知豁免，勿通过调高阈值"消除"其他 chunk 的问题。

## 二、重型依赖的加载策略

### monaco-editor

- 唯一入口是 `$utils/monacoSetup`，由 `CypherEditor.svelte` 动态 `import()`；任何代码不得静态 import monaco。
- `monacoSetup.ts` 显式列出 contrib（suggest、bracketMatching、find、hover、clipboard、contextmenu、linesOperations、wordOperations、comment、lineSelection、placeholderText、coreCommands）。**约束：contrib 必须静态引入**——Monaco 在每次 `editor.create` 时读取贡献注册表，延迟导入无法事后补注册，除非重建编辑器。
- 未引入的 contrib（folding、multicursor、cursorUndo、smartSelect、indentation、tabFocus）为刻意裁剪；需要新增 contrib 时先确认查询控制台场景确实用到。
- `editor.api.js` 而非 `monaco-editor` 根入口，避免拉入 json/css/html/typescript 语言服务及其 worker。

### web-tree-sitter 与语法 wasm

- `treeSitterCypher.ts` 对 monaco 只允许**类型引用**（`type Monaco = typeof import(...)`），不得出现运行时 import，否则 tree-sitter 高亮链路会把 monaco 拖回依赖图。
- 该模块由 `monacoCypher.ts` 在语言注册后动态 `import()`；web-tree-sitter（约 114 KB）与 `tree-sitter-linkrs.wasm`（1 MB，gzip 93 KB）仅在编辑器首次请求语义 token 时加载。
- `web-tree-sitter.js` 内部使用直接 `eval`（第三方行为），构建会告警；本地无法修复，产出物如启用 CSP 需注意。

### cytoscape

- `CytoscapeCanvas.svelte` 内动态 `import('cytoscape')`（约 816 KB raw / gzip 138 KB），整库单 chunk。
- 侧边栏 `Sidebar.svelte` 悬停 `/graph`、`/schema`、`/console` 导航项时预取该 chunk（`prefetchGraphChunk`，仅触发一次）；**约束：不要改为全局空闲预取**，否则所有页面都要为 138 KB gzip 买单。
- 结果组件（`CursorResult`/`StreamingResult`/`MaterializedResult`）通过 `LazyCytoscapeCanvas.svelte` 包装间接使用：仅在图预览真正渲染时才加载画布组件与 cytoscape。新增带图预览的结果视图应复用该包装，而非直接静态引入 `CytoscapeCanvas`。

### paraglide i18n

- 消息经 `$i18n`（`src/lib/i18n/index.ts`）单入口引入：`import { m } from '$paraglide/messages.js'`。**已知取舍**：`t()` 支持数据驱动 key（导航项、选项列表），命名空间引入使消息无法按路由 tree-shake，全量消息 chunk（gzip 约 21-119 KB 视统计口径）为共享成本。组件不得直接 import `$paraglide/messages/*`，统一走 `$i18n`。
- 语言策略：`['localStorage', 'preferredLanguage', 'baseLocale']`，显式选择优先于浏览器语言；键 `linkrs_language`。默认 locale 为 `en`。
- 消息目录由 `npm run check:i18n` 校验（`scripts/check-i18n.mjs`），新增 key 必须同时补全 en/zh。

## 三、代码与工程约束

- **类型安全**：禁止 `unwrap`/`as any` 逃逸检查；monaco 命名空间类型（`editor.ITextModel`、`languages.SemanticTokensLegend` 等）在 `editor.api.d.ts` 中不作为命名空间属性暴露，需要时用结构化接口（参见 `treeSitterCypher.ts` 的 `TextModelLike`/`TokensLegend`）。
- **检查命令**：提交前跑 `npm run check`（i18n 目录 + svelte-check）、`npm run lint`；格式化用 `prettier`（tab 缩进）。
- **路径别名**：`$lib`、`$types`、`$utils`、`$services`、`$stores`、`$config`、`$i18n`、`$components`、`$paraglide`，新模块归位到对应别名目录。
- **开发端口与代理**：dev server 端口 3011，`/v1` 与 `/api` 代理到 `localhost:9758`。

