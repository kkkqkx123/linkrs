# Linkrs Studio 前端资源请求与加载逻辑分析

| 项目 | 内容 |
| --- | --- |
| 分析对象 | `frontend/`（SvelteKit 5 + Vite 8 + TypeScript，SPA 静态部署） |
| 分析日期 | 2026-11-21 |
| 分析方法 | 静态源码走查，逐条核对前端调用点与后端 handler；每条结论附 `file:line` 引述 |
| 证据约定 | 路径相对仓库根；`frontend/src/...:N` 表示该文件第 N 行 |
| 代码版本 | 仓库 HEAD `86407d8`（递归含子模块） |

---

## 1. 执行摘要

Linkrs Studio 是一个**纯客户端渲染**的管理台：会话存于 `localStorage`，API 走 `openapi-fetch` 类型化客户端，图/查询/监控 SSE 走原生 `fetch`，Monaco / Cytoscape / tree-sitter 均为动态导入。整体分层清晰、懒加载做得较到位，但存在 **1 项高风险会话注入缺陷**、**4 项中风险语义缺陷**与 **5 项性能隐患**。

| 维度 | 结论 | 严重度 |
| --- | --- | --- |
| 会话头注入 | JSON 客户端读取的 localStorage 键与登录写入的键不一致，`X-Session-ID` 无法注入 | **严重** |
| 401 处理 | 清错键 + 同一响应触发两次登出跳转 | 中 |
| SSE 尾帧 | 两个流的尾帧未消费/未 flush，存在丢帧 | 中 |
| SSE 解析 | 三套解析器分隔符/bigint 语义不一致 | 中 |
| 监控流 | 断流后不重连，永久降级为轮询 | 中 |
| 流式渲染 | 逐行全量拷贝数组，O(n²) | 高（性能） |
| 监控取数 | SSE 与轮询双通道重叠请求 | 中（性能） |
| 请求去重 | schema/space 请求无缓存、无竞态保护 | 中（性能/正确性） |
| 镜像/工具链 | 已按约束完成配置，见附录 A | — |

---

## 2. 请求分层与架构总览

```
浏览器 (SPA, ssr=false)
 ├─ 静态资源：favicon.svg + Vite 产物（app.css / app.js / 动态 chunk / wasm / worker）
 ├─ JSON API  → $lib/api/client (openapi-fetch + bigintFetch)         [统一注入会话头]
 ├─ SSE 流    → 原生 fetch + 自定义解析器                            [http.getSessionHeaders]
 │              ├ /v1/query/stream            (streamQuery.ts)
 │              ├ /v1/statistics/stream       (monitoringStream.ts)
 │              └ /v1/migration/stream/...    (migrationStream.ts)
 └─ 文件传输  → 原生 fetch multipart / 下载                          [resolveSessionId]
```

关键事实：

- `src/routes/+layout.ts:6-8`：`ssr = false`、`prerender = false`，产物为静态 SPA（`vite.config.ts:36` 的 `adapter({ fallback: 'index.html' })`）。
- `src/lib/api/client.ts:9`：仅 `export * from './client.real'`；preview/mock 模式由 `vite.config.ts:11-20` 的 `resolve.alias` 把该路径替换为 `client.preview.ts`，实现二进制切换（编译期常量 `isMockMode`，`client.real.ts:26` / `client.preview.ts:40`）。
- 后端为本地图数据库，`X-Session-ID` 头是主要鉴权凭证（`crates/linkrs-server/src/http/middleware/auth.rs:71-85`），但 `enable_authorize=false` 或 `single_user_mode=true` 时跳过校验（`auth.rs:66-69`）——这一开关决定了下文问题的实际表现。

---

## 3. 资源请求全景

