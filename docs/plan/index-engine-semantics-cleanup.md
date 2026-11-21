# 引擎语义归位与索引 DDL 收敛方案

## 0. 目标与约束

本文处理三块遗留：`FulltextEngineType` / `engine_type` 的清理、索引 DDL 的统一、`DisabledEngine` 与 `is_disabled_engine()` 守卫的消除。三者表面无关，根因是同一个：**同一件事在三个地方各存了一份状态，而这三份状态都可以由配置在构造期唯一推出**。

- 引擎种类：既有 `FulltextEngineType`（core）、`FulltextEngineType`（config）、`FulltextBackendKind`（config），又有 `VectorEngineKind`。
- 索引 DDL：既有 "在语句里指定 ENGINE"，又有 "在 config 里指定 backend"，两者可以互相矛盾。
- vector 可用性：既有 "config 里 enabled"，又有 "运行期 `is_disabled()` 状态位"，还有 "调用点 12 处守卫"。

约束（来自 `AGENTS.md`）：

- 项目处于开发阶段，不做向后兼容，不为旧语法/旧元数据写迁移层。
- 代码、注释、日志一律英文；文档中文；代码注释不得引用计划编号。
- 子 crate 严格 DAG；`dyn` 使用需在 `docs/archive/dynamic.md` 归档。

## 1. 问题清单

### 1.1 总览

| 编号 | 问题 | 严重度 | 处理 |
| --- | --- | --- | --- |
| A | `FulltextEngineType` 有 core / config 两份，序列化与 `Display` 都不一致 | 中 | 两份都删 |
| B | `engine_type` 从 parser 一路传到 manager，唯一用途是填一个恒为 `Bm25` 的元数据字段 | 中 | 全链路删除 |
| C | `CREATE FULLTEXT INDEX` 与 `CREATE VECTOR INDEX` 的 `IF NOT EXISTS` 位置不同 | 中 | 统一到 `INDEX` 之后 |
| D | 两侧 DDL 的 `Drop` / `Show` / `Describe` 执行逻辑各写一遍 | 低 | 抽共享骨架 |
| E | `SearchIndexMetadata.engine` 一侧填 `bm25`、一侧填 `vector`，都不是后端 | 低 | 统一填后端名 |
| F | 元数据里的 `engine_type` 恒为 `Bm25`，ES 后端下是错的信息 | 中 | 改记 `backend` |
| G | `QdrantEngineHandle` 用 `engine().name() == "disabled"` 字符串嗅探判定可用性 | 高 | 删判定 |
| H | `is_disabled()` 状态位散落在 trait、handle、backend、manager、coordinator 五层 | 高 | 全部删除 |
| I | `VectorIndexManager` 有 10 处 `is_disabled_engine()` 守卫，为不可达状态兜底 | 高 | 全部删除 |
| J | `from_config_async` 的 Qdrant 分支不校验 `qdrant.enabled`，能造出 disabled 后端 | 高 | 补校验并拒绝 |
| K | `vector_disabled_skips` 指标与 `SyncDiagnostics` 字段只能恒为 0 | 低 | 删除 |

### 1.2 逐条证据

**A：两份同名枚举。**

`linkrs-core/src/types/index.rs:368-380`：

```rust
pub enum FulltextEngineType {
    #[serde(rename = "bm25")]
    Bm25,
}
...
FulltextEngineType::Bm25 => write!(f, "BM25"),
```

`linkrs-config/src/fulltext.rs:9-21`：

```rust
pub enum FulltextEngineType {
    #[default]
    Bm25,
}
...
FulltextEngineType::Bm25 => write!(f, "bm25"),
```

同一概念两种序列化形态（`"bm25"` 与 `"Bm25"`，后者是 default 派生的 variant 名）、两种 `Display`（`"BM25"` 与 `"bm25"`）。`linkrs-config/src/fulltext.rs:246-248` 的字段注释承认它的性质是"记录产地，必须保持 Bm25"——一个恒真值的字段不携带信息。

**B：`engine_type` 是装饰参数。**

`linkrs-query/src/parser/parsing/fulltext_parser.rs:127-141` 只接受 `ENGINE = BM25`，其它值直接报错，然后赋值 `FulltextEngineType::Bm25`：

```rust
if ctx.check_keyword("BM25") {
    ctx.consume_keyword("BM25")?;
} else {
    return Err(... "Expected BM25 engine type" ...);
}
let engine_type = FulltextEngineType::Bm25;
```

