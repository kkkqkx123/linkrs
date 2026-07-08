# 全文检索引擎统一方案：放弃 inversearch，统一使用 tantivy

## 背景

graphDB 当前维护两个全文检索引擎：`inversearch`（~16,400 行）和 `tantivy`（venodred ~105,860 行），通过同一 `SearchEngine` trait 适配。

两套引擎存在**明显冗余**：相同的 `(doc_id, content)` 索引模式、相同的 `search(query, limit)` 查询接口、一样的 CRUD 操作。而 graphDB 的实际使用远未发挥 inversearch 的特色能力。

本文分析两个引擎的差距，论证放弃 inversearch 统一到 tantivy。

## graphDB 实际使用模式

graphDB 的全文搜索使用**极其简单**——这是做任何决策前必须理解的核心前提：

```rust
// SearchEngine trait 的全部方法：
async fn index(&self, doc_id: &str, content: &str);      // 平坦字符串
async fn search(&self, query: &str, limit: usize);       // 简单字符串查询
async fn delete(&self, doc_id: &str);                     // 按 ID 删除
async fn commit(&self);
async fn stats(&self) -> IndexStats;
```

graphDB 的全文搜索只是**图的附属索引**——每个顶点属性被索引为独立的 `(doc_id, content)` 对。没有多字段文档、没有数值范围查询、没有聚合。

## inversearch 的能力 vs graphDB 的实际使用

| inversearch 特色功能 | graphDB 是否使用 | 说明 |
|----------------------|------------------|------|
| 13 阶段 Encoder pipeline | ❌ 不使用 | graphDB 使用 `EmbeddedConfig` 默认配置，不自定义 pipeline |
| ColdWarmCache 三级缓存 | ❌ 不使用 | graphDB 使用简单的 `FileStorage`（单文件 postcard 序列化） |
| Forward/Reverse/Full/Bidirectional 模式 | ❌ 不使用 | 默认 Strict 模式 |
| 上下文深度（context depth） | ❌ 不使用 | depth 默认为 0 |
| Soundex 语音匹配 | ❌ 不使用 | 没有任何使用场景 |
| CRC-8 KeystoreMap 哈希分桶 | ❌ 不使用 | graphDB 使用默认配置 |
| Arena 内存分配 | ❌ 不使用 | 测试未使用 |
| LCG/Radix 压缩 | ❌ 不使用 | 未暴露给 graphDB |
| 搜索高亮 | ❌ 不使用 | graphDB 的 `SearchResult.highlights` 字段在 tantivy adapter 中也为 None |
| **jieba-rs 中文分词** | ✅ **使用** | 这是 inversearch 真正提供而 tantivy 缺失的唯一关键能力 |
| **EmbeddedIndex 简单 API** | ✅ **使用** | `EmbeddedIndex::add(id, content)` 极其简单 |

核心发现：**jieba 中文分词是 inversearch 对 graphDB 不可替代的唯一能力**。其他所有 inversearch 特色功能均未被 graphDB 使用。

## 双引擎维护成本

| 维度 | inversearch | tantivy | 冗余说明 |
|------|-------------|---------|----------|
| 源码行数 | ~16,400 | ~105,860 | 两个完整的全文搜索栈 |
| 模块数 | ~50+ 模块 | ~30+ 模块 | 都是完整的索引、搜索、存储、序列化 |
| 依赖数 | 21 个 crate | ~30 个 workspace crate | 大量重复（都有 zstd, lz4, serde 等） |
| 测试 | 大量单元测试 | 本 crate 不维护测试 | 集成测试需要同时维护两套引擎 |
| 配置 | `InversearchConfig` | `TantivyConfig` | 两套配置体系 |
| Feature gate | `dep:inversearch-service` | `dep:tantivy` | 必须同时引用 |

### inversearch 依赖明细

```
tokio, tracing, serde/serde_json, postcard, anyhow, thiserror,
toml, chrono, regex, linked-hash-map, lazy_static, lru,
unicode-normalization, async-trait, zstd, base64, ahash,
bumpalo, lz4_flex, ciborium, dashmap, memmap2, jieba-rs
```