| 类别 | 资源/端点 | 发起位置 | 传输方式 | 会话凭证 |
| --- | --- | --- | --- | --- |
| 静态 | `favicon.svg` | `src/app.html:5-9` | 浏览器 | — |
| 静态 | Monaco worker chunk | `monacoSetup.ts:15,33-35` | Vite worker | — |
| 静态 | tree-sitter wasm / highlight.scm | `treeSitterCypher.ts:20-21` | `?url` / `?raw` | — |
| JSON | `/v1/statistics/*`、`/v1/transactions/metrics`、`/v1/sync/status` | `services/statistics.ts:57-105` | openapi-fetch | header（缺陷见 §5.1）|
| JSON | `/v1/query`、`/v1/query/explain`、`/v1/query/validate` | `services/query.ts:110,194,276` | openapi-fetch | header + body `session_id` |
| JSON | `/api/v1/data/...`（点/边/tag/索引） | `services/graph.ts`、`dataBrowser.ts`、`schema.ts` | openapi-fetch | header |
| SSE | `/v1/query/stream` | `services/streamQuery.ts:134-139` | 原生 fetch | `getSessionHeaders()` + body |
| SSE | `/v1/statistics/stream?interval_ms=2000` | `services/monitoringStream.ts:66-72` | 原生 fetch | `getSessionHeaders()` |
| SSE | `/v1/migration/stream/{space}/{label}` | `services/migrationStream.ts:54-60` | 原生 fetch | `getSessionHeaders()` |
| 文件 | multipart 导入 / `GET /v1/export` | `services/transfer.ts:20-40`、`utils/export.ts:88-93` | 原生 fetch | `resolveSessionId()` |
| i18n | 语言包 | `lib/i18n/index.ts:12-13` | 编译期内联（Paraglide） | — |
| 运行时 CDN | 无 | `app.html` 全文件、`app.css:1` | — | — |

结论：**运行时无国外 CDN 依赖**，所有第三方资源均由 Vite 打包为本地 chunk，与"无国外 CDN"的环境约束一致。

---

## 4. 加载逻辑详解

### 4.1 首屏与认证门禁

- 根布局 `routes/+layout.svelte:1-27` 只挂载 app.css、主题副作用与 Toast。
- 受保护布局 `routes/(app)/+layout.svelte:34-46` 在 `onMount` 中检查：未认证且 `isConnected && !isVerified` 时调用 `connectionStore.checkHealth()`（`stores/connection.ts:245-291`，发 `me()` 请求），仍失败则跳 `/login`。认证态来自 `localStorage` 持久化（`connection.ts:73-100`），含 `expiresAt` 定时登出（`connection.ts:115-124,168-170`）。
- Header 中的 `SpaceSelector.svelte:11-13` 每次挂载调用 `schemaStore.fetchSpaces()`——由于 Header 位于 `(app)/+layout.svelte:65`，**几乎每次页面导航都会重新拉 space 列表**（无缓存，见 §6.4）。

### 4.2 页面级数据加载

| 页面 | 触发方式 | 请求 |
| --- | --- | --- |
| 首页 `(app)/+page.svelte:30-33` | `onMount` → `monitoringStore.start()` | SSE + 8 路轮询（复用 store，不自取） |
| 监控 `monitoring/+page.svelte:118-121` | `onMount` → `monitoringStore.start()` | 同上 |
| data-browser `data-browser/+page.svelte:69-77,133-155` | `$effect` + 交互回调 | fetchTags / getVertices / getEdges / getStatistics |
| console `console/+page.svelte:71` | `onMount` + 编辑器校验 | 历史/draft（本地）+ 防抖 validate（800ms） |
| batch `(app)/batch/+page.svelte:78-84,102-110` | 状态驱动轮询（2s） | `batchService.status()` |
| sync / transactions / users | `onMount(load)` | 各自列表端点 |

`monitoringStore.refresh()`（`stores/monitoring.ts:139-195`）用 `Promise.allSettled` 并发 8 个统计端点并保留上次成功值，失败隔离设计合理。

### 4.3 SSE 流式通道

三条流共用"原生 fetch + `ReadableStream.getReader()` + `TextDecoder(stream:true)` + 增量解析器"骨架，但实现细节各异（见 §5.3、§5.4）：

- 查询流 `streamQuery.ts:160-219`：解析 schema/row/metadata/error/statement_begin/statement_end，遇 `done` 事件停止（`parser.doneReceived`，`sseParser.ts:249-252`）。
- 监控流 `monitoringStream.ts:80-107`：每帧 `snapshot` → `onSnapshot`，`error` → `onErrorEvent`；服务端为无限流 + 10s keepalive（`crates/.../statistics/stream.rs:94-98`）。
- 迁移流 `migrationStream.ts:74-107`：类型化事件，遇终态事件停止。

取消语义：`AbortController` 贯穿 store 与 service（`stores/stream.ts:86-108,124-126`），并用 `generation` 序号防止旧流结果覆盖新查询（`stream.ts:97-108,153`），设计正确。

### 4.4 轮询与定时器