这个值经 `ast/fulltext.rs:27` → `bind_ddl.rs:107` → `stmt_ddl.rs:117` → `fulltext_planner.rs:176` → `management.rs:18` → `arena_builder/specs/ddl/fulltext.rs:17` → `spec/fulltext.rs:16` → `fulltext_operator.rs:430` 传到 `manager.rs:500`，而 manager 的接收形参就叫 `_engine_type`，唯一用途在 `manager.rs:517`：

```rust
let engine_type = _engine_type.unwrap_or(self.default_engine);
```

随后写入 `metadata.engine_type`（`manager.rs:539`）。真正的引擎选择发生在 `manager.rs:95-117` 的 `build_engine`，按 `self.config.backend` 分派——**与 DDL 里写的 `ENGINE = BM25` 无关**。也就是说 `CREATE FULLTEXT INDEX ... ENGINE = BM25` 在 `backend = "es"` 时照样建出 ES 索引，语法与行为矛盾。

**C：`IF NOT EXISTS` 位置不一致。**

`fulltext_parser.rs:78-93`：

```
CREATE FULLTEXT INDEX IF NOT EXISTS <name> ON <schema> (<fields>) ENGINE = BM25
```

`vector_parser.rs:56-71`：

```
CREATE IF NOT EXISTS VECTOR INDEX <name> ON <schema> (<field>) WITH (...)
```

同一语法族、同一修饰子句、两个位置。前者是 SQL 惯例位置，后者不是。

**D：管理动作各写一遍。**

`fulltext_operator.rs:479-520` 的 `Drop` 与 `vector_operator/manage.rs` 的 `Drop` 都实现"按名字过滤 + `if_exists` 决定报错还是静默"；`Show`/`Describe` 同理。两侧错误文案格式不同（`"Fulltext index not found: {}"` 与 vector 侧另一套）。

**E / F：描述符与元数据的 engine 值。**

`linkrs-api/src/api_core/fulltext_api.rs:128`：`engine: m.engine_type.to_string()` → 恒为 `"bm25"`。
`linkrs-api/src/api_core/vector_api.rs:231`：`engine: "vector".to_string()` → 恒为 `"vector"`。

一个字面量常量、一个恒真枚举，谁都不是"哪个后端在服务"。

**G / H：字符串嗅探与状态位五层穿透。**

`linkrs-sync/src/backend.rs:752-754`：

```rust
/// Cached result of `manager.engine().name() == "disabled"` resolved at
/// construction time, so capability checks never compare name strings.
disabled: bool,
```

`backend.rs:770`：`let disabled = manager.engine().name() == "disabled";`

判定链：`vector-client/src/manager.rs:35-45`（`enabled == false` → `DisabledEngine`）→ `backend.rs:770`（name 嗅探）→ `backend.rs:796`（`is_disabled`）→ `metrics_handle.rs:71`（装饰器委托）→ `backend.rs:1372`（`VectorBackend::is_disabled`）→ `vector_index_manager.rs:328`（`is_disabled_engine`）→ `vector_sync.rs:231`（coordinator 转发）。

**I：10 处守卫。**

`vector_index_manager.rs:355, 676, 774, 959, 1297, 1314, 1331, 1346, 1377, 1400` 各有一份 `if self.is_disabled_engine() { return Err(EngineDisabled) }`，另有 `vector_sync.rs:237/484`、`vector_rebuild.rs:142`、`delivery.rs:963/984`。

**J：唯一能造出 disabled 后端的路径没做校验。**

`backend.rs:1408-1418`（`from_config_async` 的 Qdrant 分支）直接 `VectorManager::new(client_config)`，`client_config.enabled` 来自 `qdrant_client_config`（`backend.rs:1245`：`enabled: config.enabled`）。而 server 路径用的是 `config.is_vector_enabled()`（`linkrs-config/src/lib.rs:582-595`，Qdrant 时是 `vector.enabled && qdrant.enabled`）。两者判据不同，`vector.enabled = true` + `qdrant.enabled = false` 时：server 不构造后端（`startup.rs:150` 的 else 分支），但任何直接调 `from_config_async` 的调用者会拿到一个全失败的引擎。

其余构造点都已显式返回 `None`：`linkrs-api/src/embedded/database.rs:78-104`（`!config.vector.enabled` / `!client_config.enabled` 都 `return Ok(None)`）、`database.rs:117-131`、`builder.rs:171-180`（返回 `Err`）。所以 `DisabledEngine` 只在 `from_config_async` 这一条路上可达，而这条路的判据是漏的。

**K：恒零指标。**

`vector_sync.rs:215` 的 `disabled_skips` 只在 `vector_sync.rs:484` 的 disabled 分支自增；该分支随 H/I 一起删除后，`delivery.rs:454`、`outbox_admin.rs:229`、`linkrs-server/src/http/handlers/statistics/query.rs:310,437`、`handlers/sync.rs:190`、`linkrs-metrics/src/manager/search.rs:172` 全部失去意义。

