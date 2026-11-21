# fulltext 与 vector 后端集成方式对比分析

本文对比 `linkrs` 中两条二级索引链路（全文检索 fulltext、向量检索 vector）的集成方式，逐层指出差异、评估现有设计的合理性，并给出收敛建议。所有结论均附 `file:line` 证据，未核验项会显式标注。

## 0. 结论摘要

1. **两者共享同一套同步骨架**（WAL → SQLite outbox → claim/apply/ack、generation 级重建、publish fence、temp + 原子发布），差异集中在**上层**：引擎抽象放在哪个 crate、索引生命周期由谁持有、后端选择在何处发生。
2. **fulltext 是"引擎即管理器"**：`FulltextIndexManager` 与 `TantivySearchEngine` 同处 `linkrs-fulltext`，管理器直接 `match` 全局配置选后端（`crates/linkrs-fulltext/src/manager.rs:95-117`）。**vector 是"引擎即适配器"**：本地引擎 `simvec`、远端客户端 `vector-client` 各自独立，`linkrs-sync` 用 `VectorEngineHandle` 把两者抹平（`crates/linkrs-sync/src/backend.rs:40`），索引生命周期写在 `VectorIndexManager`（`crates/linkrs-sync/src/vector_index_manager.rs:30`）。
3. **这不是对称设计，而是两次独立演化的结果**：`vector-client` 的 trait 注释自己写明"本地引擎在 `simvec` 里暴露自己的接口，同步层通过 backend 枚举分派"（`crates/vector-client/src/engine.rs:154-159`），即远端先落地、本地后接入，靠适配层补齐；fulltext 的 ES 后端则反过来，直接实现既有 trait "drops into the existing delivery path without changes to the sync crate"（`crates/linkrs-fulltext/src/es.rs:5-7`）。
4. **能力不对等**：ES 后端不支持在线重建（`manager.rs:980-986` 显式 `NotSupported`），Qdrant 支持（logical remap）；fulltext 支持边索引（边类型与顶点 tag 共享命名空间，`manager.rs:401-425`），vector 只支持顶点（连边 payload 都映射为 0 个 point，`crates/linkrs-sync/src/manager/delivery.rs:731-735`）。
5. **存在 4 个实证缺陷**，其中两个会造成数据错乱或恢复失效：物理命名用下划线裸拼接（vector Space 粒度的 `group_id` 会让不同逻辑索引静默串数据）、vector 逻辑索引注册表只存内存（重启后列表与重建恢复双双失效）、fulltext 启动发现路径硬编码 Tantivy（切到 ES 后端后会出现混合后端）、fulltext 边文档 upsert 与 delete 的 ID 契约不一致（`fulltext_rebuild.rs:114-123` 代码注释自述会残留陈旧文档）。
6. **合理性判断**：同步/重建骨架、publish fence、capability 查询（`is_in_process` / `supports_atomic_promote` / `owns_space_slice_reclaim`）的设计是好的、值得保留；不合理的是**选择点分散**（同一决策在 3 处实现）与**元数据/一致性落盘方式各写一套**。

## 1. 前置与规范

- 仓库：`https://github.com/kkkqkx123/linkrs`，HEAD `9f04e48f`（`update sync and add fulltext-es`）。经 `gh-proxy.com` 递归拉取。
- 工具链：Rust `1.93.0` → `1.100.0`（rsproxy dist），cargo 走 `sparse+https://rsproxy.cn/index/`，npm 走 `https://registry.npmmirror.com`。
- 遵循 `AGENTS.md`：代码/注释/日志用英文，文档用中文；文档不引用计划编号；未做任何代码改动（纯分析）。

## 2. 分层视角

