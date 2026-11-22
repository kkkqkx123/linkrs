# 索引管理空间域收尾与 Vector 装配面统一方案

> 基线：`68891eea update sync and storage backend 1`（已推送的远程 HEAD）。
> 本方案是 `index-engine-semantics-cleanup.md` 第 8 节"未纳入本次"三项的落地设计，并覆盖落地过程中暴露的同类缺陷。

## 0. 目标与约束

上一轮把引擎语义、索引 DDL、可用性判据收敛到"一处事实、构造期决定"，但刻意留下三项未做：

1. `SHOW VECTOR INDEX` 的空间域（与已修的 `SHOW FULLTEXT INDEX` 同类）。
2. vector 侧的 `EmbeddedVectorEngine` 与 `linkrs_config::VectorEngineKind` 两套引擎枚举，嵌入 API 配置面重复。
3. `SHOW VECTOR INDEX` 需要连带改节点、命令、coordinator 过滤，且无测试覆盖。

本轮把这三项做完，并在过程中发现第三类根因——**同一份"装配逻辑"在三个入口各写了一遍，参数与引擎选择已经漂移**：

- server 启动 `startup.rs`、服务工厂 `factory.rs`、嵌入数据库 `database.rs` 各自把 `[vector]` 配置变成引擎，三处的目录默认值、启停判据、Qdrant 前置校验各不相同。
- `QueryApi` 与 `VectorApi` 各自创建一个 `VectorSyncCoordinator`，于是 SQL 路径和 HTTP 路径持有两个索引注册表，`CREATE VECTOR INDEX` 与 `SHOW` 看到的是不同 catalog。
- `MATCH VECTOR` / `SEARCH VECTOR` / `DROP VECTOR INDEX` 在规划期不写空间，执行期退回 `space_id=0`，查一个不存在的集合；未知索引还被误报成"位置未解析"而非"索引不存在"。

约束（来自 `AGENTS.md`）：开发阶段不做向后兼容；代码、注释、日志英文，文档中文；代码注释不得引用计划编号；子 crate 严格 DAG。

## 1. 问题清单（每条附 file:line 证据，行号基于基线 `68891eea`）

### 1.1 vector 管理语句缺空间域

- `planning/plan/core/nodes/search/vector/management.rs` 的 `ShowVectorIndexNode` / `DescribeVectorIndexNode` 基线只有 `pattern` / `from_schema` / `index_name`，无 `space_id`。对照 fulltext 侧已修的同名节点（`fulltext/management.rs`）带 `space_id`。
- `planning/vector_ddl_planner.rs:transform_show_vector_index` 基线把 `space_name` 塞进 `from_schema`，而过滤器拿 `from_schema` 比 **tag 名**——空间名永远不等于 tag 名，默认 `SHOW VECTOR INDEX` 恒返回空。这与上一轮 fulltext 侧的 7.3 是完全相同的缺陷。
- `executor/streaming/operators/vector_operator/manage.rs` 的 `Show` / `Describe` 分支对 catalog 全量遍历，没有空间过滤。
- fulltext 侧 `Drop` / `Alter` / `Describe` 节点同样不带 `space_id`（上一轮只给 `Show` 补了），跨空间同名索引会被误操作。

### 1.2 引擎枚举与装配入口重复

- `linkrs-api/src/embedded/config.rs` 基线自定义 `EmbeddedVectorEngine::{Local, Qdrant(config)}`，与 `linkrs_config::VectorEngineKind::{Local, Qdrant}` 值域相同，是第二套枚举。
- `VectorBackend` 装配在三个入口各写一遍：
  - `linkrs-server/src/startup.rs`：`VectorManager::new` + `qdrant_with_defaults`，目录取 `config.vector_data_dir()`；
  - `linkrs-server/src/graph_service/factory.rs`：`assemble_local_engine` + `from_local_arc_with_limit`，且无 error channel 时 panic；
  - `linkrs-api/src/embedded/database.rs`：`create_vector_manager` + `qdrant` 分支。
- 三处的 Qdrant 前置校验、目录默认值、`default_max_points` 传递、错误分类都不同；新增一个引擎开关就要改三处。

### 1.3 两个协调器导致 catalog 分裂