## 2. 设计

### 2.1 原则

三条原则，按顺序决定每个决策：

1. **一个事实只存一处，且在构造期确定。** 引擎种类由 config 决定；可用性由"有没有 backend"决定；索引的物理身份由确定性命名推导。
2. **语法不为已删除的能力保留入口。** 既然不支持在 DDL 里选引擎，就不解析 `ENGINE`——但保留一次"明确的拒绝"，而不是静默忽略。
3. **状态位不跨层。** 一个布尔不能被五层各自缓存一份再用 12 处守卫去查。

### 2.2 引擎语义归位

删掉 `FulltextEngineType` 的两份定义与 `engine_type` 的全链路，索引的引擎由 `FulltextConfig.backend`（`FulltextBackendKind::{Local, Es}`）唯一决定。

关键取舍：**`ENGINE` 子句保留解析、保留拒绝，但不进入 AST。**

- 保留解析与拒绝：`CREATE FULLTEXT INDEX ... ENGINE = BM25` 是既有语法，用户仍会写；静默忽略会让"我指定了 BM25 为什么数据进了 ES"无从诊断。解析器在这里给出明确错误：DDL 不再接受引擎子句，引擎由服务端配置决定。
- 不进入 AST：一旦进入 AST，它就有 8 个传递站点和 1 个 `_engine_type` 死参数。

`IndexMetadata.engine_type: EngineType` 改为 `backend: FulltextBackendKind`（`serde` 带默认值，缺省 `Local`）。这样元数据记录的是"谁在服务这个索引"，与 `build_engine` 的分派依据同源。

`FulltextConfig.default_engine` 一并删除：它的唯一消费者是 `manager.rs:69` 赋给 `default_engine` 字段，再由 `manager.rs:517` 兜底成 metadata 值。metadata 改记 backend 后，这条链断掉，字段归零。`config.toml:145` 的 `default_engine = "Bm25"` 同步删除，`FulltextConfig` 是 `deny_unknown_fields`（`linkrs-config/src/fulltext.rs:242`），留着会启动失败。

core 侧的 `FulltextIndexOptions`、`BM25IndexConfig`、`FulltextIndexField` 一并删除：三者只有 core 内部的互相引用，`types.rs:478-481` 的重导出没有任何外部使用者。留着就是在 `linkrs-core` 里维护一套与 `linkrs-config`、`linkrs-fulltext` 平行且不被使用的索引选项模型。

### 2.3 索引 DDL 统一

统一分三层，只统一**重复的逻辑**，不强行合并**不同的数据**。

**第一层：语法骨架。**

两侧共享的头部解析抽成一个函数：

```rust
// linkrs-query/src/parser/parsing/ddl_parser/index_head.rs
pub(crate) struct CreateIndexHead {
    pub span: Span,
    pub if_not_exists: bool,
    pub index_name: String,
    pub schema_name: String,
}

pub(crate) fn parse_create_index_head(ctx: &mut ParseContext) -> Result<CreateIndexHead, ParseError>;
```

消费顺序固定为 `INDEX [IF NOT EXISTS] <name> ON <schema>`，`IF NOT EXISTS` 统一到 `INDEX` 之后（SQL 惯例，也是 fulltext 现状）。两侧各自只保留自己的前缀关键字（`FULLTEXT` / `VECTOR`）与自己的尾巴（`(<fields>)` + 引擎子句 / `(<field>) WITH (...)`）。

**第二层：管理动作执行。**

新增 `linkrs-query/src/executor/streaming/operators/index_manage.rs`：

```rust
pub(crate) enum IndexKind { Fulltext, Vector }

impl IndexKind {
    /// Lowercase family name used in statement labels.
    pub(crate) const fn label(self) -> &'static str;
}

/// One-row `(action, name, status)` result emitted by management statements.
pub(crate) fn manage_result(
    output_layout: Arc<SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk;

/// Whether an index passes `SHOW <family> INDEX [LIKE pattern] [FROM schema]`.
pub(crate) fn matches_show_filter(
    pattern: Option<&str>,
    from_schema: Option<&str>,
    index_name: Option<&str>,
    tag_name: &str,
) -> bool;

/// Apply `IF EXISTS` semantics to a drop target whose location planning
/// could not resolve: `None` degrades to a `not_exists` row, `Some` is the
/// error to return.
pub(crate) fn unresolved_drop(
    kind: IndexKind,
    index_name: &str,
    if_exists: bool,
) -> Option<QueryError>;
```