| 层 | fulltext | vector |
| --- | --- | --- |
| 配置 | `[fulltext]`：`backend = local \| es`（`crates/linkrs-config/src/fulltext.rs:30`）+ `[fulltext.es]`（永远可解析） | `[vector]`：`engine = local \| qdrant`（`crates/linkrs-config/src/vector.rs:20`，**Qdrant 变体被 feature 门控**）+ `[vector.qdrant]`（同样被门控） |
| 引擎实现 | `TantivySearchEngine`（`tantivy_index.rs`）、`EsSearchEngine`（`es.rs`）都在 `linkrs-fulltext` | `simvec::LocalVectorEngine`（独立 crate）、`vector-client` 的 HTTP/gRPC 引擎（独立 crate） |
| 引擎抽象 | `FulltextSearchEngine`，本地+远端实现**同一 trait** | `RemoteVectorEngine`（远端专用）+ `VectorEngineHandle`（sync 层的双后端适配器） |
| 索引生命周期 | `FulltextIndexManager`（引擎 crate 内），元数据落 `fulltext_metadata.json` + 每个索引目录一个 `fulltext_key.json` sidecar | `VectorIndexManager`（`linkrs-sync`），逻辑索引注册表**仅内存** |
| 同步协调 | `FulltextSyncCoordinator` 只是 `Arc<FulltextIndexManager>` 的壳（`crates/linkrs-sync/src/fulltext_sync.rs:6-9`，共 76 行） | `VectorSyncCoordinator` 包 `VectorIndexManager`，另有 1848 行 `backend.rs` + 1369 行 `vector_index_manager.rs` + 994 行 `vector_rebuild.rs` |
| 查询接入 | `FulltextProvider(Arc<FulltextIndexManager>)`（`crates/linkrs-query/src/executor/base/traits.rs:84`） | `VectorProvider(Arc<VectorSyncCoordinator>)`（`traits.rs:158`） |
| 对外 API | HTTP：search（`http/handlers/fulltext.rs:52`）、list/rebuild/clear/inconsistent（`http/handlers/rebuild.rs:425/162/315/377`）；**无 create/drop/info**（只能走 GQL DDL） | HTTP 11 个 handler：create/drop/info/list/search/get/count/payload×3/scroll（`http/handlers/vector.rs`）；gRPC 14 个（`grpc/vector.rs`） vs fulltext 5 个（`grpc/fulltext.rs`） |
| 规模 | `linkrs-fulltext` 约 7.2k 行；sync 侧 fulltext 735 行 | `vector-client` 约 12.6k 行；sync 侧 vector 4.7k 行 |

## 3. fulltext 的集成方式

1. **单 trait + 全局后端**。`FulltextSearchEngine` 定义了 index/search/commit/commit_with_payload/consistency_state/clear/close 等（`crates/linkrs-fulltext/src/engine.rs:21-115`）。管理器在创建每个逻辑索引时按 `config.backend` 造具体引擎：Local → Tantivy，Es → `EsSearchEngine`（`manager.rs:95-117`），feature 未编译则报 `ConfigError`（`manager.rs:109-115`）。
2. **物理布局：一个 `(space, schema, field)` 一个 Tantivy 目录**，目录名 = `space_ft_{space}_{tag}_{field}`（`crates/linkrs-fulltext/src/metadata.rs:60-65`），落在 `get_space_storage_path()` 解析出的路径下（`manager.rs:427-456`，支持 Space 自定义目录 / Directory 隔离）。
3. **元数据持久化是"文件 + sidecar + 磁盘发现"三件套**：`fulltext_metadata.json` 原子写（`manager.rs:348-367`），每个索引目录再写 `fulltext_key.json` 防名字歧义（`manager.rs:291-313`，注释直言 `space_ft_...` 形式在名字含 `_` 时歧义），启动时先读 JSON、读不到再扫盘（`manager.rs:156-212`）。
4. **一致性 fence 藏在引擎里**：`FulltextReceiver` 从 `commit_payload()` 恢复 `applied_lsn + receipts`（`crates/linkrs-sync/src/receiver.rs:82-98`），应用完一批后把新状态序列化回 `commit_with_payload`（`receiver.rs:183-190`）。Tantivy 侧存在 segment meta 的 `payload` 字段（`tantivy_index.rs:473-475`），ES 侧存在一个保留的 fence 文档（`es.rs:18-25`）。
5. **重建只支持本地**：`create_rebuild_engine` 一开始就拦 ES（`manager.rs:980-986`），因为发布依赖"临时目录 rename 成 live"（`manager.rs:1053-1128`）；ES 的一致性靠 outbox 增量维持。
6. **重建立即换引擎 + publish fence**：每索引一把 `tokio::RwLock`，投递持读锁、发布持写锁（`manager.rs:37-41`、`manager.rs:898-910`）。
7. **边与顶点共享索引命名空间**：`validate_schema_exists` 同时接受 tag 与 edge type（`manager.rs:401-425`），`index_edge_property` 直接写同一 `(space, edge_type, field)`（`manager.rs:763-777`）。

