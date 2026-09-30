# 前端查询语句编辑器实现分析与评估

> 分析对象：`linkrs` 前端 Console 查询编辑器
> 对比对象：`nebula-studio` (vesoft-inc)
> 分析基准 commit：`linkrs@cc95ce2d`、`nebula-studio@main`(depth=1)
> 相关约束：`AGENTS.md`（No-backward-compatible；代码仅用英文；文档用中文）

---

## 0. 结论摘要

linkrs 当前查询编辑器是**基于 Monaco 的最小可用实现**，架构清晰、依赖裁剪得当，但相对成熟产品（nebula-studio）在**语法感知、多语句执行、快捷键/选区执行、schema 上下文联动**等维度存在明显功能缺口。

核心判断：

| 维度 | 现状 | 是否合理 | 建议 |
|------|------|----------|------|
| 编辑器内核选型 (Monaco 裁剪版) | 仅引入 suggest/bracket/find 等必要 contribution | ✅ 合理，保留 | 维持，无需改造 |
| 语法高亮 (Monarch) | 静态关键字表，无 schema 动态着色 | ⚠️ 基础可用 | 扩展：schema 动态着色 |
| 自动补全 | 单一 provider，扁平合并，无上下文分组 | ❌ 需扩展 | 拆分 provider + sortText 分级 |
| 多语句执行 | 前端可拆分却只执行 `queries[0]` | ❌ 设计缺陷 | 改用后端 `/v1/query/batch` |
| 选区/光标语句执行 | 无 | ❌ 缺失 | 新增 Ctrl/Cmd+Enter 分支 |
| 快捷键 | 仅 Ctrl/Cmd+Enter 执行 | ⚠️ 不足 | 增加 Shift+Enter 等 |
| schema 上下文补全 | 属性与 tag 平铺，无前缀感知 | ❌ 需扩展 | 前缀/`.`/`:` 触发 |
| 编辑器高度 | 固定 `h-40`（160px） | ⚠️ 体验差 | 可拖拽/自适应 |

一句话：**当前实现"能用"，但离"好用"还差一轮功能扩展；且有一个隐藏的语义 Bug（多语句只执行第一条）应优先修复。**

---

## 1. linkrs 当前实现事实梳理

### 1.1 组件构成

查询编辑器由 3 个文件构成，职责边界清晰：

| 文件 | 行数 | 职责 |
|------|------|------|
| `frontend/src/lib/components/common/CypherEditor.svelte` | 94 | Svelte 组件封装，生命周期管理，双向绑定 |
| `frontend/src/lib/utils/monacoCypher.ts` | 154 | Cypher 语言注册（Monarch）、补全 provider |
| `frontend/src/lib/utils/monacoSetup.ts` | 31 | Monaco 最小化入口，按需引入 contribution |

消费方仅一处：`frontend/src/lib/pages/Console/Console.svelte:113`（全项目唯一引用点，已通过 grep 确认）。

### 1.2 Monaco 引入策略（合理，值得保留）

`monacoSetup.ts:1-31` 采用**精确 contribution 引入**，注释明确说明"导入完整 `monaco-editor` 会拉入全部基础语言和 json/css/html/ts 语言服务及其 web worker"：

```ts
// monacoSetup.ts:9-28
import * as monaco from 'monaco-editor/editor/editor.api.js';
import 'monaco-editor/editor/contrib/suggest/browser/suggestController.js';
import 'monaco-editor/editor/contrib/bracketMatching/browser/bracketMatching.js';
import 'monaco-editor/editor/contrib/find/browser/findController.js';
// ... 仅 16 个必要 contribution
```

且 `CypherEditor.svelte:31` 通过动态 `import()` 懒加载，避免编辑器进入首屏 chunk：

```ts
// CypherEditor.svelte:30-33
// Load Monaco lazily so the editor bundle stays out of the initial chunk.
const { monaco } = await import('$utils/monacoSetup');
```

**评估：这是正确的工程实践**，打包体积与首屏性能收益明确，无需改造。