`fulltext_operator.rs` 与 `vector_operator/manage.rs` 的 `Drop`/`Show`/`Describe` 分支改调这套，错误文案与 `IF EXISTS` 语义只剩一份实现。结果行构造原先两份（`fulltext_operator.rs` 与 `manage.rs:23-36` 各一个 `make_manage_result`），合并成 `manage_result` 一个。

抽 **`unresolved_drop` 而非设计稿里的 `resolve_named`**：实做时发现两族 `DROP` 的差异不在"从列表里按名字挑"，而在"位置规划阶段能否解析出目标"——target 解析失败时的 `IF EXISTS` 语义才是真正重复的那段，挑列表各自只有一两行。`resolve_named` 需要泛型 + 名字闭包 + 错误构造三件套，调用点反而更长。

**第三层：描述符。**

`SearchIndexMetadata.engine` 统一填后端标识：fulltext 用 `FulltextBackendKind::to_string()`（`"local"` / `"es"`），vector 用 `VectorEngineHandle::engine_name()`（`"local"` / `"qdrant"`）。字段名是 `engine`，值就应该是引擎名。

**不做的部分与理由。** `CreateFulltextIndex` 与 `CreateVectorIndex` 的 AST/bound/plan 类型不合并：两边特有载荷差异大（`Vec<IndexFieldDef>` + BM25 options vs `field_name` + `VectorIndexConfig`），合并需要一个 `IndexPayload` 枚举，然后 57 个构造点各自补一层 `match`，收益是少两个 struct 声明。`Drop`/`Show`/`Describe` 的 AST 类型也不合并：它们是纯数据（2 到 3 个字段），没有逻辑可去重，合并只增加转换代码。判断依据是**重复的是逻辑，不是数据**。

### 2.4 可用性构造期化

删掉整个 disabled 状态链，把"不可用"变成"没有对象"。

**装配侧。** `VectorBackend::qdrant` / `qdrant_with_defaults` 改为返回 `VectorCoordinatorResult<Self>`，内部检查 `manager.is_engine_available()`，不可用则返回 `VectorError::ConfigError`。`VectorManager` 增加 `is_engine_available()`，值来自构造期记录的 `enabled`。`from_config_async` 的 Qdrant 分支在 `!config.qdrant.enabled` 时返回 `ConfigError`，与 `is_vector_enabled()` 同判据。

**引擎侧。** `RemoteVectorEngine` 保留（它服务的是"HTTP / gRPC 两种传输 + 未来引擎"，`docs/archive/dynamic.md` 1.7 有归档）；`DisabledEngine` 降为 `pub(crate)`，因为 `enabled = false` 时让 `VectorManager` 持有一个全失败的引擎是 `vector-client` 的内部降级，不该被上层识别。删除 `QdrantEngineHandle.disabled` 字段与 `name() == "disabled"` 比较——可用性已经由构造器保证，不需要运行期再嗅探一次字符串。

**消费侧。** 删除 `VectorEngineHandle::is_disabled()`、`VectorBackend::is_disabled()`、`VectorIndexManager::is_disabled_engine()`、`VectorSyncCoordinator::is_disabled_engine()`，以及它们的全部守卫。删除 `VectorEngineState`（只剩 `Active` 一个可达值）、`disabled_skips`、`disabled_skip_count()`、`VectorCoordinatorError::EngineDisabled`、`ApplyFailure.disabled` 字段与 5 秒固定重试分支。

引擎**运行期**不可用（断路器打开、健康检查失败、网络超时）本来就由 `circuit_breaker` 与 `VectorError::EngineUnavailable`（`vector_error.rs:167` 归为 `Retryable`）负责，`EngineDisabled` 是一条与它重叠的平行路径。

## 3. 代码变更清单

### 3.1 linkrs-core

- `types/index.rs`：删除 `FulltextEngineType`（368-380）、`BM25IndexConfig`（382-406）、`FulltextIndexField`（408-435）、`FulltextIndexOptions`（440-473）。
- `types.rs:477-482`：重导出只留 `ConsistencyState`（`IndexStats` / `SearchStats` 另起一行）。

### 3.2 linkrs-config

- `fulltext.rs`：删除 `FulltextEngineType`（9-21）；删除 `FulltextConfig.default_engine`（246-249）与 Default 实现中的初始化（266）。
- `config.toml:145`：删除 `default_engine = "Bm25"`。

### 3.3 linkrs-fulltext

