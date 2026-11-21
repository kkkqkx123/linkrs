# 前端资源请求与加载逻辑修复方案

**日期**：2026-11-21
**依据**：`docs/analysis/frontend-resource-loading-analysis.md`（本文所有结论的行号以该文档核验时版本为准）
**范围**：`frontend/src` 下的 API 客户端、SSE 流、stores、schema 加载逻辑
**原则**：遵循 `AGENTS.md`——代码/注释用英文、文档用中文、代码注释不引用方案标识、不破坏现有类型契约

---

## 0. 问题清单

按严重度排序。`位置` 为核验时的 `file:line`。

| 编号 | 严重度 | 类别 | 位置 | 问题摘要 |
| --- | --- | --- | --- | --- |
| F1 | 严重 | 语义 | `api/client.real.ts:163`、`utils/storage.ts:1,40` | JSON 客户端读裸键 `sessionId`，登录写带前缀 `linkrs_sessionId`，`X-Session-ID` 注入恒失效 |
| F2 | 中 | 语义 | `api/client.real.ts:48,123-124,35` | 401 触发两次登出且清错键 |
| F3 | 中 | 语义 | `services/monitoringStream.ts:84-107`、`services/streamQuery.ts:206` | SSE 尾帧未 flush / flush 结果未消费 |
| F4 | 中 | 语义 | `stores/monitoring.ts:230-246` | 监控流断线后不重连，永久降级轮询 |
| F5 | 中 | 性能 | `stores/stream.ts:197-218` | 流式结果逐行全量拷贝数组，O(n²) |
| F6 | 中 | 性能/语义 | `stores/schema.ts:61-154` | schema/space 请求无单飞、无竞态保护 |
| F7 | 中 | 性能 | `stores/monitoring.ts:139-155,259-263` | SSE 与轮询双通道重复取 overview/system/database |
| F8 | 低 | 语义 | `utils/sseParser.ts` 等三处 | 三套 SSE 解析器分隔符/bigint 语义不一致 |
| F9 | 低 | 配置 | `env.ts:11-17`、`.env.mock`、`.env.preview` | `USE_MOCK` 死配置 |

---

## 1. 分阶段方案总览

| 阶段 | 主题 | 覆盖问题 | 主要文件 | 风险 |
| --- | --- | --- | --- | --- |
| A | 会话头注入与 401 处理 | F1, F2 | `client.real.ts`、`client.preview.ts`、`http.ts` | 中（跨模块依赖） |
| B | SSE 尾帧与断流降级 | F3, F4 | `streamQuery.ts`、`monitoringStream.ts`、`stores/monitoring.ts` | 中 |
| C | 流式渲染与请求性能 | F5, F6, F7 | `stores/stream.ts`、`stores/schema.ts`、`stores/monitoring.ts` | 中 |
| D | SSE 解析器统一与死配置清理 | F8, F9 | `utils/sse*.ts`、`migrationEvents.ts`、`env.ts`、`.env*` | 较高（需回归测试） |

执行顺序：A → B → C → D。每阶段完成后更新本文第 6 节的落地状态。

---

## 2. 阶段 A：会话头注入与 401 处理

### 2.1 事实依据

- `storage.ts:1` 定义 `STORAGE_PREFIX = 'linkrs_'`，`storage.ts:40` 写入时拼接前缀并 `JSON.stringify`。
- `constants.ts:60` 定义 `SESSION_ID: 'sessionId'`。
- `http.ts:23-25` 的 `setStoredSessionId` 经 `storage.set` 写入键 `linkrs_sessionId`。
- `client.real.ts:163` 用 `localStorage.getItem('sessionId')` 读裸键，恒为 `null`。
- 后端 `auth.rs:71-85` 从 `X-Session-ID` 头取会话，缺失即 401（鉴权开启时）。

### 2.2 修改点

**A-1. 会话读取统一走 storage 抽象**（`client.real.ts`）