### 1.3 语法高亮（静态 Monarch）

`monacoCypher.ts:27-84` 注册语言 `cypher`，Monarch tokenizer 覆盖：注释（`--`、`//`、`/* */`）、字符串、反引号标识符、关键字、函数、数字、运算符、括号。

但关键字表为**硬编码静态列表**（`monacoCypher.ts:6-20`，共 45 个关键字 + 29 个函数），**与实际 schema 无关联**，tag/edge 名称无法获得独立着色。

### 1.4 自动补全（单一扁平 provider）

`monacoCypher.ts:91-154` 注册**唯一一个** `registerCompletionItemProvider`，返回三类候选的合并数组：

```ts
// monacoCypher.ts:151
return { suggestions: [...keywordItems, ...functionItems, ...schemaItems] };
```

- 关键字：`CompletionItemKind.Keyword`（`:102-107`）
- 函数：`CompletionItemKind.Function`，snippet `fn($0)`（`:109-115`）
- schema：tag 为 `Class`、tag 属性为 `Field`（无父级 detail 走查）、edge 为 `Interface`（`:117-146`）

**关键缺陷**：没有设置 `sortText`，所有候选同权重；没有 `triggerCharacters`，无前缀感知；属性补全的 `detail` 虽标注 `tag ${tag.name} property`，但**所有 tag 的属性被平铺到同一列表**，无法在 `.` 后仅提示对应 tag 的属性。

### 1.5 执行链路（存在语义缺陷）

**前端已具备多语句拆分能力，却未使用**——这是最值得关注的设计问题。

拆分函数 `frontend/src/lib/utils/gql.ts:1-30`（`splitQueries`）实现了**字符串感知的分号拆分**（正确跳过引号/反引号内的分号，处理转义），质量不低：