- `linkrs-api/src/api_core/query_api_builder.rs` 的 `QueryApi::with_vector_backend` 内部新建 `VectorSyncCoordinator`；
- `linkrs-server/src/graph_service/factory.rs` 的 `VectorApi::new` 又从 `storage.get_sync_manager()` 另取一个协调器。
- 两者不保证同一实例：SQL 语句建的索引和 HTTP `SHOW` 列的索引可能来自两个独立注册表。

### 1.4 规划期不携带空间，未知索引误报

- `planning/vector_planner.rs:transform_match_vector` 基线 `_space_id` 参数不使用，`VectorMatchNode` 无 location，执行期报"index location (tag/field) is not resolved"。`MATCH VECTOR` 的 pattern 里带标签 `(n:item)`，字段在 `vector=` 条件里，规划期完全能解析出 `(tag, field)`。
- `transform_search_vector` 对未知索引名不报错，把 `(tag, field)` 留空交给执行期，于是"索引不存在"被报成"位置未解析"。
- `vector_ddl_planner.rs:transform_drop_vector_index` 的 `IF EXISTS` 分支对未知名静默退化，但非 `IF EXISTS` 的未知名同样走到执行期才失败。

### 1.5 测试脚手架未跟上基线

- `tests/integration_embedded_api.rs` 的 `ExecutionMetadata` 字面量缺 `compile_time_us` / `execute_time_us` 字段（基线编译错误）。
- `tests/vector_local_startup_e2e.rs` / `tests/vector_query_e2e.rs` 的 `group_id` 用裸 `item_vec`，而 coordinator 用长度前缀 `4_item_3_vec` 过滤（上一轮命名修复的连带）。

## 2. 设计

### 2.1 空间域：一个 `ShowFilter`，两侧共用

把"目录条目是否属于本次 `SHOW`"收敛成一个显式结构体（`executor/streaming/operators/index_manage.rs:125`）：

```rust
pub(crate) struct ShowFilter<'a> {
    pattern: Option<&'a str>,   // LIKE 子串匹配语句级索引名
    schema: Option<&'a str>,    // FROM <schema> 比 tag 名
    space_id: u64,              // 空间域，独立于 schema
}
impl<'a> ShowFilter<'a> {
    pub(crate) fn new(pattern: Option<&'a str>, schema: Option<&'a str>, space_id: u64) -> Self;
    pub(crate) fn matches(&self, index_name: Option<&str>, tag_name: &str, space_id: u64) -> bool;
}
```

关键点：`FROM <schema>` 与 `space_id` 是**两个正交维度**。基线把空间名塞进 `from_schema` 是把它们混成一维，导致默认 `SHOW` 恒空。规划器侧（`vector_ddl_planner.rs:240`、`fulltext_planner.rs:249`）不再替换，直接透传 `show.from_schema` 并另传 `space_id`。

两侧节点、spec 命令、operator 都带 `space_id`：

| 层 | vector | fulltext |
| --- | --- | --- |
| 计划节点 | `ShowVectorIndexNode` / `DescribeVectorIndexNode` 加 `space_id` | `Show` / `Drop` / `Alter` / `Describe` 四个节点加 `space_id` |
| spec 命令 | `spec/vector.rs:26,41,49,55,68,90,111` 相关变体带 `space_id` | `spec/fulltext.rs:18,25,30,35,39,62,73,84` 全部管理变体带 `space_id` |
| operator | `vector_operator/manage.rs` 的 Show/Describe 按 `space_id` 过滤 | `fulltext_operator.rs` 的 Drop/Alter/Describe 按 `space_id` 定位 |

`Describe` 的语义：索引名只在单个空间内唯一，跨空间同名不算命中。查找时同时比对 `index_name` 与 `space_id`。

### 2.2 装配：一个 `VectorSelection`，一个 `from_config`

配置层新增判据（`linkrs-config/src/vector.rs:529,633`）：

```rust
pub enum VectorSelection { Off, Local, #[cfg(feature="vector-qdrant")] Qdrant }

impl VectorConfig {
    pub fn selection(&self) -> Result<VectorSelection, String>;
}
```

