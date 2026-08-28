# VectorQueryExpr::Text / Parameter 集成设计分析

## 一、业界通用做法

| 系统 | Text 查询处理 | 参数化查询 | 设计模式 |
|------|--------------|-----------|---------|
| **Weaviate** | `nearText("query")` → 服务端自动调用 vectorizer 模块，透明 embed | 不支持 SQL 参数绑定 | 服务端延迟 embed |
| **Milvus** | `Function` 模块，search 时接受 raw text 并自动 embed | 支持 `EmbeddedText` 类型入参 | 服务端延迟 embed |
| **Qdrant** | 客户端 `FastEmbed` 库预 embed 后传入向量 | 不支持文本参数化 | 客户端预处理 |
| **Pinecone** | 应用层 embed 后传入 | 不支持 | 客户端预处理 |
| **pgvector** | 应用层 embed 后写入 SQL | 标准 SQL prepared statement | 客户端预处理 |

**行业共识**：text-to-vector 的 embed 操作有两种主流位置：

1. **客户端预处理**（Qdrant/Pinecone/pgvector）：应用 embed 后传入纯向量
2. **服务端延迟处理**（Weaviate/Milvus）：数据库在查询执行时自动调用 embed

对于一个 SQL 接口的图数据库，更接近 Weaviate/Milvus 模式——用户写
`SEARCH VECTOR idx WITH text = '查询'`，数据库内部完成 embed。

## 二、本项目现有架构约束

从代码分析得出的关键约束：

```
Plan-build time (同步)          Execution time (async)
─────────────────────          ─────────────────────
vector_query_to_vec()          VectorOperator::next()
├── 可访问 exec_ctx            ├── 可访问 vector_coordinator
├── 可访问 exec_ctx.parameters ├── 可调用 embed_text().await
├── 同步函数                   ├── async 上下文
└── 产出 VectorSpec            └── 消费 VectorSpec
```

- `exec_ctx.parameters` 在 plan-build 时已可用（`ddl.rs:700` 的 `exec_ctx` 参数），值是 `Arc<HashMap<String, Value>>`
- `embed_text()` 是 async 方法，`vector_coordinator` 在 plan-build 时可通过 `exec_ctx.vector_coordinator` 获取，但不能 await

### 关键阻塞点

`ddl.rs:762` 的 `vector_query_to_vec` 是同步函数，拒绝 Text/Parameter：

```rust
VectorQueryType::Text => Err(PlanBuildError::CapabilityUnavailable { ... }),
VectorQueryType::Parameter => Err(PlanBuildError::CapabilityUnavailable { ... }),
```

## 三、推荐方案：分层处理（Layered Resolution）

### 3.1 Parameter — Plan-build 时立即解析

`vector_query_to_vec` 签名改为接收 `exec_ctx`，在 plan-build 时通过
`exec_ctx.get_param(&name)` 解析参数值为 `Vec<f32>`。

**理由**：
- 参数值在 plan-build 时已确定（`HashMap` 已填充），不需要延迟
- 符合 prepared statement 的行业惯例：参数绑定发生在执行前
- 避免每行重复解析同一参数

### 3.2 Text — Execution 时延迟 embed

在 `VectorSpec` 三个搜索变体（VectorSearch/VectorLookup/VectorMatch）中增加
`query_text: Option<String>`，在 `VectorOperator::next()` 中通过
`coordinator.embed_text().await` 完成延迟 embed。

**理由**：
- 与 Weaviate 的 `nearText` 模式一致：用户写 text，服务端自动 embed
- embed 是 I/O 密集操作（HTTP 调用外部 embedding API），必须在 async 上下文执行
- `vector_coordinator` 已在 `VectorOperatorKind` 中持有，无需额外注入

## 四、修改文件清单

### 4.1 docs/archive/vector_query_expr_integration_design.md（本文件）

### 4.2 crates/graphdb-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs

- `vector_query_to_vec` 签名增加 `exec_ctx: &ExecutionContext` 参数
- 3 个调用点（`build_vector_search_spec`、`build_vector_lookup_spec`、`build_vector_match_spec`）传入 exec_ctx
- `VectorQueryType::Parameter` 分支实现解析逻辑
- `VectorQueryType::Text` 返回空 `Vec<f32>` 占位 + `query_text` 标记

### 4.3 crates/graphdb-query/src/executor/streaming/operators/spec.rs

- `VectorSpec::VectorSearch` 增加 `query_text: Option<String>` 字段
- `VectorSpec::VectorLookup` 增加 `query_text: Option<String>` 字段
- `VectorSpec::VectorMatch` 增加 `query_text: Option<String>` 字段

### 4.4 crates/graphdb-query/src/executor/streaming/operators/vector_operator.rs

- `VectorOperatorKind::VectorSearch` 增加 `query_text: Option<String>` 字段
- `VectorOperatorKind::VectorLookup` 增加 `query_text: Option<String>` 字段
- `VectorOperatorKind::VectorMatch` 增加 `query_text: Option<String>` 字段
- `VectorOperator::from_spec` 线程 `query_text` 到对应 `VectorOperatorKind`
- `VectorOperator::next` 在执行前检查 `query_text`，调用 `coordinator.embed_text().await` 替换 `query_vector`

## 五、与现有架构模式的对比

| 模式 | 本项目现有实现 | Parameter 方案 | Text 方案 |
|------|--------------|---------------|----------|
| `Expression::Parameter` | Execution-time HashMap lookup | Plan-build-time HashMap lookup（更高效） | — |
| `FulltextQueryExpr` | Plan-build 时转 String | — | — |
| `SubqueryRunnerSpec` | Execution-time lazy materialize | — | Execution-time lazy embed（类似模式） |
| Weaviate `nearText` | — | — | 服务端延迟 embed |
| Milvus `Function` | — | — | 服务端延迟 embed |

## 六、总结

| 维度 | Parameter | Text |
|------|-----------|------|
| 推荐时机 | Plan-build 时立即解析 | Execution 时延迟 embed |
| 业界参照 | Prepared statement 参数绑定 | Weaviate nearText / Milvus Function |
| 核心原因 | 参数值已可用，无需延迟 | embed 是 async I/O，必须在执行上下文 |
| 架构一致性 | 与 `Expression::Parameter` 目标相同（绑定参数值），但时机更早 | 与 `SubqueryRunnerSpec` 的 lazy materialize 模式一致 |
| 复杂度 | 低（改 `vector_query_to_vec` 签名 + 3 个调用点） | 中（改 Spec 结构 + Operator 执行逻辑） |