## 4. vector 的集成方式

1. **两个 crate + 一层适配**。`vector-client` 只定义远端能力（`RemoteVectorEngine`，`crates/vector-client/src/engine.rs:160`）与 `VectorManager`（`crates/vector-client/src/manager.rs:13`）；本地能力由 `simvec` 自带。`linkrs-sync::backend` 定义 `VectorEngineHandle`（`backend.rs:40`）并给出 `LocalEngineHandle`（`backend.rs:297`）与 `QdrantEngineHandle`（`backend.rs:748`）两个实现，`VectorBackend` 是唯一的对外门面（`backend.rs:1283`）。
2. **能力查询代替引擎名判断**：`is_in_process()`（`backend.rs:48`）、`supports_atomic_promote()`（`backend.rs:110`）、`owns_space_slice_reclaim()`（`backend.rs:128`）、`default_collection_config()`（`backend.rs:117`），调用点不再写 `if engine == "qdrant"`。
3. **物理布局有两种粒度**：`Space` 粒度整个 space 一个 collection + `group_id = {tag}_{field}` payload 过滤（`crates/linkrs-sync/src/vector_sync.rs:124-143`，过滤注入见 `vector_index_manager.rs:1239-1242`）；`Field` 粒度一 `(space,tag,field)` 一个 collection（同文件 :126-131）。粒度是运行期可切的状态（`vector_index_manager.rs:54`）。
4. **逻辑索引注册表只在内存**：`logical_indexes: DashMap<VectorIndexLocation, IndexMetadata>`（`vector_index_manager.rs:33`），只有 `create_*` 与 `register_logical_index` 会写入，而后者**仅被测试调用**（全仓 grep 只有 `crates/linkrs-sync/tests/vector_rebuild.rs:160,677`）。没有对应的磁盘清单，也没有启动期重建。
5. **一致性状态走旁路文件**：`VectorReceiver` 把 `applied_lsn + receipts` 序列化到 `<recovery>/vector_receiver_state.bin`（`receiver.rs:284-302`、`receiver.rs:364-368`），因为引擎抽象层没有 commit payload 概念。
6. **重建双后端都支持**：本地 rename 目录（`backend.rs:861` 之前的能力判断 + `vector_index_manager.rs:831` 的 `publish_temp_collection`），Qdrant 走"逻辑指针重映射"（`vector_index_manager.rs:39-43` 的注释 + `backend.rs:861-870` 显式拒绝物理 rename）；启动期还要"重新收养"已发布的远端 temp（`vector_rebuild.rs:886-927`）。
7. **投递参数按后端分档**：`BackendDeliveryPolicy`（`backend.rs:1167`）local 为 `batch 128 / lease 30s / retries 16 / concurrency 1`，qdrant 为 `512 / 60s / 32 / 4`（`backend.rs:1188-1207`），由 `from_config` 一处决定（`backend.rs:1213-1219`）。

## 5. 差异矩阵