```diff
 import { API_BASE_URL } from '$app/env/public';
 import { STORAGE_KEYS } from '$config/constants';
+import { storage } from '$utils/storage';
@@
 networkClient.use({
 	async onRequest({ request }) {
 		try {
-			const sessionId = localStorage.getItem(STORAGE_KEYS.SESSION_ID);
+			const sessionId = storage.get<string>(STORAGE_KEYS.SESSION_ID);
 			if (sessionId) request.headers.set('X-Session-ID', sessionId);
```

`storage.get` 内部做 `JSON.parse`，正好还原 `storeSessionId(String(id))` 写入时的字符串值，消除引号串问题。

**A-2. 401 处理单点化**（`client.real.ts`）

- 删除 `bigintFetch` 中重复的 401 处理（原 `:48`），仅保留 `call()` 中的判断（原 `:123-124`）。理由：`bigintFetch` 是所有请求的 fetch 包装层，`call()` 是所有 JSON 调用的解包层，二者都会看到 401；保留后者即可。
- `handleUnauthorized` 改用统一的 `clearSessionId()`（`http.ts:61-63`），它同时清理带前缀键与遗留裸键。

```diff
-import { STORAGE_KEYS } from '$config/constants';
+import { clearSessionId } from '$utils/http';
@@
 function handleUnauthorized() {
-	try {
-		localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
-	} catch {
-		/* storage unavailable */
-	}
+	clearSessionId();
 	if (page.url.pathname !== '/login') void goto('/login');
 }
@@
 	const response = await fetch(input, init);
-	if (response.status === 401) handleUnauthorized();
 	const contentType = response.headers.get('content-type') ?? '';
```

注意：由于 `client.real.ts` 不再使用 `STORAGE_KEYS`（A-1 改用 storage 但仍需 key 常量——保留 import）。确认 A-1 仍引用 `STORAGE_KEYS`，故保留该 import。

**A-3. preview 模式同步修正**（`client.preview.ts`）

```diff
 import { STORAGE_KEYS } from '$config/constants';
+import { clearSessionId } from '$utils/http';
@@
 function handleUnauthorized() {
-	try {
-		localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
-	} catch {
-		/* storage unavailable */
-	}
+	clearSessionId();
 	if (page.url.pathname !== '/login') void goto('/login');
 }
```

`client.preview.ts` 的 mock 客户端不经 `bigintFetch`，401 仅在 `mockFailureResult`（原 `:148`）与 `call()`（原 `:112-113`）出现，存在与 real 相同的重复触发。处理方式：删除 `mockFailureResult` 中的 401 处理，保留 `call()` 中的。

**A-4. 依赖方向核验**

`http.ts:1` 当前 `import { BASE_URL } from '$lib/api/client'`，而 `client.real.ts` 将改为 import `http.ts` 的 `clearSessionId`，形成循环依赖风险。处理方式：`http.ts` 改为直接读 `API_BASE_URL`，切断对 client 的依赖：

```diff
-import { BASE_URL } from '$lib/api/client';
+import { API_BASE_URL } from '$app/env/public';
@@
 export function getApiBaseUrl(): string {
-	return BASE_URL;
+	return API_BASE_URL;
 }
```

`client.ts` 与 `client.preview.ts` 的 `BASE_URL = API_BASE_URL`（各自 `:23` / `:37`）语义不变，故无行为变化。

### 2.3 开放点（A）

- A-2 删除 `bigintFetch` 的 401 处理后，若有绕过 `call()` 直接 `await client.GET(...)` 的调用点，401 将不再触发登出。需全局核验 `client.` 调用是否都经过 `call()`/`unwrap()`。

---

## 3. 阶段 B：SSE 尾帧与断流降级

### 3.1 事实依据

- `sseFrames.ts:38-43` 提供 `flush()`，`monitoringStream.ts` 未调用。
- `migrationStream.ts:88-91` 正确消费 flush 结果，可作为范式。
- `streamQuery.ts:206` 的 `parser.feed(decoder.decode())` 返回值被丢弃。
- `monitoring.ts:241-245` 流结束后仅置状态，无重连。

