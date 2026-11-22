# fulltext 与 vector 后端收敛方案

本文是《fulltext 与 vector 后端集成方式对比分析》（`docs/analysis/fulltext-vector-backend-integration.md`）的后续行动方案：给出完整问题清单、分阶段修改内容与实际落地的代码改动。文中引用的行号为本次修改完成后的位置。

## 0. 目标与约束

目标是消除两条二级索引链路中会造成错误结果与恢复失效的缺陷，并把重复决策收敛到单一入口，同时保持 `AGENTS.md` 要求的严格 DAG：`linkrs-fulltext` 不依赖 `linkrs-sync`，共享逻辑只能下沉到 `linkrs-core`。

约束为：不做兼容层，不保留已确认不可达的分支，不为"对称"而强推抽象。仓库处于开发阶段，物理命名编码可以一次改净，不写迁移脚本。

## 1. 问题清单

### 1.1 总览

| 编号 | 问题 | 严重度 | 影响 | 本次处理 |
| --- | --- | --- | --- | --- |
| A | 物理命名用 `_` 裸拼接 | 高 | 不同逻辑索引映射到同一物理名/组，搜索静默串数据 | 已修 |
| B | fulltext 文档 ID 在 upsert/delete/rebuild 间不一致 | 高 | 平行边互相覆盖，删边后陈旧文档残留 | 已修 |
| C | vector 逻辑索引注册表只在内存 | 高 | 重启后列表为空、重建恢复空转 | 已修 |
| D | fulltext 引擎构造点硬编码 Tantivy | 中 | `backend=es` 时可能被本地目录接管，形成混合后端 | 已修 |
| E | 每次字段 apply 重建 receiver | 中 | 每条变更读一次 Tantivy metas、解析并克隆 receipts | 已修 |
| F | ES 后端不支持在线重建但仍接受请求 | 中 | 任务被接受后才失败，错误延后到运行期 | 已修 |
| G | disabled 引擎仍「注册索引并返回成功」 | 低 | 与实际行为（无 coordinator）矛盾，误导读者与调用方 | 已修 |
| H | `vector_index_exists` 无调用者 | 低 | 死代码 | 已修 |
| I | 查询层两套门面（manager / sync coordinator） | 低 | 管理端点、健康检查、重建调度都要写两遍 | 未纳入 |

### 1.2 逐条证据与影响

**A 物理命名裸拼接。** 修改前 `IndexKey::to_index_id` 用 `space_ft_{space}_{tag}_{field}`（`crates/linkrs-fulltext/src/metadata.rs`），`VectorIndexLocation::to_collection_name_with` 的 Field 粒度用 `space_vec_{space}_{tag}_{field}`，Space 粒度派生 `group_id = {tag}_{field}`（`crates/linkrs-sync/src/vector_sync.rs`）。四段下划线拼接在名字本身含 `_` 时不可逆：`("a_b","c")` 与 `("a","b_c")` 得到同一字符串。fulltext 的 `fulltext_key.json` sidecar 只能保证回读不歧义，不能阻止两个逻辑索引打开同一目录；vector 的 Space 粒度下两者共享同一 `group_id`，**搜索会把彼此的 point 一起返回**。另外 `drop_vector_index` 里还有一处独立复刻的 `format!("{tag_name}_{field_name}")` 用于按组清理，绕过了 `VectorIndexLocation`，属于同一缺陷的重复实现。

**B fulltext 文档 ID 不一致。** `FulltextReceiver::apply_index_batch` 对 `EntityRef::Edge` 只拼 `{src}->{dst}`（`crates/linkrs-sync/src/receiver.rs`），丢掉 `ranking`；而 `fulltext_rebuild.rs` 的 backfill 幂等键用 `{src}->{dst}#{ranking}`，`delivery.rs` 的 `edge_entity_id` 也带 ranking 却只用于一行日志。结果：同 `(src,dst)` 的多条平行边互相覆盖成一份文档；删边只按无 ranking 的 ID 删除，带 ranking 的旧文档永远残留。原代码注释已自述"stale edge versions can orphan exactly as they do on the live path"。顶点侧同样有分歧：`VertexId` 的 `Display` 对 text 类型带引号，而 `Value::from(vid)` 的 `Display` 不带，rebuild 与 live 在文本顶点 ID 上算出不同文档 ID。