| 维度 | fulltext | vector | 性质 |
| --- | --- | --- | --- |
| 引擎 trait 归属 | 引擎 crate（`linkrs-fulltext`） | sync crate（`linkrs-sync`）+ 客户端 crate 各一套 | 结构差异 |
| 本地引擎位置 | 与管理器同 crate（`tantivy_index.rs`） | 独立 crate `simvec` | 结构差异 |
| 远端实现 | 同 trait，`reqwest` 裸 HTTP，无 SDK（`es.rs:4-7`） | 独立 trait `RemoteVectorEngine` + HTTP/gRPC 双传输 | 结构差异 |
| 后端选择点 | 1 处（`FulltextIndexManager::build_engine`） | 3 处（`startup.rs:146-192`、`backend.rs:1406`、`crates/linkrs-api/src/embedded/database.rs:72-100` 且用另一套 `EmbeddedVectorEngine` 配置类型） | **缺陷（重复决策）** |
| 配置解析健壮性 | `[fulltext.es]` 永远可解析，后端不可用时报在建索引时（`manager.rs:109-115`） | `[vector.qdrant]` 在 feature 关闭时是未知字段，`deny_unknown_fields` 直接让配置加载失败（`vector.rs:527-536`） | 不一致 |
| 索引元数据落盘 | JSON + sidecar + 磁盘发现，重启自愈 | 仅内存，重启即丢 | **缺陷** |
| 物理命名 | `space_ft_{space}_{tag}_{field}`，含 `_` 时歧义，用 sidecar 兜底回读 | `space_vec_{space}` 或 `space_vec_{space}_{tag}_{field}`；Space 粒度再派生 `group_id = {tag}_{field}` | **缺陷（vector 侧会串数据）** |
| 隔离强度 | 恒为 field 级目录隔离 | Space（共享图 + 过滤）/ Field（物理隔离）可配 | 能力差异（vector 更强） |
| 边索引 | 支持，与顶点共享命名空间 | 不支持，DDL 明确拒绝（`vector_operator/manage.rs:73-88`） | 能力差异 |
| 一致性 fence | 引擎内 commit payload（`receiver.rs:183-190`） | 旁路 bin 文件（`receiver.rs:288-302`） | 实现分歧 |
| receiver 生命周期 | 每次 field apply 新建（`delivery.rs:679`），即每次 `load_metas` 读盘 | SyncManager 持有单例（`manager.rs:56`） | **缺陷（性能）** |
| 在线重建 | 仅本地；ES 返回 `NotSupported` | 双后端都支持 | 能力差异 |
| 重建发布 | 目录 rename + `.old-<gen>` 备份 | 本地同款；远端逻辑指针 + 启动期重新收养 | 实现分歧 |
| 投递调优 | 复用 sync 层 `failure_policy`（`SyncConfig`） | 独立 `BackendDeliveryPolicy` 四参数分档 | 实现分歧 |
| 流式接口 | trait 无 stream 方法 | `search_stream` / `scroll_stream`（远端为缓冲实现，`backend.rs:1084-1096`） | 能力差异 |
| 文档 ID | 顶点裸显示形式（`fulltext_rebuild.rs:110-112`）；边 `{src}->{dst}`（live）vs `{src}->{dst}#{rank}`（rebuild） | `{vertex}#{tag}#{field}`，`%`/`#` 先转义（`intents.rs:174-186`） | **缺陷（fulltext 边）** |
| 查询层门面 | manager | sync coordinator | 抽象层级不一致 |
| 禁用语义 | `enabled=false` → 无 manager，查询报"no fulltext manager"（`fulltext_operator.rs:731-733`） | `enabled=false` → 无 backend；但 `DisabledEngine` + "disabled 仍注册元数据"分支仍存在（`vector_index_manager.rs:228-244`、`vector-client/src/engine.rs:21-152`），在 server 路径不可达 | **缺陷（死代码）** |
| RYW | `let _ = ryw_config;`（`fulltext_operator.rs:705`） | 同（`vector_operator/search.rs:67`） | 共同的未实现 |
| 指标装饰 | `MetricsSearchEngine` | `MetricsEngineHandle`（`metrics_handle.rs:1-8` 自述"mirrors"） | 已对齐 |

## 6. 合理性评估

### 6.1 值得保留的设计