- `lib.rs:26-29`：重导出去掉 `FulltextEngineType as EngineType`。
- `manager.rs`：删字段 `default_engine`（31）与初始化（69）；`create_index` 去掉 `engine_type` 形参（476）与转发（484）；`create_index_with_engine_config` 去掉 `_engine_type`（500）与 `unwrap_or`（517）；两处 `IndexMetadata` 字面量（249、275）与 539 行的 `engine_type` 改为 `backend: self.config.backend`。
- `metadata.rs:2,12`：`engine_type: EngineType` → `backend: FulltextBackendKind`，`serde(default)`。
- `isolation_test.rs`、`warmup.rs`、`tests/fulltext_tests/*`、`benches/fulltext_bench.rs`：删 `default_engine` 初始化、`create_test_index` 的引擎参数、`metadata.engine_type` 断言改为 `metadata.backend`。

### 3.4 linkrs-query

- 新增 `parser/parsing/ddl_parser/index_head.rs`（`CreateIndexHead` + `parse_create_index_head`）。
- `parser/parsing/fulltext_parser.rs`：`73-145` 改用共享头部（调用点 77）；`ENGINE` 子句在 `115-126` 显式拒绝。
- `parser/parsing/vector_parser.rs`：`52-76` 改用共享头部（调用点 56），`IF NOT EXISTS` 移到 `INDEX` 之后。
- `parser/ast/fulltext.rs`：删 `engine_type` 字段（27）、构造参数（203/210）、测试断言（304）。
- `binder/bind/bind_ddl.rs:107`、`binder/bound/stmt_ddl.rs:9,117`：删字段。
- `planning/fulltext_planner.rs:87,161,176,214`：删参数与传递。
- `planning/plan/core/nodes/search/fulltext/management.rs:9,18,29,38`：删字段与构造参数。
- `executor/streaming/plan/arena_builder/specs/ddl/fulltext.rs:17`：删字段。
- `executor/streaming/operators/spec/fulltext.rs:7,16`：删字段。
- `executor/streaming/operators/fulltext_operator.rs`：删 `map_engine_type`（60-64）与 `engine_type_maps_bm25`（925-930），Create 分支不再传引擎。
- 新增 `executor/streaming/operators/index_manage.rs`（`IndexKind` + `label`、`manage_result`、`matches_show_filter`、`unresolved_drop`，含 3 个单测）；`operators.rs` 注册 `pub mod index_manage;`。
- `fulltext_operator.rs`、`vector_operator/manage.rs`：`Drop`/`Show`/`Describe` 改调共享实现。
- `executor/base/traits.rs:137`：fulltext 描述符 `engine` 改填后端。

### 3.5 linkrs-api

- `api_core/fulltext_api.rs`：`create_index` / `create_index_with_engine_config` 去掉引擎参数（53、56、68、77）；`list_index_descriptors` 的 `engine` 改为 `m.backend.to_string()`（128）。
- `api_core/vector_api.rs:231`：`engine` 改为 `self.coordinator.backend().engine_name().to_string()`。

### 3.6 linkrs-sync

- `backend.rs`：删 trait 方法 `is_disabled`（53）与两个实现（333、796）；删 `QdrantEngineHandle.disabled` 字段（752-754）与构造期嗅探（770）；`qdrant` / `qdrant_with_defaults` 返回 `Result`；`from_config_async` 补 `qdrant.enabled` 校验；删 `VectorBackend::is_disabled`（1371-1373）；测试 `disabled_manager_yields_disabled_handle` 改为断言构造被拒绝。
- `metrics_handle.rs:71`：删 `is_disabled` 委托。
- `vector_index_manager.rs`：删 `is_disabled_engine`（328-330）与 10 处守卫；`register_logical_index` 的文档注释去掉 "disabled-engine mode"。
- `vector_sync.rs`：删 `VectorEngineState`（174-178）、`is_disabled_engine`（231-233）、`engine_state`（236-241）、`disabled_skips`（215/247/255）、`disabled_skip_count`（246-248）、`deliver_change_batch` 的 disabled 分支（484-500）。
- `vector_rebuild.rs:142-146`：删守卫。
- `manager/delivery.rs`：删 `ApplyFailure.disabled`（78/86/94/100-108/130）、`handle_delivery_error` 的 disabled 分支（171-177）、两处 `is_disabled_engine()` 条件（963、984）、`disabled_skip_count` 上报（454）。
- `manager/outbox_admin.rs:229`：删诊断赋值。
- `outbox_types.rs:34`：删 `vector_disabled_skips`。
- `sqlite_outbox/frontier.rs:170`：删初始化。
- `lib.rs:96`：删 `VectorEngineState` 导出。

### 3.7 linkrs-metrics / linkrs-server

- `manager/metric_type.rs:342`：删 `VectorDisabledSkips`。
- `manager/search.rs:172-174`：删 `record_vector_disabled_skips`。
- `http/handlers/statistics/query.rs:310-312,437`：删字段。`handlers/sync.rs:190`：删字段。