### 3.2 修改点

**B-1. 监控流 flush**（`monitoringStream.ts`）

将 `done` 时的 `return` 改为 `break`，循环结束后 flush：

```diff
 	const frameReader = new SseFrameReader();
 	onStatus?.('live');
+	const consumeFrames = (frames: SseFrame[]) => {
+		for (const frame of frames) {
+			if (frame.event === 'snapshot') {
+				const parsed = parseJsonTolerant(frame.data);
+				if (parsed && typeof parsed === 'object') {
+					onSnapshot(parsed as unknown as OverviewResponse);
+					snapshots += 1;
+				}
+			} else if (frame.event === 'error') {
+				const parsed = parseJsonTolerant(frame.data) as { message?: unknown } | null;
+				onErrorEvent?.(
+					typeof parsed?.message === 'string' ? parsed.message : 'snapshot collection failed',
+				);
+			}
+		}
+	};
 	for (;;) {
 		const { done, value } = await reader.read();
-		if (done) {
-			return { cancelled: false, snapshots, streamError: null };
-		}
-		for (const frame of frameReader.feed(decoder.decode(value, { stream: true }))) {
-			if (frame.event === 'snapshot') { ... }
-		}
+		if (done) break;
+		consumeFrames(frameReader.feed(decoder.decode(value, { stream: true })));
 	}
+	consumeFrames(frameReader.flush());
+	return { cancelled: false, snapshots, streamError: null };
```

**B-2. 查询流 flush 消费**（`streamQuery.ts`）

抽取事件分发为局部函数，供主循环与末尾 flush 复用：

```diff
 	const reader = response.body.getReader();
 	const decoder = new TextDecoder();
+	const dispatch = (events: StreamEvent[]) => {
+		for (const event of events) { /* 原 :168-202 的分发逻辑原样搬入 */ }
+	};
 	try {
 		for (;;) {
 			const { done, value } = await reader.read();
 			if (done) break;
-			const events = parser.feed(decoder.decode(value, { stream: true }));
-			for (const event of events) { ... }
+			dispatch(parser.feed(decoder.decode(value, { stream: true })));
 			if (parser.doneReceived) break;
 		}
-		parser.feed(decoder.decode());
+		dispatch(parser.feed(decoder.decode()));
```

需要 import `StreamEvent` 类型（从 `$utils/sseParser`）。

**B-3. 监控流断线重连**（`stores/monitoring.ts`）

在 store 闭包内新增退避状态与重试函数：

```diff
 	let timer: ReturnType<typeof setInterval> | null = null;
 	let streamAbort: AbortController | null = null;
+	let retryTimer: ReturnType<typeof setTimeout> | null = null;
+	let retryDelayMs = 2_000;
@@
 	async function startStreaming() {
 		if (streamAbort !== null) return;
 		const controller = new AbortController();
 		streamAbort = controller;
 		const outcome = await streamMonitoring({ ... });
-		streamAbort = null;
-		update((s) => ({ ...s, streamStatus: outcome.streamError ? 'unavailable' : 'stopped' }));
+		if (streamAbort !== controller) return;
+		streamAbort = null;
+		if (outcome.cancelled) {
+			update((s) => ({ ...s, streamStatus: 'stopped' }));
+			return;
+		}
+		if (!outcome.streamError) {
+			retryDelayMs = 2_000;
+			update((s) => ({ ...s, streamStatus: 'stopped' }));
+			return;
+		}
+		update((s) => ({ ...s, streamStatus: 'unavailable' }));
+		retryTimer = setTimeout(() => {
+			retryTimer = null;
+			void startStreaming();
+		}, retryDelayMs);
+		retryDelayMs = Math.min(retryDelayMs * 2, 30_000);
 	}
 
 	function stopStreaming() {
+		if (retryTimer !== null) {
+			clearTimeout(retryTimer);
+			retryTimer = null;
+		}
 		streamAbort?.abort();
 	}
```