- 监控：轮询 10s（`stores/monitoring.ts:26,197-201`）+ SSE 2s 并行，见 §6.2。
- batch：`setInterval` 2000ms，终态即 `clearInterval`（`batch/+page.svelte:78-84,108`），正确。
- 编辑器：validate 防抖 800ms、草稿保存 300ms（`EditorPanel.svelte:135-148,223-228`），有 `onDestroy` 清理。
- 会话过期：`setTimeout(Math.min(delay, 2^31-1))`（`connection.ts:123`），对超长会话做了 32 位溢出保护。

### 4.5 静态与重型资源加载

| 资源 | 加载方式 | 证据 | 评价 |
| --- | --- | --- | --- |
| Cytoscape (~800KiB) | 动态 import + hover 预取 | `LazyCytoscapeCanvas.svelte:15-19`、`CytoscapeCanvas.svelte:142`、`Sidebar.svelte:117-128` | 优 |
| Monaco | 动态 import，且只引入必要 contribution | `CypherEditor.svelte:97`、`monacoSetup.ts:1-29` | 优 |
| tree-sitter wasm + highlight | 二级动态 import，`?url`/`?raw`，单例初始化 + 缓存 | `monacoCypher.ts:203-207`、`treeSitterCypher.ts:20-21,43-73` | 优 |
| grammar wasm | 构建期生成 + 时间戳缓存 | `vite/grammarWasm.ts:52-68,101-108` | 优 |
| 路由 chunk | `data-sveltekit-preload-data="hover"` | `app.html:13` | 合理 |

---

## 5. 语义正确性问题

### 5.1 【严重】JSON 客户端的会话头注入键与存储键不一致

**证据链：**

1. 会话写入路径：`connection.ts:183` 登录成功后 `storeSessionId(result.session_id)` → `http.ts:56-58 setStoredSessionId(String(id))` → `http.ts:24 storage.set(STORAGE_KEYS.SESSION_ID, id)` → `storage.ts:40 localStorage.setItem(\`${STORAGE_PREFIX}${key}\`, ...)`。其中 `STORAGE_PREFIX = 'linkrs_'`（`storage.ts:1`），`STORAGE_KEYS.SESSION_ID = 'sessionId'`（`constants.ts:60`）。**实际存储键为 `linkrs_sessionId`，且值经 `JSON.stringify`（字符串带引号）。**
2. JSON 客户端读取路径：`client.real.ts:160-170` 的 `onRequest` 中间件用 `localStorage.getItem(STORAGE_KEYS.SESSION_ID)`，即 **`getItem('sessionId')`——裸键，无前缀**（`client.real.ts:163`）。
3. 非 JSON 传输读取路径：`http.ts:37-44 getSessionHeaders()` 用 `storage.get(...)`（带前缀，正确），SSE 与下载走此路径（如 `streamQuery.ts:136`、`export.ts:88-93`）。

**影响：** 键名不匹配导致 `getItem('sessionId')` 恒为 `null`，`openapi-fetch` 客户端**不会注入 `X-Session-ID`**。后端鉴权中间件要求该头（`auth.rs:71-85`），因此所有仅依赖 header、body 不带 `session_id` 的 JSON GET 请求（statistics、schema、graph、dataBrowser 等，`services/statistics.ts:57-105`）在 `enable_authorize=true` 时将返回 401 并被重定向登录。此外，即便把键名改对，`localStorage.getItem` 返回的是 JSON 引号串（如 `"424242"`），直接 `setHeader`（`client.real.ts:164`）会带上多余引号——**存在双重缺陷**。

> 注：在默认关闭鉴权的本地部署下（`auth.rs:66-69`）不会暴露，故 mock/preview 模式测试难以发现。是否在启用鉴权的实例上验证，见 §9。

**建议：** 让客户端统一走 `storage` 抽象，例如 `import { storage } from '$utils/storage'`，在 `client.real.ts:163` 改用 `storage.get<string>(STORAGE_KEYS.SESSION_ID)`；preview 版 `client.preview.ts:45-52` 的 `handleUnauthorized` 同源修正（或统一调用 `http.ts:61-63 clearSessionId()`）。

### 5.2 【中】401 处理重复触发且清理错误的键