## 4. 验收标准

每一条都可机械核对：

1. `grep -rn "FulltextEngineType" crates/` 无输出。
2. `grep -rn "engine_type" crates/ --include=*.rs` 仅剩存储引擎字段（`linkrs-core/src/types/space.rs` 的 `EngineType`、`query_context_builder.rs` 的两处初始化），与索引 DDL、fulltext 元数据无关。
3. `grep -rn "is_disabled" crates/linkrs-sync/src` 无输出；`grep -rn "DisabledEngine" crates/` 只剩 `vector-client` crate 内部。
4. `grep -rn "EngineDisabled\|disabled_skips\|VectorEngineState" crates/` 无输出。
5. `CREATE VECTOR INDEX IF NOT EXISTS` 与 `CREATE FULLTEXT INDEX IF NOT EXISTS` 位置一致；`CREATE IF NOT EXISTS VECTOR INDEX` 报语法错误。
6. `CREATE FULLTEXT INDEX ... ENGINE = BM25` 返回明确错误，错误信息说明引擎由配置决定。
7. 元数据文件里 `backend` 字段存在，`engine_type` 不存在。
8. `cargo check` 与 `cargo test` 在 fulltext / vector 两组特性下全绿。

## 5. 验证

### 5.1 执行的命令

```shell
cargo check -p linkrs-core -p linkrs-config
cargo check -p linkrs-fulltext --features fulltext,fulltext-es --tests
cargo check -p linkrs-query --features fulltext,vector
cargo check -p linkrs-sync --features fulltext,vector
cargo check -p linkrs-server --features fulltext,vector,vector-qdrant
cargo test -p linkrs-fulltext --features fulltext
cargo test -p linkrs-sync --features fulltext,vector --lib
cargo test -p linkrs-query --features fulltext,vector --lib
```

### 5.2 结果

| 目标 | 结果 |
| --- | --- |
| `linkrs-core --lib` | 482 passed |
| `linkrs-config --lib` | 63 passed |
| `linkrs-fulltext --features fulltext` | 32 passed（lib）+ 59 passed（`tests/fulltext_tests.rs`） |
| `linkrs-query --features fulltext,vector --lib` | 2329 passed |
| `linkrs-query --test ddl` | 214 passed |
| `linkrs-query --test search` | 3 passed |
| `linkrs-sync --features fulltext,vector --lib` | 53 passed |
| `linkrs-migration` | 67 passed |
| 根 crate `--test integration_streaming_executor` | 26 passed |
| 根 crate `--test sync vector_sync` | 3 passed |
| 根 crate `--test sync`（整套） | 首次 88 passed + 1 failed（`test_vector_index_location_tc229`，命名断言没跟上长度前缀编码），修正后该用例单独复跑通过；未整体复跑整套 |
| sync 集成（`vector_local_backend` / `vector_rebuild` / `vector_outbox_delivery`） | 7 / 10 / 2 passed |
| `linkrs-fulltext --features fulltext,fulltext-es --tests` | 编译通过 |
| `cargo check --workspace --features fulltext,vector,vector-qdrant` | 通过，`unused_dependencies` 警告清零 |

原先记录的预存在失败（`ddl` 2 条、`search` 2 条、`linkrs-migration` 与 `integration_streaming_executor` 的编译错误）已全部修复，根因与修法见第 7 节；这些套件当前无已知失败。

### 5.3 验收标准逐条核对

| # | 核对方式 | 结果 |
| --- | --- | --- |
| 1 | `grep -rn "FulltextEngineType" crates/` | 0 命中 |
| 2 | `grep -rn "engine_type" crates/ --include=*.rs` | 8 命中，全部是存储引擎（`space.rs` 6 处、`query_context_builder.rs` 2 处） |
| 3 | `grep -rn "is_disabled" crates/linkrs-sync/src`；`grep -rln "DisabledEngine" crates/` | 0 命中；`DisabledEngine` 仅存在于 `vector-client`（`engine.rs`、`manager.rs`、`client_impl.rs`），已降为 `pub(crate)` |
| 4 | `grep -rn "EngineDisabled\|disabled_skips\|VectorEngineState" crates/` | 0 命中 |
| 5 | 两侧均调用 `parse_create_index_head`，消费顺序 `INDEX [IF NOT EXISTS] <name> ON <schema>` | `fulltext_parser.rs:77`、`vector_parser.rs:56` 一致 |
| 6 | `test_create_fulltext_engine_clause_is_rejected` | 通过，错误文案为 "the ENGINE clause is not accepted; the fulltext backend is selected by server configuration" |
| 7 | `IndexMetadata` 定义 | 字段为 `backend: FulltextBackendKind`（`#[serde(default)]`），无 `engine_type`；序列化为 `"backend": "local"/"es"` |
| 8 | 见 5.2 | 通过 |