**C vector 注册表不落盘。** `logical_indexes` 只是内存 `DashMap`（`crates/linkrs-sync/src/vector_index_manager.rs`），写入口只有 `create_vector_index` / `create_index_with_config` / `register_logical_index`，其中后者仅测试调用。重启后 `list_indexes()` 为空 → `GET /v1/vector/indexes` 空、`recover_stale_vector_rebuilds` 的修复循环直接空转、重建前置校验找不到索引。由于搜索路径用确定性命名推导 collection 名，故障表现为"看起来没索引但能查"。

**D fulltext 引擎构造点硬编码。** `try_restore_bm25_index`、`create_rebuild_engine`、`restore_rebuild_backup` 直接 `TantivySearchEngine::open_or_create`，只有 `restore_index_from_metadata` 走 `build_engine`。`backend=es` 且元数据文件缺失但旧本地目录仍在时，会用 Tantivy 接管索引，形成"配置说 ES、实际跑 Tantivy"的混合后端。

**E receiver 每次重建。** `apply_fulltext_fields` 每个字段都 `FulltextReceiver::new(engine)`，构造函数调 `engine.commit_payload()`，Tantivy 实现是 `index.load_metas()`，即每条字段变更读一次盘并克隆整个 receipts 集合。vector 侧由 `SyncManager` 持有单例，不受影响。

**F ES 重建无前置拒绝。** `create_rebuild_engine` 返回 `NotSupported`，但 `POST /v1/fulltext/indexes/rebuild` 先注册任务再 spawn，错误只能在任务状态里看到。

**G/H 残留语义。** `create_vector_index` 曾有一个"引擎禁用时仍写元数据并返回成功"的分支，与 `is_vector_enabled()` 已把禁用挡在构造之前的现实冲突；`SyncManager::vector_index_exists` 无任何调用者。

## 2. 修改总览

| 阶段 | 内容 | 涉及文件 |
| --- | --- | --- |
| 正确性整治 | 命名编码、文档 ID、注册表落盘 | `linkrs-fulltext/src/metadata.rs`、`linkrs-fulltext/src/manager.rs`、`linkrs-sync/src/vector_sync.rs`、`linkrs-sync/src/vector_index_manager.rs`、`linkrs-core/src/wal/sync.rs`、`linkrs-sync/src/receiver.rs`、`linkrs-sync/src/manager/delivery.rs`、`linkrs-sync/src/fulltext_rebuild.rs` |
| 一致性收敛 | 引擎构造单一入口、receiver 常驻 | `linkrs-fulltext/src/manager.rs`、`linkrs-sync/src/manager.rs`、`linkrs-sync/src/manager/delivery.rs`、`linkrs-sync/src/manager/lifecycle.rs`、`linkrs-sync/src/manager/outbox_admin.rs` |
| 清理与可观测 | 删不可达分支与死代码、能力查询前置拒绝 | `linkrs-sync/src/vector_index_manager.rs`、`linkrs-sync/src/manager/accessors.rs`、`linkrs-fulltext/src/manager.rs`、`linkrs-server/src/http/handlers/rebuild.rs` |

## 3. 正确性整治

### 3.1 物理命名改为长度前缀

tag 与 field 都按"长度 + 原文"编码，分隔符仍是 `_`，因此目录名可读、跨平台合法（不含 `:`），且解析唯一：

```
space_ft_1_7_Article_7_content      → space=1, tag="Article", field="content"
space_ft_1_12_User_Profile_13_email_address → tag="User_Profile", field="email_address"
space_vec_1_3_a_b_1_c               → Field 粒度
group_id = "3_a_b_1_c"              → Space 粒度
```

`metadata.rs` 新增逆向解析 `IndexKey::from_index_id`，解析时用 `str::get(..n)` 而不是字节切片，长度落在多字节字符中间时返回 `None` 而非 panic：

