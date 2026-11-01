# 全文检索操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的全文检索操作（BM25），包括 DDL 管理命令、检索算子、
读写一致性与与向量检索的对称性。本文档与 2026-11-01 代码现状对齐。

底层引擎为 tantivy（BM25，支持 jieba 中文分词，见根 README），
查询模块通过 `FulltextSpec` 驱动 `FulltextOperator` 完成索引管理与检索。

---

## 一、语义类型

全文检索操作分两类：

1. **DDL 管理命令**：索引的创建/删除/变更/查看；
2. **检索语句**：`LOOKUP ... WHERE`、`MATCH ... WHERE <全文匹配条件>` 等形式的文本检索。

---

## 二、物理层 FulltextSpec

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/fulltext.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::FulltextSpec`）

### 2.1 索引管理命令（`FulltextManageCommand`）

| 命令 | 说明 |
|------|------|
| `Create` | 创建索引：`index_name` + `schema_name` + 字段列表（`IndexFieldDef`）+ 引擎类型（`FulltextEngineType`）+ `IndexOptions` + `if_not_exists` |
| `Drop` | 删除索引（`if_exists`） |
| `Alter` | 变更索引（`AlterIndexAction` 列表） |
| `Show` | 列出索引（`pattern` 过滤） |
| `Describe` | 查看索引详情 |

`is_write()` 区分写命令（Create/Drop/Alter，需要持久化）与只读命令（Show/Describe）。

### 2.2 检索变体（`FulltextSpec`）

| 变体 | 说明 |
|------|------|
| `FulltextSearch` | 独立全文检索：结构化查询 `FulltextQuery` 定位到 (tag, field)，带 `limit` |
| `FulltextLookup` | LOOKUP 语句携带的全文检索 |
| `MatchFulltext` | MATCH 语句 WHERE 中的全文匹配条件（`match_expr` 编译为 `structured_query`） |
| `FulltextManage` | 承载上述 DDL 管理命令 |

三种检索变体均携带 `ryw_config: Option<ReadYourWritesConfig>`——
**读己之写一致性**配置：`None` 为最终一致，`Some` 时检索必须看到本会话已提交的写入
（tantivy 索引与存储之间存在索引延迟，RYW 通过提交点/可见性控制弥合）。

---

## 三、FulltextOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/fulltext_operator.rs`

`FulltextOperator` 的状态枚举 `FulltextOperatorKind` 与 spec 对应。
关键实现要点：

- `from_spec`：将 spec 转为运行时状态；DDL 命令走 `FulltextManageCommand` 分发；
- `fulltext_engine_config`（`:73`）：构造引擎配置，携带 BM25 与公共选项
  （测试 `engine_config_carries_bm25_and_common_options` 保障）；
- `plain_option_json`：无附加选项时的空选项 JSON 形态；
- `next()`（`:327`，约 500 行）：检索主循环——执行 `FulltextQuery`、
  将命中文档（doc_id + score）与图存储关联，产出顶点行流；
- DDL 的写命令经事务语义执行（`is_write()` 判定）。

---

## 四、从查询到执行的完整流程

```
1. 解析：LOOKUP / MATCH ... WHERE 全文条件 → parser/ast/fulltext.rs AST
2. 规划：全文条件编译为 FulltextQuery，生成 FulltextSpec
   （MatchFulltext / FulltextLookup / FulltextSearch）
3. 执行：FulltextOperator 调用 linkrs-fulltext（tantivy BM25）执行检索
4. 关联：命中文档映射回图顶点，交上层算子继续处理
   （可与 Expand/Join 等组合完成"文本召回 + 图扩展"混合查询）
```

---

## 五、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **Fulltext 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/fulltext_operator.rs` |
| **物理 FulltextSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/fulltext.rs` |
| **全文 AST** | `crates/linkrs-query/src/parser/ast/fulltext.rs` |
| **BM25 引擎** | `crates/linkrs-fulltext/`（tantivy 封装，`query::FulltextQuery`） |
| **HTTP 检索端点** | `crates/linkrs-server/src/http/router.rs`（`/v1/fulltext/search`，与 `/v1/vector/search` 对称） |

---

## 六、总结

1. **DDL 与检索一体**：索引管理（Create/Drop/Alter/Show/Describe）与三种检索形态
   共用同一算子族，写命令走事务语义；
2. **三种检索入口**：独立 Search、LOOKUP、MATCH WHERE 覆盖全部查询语法位；
3. **RYW 一致性显式化**：每个检索 spec 携带 `ReadYourWritesConfig`，默认最终一致、
   按需提升为读己之写；
4. **与向量检索对称**：HTTP 层 `/v1/fulltext/search` 与 `/v1/vector/search` 结构对称，
   调用方可按语义/性能选择后端。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