### 5.4 改动规模

```text
101 files changed, 2250 insertions(+), 1195 deletions(-)
```

净增 1055 行，其中约 700 行是本目录的三份文档。代码净减约 200 行。新增代码文件两个：`crates/linkrs-query/src/parser/parsing/ddl_parser/index_head.rs`、`crates/linkrs-query/src/executor/streaming/operators/index_manage.rs`。

## 6. 迁移影响

- **配置**：`[fulltext] default_engine` 必须删除，否则 `deny_unknown_fields` 拒绝启动。这是刻意的：留着一个不生效的配置项比报错更糟。
- **SQL**：`CREATE VECTOR INDEX` 的 `IF NOT EXISTS` 位置变化；`CREATE FULLTEXT INDEX ... ENGINE = BM25` 从接受变为拒绝。
- **元数据**：`IndexMetadata.engine_type` 字段消失。旧 `fulltext_metadata.json` 里的该字段会被 `serde` 忽略，`backend` 走默认 `Local`——与旧数据实际来源一致，因此不需要重建。
- **HTTP**：`/statistics/query` 的 `vector.disabled_skips` 与 `/sync` 诊断的 `vector_disabled_skips` 消失。
- **SQL 行为**：`SHOW FULLTEXT INDEX` 现在按当前空间过滤，`FROM <schema>` 只过滤 tag（此前被当成空间名，导致不带 `FROM` 时恒为空）；`DESCRIBE ... INDEX` 的 not-found 行由 `(action, name, status)` 改为与索引卡片同宽、缺失列填 `NULL`。
- **内部 API**：`QueryPipelineManager` 新增 `with_fulltext_coordinator`；`FulltextOperator::from_spec` 增加 manager 参数；`FulltextManageCommand::Show` 与 `ShowFulltextIndexNode` 增加 `space_id`。

## 7. 预存在缺陷修复

验证过程中暴露出 5 个与本方案同源（"同一事实存多处"）的既有缺陷，以及 6 条未使用依赖警告，一并修掉。它们的共同点是：**同一份事实在两处各存一份，且两处已经不一致**。

### 7.1 SEARCH 用错空间 ID

`arena_builder/specs/ddl/fulltext.rs` 的 `build_fulltext_search_spec`（`:60`）与 `build_fulltext_lookup_spec`（`:76`）取 `exec_ctx.current_space_id().unwrap_or(0)`，而规划阶段已经从 schema 元数据解析出索引位置并写在节点上（`FulltextSearchNode.space_id`）。

两条路径在同一语句里给出不同答案：`CREATE FULLTEXT INDEX` 用 `qctx.space_id()` 得到 1，`SEARCH INDEX` 的 spec 退回 0，于是查询打到 `Index not found: 0.article.content`。改为直接用 `node.space_id`——规划期已验证过的那份。

`MATCH FULLTEXT` 的 spec 本来就用 `node.space_id`，所以只有 `SEARCH` / `LOOKUP` 两条受影响。

### 7.2 管理语句的输出布局只有一种

`arena_builder/metadata.rs:835-851` 给全部 `FulltextManage` / `VectorManage` 声明 `(action, name, status)` 三列，但 `SHOW` 实际吐 5 列（目录列表）、`DESCRIBE` 吐 4 列（索引卡片）。结果为 `SHOW FULLTEXT INDEX` 返回 `Chunk has 5 columns, expected 3`。

修复方式是把形状变成显式概念：`index_manage.rs` 新增 `ManageShape { Status, Show, Describe }` 与 `layout_of(kind, shape)`，`metadata.rs` 按命令变体选取，操作符的 `SHOW` / `DESCRIBE` 列名也从同一处派生。`DESCRIBE` 的 not-found 行原本是三列，改为与卡片同宽（缺失字段填 `Null`），否则同一语句的两种结果宽度不同。

### 7.3 `SHOW FULLTEXT INDEX` 恒为空

`transform_show`（`fulltext_planner.rs:240`）在用户没写 `FROM <schema>` 时把 `space_name` 塞进 `from_schema`，而 `matches_show_filter` 拿 `from_schema` 比的是 **tag 名**。空间名永远不等于 tag 名，于是默认的 `SHOW FULLTEXT INDEX` 一条也返回不了。

修复：`FROM <schema>` 只表示 tag 过滤（保持原样传 `None`）；空间域另开一维——`ShowFulltextIndexNode` 与 `FulltextManageCommand::Show` 增加 `space_id`，操作符按 `metadata.space_id == space_id` 过滤。