要点：`stop()` 通过 `clearTimeout` 取消待重试；正常结束（非错误）重置退避；`outcome.cancelled`（用户主动）不重试。

### 3.3 开放点（B）

- B-3 的退避在页面后台（tab 不可见）也会运行。是否结合 `visibilitychange` 暂停以省电，待产品决策。
- B-1 的 flush 在服务端正常以 `\n\n` 结尾时通常为空帧，属健壮性补强，不影响稳态语义。

---

## 4. 阶段 C：流式渲染与请求性能

### 4.1 事实依据

- `stream.ts:205` 每行 `rows: [...card.rows]` 全量浅拷贝，N 行累计 O(N²)。
- `schema.ts:97-109,142-154` 的 `fetchTags`/`fetchEdgeTypes` 无单飞、无竞态保护。
- `monitoring.ts:144-155` 轮询含 overview/system/database，与 SSE 推送重叠（`monitoring.ts:216-227`）。

### 4.2 修改点

**C-1. 流式渲染改为按索引写入 + 节流提交**（`stores/stream.ts`）

核心思路：用可变数组按 `index` 直接写槽位，只在"一批结束"或到达节流窗口时 `update()` 一次。为避免破坏 `receivedCount` 语义，`placeCardRow` 保持原样，改动提交频率。

```diff
 let streamController: AbortController | null = null;
 let streamGeneration = 0;
+const ROW_FLUSH_MS = 50;
@@
 	async function run(...) {
 		...
+		let pendingRows: Array<{ row: Record<string, unknown>; index: number; stmt: number }> = [];
+		let flushTimer: ReturnType<typeof setTimeout> | null = null;
+		const flushRows = () => {
+			if (flushTimer !== null) { clearTimeout(flushTimer); flushTimer = null; }
+			if (pendingRows.length === 0 || !isCurrent(generation)) return;
+			const batch = pendingRows;
+			pendingRows = [];
+			update((s) => {
+				const cards = s.cards.map((card) => {
+					const rowsForCard = batch.filter((r) => r.stmt === card.index);
+					if (rowsForCard.length === 0) return card;
+					const rows = card.rows.slice();
+					for (const { row, index } of rowsForCard) placeCardRow({ ...card, rows }, row, index);
+					return { ...card, rows };
+				});
+				return { ...s, status: 'receiving', cards, firstRowAt: s.firstRowAt ?? Date.now() };
+			});
+		};
@@
 			onRow: (row, index, stmt) => {
 				if (!isCurrent(generation)) return;
-				const target = stmt;
-				update((s) => { ... 原 O(n²) 逻辑 ... });
+				pendingRows.push({ row, index, stmt });
+				if (flushTimer === null) flushTimer = setTimeout(flushRows, ROW_FLUSH_MS);
 			},
@@
-			if (!isCurrent(generation)) return { ... };
+			flushRows();
+			if (!isCurrent(generation)) return { ... };
```

注意：`placeCardRow` 会自增 `receivedCount`，但传入临时对象 `{ ...card, rows }` 会导致计数写回丢失。实现时改为让 `placeCardRow` 接受 `(rows, card)` 分别更新，或在 flush 内直接按 index 写 `rows` 并单独累加 `receivedCount`。以最终代码为准，需保证 `receivedCount` 与行数一致。

**C-2. schema 请求单飞与竞态保护**（`stores/schema.ts`）

在 store 闭包内新增通用 helper：

```diff
 	const store = {
 		subscribe,
+		fetchSpaces: async () => {
+			return requestOnce('spaces', () => schemaService.spaces.list(), (response) => {
+				const spaces = Array.isArray(response) ? response : (response as { data?: Space[] }).data || [];
+				update((s) => { ... 原 :68-75 逻辑 ... });
+			});
+		},
```

`requestOnce` 定义（闭包顶部）：