其中许多依赖与 graphDB 主 crate 已存在依赖重复（tokio, serde, lru, zstd, dashmap, chrono, regex 等）。

## tantivy 缺失能力分析

### 1. CJK 中文分词（jieba）❌ tantivy 无内置支持

这是唯一真正缺失的关键能力。

**解决方案：为 tantivy 实现 `JiebaTokenizer`**

tantivy 通过 `tokenizer-api` crate 暴露 `Tokenizer` trait，允许不依赖 tantivy 主体就实现自定义 tokenizer：

```rust
// tokenizer-api crate（独立轻量依赖）
pub trait Tokenizer: 'static + Clone + Send + Sync {
    type TokenStream<'a>: TokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a>;
}
```

实现 `JiebaTokenizer` 约需 **200-300 行**，核心逻辑：

```rust
#[derive(Clone)]
pub struct JiebaTokenizer {
    jieba: Arc<Jieba>,
}

impl Tokenizer for JiebaTokenizer {
    type TokenStream<'a> = JiebaTokenStream<'a>;
    
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let tokens = self.jieba.tokenize(text, TokenizeMode::Search, false);
        JiebaTokenStream { tokens: tokens.into_iter(), text }
    }
}
```

然后在 graphDB 启动时注册：

```rust
index.tokenizers().register("jieba", JiebaTokenizer::default());
// schema 中引用
let text_field = TextFieldIndexing::default()
    .set_tokenizer("jieba");
```

### 2. 搜索高亮 ✅ tantivy 已有

tantivy 的 `src/snippet/` 模块提供内置 `SnippetGenerator`：

```rust
let snippet_generator = SnippetGenerator::create(&searcher, &query, content_field)?;
let snippet = snippet_generator.snippet_from_doc(doc);
```

可以直接填充 `SearchResult.highlights` 字段。

### 3. 单文件持久化 ✅ 不必要

tantivy 使用目录持久化（`meta.json` + segment 文件），在 graphDB 场景下同样可靠。`FulltextIndexManager` 已支持目录结构 (`dir_name` + `meta.json` 检测)。

### 4. ColdWarmCache 三级缓存 ❌ graphDB 不需要

graphDB 的全文索引是图的附属索引，每次搜索后从主存储取顶点。全文索引的热点数据管理由操作系统 mmap 处理即可。tantivy 的 `MmapDirectory` 天然高效。

### 5. 前向/反向/全向 tokenize 模式 ❌ graphDB 不需要

graphDB 使用默认 Strict 模式不需要子短语索引。如果需要，tantivy 的 `PhraseQuery` 通过 term positions 在查询时解决，比索引时预生成子短语更高效。

## 迁移方案

### 涉及的文件

| 操作 | 文件 | 说明 |
|------|------|------|
| **删除** | `crates/inversearch/` | 整个 crate 目录 |
| **删除** | `src/search/adapters/inversearch_adapter.rs` | 186 行 adapter |
| **删除** | `src/search/adapters/mod.rs` | 5 行（导出 InversearchEngine 和 InversearchConfig） |
| **修改** | `Cargo.toml` | 移除 `inversearch-service` 依赖 |
| **清理** | `src/search/engine.rs` | 移除 `EngineType::Inversearch` 变体 |
| **清理** | `src/search/factory.rs` | 移除 inversearch 分支（66→25 行） |
| **清理** | `src/search/config.rs` | 移除 `inversearch: InversearchConfig` 字段 |
| **清理** | `src/search/manager.rs` | 移除 `try_restore_inversearch_index()`、`.bin` 文件处理 |
| **删除** | `src/search/index_cache.rs` | graphDB 层面的搜索结果缓存（交给 tantivy 管理） |
| **清理** | `src/search/mod.rs` | 移除 `index_cache` 导出 |
| **清理** | `src/core/types/index.rs` | 移除 `FulltextEngineType::Inversearch`、`InversearchIndexConfig`、`TokenizeMode`、`CharsetType` |
| **清理** | `src/query/parser/parsing/fulltext_parser.rs` | 移除 inversearch 解析分支 |
| **清理** | `src/query/validator/fulltext_validator.rs` | 移除 inversearch 验证分支 |
| **清理** | `src/query/executor/admin/index/fulltext_index/create_fulltext_index.rs` | 移除 `convert_engine_type()` 中的 Inversearch 映射 |
| **删除** | `tests/fulltext/engine_comparison.rs` | 8 个引擎对比测试 |
| **修改** | `tests/fulltext/basic.rs` | 移除 inversearch 专有测试（如 `test_create_fulltext_index_inversearch`） |
| **修改** | `tests/fulltext/common.rs` | 简化，不再需要 `get_engine_type()` |
| **清理** | 其他引用 | `FulltextEngineType::Inversearch` 的 31 处引用 |
| **新增** | `src/search/tokenizer/jieba.rs` (新建) | 实现 `JiebaTokenizer`（遵循 tantivy tokenizer-api） |
| **修改** | `src/search/tantivy_index.rs` | 注册 jieba tokenizer，使用 jieba 作为默认 tokenizer |