**未同步处理**：`SHOW VECTOR INDEX` 仍是全空间列表（`ShowVectorIndexNode` 不带 `space_id`）。两者是同一类缺陷，但 vector 侧需要一并改节点、命令与 coordinator 过滤，且当前没有任何测试覆盖到该路径，留待单独处理时与 vector 管理语句的其他收尾一起做。

### 7.4 `FULLTEXT SEARCH` 强依赖 sync 协调器

`FulltextOperator::from_spec` 只从 `fulltext_coordinator` 取管理器，导致管理语句（`CREATE` / `DROP` / `SHOW` / `DESCRIBE` / `ALTER`）在没有 sync 层时也无法执行——它们根本不需要协调器，只需要管理器。

修复：`from_spec`（`fulltext_operator.rs:261`）增加 `fulltext_manager` 参数（调用点 `materializer.rs:324`），管理语句优先用它、缺失时回落到协调器托管的那份；`pipeline` 增加 `with_fulltext_coordinator`（`pipeline.rs:576`，与既有的 `with_vector_coordinator` 对称），执行上下文（`execution.rs:454`）按"显式注入优先，其次 sync manager"解析协调器。

### 7.5 测试脚手架与测试本身的错误

- `linkrs-migration/src/executor/tests.rs` 的 `TestStorage` 缺 `count_visible_vertices` / `count_visible_edges`，`cargo test` 编译失败（`StorageReader` 早已加了这两个方法）。按 trait 语义补齐：前者按 space/tag 聚合，后者按 space/edge_type 计数。
- `tests/integration_streaming_executor.rs` 的 `UnaryOperatorKind::Project` 缺 `emit_rowless` / `identity_as_unit`、`BlockingOperatorKind::Aggregate` 缺 `aggregate_filters`。常量投影取 `false` / `false`，单聚合函数取 `vec![None]`。
- `tests/ddl/contract_gaps.rs` 的 `test_create_vector_invalid_hnsw_rejected` 用 `hnsw_m=1` 当"非法参数"，但引擎校验的是 `ef_construct >= max(2*m, 4)`（`m` 本身无上下界）。改成 `hnsw_m=16, hnsw_ef_construct=4`，仍测"非法 HNSW 参数必须被拒"这条契约。
- 同文件的 `compression=x16 + quantization=SCALAR` 用例原本也过不去，因为校验写在 `build_collection_config` 里、而调用点在"有没有 coordinator"分支内。把校验提到分支外：**语句是否合法不依赖运行期装配**，无引擎时也应报参数错误而不是 "no vector coordinator is configured"。
- `tests/search/fulltext_contract.rs` 的两条用例：需要真实 fulltext 引擎，改用 `TestScenario::with_fulltext_indexes()`（临时目录 + Tantivy 管理器 + 协调器）；`INSERT VERTEX article(title, content) VALUES 1:('a', ...)` 违反"首列必须镜像顶点 ID"，改为 `1:('1', ...)`。

### 7.6 未使用依赖

| crate | 依赖 | 处理 |
| --- | --- | --- |
| `linkrs-core` | `uuid`、`zstd` | 删除（crate 内的 `value::uuid` 是自有模块，`wal/types.rs` 的 `zstd` 是自有意压缩枚举） |
| `simvec` | `tokio-stream` | 删除 |
| `vector-client` | `anyhow` | 删除 |
| `linkrs-client` | `async-trait`、`futures` | 删除 |
| `linkrs-client` | `linkrs-core` | 移到 `[dev-dependencies]`（只有 `#[cfg(test)] mod tests` 里的 `DataType` 对拍用到） |
| 工作区 | `once_cell`、`prost-build` | 删除 `[workspace.dependencies]` 中的定义（子 crate 用的是自己的版本） |

`cargo check --workspace` 的 `unused_dependencies` 警告已清零。

## 8. 未纳入本次

- ~~`VectorEngineKind` 与 `FulltextBackendKind` 合并为一个 `SearchBackendKind`~~：已决策**不合并**。fulltext 不会跑在 Qdrant 上，两者值域本就不同（`Local`/`Qdrant` 与 `Local`/`Es`），合并只会换来一个需要 match 两次的宽枚举。
- `CreateFulltextIndex` / `CreateVectorIndex` 的 AST 与 bound 类型合并：理由见 2.3 末段。
- vector 侧的 `EmbeddedVectorEngine` 与 `linkrs_config::VectorEngineKind` 两套引擎枚举：`linkrs-api/src/embedded/database.rs` 独立定义，与 `startup.rs` 重复，属于嵌入 API 的配置面收敛，留待单独处理。
- `SHOW VECTOR INDEX` 的空间域（见 7.3 末段）。