- **同步骨架单一化**：两条链路共用 `SqliteOutbox` + claim/apply/ack + 死信 + 快照恢复，DDL 与数据都从同一 outbox 重放（`delivery.rs:442-455` 按 `target` 字符串分派）。这让"新增一种索引"只需要实现投递分支，不必重写持久化。
- **capability 查询优于引擎名分支**：`supports_atomic_promote` / `is_in_process` / `owns_space_slice_reclaim`（`backend.rs:48-130`）把"能不能做"和"是谁"分开，`QdrantEngineHandle` 甚至缓存了 `disabled` 布尔避免比较名字（`backend.rs:752-757`）。
- **publish fence 对称正确**：两侧都是"投递持读锁、发布持写锁"，fulltext 的注释把语义写清楚了（`manager.rs:37-41`），vector 也有同一形状（`vector_index_manager.rs:47-51`）。
- **重建失败保留旧数据**：fulltext `discard_rebuild_engine` 只删 scratch、标记 `Error`（`manager.rs:1253-1294`），vector 失败则保留 live slice 以便重试（`vector_rebuild.rs:113-120` 注释）。

### 6.2 可接受的历史折衷

- **两条链路不一致的 trait 归属**：fulltext 要在引擎 crate 里做 `match backend` 是因为 `linkrs-fulltext` 不依赖 `linkrs-sync`（DAG 约束，见 `es.rs:58-63` 为绕开依赖而复制了一个 `EsBreaker`），所以"管理器 + 引擎"只能同 crate；vector 反过来因为 `simvec` 是独立子模块仓库，适配层只能上移到 sync。**这个不对称是仓库拓扑决定的，短期内不值得强行统一**，但应该收敛抽象形状，让上层看到同一个门面。
- **ES 复用 `fulltext_metadata.json` + 本地目录语义**：ES 后端的 `storage_path` 其实无意义（`build_engine` 明确忽略路径，`manager.rs:85-87`），但仍会算目录、写 sidecar、drop 时删目录。属于"复用得太彻底"，不是错误但有噪音。
- **vector Space 粒度共享 collection**：把 HNSW 图跨 tag/field 共享，换来更少 collection 与更省内存；`simvec` 用保守前置 bitmap 过滤（`crates/simvec/src/storage/filter_bitmap.rs:1-30`，只覆盖等值条件，其余后置过滤）保证结果正确但可能降低高选择性过滤下的召回。这是**有意识的权衡**，且有 `Field` 粒度作为逃生通道；问题只在命名冲突（见 D1）。

### 6.3 明确不合理的部分

- **同一个决策写三遍**：vector 后端选择在 `startup.rs`、`VectorBackend::from_config_async`、`linkrs-api` 的 `create_vector_backend` 各实现一次，且第三处用的是另一套配置类型 `EmbeddedVectorEngine`。三者对"enabled 但 qdrant.enabled=false"的处理语义也不同（server 侧整体降级为 None，`linkrs-config/src/lib.rs:582-589`；embedded 侧直接返回 None，`database.rs:79-100`）。
- **元数据持久化两套哲学**：fulltext "文件为准、可自愈"，vector "内存为准、重启归零"。直接后果是 D2（见下）。
- **一致性窗口两种落盘位置**：fulltext 把 receipts 和数据放在同一次 commit（真正的原子边界），vector 分两次写（先写点、再写 bin），崩溃窗口内需要靠 SQLite `idempotency` 表兜底（`receiver.rs:316-318` 注释承认）。两者对"到底谁负责幂等"的答案不同。

## 7. 缺陷明细

### D1 物理命名裸拼接 → 冲突（高）

- `space_ft_{space}_{tag}_{field}`（`metadata.rs:60-65`）、`space_vec_{space}_{tag}_{field}`（`vector_sync.rs:124-131`）、`group_id = {tag}_{field}`（`vector_sync.rs:138-141`）都用 `_` 无转义拼接。
- 后果：`tag="a_b", field="c"` 与 `tag="a", field="b_c"` 映射到同一物理名。fulltext 有 sidecar 只能保证**回读**不歧义（`manager.rs:272-275` 注释），不能阻止两个逻辑索引打开同一目录；vector Space 粒度下两者共享同一 `group_id`，**搜索会静默返回彼此的 point**，属于数据正确性问题。
- 方向：改用长度前缀或 hex 编码（`{tag.len()}:{tag}`），`group_id` 至少加分隔符转义；迁移时需要一次性重命名脚本。

### D2 vector 逻辑索引注册表不落盘（高）