`selection()` 把"主开关 + 引擎 + 子引擎 enabled"三件事合成一次判定。`engine = "qdrant"` 但 `[vector.qdrant].enabled = false` 是**矛盾配置**（选了引擎又关掉它唯一的参数块），报 `Err` 而不是静默 `Off`。`Config` 的三个谓词 `is_vector_enabled` / `is_local_vector` / `vector_engine` 改为代理 `selection()`，不再各自 match 一遍枚举。

装配层把 `VectorBackend::from_config` 改成唯一的异步入口（`linkrs-sync/src/backend.rs:1382`）：

```rust
pub async fn from_config(&config: VectorConfig) -> VectorCoordinatorResult<Option<Self>> {
    match config.selection().map_err(ConfigError)? {
        Off => Ok(None),
        Local => { /* data_dir 由调用方解析；assemble_local_engine + from_local_arc_with_limit */ }
        #[cfg(feature="vector-qdrant")]
        Qdrant => { /* VectorManager::new + qdrant_with_defaults */ }
    }
}
```

`Ok(None)` 只表示"明确关闭"；矛盾配置与构造失败都是 `Err`。`data_dir` 必须由调用方解析后写入配置——`<storage_path>/vector`（server）还是数据库文件旁（embedded）是部署决策，不属于统一装配。

三个入口改为调用它：

| 入口 | 改动 |
| --- | --- |
| `startup.rs:147-180` | 解析 `data_dir` 后 `from_config(...).await?`，`Ok(None)` 表示不开；错误是环境问题，abort 启动 |
| `factory.rs:226` | 无 error channel，`from_config(...).await` 失败则 panic（与基线一致的 fail-fast） |
| `embedded/database.rs:38` | `to_engine_config()` + `runtime.block_on(from_config(...))` |

`EmbeddedVectorEngine` 删除（`embedded/config.rs`），embedded 直接用 `VectorEngineKind`：`with_vector_qdrant(config: QdrantConfig)` 强制 `config.enabled = true`（选引擎即启用，embedded 没有独立的 `[vector.qdrant]` 配置面可残留矛盾）。

### 2.3 单协调器：QueryApi 与 VectorApi 共享

`QueryApi::with_vector_backend` 改为 `QueryApi::with_vector_coordinator`（`query_api_builder.rs:221`）：接收外部传入的 `Arc<VectorSyncCoordinator>`，同步构造（不再 internally new）。

`factory.rs:240-279` 建一个 `coordinator`，同时传给 `QueryApi::with_vector_coordinator` 与 `VectorApi::new`；`VectorApi` 需要的 `SyncManager` 优先复用 `storage.get_sync_manager()` 中已含 vector coordinator 的那个，否则用 `SyncManager::new_without_fulltext().with_vector_coordinator(...)` 临时装一个。这样直接构造的测试服务也有完整的 catalog API，不会静默缺一路。

### 2.4 规划期解析位置与错误分类

- `MATCH VECTOR`（`vector_planner.rs:195,795`）：新增 `pattern_label(pattern)` 从 `(n:item)` 提取标签，配合 `vector=` 里的字段，在规划期 `.with_metadata(space_id, tag, field)`；无标签的 pattern 返回空 tag，由执行期报"位置未解析"而不是猜。
- `SEARCH VECTOR`（`vector_planner.rs:104`）：有 schema 元数据时未知索引名直接 `PlannerError::IndexNotFound`；无元数据时才把空位置留给执行期（保留无 schema 场景的诊断）。
- `DROP VECTOR INDEX`（`vector_ddl_planner.rs:204`）：有元数据且未知名、且未写 `IF EXISTS` 时规划期报 `IndexNotFound`；`IF EXISTS` 仍退化为 `not_exists` 状态行。

错误分类原则：**语句在给定 schema 下引用不存在的索引，是规划期错误，不是运行期"位置未解析"**。后者只保留给确实无法静态解析的场景（无元数据的裸 `MATCH`、无标签 pattern）。

### 2.5 测试脚手架修正

- `ExecutionMetadata` 两处字面量补 `compile_time_us: 0` / `execute_time_us: 0`。
- e2e 的 `group_id` 改成长度前缀 `4_item_3_vec`（与 coordinator 的 `group_id_with` 一致）。
- 新增空间域回归测试：`vector_query_e2e.rs` 的 `vector_show_index_is_scoped_to_the_current_space`（两个空间各建索引，断言 `SHOW` 只列当前空间、`DESCRIBE` 跨空间名不命中）；`fulltext_contract.rs` 的 `test_fulltext_management_is_scoped_to_the_current_space`（fulltext 的 Show/Describe/Drop 同样验证）。
- 新增 `pattern_label` 单元测试。