```ts
// gql.ts:13-24
if (!inString && (char === '"' || char === "'" || char === '`')) { inString = true; ... }
if (inString && char === stringChar) { inString = false; ... }
if (!inString && char === ';') { /* push */ }
```

但 `stores/console.ts:80-86` 拆完之后**只取第一条丢弃其余**：

```ts
// console.ts:80-86
const queries = splitQueries(state.editorContent);
if (queries.length === 0) { /* EMPTY_QUERY */ return; }
const query = queries[0];                       // ← 仅执行第一条
const response = await queryService.execute({ query });
```

**后果**：用户在编辑器输入 `CREATE TAG a(...); CREATE TAG b(...);` 时，第二条静默丢失，无任何提示。这是**功能性 Bug**，而非单纯"功能缺失"。

同时注意 `gql.ts:32-49` 的 `getQueryAtCursor`（用于"执行光标所在语句"）**已实现但全项目无引用**（grep 确认仅定义处出现）；`validateQuery`（`:78-98`）、`formatQuery`（`:51-69`）、`extractQueryInfo`（`:100-111`）同样均为**死代码**。

### 1.6 后端能力 vs 前端使用（契约错配）

后端已提供三个查询端点（`crates/graphdb-server/src/http/handlers/query.rs`）：

| 端点 | 行号 | 用途 | 前端是否使用 |
|------|------|------|--------------|
| `POST /v1/query` | `query.rs:18` | 单语句执行 | ✅ 使用 |
| `POST /v1/query/batch` | `query.rs:96` | **多语句共享 auto-commit 批窗口** | ❌ 未使用 |
| `POST /v1/query/validate` | `query.rs:137` | 仅解析+绑定，不执行 | ❌ 未使用 |

`BatchQueryRequest`（`crates/graphdb-wire/src/query.rs:35-38`）字段为 `{ session_id, statements: Vec<String> }`，响应 `BatchQueryResponse.results: Vec<QueryResponse>` 按输入顺序逐条返回（`query.rs:40-44`）。

而前端 `services/query.ts:66-74` 定义的 `executeBatch` 是**前端循环串行调用单语句端点**，且 grep 确认**无任何调用点**：

```ts
// query.ts:66-74 —— 前端自造 batch，未走后端批端点
executeBatch: async (queries: string[], sessionId?: string) => {
  const results: ExecuteQueryResponse[] = [];
  for (const query of queries) {                    // 逐条串行
    const result = await queryService.execute({ query, sessionId });
    ...
  }
}
```

**评估**：后端 batch 端点是**为一次请求内多语句原子提交**设计的（`AutoCommitBatchOps`），前端自造串行 batch 既绕过了后端优化，也丧失了共享提交窗口的语义保证。前端应直接对接 `/v1/query/batch`。

---

## 2. nebula-studio 实现对比

### 2.1 编辑器封装

`app/components/MonacoEditor/index.tsx`（358 行）为独立通用组件，关键差异：

| 对比项 | linkrs | nebula-studio |
|--------|--------|---------------|
| 框架 | Svelte 5 runes | React + `@monaco-editor/react` |
| 引入方式 | 手工裁剪 contribution | 完整 `monaco-editor` |
| 主题 | `vs` / `vs-dark` | **自定义 `studio` 主题**（`:276-293`），为 tag/edge/field/keyword 定义专属色 |
| 触发键 | `Ctrl/Cmd+Enter` | **`Shift+Enter`**（`:309-311`） |
| 执行选区 | ❌ | ✅ context menu action "运行选中行"（`Console/index.tsx:197-220`） |
| 语法着色 | 静态关键字 | **schema 动态着色**（`:57-69` `createSchemaRegex`，tag/edge 独立 token） |

### 2.2 自动补全：多 provider + sortText 分级（核心差异）

nebula-studio 注册了 **6 个独立 provider**（`MonacoEditor/index.tsx:103-262`），并通过 `sortText` 明确排序优先级（注释：`1: keyword 2: tag 3: edge 4: field 5: function`）：

| Provider | 行号 | triggerCharacters | sortText |
|----------|------|-------------------|----------|
| keyword | `:132-146` | 无（默认） | `1` |
| parameter (`:` 声明) | `:147-161` | `[':']` | `5` |
| propertyReference (`$`) | `:163-177` | `['$']` | `1` |
| schemaInfo (tag/edge/field) | `:179-208` | `[':', '.']` | tag `2` / edge `3` / field `4` |
| schemaInfoTrigger (field after `.`) | `:211-236` | `['.']` | `4` |
| function | `:238-252` | 无 | `5` |

尤其 `schemaInfoTriggerProvider`（`:211-236`）实现了**前缀感知的属性补全**：

```ts
// MonacoEditor/index.tsx:221-231
const words = textUntilPosition.split(' ');
const lastWord = words[words.length - 1].slice(0, -1); // 取最后一个词，去掉末尾 '.'
const suggestions = fields
  ?.filter((field) => field.parent === lastWord || regex.test(lastWord)) // 仅提示该 tag 的属性
  .map((field) => ({ ..., sortText: '4' }));