- `bigintFetch` 在响应 401 时调用 `handleUnauthorized()`（`client.real.ts:48`）；随后 `call()` 在错误分支对 401 再次调用（`client.real.ts:123-124`）。同一次 401 触发**两次** `removeItem` + `goto('/login')`。
- `handleUnauthorized`（`client.real.ts:33-40`、`client.preview.ts:45-52`）删除的是裸键 `sessionId`，真正的 `linkrs_sessionId` 未被清除；而正确实现 `clearStoredSessionId`（`http.ts:27-35`）同时清理两者。
- 跳转条件 `page.url.pathname !== '/login'` 无法阻止两次调用间的竞态（两次都在 `goto` resolve 前执行）。

**建议：** 移除 `bigintFetch` 中的 401 处理，仅保留 `call()` 一处；`handleUnauthorized` 直接复用 `clearSessionId()`。

### 5.3 【中】SSE 尾帧丢失

- **监控流**：`monitoringStream.ts:84-107` 循环在 `reader.read()` 返回 `done` 时直接 `return`（`:86-88`），从未调用 `frameReader.flush()`；而 `SseFrameReader.flush()` 明确存在（`sseFrames.ts:38-43`）用于"无尾部分隔符的尾帧"。服务端异常断开时最后一批快照可能丢失。
- **查询流**：`streamQuery.ts:206` 的 `parser.feed(decoder.decode())` **丢弃了返回值**（未分发 events）。对比正确实现 `migrationStream.ts:88-91`——后者逐条分发 flush 产出的事件。由于 `SseParser` 在 `done` 时会清空 buffer（`sseParser.ts:249-252`），多数情况下尾数据已被消费，但 decoder 的最终 flush 内容仍被静默丢弃，属于逻辑不完整。

**建议：** 监控流在循环结束后 `for (const f of frameReader.flush())` 分发 snapshot/error；查询流改为 `for (const e of parser.feed(decoder.decode())) { ...分发... }`，复用现有分发逻辑。

### 5.4 【中】三套 SSE 解析器语义不一致

| 解析器 | 分帧实现 | bigint 保真 | 无冒号字段行 |
| --- | --- | --- | --- |
| `SseParser`（`sseParser.ts:76-104`） | 手写 `\n`/`\r`/`\r\n` 双换行 | 是（`json-bigint`，`:65`） | 忽略（`:113`） |
| `SseFrameReader`（`sseFrames.ts:16`） | 正则 `/\r?\n\r?\n/` | 交由调用方（监控流用 `parseJsonTolerant`）| 支持（`:53`） |
| `MigrationSseParser`（`migrationEvents.ts:143`） | 正则 `/\r\n\r\n|\n\n|\r\r/` | 否（`JSON.parse`，`:189`） | 忽略（`:173`） |

三者对分隔符、字段格式、大整数的处理各异。迁移事件的数字（行数、步数）虽在安全整数范围，但规范分歧会增加维护成本，且 `SseParser` 的手写分帧在 `\r` 单独分隔等边界上更复杂、更易错。

**建议：** 抽象一个统一的分帧器（保留 `SseFrameReader` 的正则方案并补 `flush`），各端点仅保留各自的事件映射层。

### 5.5 【中】监控流断线后不重连，永久降级为轮询

`monitoringStore.startStreaming()`（`stores/monitoring.ts:230-246`）只启动一次；流结束或出错后 `streamAbort = null`、状态置为 `stopped/unavailable`（`:241-245`），**无任何重试**。代码注释声称"polling stays on as the fallback"（`monitoring.ts:229`、`monitoringStream.ts:6-7`），但实际是永久退回 10s 轮询，不会再恢复 live。对"近实时"卖点是功能性退化。

**建议：** 在 `startStreaming` 的 outcome 处理中，若非用户主动取消且页面仍在监控/首页，按退避策略（如 2s→4s→8s，上限 30s）重新调用 `startStreaming()`；`stop()` 时清除重试定时器。

### 5.6 【轻】`USE_MOCK` 环境开关为死配置

`env.ts:11-17` 声明 `USE_MOCK`，`frontend/.env.mock:3`、`.env.preview:4` 设置 `USE_MOCK=true`，但全仓库无任何代码读取该变量（仅注释提及）。真正的模式切换靠 `vite.config.ts:11-20` 的 `mode → alias` 与编译期 `isMockMode`。该开关会误导运维（以为改 `.env` 即可切 mock）。

**建议：** 删除该变量与两处 `.env` 设置，或让 `client.ts` 真正据其选择实现。

---

## 6. 性能问题

### 6.1 【高】流式结果逐行全量拷贝，O(n²)