## 3. 代码变更清单（27 files, +691/−373）

按关注点分组，行号为改动后位置：

| 关注点 | 文件 |
| --- | --- |
| 空间域核心 | `operators/index_manage.rs:99,125,132,141,159`；`spec/{vector,fulltext}.rs`；`planning/{vector_ddl_planner.rs:204,240,255,vector_planner.rs:104,195,795,fulltext_planner.rs:218,227,249,263}`；`planning/plan/core/nodes/search/{vector,fulltext}/management.rs`；`arena_builder/specs/ddl/{vector,fulltext}.rs` |
| operator | `operators/fulltext_operator.rs`；`operators/vector_operator/manage.rs` |
| 装配判据 | `config/vector.rs:529,633`；`config/lib.rs:582`（三个谓词代理 selection） |
| 装配入口 | `sync/backend.rs:1382`；`server/startup.rs:147,154,174,180`；`server/graph_service/factory.rs:226,240,246,275,276,279`；`api/embedded/config.rs:71,106,138,268`；`api/embedded/database.rs:38` |
| 单协调器 | `api/api_core/query_api_builder.rs:221`；`server/graph_service/factory.rs` |
| 测试修正 | `tests/vector_local_startup_e2e.rs`；`tests/vector_query_e2e.rs`；`crates/linkrs-query/tests/{factorization_schema.rs,search/fulltext_contract.rs}`；`tests/integration_embedded_api.rs`；`linkrs-client/Cargo.toml`（恢复被下游使用的 `futures`） |

## 4. 验收标准（grep 可核对）

1. `grep -rn "EmbeddedVectorEngine" crates/` → 0 命中。
2. `grep -rn "is_local_vector\|vector_engine()" crates/linkrs-config/src/lib.rs` → 仅谓词定义与测试断言，无残留 `match self.vector.engine`。
3. `grep -rn "group_id.*item_vec\|\"item_vec\"" tests/` → 0 命中裸值。
4. `grep -rn "VectorBackend::from_config" crates/` → 三入口（startup / factory / embedded）均调用统一入口，无独立的 `VectorManager::new` 装配。
5. `grep -rn "with_vector_backend" crates/` → 0 命中；`with_vector_coordinator` 至少在 builder 与 factory 各一处。
6. `grep -rn "transform_show_vector_index" ...` → 函数签名收 `space_id: u64`，不再出现 `space_name.to_string()` 塞进 `from_schema`。
7. `ShowFilter` 为唯一的 SHOW 过滤实现（`matches_show_filter` 自由函数 0 命中）。
8. 新增两个空间域回归测试 + `pattern_label` 两个单测通过。

## 5. 验证

验证命令与结果（基于基线 `68891eea`，本轮改动后）：

| 目标 | 结果 |
| --- | --- |
| `cargo check --workspace --features linkrs/fulltext,linkrs/vector,linkrs/vector-qdrant` | 通过（仅剩基线既有的 cfg 组合 unused 警告） |
| `linkrs-core --lib` | 482 passed |
| `linkrs-config --lib`（默认特性） | 63 passed |
| `linkrs-config --features vector,vector-qdrant,server --lib` | 100 passed + 1 failed（见下） |
| `linkrs-fulltext --features fulltext` | 32 + 59 passed |
| `linkrs-query --lib / --test ddl / --test search` | 2332 / 214 / 4 passed |
| `linkrs-sync --lib` | 53 passed |
| 根 crate `--test sync vector_sync` | 3 passed |
| 根 crate `--test vector_local_startup_e2e` | 2 passed |
| 根 crate `--test vector_query_e2e` | 9 passed |
| 根 crate `--test integration_embedded_api`（embedded,vector） | 72 passed + 1 failed（见下） |

（详细的失败归因与基线复跑结论见第 9 节。）

## 6. 迁移影响