```

**这正是 linkrs 当前缺失的能力**：`.` 后能精准提示对应 tag 的属性，而非平铺全部属性。

### 2.3 多语句执行（对比佐证）

nebula-studio 走**后端 batch 端点**：

```ts
// app/stores/console.ts:103-119
const gqlList = splitQuery(gql);
const { code, data } = await service.batchExecNGQL({
  gqls: gqlList.filter((item) => item !== '').map(/* 去尾部反斜杠续行 */),
  space: this.currentSpace,
});
```

其 `splitQuery`（`console.ts:14-29`）还额外支持：注释行剔除、**`\` 续行**、`:` 前缀命令特殊处理。

结果以**多条结果卡片叠加**展示（`Console/index.tsx:300-319`，`results.map` 渲染多个 `OutputBox`），每条结果独立成卡。

### 2.4 其他产品化能力

- **历史记录补全**：`Console/index.tsx:142-190`，输入 `/` 触发历史语句补全
- **参数面板**：`CypherParameterBox`（`:286`），`$param` 可视化管理
- **Schema 抽屉**：`SchemaDrawer` 侧栏浏览 schema，支持点击插入
- **语言资源**：`app/config/nebulaQL.ts`（424 行）维护完整关键字/函数/类型字典

---

## 3. 差距与问题清单

按优先级排序（P0 必修 / P1 应修 / P2 可增强）：

| # | 问题 | 位置（linkrs） | 优先级 | 类型 |
|---|------|----------------|--------|------|
| 1 | 多语句仅执行第一条，其余静默丢弃 | `stores/console.ts:85` | **P0** | 功能 Bug |
| 2 | `executeBatch` 前端串行自造，未对接后端 `/v1/query/batch` | `services/query.ts:66-74` | **P0** | 契约错配 |
| 3 | 补全无 `sortText` 分级，候选同权重 | `monacoCypher.ts:102-146` | P1 | 体验 |
| 4 | 属性补全平铺全部 tag，无 `.` 前缀感知 | `monacoCypher.ts:128-136` | P1 | 体验 |
| 5 | 无触发字符（`:`/`.`/`$`） | `monacoCypher.ts:92` | P1 | 体验 |
| 6 | 无选区/光标语句执行（`getQueryAtCursor` 死代码） | `gql.ts:32-49` 无引用 | P1 | 功能缺失 |
| 7 | tag/edge 无 schema 动态着色 | `monacoCypher.ts:35-69` | P2 | 体验 |
| 8 | 无 `/v1/query/validate` 提前校验接入 | 无 | P2 | 功能缺失 |
| 9 | 编辑器固定高 160px | `CypherEditor.svelte:94` | P2 | 体验 |
| 10 | 无历史语句补全 / 参数面板 | — | P2 | 功能缺失 |
| 11 | `formatQuery`/`validateQuery`/`extractQueryInfo` 死代码 | `gql.ts:51-111` | P2 | 代码卫生 |

> 注：`AGENTS.md` 要求"No-backward-compatible"，因此上述改造无需兼容旧行为，可直接重构接口。

---

## 4. 扩展建议与实现方向

### 4.1 P0-1：修复多语句执行

**方向**：`stores/console.ts:executeQuery` 改为对全部语句执行，并展示多条结果。

有两种落地选择（需拍板，见 §5）：

- **方案 A（推荐）**：前端 `splitQueries` + 后端 `/v1/query/batch` 一次性提交，结果数组渲染多张结果卡。
  - 优点：复用已有拆分逻辑、命中后端原子批窗口、单次网络往返。
  - 改动：`services/query.ts` 新增 `executeBatchRemote`；`Console.svelte` 结果区改为 `results[]` 列表（参考 nebula-studio `OutputBox` 叠加）。
- **方案 B**：前端循环调用单语句端点，逐条渲染。
  - 缺点：丢失后端批窗口语义、N 次往返；仅在 batch 端点不可用时兜底。

需同时处理拆分器的**注释行剔除**与**`\` 续行**，向 nebula-studio `splitQuery` 对齐（当前 linkrs `splitQueries` 未剔除注释行）。

### 4.2 P0-2：对接后端 batch 契约

按 `BatchQueryRequest`（`graphdb-wire/src/query.rs:35-38`）实现：

```ts
// 建议形态（services/query.ts）
executeBatch: async (statements: string[], sessionId: number) =>
  await post<BatchQueryResponse>('/v1/query/batch', { session_id: sessionId, statements });
