# 前端编辑器语法高亮问题清单与修改方案

## 现状概述

前端 Cypher 编辑器采用 Monaco 双层高亮:

- **Monarch 层** (`frontend/src/lib/utils/monacoCypher.ts`):关键字、字符串、数字、注释、schema 实体(tag/edge/field)着色,tokenizer 随 `schemaStore` 动态重建。
- **tree-sitter 语义层** (`frontend/src/lib/utils/treeSitterCypher.ts`):加载 `crates/linkrs-grammar` 编译出的 wasm 与 `queries/highlights.scm`,以 `SemanticTokensProvider` 叠加;故意丢弃裸 identifier 的 `@variable` 捕获,把标识符让给 Monarch 做 schema 着色。
- **构建期**:`frontend/vite/grammarWasm.ts` 在 buildStart 用 tree-sitter CLI 编译 wasm,失败即构建失败。
- **代码生成**:`tools/grammar-codegen/gen.mjs` 从 `crates/linkrs-grammar/data/keywords.json` 生成关键字/函数清单。

## 问题清单与方案

### 1. token 缓存全局单例,跨编辑器实例串色(严重)

`treeSitterCypher.ts` 的 `cachedVersionId/cachedTokens` 是模块级单值,而 Monaco 的 `versionId` 是每个 model 从 1 开始的局部计数。两个编辑器实例 versionId 相同时会返回另一份文档的 token,导致高亮错乱。

**方案**:缓存键改为 `model.id + versionId`;不匹配则重算。

### 2. 返回 `resultId` 但未实现增量接口(严重)

`provideDocumentSemanticTokens` 返回 `resultId`,Monaco 可能据此调用 `provideDocumentSemanticTokensEdits` 请求增量——该接口未实现,可能导致后续编辑不刷新。

**方案**:移除 `resultId`,始终返回全量 token(查询文本短,全量成本可忽略)。

### 3. 捕获冲突消解偏离 tree-sitter 优先级语义(中)

现实现按 startIndex 排序后贪心去重,"位置靠前者赢";tree-sitter 的语义是"查询中靠后的捕获覆盖靠前的"。同一节点同时命中 `@variable` 与 `@function`(如 `function_call` 内的 identifier)时结果不确定。

**方案**:按捕获出现顺序消解——遍历捕获序列,同一重叠区域后出现的捕获覆盖先出现的;再排序输出。

### 4. 运算符词表未纳入 codegen,三层词表可漂移(中)

highlights.scm 的 kw_* 捕获、codegen 的 `GENERATED_KEYWORDS`、Monarch 手写 operators 列表(`monacoCypher.ts`)三份独立维护;运算符列表缺 `->`、`<-`、`**` 等,新运算符在兜底层显示错误颜色,语义层到达后才修正(闪烁)。

**方案**:`keywords.json` 已含 `operators` 字段,扩展 `gen.mjs` 生成 `GENERATED_OPERATORS`,Monarch tokenizer 改用生成清单;重新执行 codegen。

### 5. 小瑕疵(轻)

- `highlights.scm` 中 `kw_desc` 重复列出两次:删除重复项。
- `captureTypeIndex` 手工维护 capture 名到图例索引的映射,与 `TOKEN_TYPES` 重复:改为从 `TOKEN_TYPES` 派生。
- `(boolean) @number` 布尔染成数字色:接受现状(基础主题中 number 颜色对布尔值视觉可接受,避免扩大改动面),在注释中说明意图。

## 验证

全部修改完成后一次性执行最小验证:`cd frontend && npm run check`。