- 配置面：`EmbeddedVectorEngine` 是 Rust API 类型，非 TOML 字段，删除只影响使用 `DatabaseConfig` 的 Rust 调用方；`with_vector_qdrant` 参数由 `VectorClientConfig` 改为 `QdrantConfig`，需同步调用方。
- SQL 行为变化（均为修 bug）：默认 `SHOW VECTOR INDEX` 从"恒空"变为列出当前空间索引；`SHOW/DESCRIBE/DROP/ALTER` 跨空间名不再命中；未知索引从"位置未解析"变为 `IndexNotFound`。
- 装配行为：`engine = "qdrant"` 但 `qdrant.enabled = false` 从"静默关闭"变为启动期 `Err`。开发阶段不做兼容，这是刻意的。
- 无持久化格式变化，无需数据迁移。

## 7. 预存在缺陷修复

本轮落地时补齐的基线缺陷（与方案同源）：

- `tests/integration_embedded_api.rs` 的 `ExecutionMetadata` 缺字段（基线编译错误）。
- e2e `group_id` 裸值与长度前缀命名不一致（上一轮命名的连带）。
- `linkrs-client/Cargo.toml` 的 `futures`：上一轮按 unused 删除，但本基线 `http_client.rs` 的 SSE 进度流用到 `StreamExt`，恢复声明（带注释说明用途）。

## 8. 未纳入本次

- fulltext 与 vector 管理语句的 AST/bound 类型合并：重复的是逻辑不是数据，合并要给数十个构造点加一层 match，收益不足。
- `VectorEngineKind` 与 `FulltextBackendKind` 合并：上一轮已决策不合并，理由不变。
- baseline 既有的 cfg 组合 unused 警告（`CollectionDefaults` / `VectorCoordinatorError` / `ClassifySearchError` / `linkrs-cli` 的 `async-trait`）：本轮未触及，不扩大范围。

## 9. 验证结论与失败归因

本轮所有目标特性组合编译通过，22 个测试目标中 20 个全绿、2 个各有 1 条失败（均**在基线复现**，与本轮改动无关）。下表记录归因及后续修复状态：

| 失败用例 | 复跑方式 | 结论 |
| --- | --- | --- |
| `linkrs-config --features vector,vector-qdrant,server --lib` 的 `test_doc_prod_example_config_loads_and_validates` | 基线 `68891eea` 报 `unknown field 'embedding'`：`QdrantConfig` 带 `deny_unknown_fields`（`vector.rs:487`），`embedding` 字段受 `#[cfg(feature = "embedding")]` 门控（`vector.rs:499`），而测试配置含该段（原 `lib.rs:1552`，对应文档生产示例 `07_configuration_reference.md:928`） | **已修复**：测试配置的 embedding 段改为按 feature 条件拼入（`lib.rs:1561-1571`），两种 feature 组合均运行并各自验证匹配的配置面 |
| `integration_embedded_api` 的 `test_session_text_transaction_commands` | 基线该文件因缺 `ExecutionMetadata::compile_time_us/execute_time_us` 无法编译；仅补编译字段后复跑，仍在 `ROLLBACK TO must undo post-savepoint writes only` 断言失败 | 基线同样失败，预存在；**已在后续独立方案修复**，见 `transaction-count-plan-cache-staged-visibility.md`（根因是计划缓存跨事务复用元数据计数计划，非 savepoint 回滚） |

`linkrs-config` 全特性测试本轮因恢复了 `is_local_vector` / `vector_engine`（代理 `selection()`）而可编译，结果 100 passed + 1 上述预存在失败；该失败修复后全绿。`integration_embedded_api` 本轮补齐编译字段后可编译，结果 72 passed + 1 上述预存在失败；该失败在独立方案中修复后为 74 passed / 0 failed。

改动规模：**27 files, +691 / −373**（净增 318 行），无新增文件。基于新基线 `68891eea` 生成 patch。

## 10. 待拍板开放点

1. `SHOW VECTOR INDEX` 的 `FROM <schema>` 是否应支持多 schema / 通配符：当前保持单 tag 精确匹配（与 fulltext 一致）。
2. `MATCH VECTOR` 的多标签 pattern（如 `(a:item)-[:rel]->(b:doc)`）当前只取第一个标签；多标签索引定位需在语法层明确语义后再做。
3. embedded 的 `with_vector_qdrant` 强制 `enabled = true`：是否需要在 embedded API 保留"配置了但不启用"的中间态——当前判定不需要。