```

> ⚠️ **字段命名不一致（需重点确认）**：后端 `QueryRequest.session_id` 为 `i64` 必填字段（`graphdb-wire/src/query.rs:12-14`），而前端 `services/query.ts:32-34` 写入的是**驼峰** `sessionId`：
>
> ```ts
> // query.ts:32-34
> const body: Record<string, unknown> = { query: params.query };
> if (params.space !== undefined) body.space = params.space;
> if (params.sessionId !== undefined) body.sessionId = params.sessionId;  // 后端期望 session_id
> ```
>
> 且 `space` 字段在后端 `QueryRequest` 中**并不存在**（后端通过 `USE` 语句切换空间，见 `graphdb-wire/src/query.rs:226-260` 的 USE 响应测试）。需确认：① 后端是否有 serde 别名或默认会话；② 前端 `space` 是否为无效字段。这属于**前后端契约需核对的疑点**。

### 4.3 P1：补全体验重构

对齐 nebula-studio 的**多 provider + sortText** 模式（`MonacoEditor/index.tsx:103-262`）：

1. 拆分 `registerCypherCompletions` 为多个 provider，分别设置 `triggerCharacters`：
   - tag/edge/属性：`[':', '.']`
   - 参数/变量：`['$']`
2. 全部候选补 `sortText`（建议 `1` keyword / `2` tag / `3` edge / `4` field / `5` function）
3. 新增 `.` 前缀感知 provider：解析光标前文最后一个 tag，仅提示其属性
4. `range` 计算沿用现有 `getWordUntilPosition`（`monacoCypher.ts:94-100`），保持正确替换

### 4.4 P1：选区执行

启用已有 `getQueryAtCursor`（`gql.ts:32-49`），在 `CypherEditor.svelte:60` 的 `Ctrl/Cmd+Enter` 分支中：

```
if (存在选区) → 执行选中文本
else → 用 getQueryAtCursor 定位光标所在语句执行
else → 执行全文
```

参考 nebula-studio `Console/index.tsx:197-220` 的 `addAction`（context menu "运行选中行"）。

### 4.5 P2：schema 动态着色

参考 nebula-studio `createSchemaRegex`（`MonacoEditor/index.tsx:57-69`），在 `monacoCypher.ts:35-69` 的 tokenizer `root` 顶部插入 tag/edge 正则规则，并在 schema 变化时**重建 tokenizer**（保留一个 `setMonarchTokensProvider` 的 dispose 句柄）。

### 4.6 P2：编辑器尺寸

`CypherEditor.svelte:94` 的 `h-40` 建议改为可配置 `height` prop（默认更高）或支持拖拽调整，参考 nebula-studio 传入 `height="100%"`（`Console/index.tsx:293`）。

### 4.7 P2：validate 接入

利用 `POST /v1/query/validate`（`query.rs:137`）做**执行前校验**（解析+binder，不执行），返回 `ValidateResponse{valid,message}`（`query.rs:92-96`），可在执行按钮前预检并高亮错误位置。

---

## 5. 待拍板开放点

| 编号 | 开放点 | 建议默认 | 影响面 |
|------|--------|----------|--------|
| A | 多语句结果展示形态：多卡叠加 vs 标签页切换 vs 仅末条 | **多卡叠加**（对齐 nebula-studio） | Console 结果区重构 |
| B | `session_id` 来源：后端默认会话 vs 前端显式管理 | 需后端确认 | 所有 query 请求体 |
| C | batch 失败策略：遇错即停 vs 继续执行剩余语句 | 需后端 `execute_batch` 语义确认 | 错误处理 |
| D | 快捷键方案：`Ctrl/Cmd+Enter`（现状）vs `Shift+Enter`（nebula）vs 双支持 | **双支持** | 编辑器交互 |
| E | 是否引入 `/v1/query/stream`（SSE 流式结果） | 暂不，后续按需 | 大数据集场景 |
| F | 死代码（`formatQuery` 等）删除 vs 接线启用 | 接线启用 format/validate | 代码卫生 |

---

## 6. 下一步

1. **确认 §5 的 A/B/C 三个开放点**（依赖后端契约），再启动 P0 改造。
2. P0 改造落地后，按 §4.3/4.4 推进 P1 补全与选区执行。
3. P2 项按迭代节奏评估，schema 动态着色与 validate 接入可单独排期。

> 附：本文分析依据的所有行号均已通过读取源文件核验；`linkrs` 侧引用的文件均位于 `frontend/src/`，后端引用位于 `crates/`。