- 注册表只有内存（`vector_index_manager.rs:33`），`register_logical_index` 仅测试调用。
- 后果：进程重启后 `list_indexes()` 为空 → `GET /v1/vector/indexes` 空（`linkrs-api/src/api_core/vector_api.rs:206-217`）、`recover_stale_vector_rebuilds` 的修复循环 `for metadata in coordinator.list_indexes()` 直接空转（`vector_rebuild.rs:781-806`）、rebuild 前置校验找不到索引。搜索路径不受影响（collection 名由 key + 粒度确定性推导），所以**故障是"看起来没索引但能查"**，最容易误判。
- 方向：照 fulltext 做法落一份 `vector_metadata.json`，或启动时从 schema 元数据（索引定义已持久化在存储层）回填注册表。

### D3 fulltext 启动发现硬编码 Tantivy（中）

- `discover_indexes_from_disk` 与 `try_restore_bm25_index` 直接 `TantivySearchEngine::open_or_create`（`manager.rs:221`、`manager.rs:246`），`create_rebuild_engine`（:1016）、`restore_rebuild_backup`（:1229）同样硬编码；只有 `restore_index_from_metadata` 走 `build_engine`（:325）。
- 后果：`backend = es` 且 `fulltext_metadata.json` 缺失但旧目录仍在时，会用 Tantivy 引擎接管索引，形成"配置说 ES、实际跑 Tantivy"的混合后端；`create_rebuild_engine` 的 ES 检查只挡住了显式调用，恢复路径仍可能造出 Tantivy scratch。
- 方向：所有构造点统一走 `build_engine`；ES 后端在发现阶段直接跳过本地目录扫描。

### D4 ES 无在线重建（中）

- `manager.rs:980-986` 返回 `NotSupported`，理由是"published 需要 alias swap，尚未实现"。但对外 API 仍暴露 rebuild 入口（`http/handlers/rebuild.rs:162`），错误只能在运行期拿到。
- 方向：要么补 alias swap 的重建（写 `linkrs_es_{index}_{gen}` + `_aliases` 切换），要么在能力查询里暴露 `supports_online_rebuild`，让 API 直接 4xx 拒绝。

### D5 fulltext 每次 apply 重建 receiver（中）

- `apply_fulltext_fields` 每次循环都 `FulltextReceiver::new(engine)`（`delivery.rs:679`），构造函数会 `engine.commit_payload()`，Tantivy 实现是 `index.load_metas()`（`tantivy_index.rs:473-475`），即**每条字段变更读一次盘、解析一次 JSON 并克隆整个 receipts 集合**。vector 侧是 `SyncManager` 持有单例（`manager.rs:56`）。
- 方向：按 index 缓存 receiver（`DashMap<IndexKey, Arc<FulltextReceiver>>`），失效点收敛在 publish/clear。

### D6 fulltext 边文档 ID 契约不一致（中）

- live upsert 用 `{src}->{dst}#{ranking}`（`delivery.rs:33-39` 的 `edge_entity_id`），live delete 走 `{src}->{dst}`（`receiver.rs:130-135` 对 `EntityRef::Edge` 只 format src/dst，丢掉 ranking），rebuild 用 `{src}->{dst}#{ranking}`（`fulltext_rebuild.rs:121-123`）。
- 代码注释已自述后果："stale edge versions can orphan exactly as they do on the live path. Rebuild stays faithful to live behavior here."（`fulltext_rebuild.rs:118-120`）——即**删边后陈旧全文文档残留**。
- 方向：delete 也带 ranking（`EdgeRef` 已有 ranking 字段，`manager.rs:236` 附近测试可见），并补一条"删边后检索不到"的回归测试。

### D7 禁用语义残留（低）

- `DisabledEngine` 把 20+ 方法全部返回 `EngineNotAvailable`（`vector-client/src/engine.rs:21-152`），`create_vector_index` 还有一个"disabled 也写元数据并返回成功"的分支（`vector_index_manager.rs:228-244`）。但 `is_vector_enabled()` 已把 `vector.enabled && vector.qdrant.enabled` 为假的情况挡在构造之前（`linkrs-config/src/lib.rs:582-589`），server 路径下这些分支不可达。
- 后果：读代码的人会以为"禁用后仍可建索引、只是查不到"，与真实行为（根本没有 coordinator，`SEARCH VECTOR` 直接报错）不符。
- 方向：删除不可达分支，或让 `is_vector_enabled` 的三态语义显式化（Disabled / Local / Remote）。

