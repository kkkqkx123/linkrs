# 全文检索引擎集成方案

## 背景

graphdb 支持两个全文检索引擎后端：`Inversearch`（`crates/inversearch`）和 `BM25`（vendored `crates/tantivy`），通过 `fulltext-search` feature 同时启用。之前存在一个冗余的 `crates/bm25` 适配层，已删除，但残留了 `src/search/` 中的空模块声明导致编译失败。

本文档分析两个引擎的集成方案，以及是否应将全文检索统一为一个 crate 引用。

## 引擎架构现状

```
graphdb (src/search/)
├── engine.rs          → SearchEngine trait, EngineType enum
├── factory.rs         → SearchEngineFactory 分发
├── adapters/
│   └── inversearch_adapter.rs → InversearchEngine(wrapping EmbeddedIndex)
├── tantivy_index.rs   → TantivySearchEngine(wrapping tantivy::Index)
├── manager.rs         → FulltextIndexManager 统一编排
└── config.rs          → FulltextConfig(tantivy + inversearch 配置)
```

### 两个引擎的核心差异

| 维度 | Inversearch | Tantivy(BM25) |
|------|-------------|---------------|
| Tokenizer 体系 | 运行时 13 阶段 Encoder pipeline（配置驱动） | 编译期 Tokenizer/TokenFilter trait（类型参数化） |
| CJK 支持 | jieba-rs 完整分词 | 仅字符级拆分 |
| 存储格式 | 单 `.bin` 文件（MessagePack+Zstd） | 目录（含 meta.json, segments） |
| 查询模式 | 自定义 forward/reverse/full/ngram | BM25 评分 + TopDocs collector |
| 成熟度 | 原生集成，完整适配器 | 有完整 tantivy 源码（vendored），本次新增适配器 |

## 双引擎集成方案对比

### 方案 A：当前适配器模式（推荐）

```
graphdb
├── 依赖 inversearch-service (optional)
├── 依赖 tantivy (optional)
└── src/search/
    ├── adapters/inversearch_adapter.rs
    └── tantivy_index.rs
```

**优点：**
- 职责清晰，两个引擎完全隔离
- 编译隔离：`fulltext-search` feature 可分别开关
- 每个引擎可使用自己的 tokenizer 体系
- EngineType 用户选择，运行时无争议

**缺点：**
- graphdb 依赖两个外部 crate
- 两个引擎的 SearchEngine trait 实现有代码重复（id mapping, error handling 等）

### 方案 B：inversearch 包裹 tantivy

```
graphdb
└── 依赖 inversearch-service (全功能)
    └── inversearch 可选依赖 tantivy
        └── api::embedded 暴露 TantivyIndex 和 EmbeddedIndex
```

**优点：**
- graphdb 只依赖一个 crate

**缺点：**
- inversearch 引入 tantivy 大量依赖（且 tantivy 依赖链很深）
- 两个 tokenizer 体系（Encoder vs Tokenizer/TokenFilter）冲突，强行耦合
- inversearch 的架构纯净度被破坏
- 编译时间显著增加（tantivy + tokenizer-api + stacker + sstable + ...）
- 边际收益低：`EngineType::Bm25` 用户其实只关心 BM25 评分，不需要 inversearch 的能力

### 方案 C：新建 `graphdb-fulltext` 统一 crate

```
graphdb
└── 依赖 graphdb-fulltext
    ├── 可选依赖 inversearch-service
    └── 可选依赖 tantivy
```

**优点：**
- 单一依赖入口
- 可定义统一的 `TextTokenizer` trait 屏蔽后端差异

**缺点：**
- 额外抽象层，当前项目处于开发阶段，抽象时机未到
- `TextTokenizer` trait 已有（`crates/inversearch/src/encoder/tokenizer_trait.rs`），无需新增 crate

## 结论：保持方案 A

**理由：** 两个引擎的技术栈差异过大（inversearch 运行时配置 vs tantivy 编译期类型化），强制合并带来的耦合成本 > 收益。方案 A 已在本次修改中实际可用（`TantivySearchEngine` 已实现全部 `SearchEngine` trait 方法）。

## 后续演进方向

1. **TextTokenizer trait**（已完成）作为引擎接口层的轻量抽象，允许自定义 tokenizer 绕过 Encoder pipeline
2. **Tantivy CJK 支持**（中优先级）：如需提升 EngineType::Bm25 的中文搜索质量，可为 tantivy 实现 `JiebaTokenizer`（需遵循 tantivy 的 Tokenizer/TokenStream/TokenFilter 模式）
3. **缩减 tantivy 依赖体积**（低优先级）：当前 vendored tantivy 包含大量 graphdb 不需要的模块（facet, aggregation 等），可裁剪

## 相关文件

- `src/search/engine.rs` — SearchEngine trait + EngineType
- `src/search/factory.rs` — 引擎工厂
- `src/search/adapters/inversearch_adapter.rs` — InversearchEngine
- `src/search/tantivy_index.rs` — TantivySearchEngine（本次新增）
- `crates/inversearch/src/encoder/tokenizer_trait.rs` — TextTokenizer trait（本次新增）
- `crates/tantivy/` — vendored tantivy 源码
- `crates/inversearch/` — inversearch 源码