```ts
const inflight = new Map<string, Promise<void>>();
const requestSeq = new Map<string, number>();
async function requestOnce<T>(
  key: string,
  loader: () => Promise<T>,
  apply: (value: T) => void,
): Promise<void> {
  const existing = inflight.get(key);
  if (existing) return existing;
  const seq = (requestSeq.get(key) ?? 0) + 1;
  requestSeq.set(key, seq);
  const task = loader()
    .then((value) => { if (requestSeq.get(key) === seq) apply(value); })
    .catch((err) => { console.error(`Fetch ${key} error:`, err); })
    .finally(() => { if (inflight.get(key) === task) inflight.delete(key); });
  inflight.set(key, task);
  return task;
}
```

`fetchTags`/`fetchEdgeTypes`/`fetchIndexes` 的 key 用 `tags:${spaceName}` 等，使竞态判定粒度到 space。`createTag`/`deleteTag` 等变更后调用 `fetchTags` 需要强制刷新——在成功后 `requestSeq.set(key, (requestSeq.get(key) ?? 0) + 1)` 再调用，或提供 `invalidate(key)`。实现时以"变更后能拿到最新数据"为验收标准。

**C-3. 监控双通道合并**（`stores/monitoring.ts`）

`refresh()` 根据 `streamStatus` 跳过与 SSE 重叠的三个端点：

```diff
 	async function refresh() {
-		const { paused, timeRange } = get({ subscribe });
+		const { paused, timeRange, streamStatus } = get({ subscribe });
 		if (paused) return;
+		const live = streamStatus === 'live';
 		const queryBounds = rangeQueryBounds(timeRange);
 		update((s) => ({ ...s, loading: true }));
 		const settled = await Promise.allSettled([
-			statisticsService.overview(),
-			statisticsService.system(),
-			statisticsService.database(),
+			live ? Promise.resolve(undefined) : statisticsService.overview(),
+			live ? Promise.resolve(undefined) : statisticsService.system(),
+			live ? Promise.resolve(undefined) : statisticsService.database(),
 			...
 		]);
@@
-			if (overview.status === 'fulfilled') next.overview = overview.value;
+			if (overview.status === 'fulfilled' && overview.value !== undefined) next.overview = overview.value;
```

`start()` 仍同时启动二者：SSE live 时轮询只拉剩余 5 个端点，SSE unavailable（含重连退避期）时轮询回补全部 8 个，保证数据不降级。

### 4.3 开放点（C）

- C-1 的 `ROW_FLUSH_MS = 50` 是交互流畅度与更新延迟的折中；是否需按结果集大小动态调整，待实测。
- C-2 变更后刷新若命中单飞会返回同一 promise，需确保变更类操作先 `invalidate` 再 `fetch*`，否则 UI 不更新。
- C-3 依赖 `streamStatus` 准确反映连接状态；B-3 的重连状态机是其前提，故 C 在 B 之后实施。

---

## 5. 阶段 D：SSE 解析器统一与死配置清理

### 5.1 事实依据

- `sseParser.ts:76-104`（手写分帧 + json-bigint）、`sseFrames.ts:16`（正则 `/\r?\n\r?\n/`）、`migrationEvents.ts:143`（正则 `/\r\n\r\n|\n\n|\r\r/` + `JSON.parse`）三套实现语义分歧。
- `env.ts:11-17` 声明 `USE_MOCK`，但全仓仅注释引用；模式切换靠 `vite.config.ts:11-20` 的 alias 与编译期 `isMockMode`。

### 5.2 修改点

**D-1. 抽取公共分帧器**（新建 `utils/sseFraming.ts`，或复用 `sseFrames.ts`）

统一分隔符正则为 `/\r\n\r\n|\n\n|\r\r/`，提供 `splitFrames(buffer): { frames: string[]; rest: string }` 与 `parseFrame(raw)`。让 `SseParser` 与 `MigrationSseParser` 改为基于公共分帧器，各自只保留事件映射层（json-bigint 保真差异保留在事件映射层，不强行统一）。

> 此阶段风险最高：现有 `sseParser.test.ts`、`sseFrames.test.ts`、`sseParser.test.ts` 等覆盖了边界语义。实施前先跑通测试基线，改动后必须回归通过。若测试无法运行（依赖缺失），本阶段降级为"仅清理死配置"，解析器统一转入后续独立方案。