```rust
pub fn to_index_id(&self) -> String {
    format!(
        "{}_{}_{}_{}_{}_{}",
        FULLTEXT_INDEX_PREFIX,
        self.space_id,
        self.tag_name.len(),
        self.tag_name,
        self.field_name.len(),
        self.field_name
    )
}
```

`manager.rs` 的目录名回退解析改为委托它，`try_restore_bm25_index` 的 `metadata.index_id` 统一用 `key.to_index_id()` 规范化。`vector_index_manager::drop_vector_index` 里独立复刻的组名改用 `VectorIndexLocation::group_id_with`，不再手拼。

重建临时集合在同一维度上也有同一缺陷：Space 粒度的 temp 名是 `{live}.rebuild-{gen}__{tag}_{field}`，owner 后缀原来手拼 `{tag}_{field}`，两个切片会撞到同一个 temp 目录。现改为直接使用 `group_id` 编码，让"组标识"和"临时集合后缀"共用一套编码：

```rust
let owner = (self.granularity() == CollectionGranularity::Space)
    .then(|| loc.group_id_with(CollectionGranularity::Space).unwrap_or_default());
```

Space 粒度的 collection 名 `space_vec_{space}` 本身无歧义，保持不变，因此大量既有 vector 测试无需改动。

### 3.2 文档 ID 单点派生

在 `linkrs-core::wal` 给 `EntityRef` 加 `doc_id()`，让 live upsert、live delete、rebuild backfill 与幂等键共用同一形式：

```rust
pub fn doc_id(&self) -> String {
    match self {
        Self::Vertex(vertex_id) => vertex_id.to_string(),
        Self::Edge { src, dst, ranking, .. } => format!("{src}->{dst}#{ranking}"),
    }
}
```

派生结果：`receiver.rs` 的删除与写入都调 `mutation.entity_ref.doc_id()`（删边因此能精确命中带 ranking 的文档）；`delivery.rs` 删除 `FulltextFieldApply::entity_id` 字段与 `edge_entity_id` 函数，日志直接用 `entity_ref.doc_id()`；`fulltext_rebuild.rs` 删除 `fulltext_vertex_doc_id` / `fulltext_edge_doc_basis`，幂等键改用同一个 `doc_id()`，`linkrs-sync` 的对应重导出同步移除。

顶点侧随之统一到 `VertexId` 的显示形式（文本 ID 带引号），rebuild 与 live 不再算出两个 ID。

### 3.3 vector 注册表落盘

`VectorIndexManager` 增加 `registry_file`，由 `SyncManager` 在 outbox 工作目录就绪时挂载（`work_dir/vector_indexes/vector_metadata.json`），与 fulltext 的 `fulltext_metadata.json` 采用同一套"原子写 + 启动加载"哲学。

落盘条目只存逻辑字段，**不存物理 collection 名**——名字由 `(location, granularity)` 现算，粒度切换不会留下过期名字：

```rust
struct PersistedLogicalIndex {
    space_id: u64,
    tag_name: String,
    field_name: String,
    config: CollectionConfig,
    created_at: chrono::DateTime<chrono::Utc>,
    index_name: Option<String>,
}
```

写点覆盖 `create_vector_index`、`create_index_with_config`、`register_logical_index`、`drop_vector_index`；写失败只告警，内存注册表继续服务（与 fulltext 元数据缓存的降级方式一致）。加载在 `attach_registry_directory` 内完成，因此 `list_indexes()`、重建恢复、管理端点在重启后与投递路径看到同一份索引集合。

`collection_overrides`（Qdrant publish 指针）与 `promote_backups` 已纳入注册表持久化，详见 `sync-backend-design-decisions.md` 阶段 4；`vector_count` 是运行期统计，重启归零。

## 4. 一致性与性能收敛

### 4.1 引擎构造单一入口