### D8 遗留死代码（低）

- `SyncManager::vector_index_exists`（`accessors.rs:221-227`）无任何调用者；`FulltextEngineType` 只剩 `Bm25` 一个变体（`fulltext.rs:9-12`）却仍出现在 `FulltextConfig.default_engine`（:249）与 `IndexMetadata.engine_type`（`metadata.rs:12`），与真正起作用的 `backend` 形成两套词汇。

### D9 查询层两套门面（低，但影响收敛）

- `SearchProvider` 只有 `list_indexes` / `drop_index` 两个方法（`traits.rs:55-80`），且 fulltext 包装的是 manager、vector 包装的是 coordinator（`traits.rs:84` vs `:158`）；`drop_index` 甚至在 fulltext 里用 `block_in_place` + `block_on` 桥接异步（`traits.rs:143-153`）。
- 后果：任何跨后端的通用逻辑（管理端点、健康检查、rebuild 调度）都要分别写两遍，这也正是 `accessors.rs:130-160` 要"先问 fulltext 再问 vector"的原因。

## 8. 收敛建议

**高优先级（正确性）**

1. 统一物理命名编码，消除 D1；`group_id` 必须无歧义。可先加"创建时检测重名冲突并拒绝"的即时护栏，再排迁移。
2. vector 索引元数据落盘或启动期从 schema 回填（D2）。
3. 修 D6 边文档 ID，并给 fulltext 加边删除的回归用例。

**中优先级（一致性与性能）**

4. fulltext 所有引擎构造点收敛到 `build_engine`，发现阶段按 backend 分流（D3）。
5. receiver 按 index 长驻（D5）。
6. 定义一次"能力"清单（`supports_online_rebuild` / `supports_edge_index` / `supports_streaming`），由两个门面共同实现，API 层据此 4xx，而不是运行期撞错误（D4）。
7. 引擎构造单一入口：`linkrs-api` 复用 `VectorBackend::from_config_async`，删掉 `create_vector_backend` 与 `EmbeddedVectorEngine`（6.3 第一条）。

**低优先级（清理）**

8. 删除 `DisabledEngine` 不可达分支与 `vector_index_exists`（D7、D8）。
9. 把 `/v1/fulltext/indexes` 从 `rebuild.rs` 移回 `handlers/fulltext.rs`，让 fulltext 的 API 面与 vector 对称（哪怕功能少，位置也该对）。
10. `SearchProvider` 扩成能表达"创建/描述/查询"的统一门面，两个包装类型只在"引擎特有参数"处分叉（D9）。

> 若目标是"一步到位统一"，正确做法不是把 fulltext 塞进 `VectorEngineHandle` 那套适配器，而是把 `VectorEngineHandle` 里**与引擎无关的能力声明**抽到一个中立的 `linkrs-search-api`（或 `linkrs-core::search`）里，两侧各自实现；否则会为了对称而让 `linkrs-fulltext` 依赖 sync 层，违反 AGENTS.md 的严格 DAG 要求（`AGENTS.md:105`）。

## 9. 验证方式

```shell
# 结构核对
cargo metadata --no-deps --format-version 1 | jq '.packages[].name'
cargo tree -p linkrs-fulltext --depth 1
cargo tree -p linkrs-sync --features fulltext,vector-qdrant --depth 1

# fulltext 行为（含 ES 对照）
cargo test -p linkrs-fulltext --features fulltext
cargo test -p linkrs-fulltext --features fulltext,fulltext-es

# vector 行为
cargo test -p vector-client
cargo test -p linkrs-sync --features vector

# 查询层集成
cargo test -p linkrs-query --features fulltext,vector
```

> 说明：本次未执行 `cargo test`（仓库含 `tantivy` / `simvec` / `llm-suite` 子模块，全量构建耗时较长），以上为可直接复现的验证命令。