**D-2. 清理 USE_MOCK**

- `env.ts:11-17`：删除 `USE_MOCK` 定义及相关 doc 注释。
- `.env.mock`、`.env.preview`：删除 `USE_MOCK=true` 行，保留注释说明模式由 npm script 的 `--mode` 决定。
- `docs/plan/frontend_preview_architecture.md` 中关于 "USE_MOCK=true" 的描述需在落地后同步更正（跨文档一致性）。

### 5.3 开放点（D）

- D-1 是否统一到单一解析器需评估收益：三套实现各自服务不同事件模型，强统一可能过度设计；折中方案是仅统一"分帧"层，事件映射保持独立。
- `frontend_preview_architecture.md` 描述的独立 preview 项目当前仓库中不存在，清理其描述需确认是否属于有效规划。

---

## 6. 落地状态跟踪

> 更新时间：2026-11-21。所有阶段代码已落地，静态验证（单测 / 类型检查 / lint / check 全流程）全部通过。

| 阶段 | 问题 | 状态 | 落地文件 | 验证 |
| --- | --- | --- | --- | --- |
| A | F1, F2 | 已完成 | `api/client.real.ts`、`api/client.preview.ts`、`utils/http.ts` | 类型检查/lint 通过；需联调验证启用鉴权下的会话注入 |
| B | F3, F4 | 已完成 | `services/streamQuery.ts`、`services/monitoringStream.ts`、`stores/monitoring.ts` | 类型检查/lint/单测通过；重连行为需浏览器实测 |
| C | F5, F6, F7 | 已完成 | `stores/stream.ts`、`stores/schema.ts`、`stores/monitoring.ts` | 类型检查/lint/单测通过；性能收益需大结果集实测 |
| D | F8, F9 | F9 已完成；F8 评估后不实施 | `env.ts`、`.env.mock`、`.env.preview`、`mock/index.ts`、`README.md` | check 全流程通过（含 check:mock）；F8 理由见下 |

### 6.1 相对方案的实施偏差（重要）

1. **A-2 的实现调整**：方案建议删除 `bigintFetch` 的 401 处理后，经核验后端 `auth.rs:19-23` 的 deny 响应带标准 `{ error, status }` JSON，`call()` 的 `res.error` 判断能捕获 401，故单点处理安全，按方案执行。同时确认全仓 `client.` 调用均经过 `call()`/`unwrap()`，无绕过点。
2. **A-3 / A-4 落地**：`client.preview.ts` 同步修正——`handleUnauthorized` 复用 `clearSessionId()`、删除 `mockFailureResult` 中重复的 401 处理（保留 `call()` 单点），并移除不再使用的 `STORAGE_KEYS` import。已切断 `http.ts → client` 的反向 import（改为直接读 `API_BASE_URL`），避免与两个客户端的 `→ http.ts` 形成循环依赖。`client.ts`、`storage.ts` 无需改动（前者仅 re-export，后者抽象本身正确）。
3. **B-1 的 flush 时机**：监控流 `done` 由 `return` 改为 `break`，循环后统一 `frameReader.flush()` 并 `consumeFrames` 分发；`consumeFrames` 从循环体抽出复用。
4. **B-2 的事件分发**：`streamQuery.ts` 抽出 `dispatch(events)` 供主循环与 `decoder.decode()` flush 复用，新增 `StreamEvent` 类型 import。
5. **C-1 的 receivedCount 保真**：方案草稿中"临时对象"写法会丢失 `receivedCount` 写回。实际实现改为在 flush 内对目标 card 先 `rows: card.rows.slice()` 建可变副本，再对整批行调用 `placeCardRow(next, ...)`，保证去重计数正确；所有提前 return 路径前均调用 `flushRows()`。
6. **C-2 的 invalidate**：`requestOnce` 增加 `onError` 参数以保留 loading 状态复位；所有变更方法在 `fetch*` 前显式 `invalidate(key)`，避免单飞命中旧 promise 导致 UI 不刷新。
7. **C-3 的占位类型**：`Promise.resolve(undefined)` 使 settled 前三项正确类型为 `Xxx | undefined`，赋值前加 `!== undefined` 判定。
8. **F8（D-1）不实施的决定**：方案开放点 D-1 已警示"强统一可能过度设计"。实测核验后确认三套解析器服务不同事件模型（强类型事件 / 通用帧 / 类型化迁移事件）且 bigint 策略有意不同（query 流保真、migration 用普通 JSON）；服务端 axum `Event`（`statistics/stream.rs:83`）按 SSE 规范输出 `\n\n`，三套均能正确处理标准帧。分隔符分歧仅存在于无实际意义的畸形边界（如单独 `\r\r` 分隔）。强行抽取共享分帧器会侵入 `SseParser` 的 done-buffer 语义并重写大量边界测试依赖的行为，风险高于收益，故保留现状，仅将 F8 降级为"已知低风险分歧"记录。未经浏览器实测与长期运行观察前，不建议再动。