`try_restore_bm25_index`（两个分支）、`create_rebuild_engine`、`restore_rebuild_backup` 全部改走 `build_engine`，`TantivySearchEngine::open_or_create` 在管理器内只剩 `build_engine` 一处调用。

发现阶段按后端分流：先读元数据文件（本地与 ES 都读，ES 重启后靠它恢复逻辑索引列表），文件缺失时本地才扫目录，ES 直接返回——ES 的索引状态在服务端，按逻辑索引 ID 寻址，残留本地目录属于更早的本地运行，绝不能被接管：

```rust
if self.config.backend == FulltextBackendKind::Es {
    return Ok(());
}
self.discover_indexes_from_disk()
```

### 4.2 receiver 常驻

`SyncManager` 新增 `fulltext_receivers: Arc<DashMap<u64, FulltextReceiverSlot>>`，槽位同时持有引擎与 receiver：

```rust
struct FulltextReceiverSlot {
    engine: Arc<dyn FulltextSearchEngine>,
    receiver: Arc<crate::receiver::FulltextReceiver>,
}
```

按 `index_id` 命中且 `Arc::ptr_eq(engine)` 时复用，否则重建并覆盖。持有引擎有两个作用：避免引擎释放后地址被新引擎复用导致误命中；引擎被 publish 替换时指针自然变化，缓存自动失效，无需额外失效通知。

重建补放（catch-up）写入的是 scratch 引擎，与 live 引擎交替出现，按 `index_id` 缓存会来回抖动，因此 override 路径仍每次新建 receiver，缓存只服务 live 投递。`DropIndex` / `DropSpace` / `DropTag` 投递后清空缓存，条目数等于字段索引数，代价可忽略。

## 5. 清理与可观测

### 5.1 删除不可达语义与死代码

`create_vector_index` 的"禁用引擎仍注册元数据"分支删除，改为在发出 `VectorBuildStarted` 事件之前直接返回 `EngineDisabled`——先发事件再失败会留下没有对应完成事件的孤立开始事件。判定依据是 `Config::is_vector_enabled()` 已把 `vector.enabled=false` 与 `qdrant.enabled=false` 挡在 coordinator 构造之前，server 与 embedded 两条装配路径都不会拿到禁用后端。

`SyncManager::vector_index_exists` 删除。

另外补齐了一处门面缺口：`VectorSyncCoordinator` 缺少 `search_by_location` 委派，而集成测试已经按该签名调用，导致 `--features vector` 下测试目标无法编译。新增委派让 coordinator 覆盖 manager 的读路径，属于问题清单 I 的最小修复。

### 5.2 能力查询前置拒绝

`FulltextIndexManager::supports_online_rebuild()` 返回 `backend == Local`；`VectorIndexManager` 的同名方法返回 `true`（本地 rename 与 Qdrant 指针重映射都已实现）。

`POST /v1/fulltext/indexes/rebuild` 在注册任务、spawn 之前检查该能力，不满足直接 400 并说明"由持久 outbox 维持一致性"，不再让调用方从任务状态里读出一个必然失败的错误。

## 6. 未纳入本次的问题

| 问题 | 理由 |
| --- | --- |
| `FulltextEngineType` 只剩 `Bm25` 一个变体却仍出现在 `FulltextConfig.default_engine` 与 `IndexMetadata.engine_type` | 与真正起作用的 `backend` 形成两套词汇，但该类型出现在数十处测试与 bench 的 `Some(EngineType::Bm25)` 参数里；删除是纯词汇清理，收益低于改动面，留待统一索引 DDL 时一并处理 |
| `DisabledEngine` 与散落在搜索路径的 `is_disabled_engine()` 守卫 | `vector-client` 对外仍把它当作 feature 关闭时的占位实现；守卫是防御性的，删除会牵动十余个函数与错误类型，本次只清掉"禁用仍成功建索引"这一条会造成错误认知的语义 |
| 查询层两套门面（`FulltextProvider` 包 manager，`VectorProvider` 包 coordinator） | 真正统一需要把与引擎无关的能力声明下沉到中立 crate，属于架构决策，需单独立项；本次只把能力查询做成同名方法，让上层的判定点先统一 |
| ES 后端的在线重建（alias swap） | 需要 `linkrs_es_{index}_{gen}` + `_aliases` 切换，本次先在能力查询处显式拒绝 |
| Qdrant `collection_overrides` 跨重启 | 已由 `sync-backend-design-decisions.md` 阶段 4 落地：随注册表持久化，重采纳仅作 crash 窗口兜底 |
| 读己之写（RYW） | 两侧都是 `let _ = ryw_config;`，需要语义设计而非收敛 |

