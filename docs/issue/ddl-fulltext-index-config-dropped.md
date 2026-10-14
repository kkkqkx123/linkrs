# P0 [query] 全文索引的 ANALYZER / BOOST / OPTIONS / index_name 全部静默丢弃

## 现象

文档 §6 定义 `CREATE FULLTEXT INDEX` 支持：

- 字段级分词器 `ANALYZER '<analyzer>'` 与权重 `BOOST <weight>`
- `ENGINE = BM25`
- `OPTIONS (k1=..., b=..., analyzer=..., 其他键)`，并给出默认值表（k1=1.5、b=0.75）

parser 侧全部正确产出：`crates/linkrs-query/src/parser/ast/fulltext.rs:34`（`IndexFieldDef { analyzer, boost }`）、
`:42`（`IndexOptions { bm25_config, common_options }`）、`:49`（`BM25Options { k1, b, field_weights, analyzer, store_original }`），
解析逻辑在 `parser/parsing/fulltext_parser.rs:96-196`。

**但从 planner 节点往下全部丢失**，共四处：

| 层 | 位置 | 丢失内容 |
|---|---|---|
| spec 命令 | `executor/streaming/operators/spec/fulltext.rs:10-16` | `FulltextManageCommand::Create` 只有 `index_name / schema_name / fields: Vec<String> / space_id`，无 `engine_type`、无 `options`、字段只剩名字 |
| spec builder | `executor/streaming/plan/arena_builder/specs/ddl.rs:598-603` | `fields` 映射为 `f.field_name`，`analyzer` 与 `boost` 被丢弃 |
| executor | `executor/streaming/operators/fulltext_operator.rs:246-275` | `manager.create_index(space_id, schema_name, field_name, None)`——options 直接传 `None` |
| executor | 同上 | `index_name` **从未传入** `create_index`，用户声明的索引名对实际全文索引无效 |

也就是说：`ANALYZER 'standard'`、`BOOST 2.0`、`OPTIONS (k1=1.2, b=0.8, analyzer=standard)` 都是**可解析但完全无效果**的语法糖。
而 `DROP FULLTEXT INDEX <name>` 与 `SHOW FULLTEXT INDEX <name>` 又按 `index_name` 做 metadata 匹配，导致建索引用的键和删索引用的键不是同一个。

## 影响

1. 文档 §6 的选项表整体是虚假承诺，且失败完全静默——用户以为调了 BM25 参数，实际用的是硬编码默认值。
2. `index_name` 语义断裂：创建时不使用它，删除/查询时依赖它，可能出现"创建成功但按名字删不掉"或"删掉了另一个同名索引"。
3. 与向量索引形成刺眼反差：`VectorManageCommand::Create`（`spec/vector.rs:20-33`）把 `vector_size/distance/hnsw_m/hnsw_ef_construct/quantization/quantile/compression/always_ram` 九个参数完整贯通到
   `vector_operator.rs:271-386`。同样的 DDL 设计，一个贯通一个断链，说明这是漏做而非设计取舍。

## 根因

`FulltextManageCommand::Create` 的字段集在设计时被窄化为"一个索引一个字段一个 schema"的最小集，而 parser 与 planner 节点
（`CreateFulltextIndexNode`）是按文档的多字段 + 分数字典设计的。spec 层是断点。

## 建议

1. `FulltextManageCommand::Create` 扩充为携带 `engine_type`、`Vec<{field_name, analyzer, boost}>`、`options`。
2. spec builder 全量传递，executor 把 BM25 选项真正传入 `manager.create_index`。
3. 统一索引命名：要么 `create_index` 接受用户 `index_name`（并让 Drop/Show/Describe 按同一键匹配），
   要么文档明确"索引名仅为展示标识，实际键为 (space, schema, field)"。
4. 若 `linkrs-fulltext` 侧暂不支持 per-field analyzer 与 BM25 调参，则**先从文档删除这些子句与选项表**，
   不要让语法悬空。禁止保留可解析但无效的分支。
5. 补执行层测试：断言 `OPTIONS (k1=1.2)` 之后索引元数据中 k1 确为 1.2，而不是只测解析。

## 严重级

**P0（正确性 / 静默失效）** — 用户可见参数被无声吞掉，且无任何报错路径。

## 代码位置

- `crates/linkrs-query/src/parser/ast/fulltext.rs:22-56`
- `crates/linkrs-query/src/parser/parsing/fulltext_parser.rs:74-209`
- `crates/linkrs-query/src/executor/streaming/operators/spec/fulltext.rs:10-16`
- `crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs:592-603`
- `crates/linkrs-query/src/executor/streaming/operators/fulltext_operator.rs:246-275`
- 对照（正例）：`crates/linkrs-query/src/executor/streaming/operators/spec/vector.rs:20-33`、`vector_operator.rs:271-403`