`stores/stream.ts:197-218` 的 `onRow` 回调对**每一行**执行一次 `update()`，并在 `:205` 用 `rows: [...card.rows]` 浅拷贝整个数组。设结果 N 行，则产生 N 次数组拷贝（累计 O(N²) 元素复制）与 N 次 Svelte store 通知/组件重渲染。配合 `placeCardRow`（`:71-84`）按 index 写入，大结果集流式查询会出现明显卡顿甚至阻塞主线程。

**建议：** 将行累积进可变数组，用单个 `version`/`flush` 触发一次更新；或按时间片（如每 16ms / 每 100 行）批量提交。

### 6.2 【中】监控 SSE 与轮询双通道重叠取数

`monitoringStore.start()`（`stores/monitoring.ts:259-263`）同时启动：
- 轮询：每 10s 并发 8 个端点（`:144-155,197-201`），含 `overview/system/database`；
- SSE：每 2s 推送一次完整 `OverviewResponse`（`monitoringStream.ts:66-72`），其 `applyStreamSnapshot`（`monitoring.ts:216-227`）已同步 `overview/system/database`。

即 `overview/system/database` 被两条通道重复拉取，稳态下每 10s 8 个 HTTP 请求 + 每 2s 1 个 SSE。虽有"失败保留旧值"的容错价值，但常开双通道的带宽与后端负载偏高。

**建议：** 让 SSE 成为主通道，仅在 `streamStatus !== 'live'` 时启用轮询作为 fallback；或把轮询精简为 SSE 未覆盖的 5 个端点。

### 6.3 【中】bigintFetch 对所有 JSON 响应做二次全量解析

`client.real.ts:43-63` 对每个 JSON 响应执行 `text()` → `JSONBig.parse` → `JSON.stringify` → `new Response(...)`，上层 `openapi-fetch` 再解析一次。大响应体（监控快照、查询结果）CPU/内存开销翻倍，且重建 `Response` 时仅透传原 headers（`:61`），可能丢失 trailers 等语义。这是为保留大整数精度的代价。

**建议：** 仅对确含大整数字段的端点启用该包装；或评估用流式 token 转换只改写超长整数字面量。

### 6.4 【中】schema/space 请求无缓存、无去重、无竞态保护

- `SpaceSelector.svelte:11-13` 每次挂载 `fetchSpaces()`；`schema.ts:61-80 fetchSpaces`、`:97-109 fetchTags`、`:142-154 fetchEdgeTypes` 均无 in-flight 去重与结果缓存，页面导航造成重复请求。
- 这些 `fetch*` 为"后写覆盖"：快速切换 space 时，旧 space 的慢响应可能晚于新响应到达并覆盖 store（`schema.ts:104,149`），导致 UI 显示错误 space 的 schema。
- `data-browser/+page.svelte:69-71` 的 `$effect` 在 `currentSpace` 变化时再次 `fetchTags`（与 `handleTabChange :173` 可能重复）。

**建议：** 在 store 内加"按 spaceName 的请求序号/AbortController"，只接受最新响应；对 space/tag 列表加短 TTL 缓存或单飞（single-flight）。

### 6.5 【轻】逐响应 `new Response` 带来 GC 压力

承 §6.3，`client.real.ts:58` 每次解析都构造新 `Response` 对象，高频轮询下增加 GC 负担。合并优化时一并处理。

---

## 7. 现有实现的合理之处

为平衡结论，以下设计为正确/优秀实践，不建议改动：

1. **懒加载体系完整**：Monaco、Cytoscape、tree-sitter 全部动态导入（`§4.5`），首屏 chunk 体积小。
2. **Monaco 精简入口**：`monacoSetup.ts:1-29` 只注入查询台所需的 contribution，避免拉入全部语言服务与 worker。
3. **grammar wasm 构建期生成 + 时间戳缓存**（`grammarWasm.ts:52-68`），缺失 grammar 时构建失败而非运行时崩溃。
4. **SSE 取消与代际保护**：`AbortController` + `generation` 序号（`stores/stream.ts:86-108`）避免竞态污染。
5. **集中式错误处理**：`call()`/`unwrap()` 统一拆分错误码与消息（`client.real.ts:107-151`），失败载荷解析兼容多种形状（`:84-101`）。
6. **监控失败隔离**：`Promise.allSettled` 保留上次成功快照（`monitoring.ts:144-194`），单端点故障不拖垮整页。
7. **资源释放**：编辑器 `onDestroy` dispose（`CypherEditor.svelte:267-273`）、`EditorPanel` 清理定时器（`:145-148`）、batch 轮询终态清理。
8. **无运行时国外 CDN 依赖**，符合环境约束。