### 6.2 静态验证结果（本环境实测）

| 验证项 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试 | `npm run test` | 99 passed / 0 failed |
| 类型检查 | `npx svelte-check`（含 `gen:i18n`） | 0 errors / 0 warnings |
| 代码规范 | `npm run lint` | 通过 |
| 完整检查 | `npm run check`（i18n/tokens/gen/sync/check/mock） | 全部通过；Mock coverage 125 registered / 122 used |
| 生产构建 | `npm run build` | **未执行**：grammar wasm 需 tree-sitter CLI，其二进制来自 `release-assets.githubusercontent.com`，本环境无国外 CDN 无法下载（符合任务约束）；此项留待有网环境验证 |

---

## 7. 待拍板开放点汇总

1. **鉴权默认态**：目标部署 `enable_authorize` / `single_user_mode` 取值决定 F1/F2 是否在现网可见。已无论取值均修复，但**启用鉴权下的实际会话注入仍需联调实测**（需浏览器 Network 面板确认 `X-Session-ID` 已注入）。
2. **后台 tab 的重连策略**（B-3）：当前实现未结合 `visibilitychange`，后台 tab 仍按退避重连。是否暂停以省电待产品决策。
3. **流式刷新节流值**（C-1）：当前取 50ms，大结果集下的流畅度与 `firstRowAt` 延迟需实测确认。
4. **解析器统一范围**（F8 / D-1）：已决策**不实施**，理由见 §6.1 第 8 条。如后续要动，需先补齐跨实现差分测试。
5. **preview 架构文档**（D-2）：`docs/plan/frontend_preview_architecture.md` 仍描述 `USE_MOCK=true`，**尚未同步更正**，待确认该 preview 项目是否为有效规划后再改（避免改错历史文档）。
6. **生产构建**：本环境缺 tree-sitter CLI（无法从 github releases 下载），`npm run build` 未验证，留待有网环境或预置 wasm 产物后执行。

---

## 8. 验证清单

静态验证（本环境已完成）：
- [x] `npm run test` 全绿（99 passed）
- [x] `npx svelte-check` / `npm run check` / `npm run lint` 通过
- [x] D-2 死配置清理后 `check:mock` 仍通过（125 registered / 122 used）

运行/联调验证（待有网环境或真实后端）：
- [ ] A：`client.real.ts` 的 `X-Session-ID` 注入在 `enable_authorize=true` 下生效（浏览器 Network 面板）
- [ ] A：401 仅触发一次登出，`linkrs_sessionId` 被清除
- [ ] B：三个 SSE 流的尾帧均被消费；断流后按 2s/4s/8s… 重连，stop 后停止
- [ ] C：大结果集流式渲染无逐行卡顿；快速切 space 不串数据；SSE live 时轮询不请求 overview
- [ ] `npm run build` 生产构建（需 tree-sitter CLI / wasm 产物）