## 7. 验证

编译（`crates/vector-client` 的 `qdrant-grpc` 特性需要 `protoc`，本机以 `protoc 25.1` 二进制补齐）：

```shell
cargo check -p linkrs-fulltext --features fulltext,fulltext-es --tests
cargo check -p linkrs-sync --features fulltext
cargo check -p linkrs-sync --features fulltext,vector
cargo check -p linkrs-query --features fulltext,vector
cargo check -p linkrs-api --features fulltext,vector
cargo check -p linkrs-server --features fulltext,vector
cargo check --workspace --all-targets
```

测试：

```shell
cargo test -p linkrs-core --lib
cargo test -p linkrs-fulltext --features fulltext
cargo test -p linkrs-sync --features fulltext,vector --lib
cargo test -p linkrs-sync --features vector --test vector_local_backend
cargo test -p linkrs-sync --features fulltext,vector --test vector_rebuild
cargo test -p linkrs-sync --features fulltext,vector --test vector_outbox_delivery
```

结果：

| 目标 | 结果 |
| --- | --- |
| `linkrs-core --lib` | 482 passed |
| `linkrs-fulltext --features fulltext` | lib 32 passed，集成 59 passed |
| `linkrs-sync --features fulltext,vector --lib` | 53 passed（含新增注册表用例 2 个） |
| `linkrs-sync --test vector_local_backend` | 7 passed |
| `linkrs-sync --test vector_rebuild` | 10 passed |
| `linkrs-sync --test vector_outbox_delivery` | 2 passed |
| `cargo check --workspace --all-targets` | 仅根 crate `tests/integration_streaming_executor.rs` 报 5 个 `UnaryOperatorKind` / `BlockingOperatorKind` 缺字段错误，该测试与索引链路无关，为改动前既有状态 |

新增/更新的用例：

- `linkrs-core`：`doc_id_distinguishes_parallel_edges`（同端点不同 ranking 得到不同 ID，顶点形式与 `VertexId` 显示一致）。
- `linkrs-fulltext`：`index_id_roundtrips_through_parser`（含下划线、中文、单字符名）、`index_id_disambiguates_underscore_names`（`a_b/c` 与 `a/b_c` 不再相等）、`legacy_index_id_is_not_parsed`（旧格式被拒且不 panic）。
- `linkrs-sync`：`logical_indexes_survive_reopen`、`dropped_index_is_removed_from_registry`（注册表跨重启、drop 后不复活），以及 Field 粒度 collection 名与 Space 粒度 temp 名断言按新编码更新。
- `linkrs-fulltext` 既有命名断言按新编码更新；`linkrs-sync` 的 `group_id` 断言更新为长度前缀形式。

改动文件均已通过 `rustfmt --check`（仓库内 `es.rs`、`query_api.rs`、`backend.rs` 等既有文件本就不符合 rustfmt，未一并格式化以免制造无关差异）。

## 8. 风险与后续

- 物理命名变更不可与旧数据共存：旧目录/旧 collection 不会被新代码识别，需要重建索引或按新命名重命名。仓库处于开发阶段，不做兼容。
- 注册表落盘是"以文件为准"，与 fulltext 一样允许手工修复；文件损坏时只告警、加载为空，不会阻断启动。
- 后续若要继续收敛，最值得做的是把 `supports_*` 能力清单与索引 CRUD 门面下沉到一个中立的搜索抽象，让 `linkrs-fulltext` 与 vector 两侧各自实现，而不是让 `linkrs-fulltext` 依赖 sync 层。