---

## 8. 建议优先级清单

| 优先级 | 问题 | 位置 | 预期收益 |
| --- | --- | --- | --- |
| P0 | 修复会话头注入键不一致 | `client.real.ts:160-170`、`client.preview.ts:45-52` | 启用鉴权时全部 JSON API 可用 |
| P1 | 流式结果批量渲染 | `stores/stream.ts:197-218` | 大结果集流畅度 |
| P1 | 401 处理去重 + 复用 `clearSessionId` | `client.real.ts:33-48,123-124` | 登录态一致性 |
| P2 | SSE 尾帧 flush/消费 | `monitoringStream.ts:84-107`、`streamQuery.ts:206` | 数据完整性 |
| P2 | schema 请求单飞 + 竞态保护 | `stores/schema.ts:61-154` | 正确性与带宽 |
| P2 | 监控双通道合并 | `stores/monitoring.ts:259-263` |降低后端负载 |
| P3 | SSE 解析器统一 | `sseParser.ts` / `sseFrames.ts` / `migrationEvents.ts` | 可维护性 |
| P3 | 监控流断线重连 | `stores/monitoring.ts:230-246` | 近实时体验 |
| P3 | 清理 `USE_MOCK` 死配置 | `env.ts:11-17`、`.env.mock`、`.env.preview` | 消除误导 |

---

## 9. 开放问题与待验证点

1. **鉴权是否默认开启**：需确认目标部署的 `enable_authorize` / `single_user_mode`（`auth.rs:66-69`）。若为关闭态，§5.1/§5.2 在现网不可见，但仍是必须修复的潜在缺陷。
2. **`bigintFetch` 的 header 透传**：重建 `Response`（`client.real.ts:58-62`）是否影响 `content-encoding`/trailers，需结合后端压缩配置在联调中确认。
3. **SSE 尾帧实际发生率**：取决于服务端各 SSE handler 是否在末帧写 `\n\n`，需在抓包中确认以决定 §5.3 的紧迫度。
4. **前端校验**：当前环境未安装前端依赖，`npm run check/test/lint` 未执行；建议在有依赖的环境跑一遍以排除类型层面问题。
5. **未修改任何源码**：本文档仅为分析产物；所有修复建议待评审后再落地。

---

## 附录 A：分析环境准备记录

按任务约束完成了环境与代码准备（均为本次实际执行结果）：

| 项目 | 操作 | 结果 |
| --- | --- | --- |
| 代码拉取 | 配置 `git config --global url."https://gh-proxy.com/https://github.com/".insteadOf "https://github.com/"` 后 `git clone --recursive` | 仓库 HEAD `86407d8`；5 个顶层子模块（gpui-kit / llm-suite / simvec / tantivy / graphdb-ben）+ gpui-kit 下 5 个嵌套子模块全部 checkout 成功 |
| Rust 镜像（rsproxy） | 写入 `/etc/profile.d/rsproxy.sh`、`~/.bashrc`、`~/.profile`：`RUSTUP_DIST_SERVER=https://rsproxy.cn`、`RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup`；`~/.cargo/config.toml` 配 `rsproxy-sparse`（`sparse+https://rsproxy.cn/index/`） | 全局生效 |
| Node 镜像 | `npm`/`pnpm` registry 均设为 `https://registry.npmmirror.com`（用户级 `~/.npmrc` + 全局 npmrc） | `npm view svelte version` 返回 `5.59.1`，可用 |
| Rust 工具链 | `rustup toolchain install stable` + `rustup default stable` | 由 `1.93.0 (2026-01-19)` 升级至 **`1.100.0 (0ef1bc386 2026-11-09)`** |

注意：git 全局 `insteadOf` 重写会影响本会话之后所有 `github.com` 操作；如需恢复，执行 `git config --global --unset-all url."https://gh-proxy.com/https://github.com/".insteadOf`。

## 附录 B：SSE 解析器对照速查

- `frontend/src/lib/utils/sseParser.ts` — 查询流，强类型事件，json-bigint 保真，手写分帧。
- `frontend/src/lib/utils/sseFrames.ts` — 通用最小分帧器，正则分隔，提供未使用的 `flush()`。
- `frontend/src/lib/utils/migrationEvents.ts:134-165` — 迁移流分帧 + 类型映射，JSON.parse，终态即止。