### 变更统计

- **删除代码**: ~29 个 inversearch 模块 + 186 行 adapter，约 16,500 行
- **graphDB 层清理**: 约 10 个文件，200 行代码修改
- **新增代码**: JiebaTokenizer ~250 行
- **测试调整**: 约 3 个测试文件

**净减少**: ~16,000 行代码

### 清理顺序

```
Phase 1: 实现 JiebaTokenizer（无风险准备工作）
Phase 2: 替换 graphDB 的 Inversearch → BM25（内部变更，外部接口不变）
Phase 3: 删除 inversearch 依赖和 crate
Phase 4: 清理测试
Phase 5: 可选——移除 index_cache.rs（缓存交给 tantivy）
```

## 风险与注意事项

1. **数据迁移**: 现有 inversearch 格式的 `.bin` 文件无法被 tantivy 读取。需要迁移脚本或通过 `search` + `re-index` 重建。建议在新版本中执行破坏性迁移（不向前兼容）。

2. **BM25 评分差异**: inversearch 和 tantivy 的 BM25 默认参数不同：
   - tantivy 默认: `k1=1.8, b=0.4`
   - 标准 Lucene: `k1=1.2, b=0.75`
   - inversearch 评分: 基于位置，非 BM25
   这意味着统一后搜索结果的排序可能变化。请在测试中确认可接受。

3. **查询语义差异**: tantivy 的 `QueryParser` 默认使用 `Occur::Should`（OR），inversearch 也是 OR。两者行为基本一致。

4. **commit 语义**: tantivy 是 segment 级 commit，inversearch 是单文件持久化。tantivy 的 `commit()` 是真正的原子提交，可靠性更高。

5. **CJK 分词覆盖**: 需要确保 jieba tokenizer 在所有中文搜索路径上生效——包括 schema 定义、query parser 分词。

## 结论

**建议统一到 tantivy，放弃 inversearch。**

原因总结：
1. **极低边际收益**：inversearch 的 16,400 行代码和 21 个依赖仅提供 graphDB 当前使用中的一个独特能力（jieba 分词）
2. **高维护成本**：两个完整全文搜索栈的并行维护、调试、依赖管理
3. **tantivy 的成熟度**：~105,860 行生产级代码、Quickwit 验证、BM25 评分、丰富查询类型
4. **jieba 缺失可修复**：200-300 行实现 JiebaTokenizer，工作量远低于维护两套引擎
5. **净减少 ~16,000 行代码**：简化架构、减少编译时间、降低安全风险面

唯一需要投入的工作是 tantivy 的 `JiebaTokenizer` 实现，这对于消除两套引擎的冗余来说是值得的。

## 相关文件

- `crates/inversearch/` — 将删除的整个 crate（~16,400 行）
- `src/search/adapters/inversearch_adapter.rs` — 将删除的 adapter
- `src/search/tantivy_index.rs` — 将新增 JiebaTokenizer 集成
- `src/search/engine.rs` — 移除 EngineType::Inversearch
- `src/search/factory.rs` — 简化，仅 BM25 分支
- `src/core/types/index.rs` — 移除 inversearch 类型定义
- `Cargo.toml` — 移除 inversearch-service 依赖
- `search_engine_integration.md` — 之前的双引擎集成方案（此文档为其结论替代）
