# 存储、索引与同步架构迁移计划

> 当前进度（2026-07）：阶段 0 已完成；阶段 1 和阶段 3 已形成基础闭环但尚未完成旧路径删除与全部故障验收；阶段 4 已落地 ordered codec、typed predicate 和统一 cursor 的主要结构；阶段 5 已接入 edge index，但 rebuild、covering record 和 cursor 正确性仍未闭环；阶段 6 已完成持久 manifest、结构化 shard 路由、reader handle fence 和安全回收 primitive，在线 split 仍因阶段 5 的增量追平协议缺失而禁用；阶段 7 仅完成部分测试和指标，尚未收尾。

## 1. 执行原则

本计划将现有实现迁移到[目标架构](../analysis/storage_sync_target_architecture.md)。项目不要求运行时向后兼容，但每个阶段都必须形成可编译、可测试的单一路径；不得先删除生产路径、到后续阶段才补上替代闭环。

每阶段遵循：

1. 先冻结数据模型、持久边界和失败语义；
2. 以垂直闭环接入真实调用点；
3. 新路径通过故障测试后立即删除同职责旧路径；
4. 不以 feature flag 长期保留两个运行实现；
5. 文件迁移可保留只读备份，但备份不参与正常运行；
6. 阶段验收未通过，不进入下一阶段。

## 1.1 阶段状态总览

| 阶段 | 状态 | 已完成范围 | 主要剩余项 |
| --- | --- | --- | --- |
| 0 | 已完成 | 强类型、WAL wire type、manifest/cursor 基础类型、roundtrip/版本校验 | 补充更多 crash fixture |
| 1 | 基本闭环，未最终收尾 | WAL batch、commit LSN、committed recovery、自动/显式 commit sink、SQLite projection、claim/lease/retry/frontier | 删除 JSON 运行依赖；补齐 DDL/所有写 API 垂直闭环；移除兼容 direct API |
| 2 | 未开始 | SQLite 已有 generation gate 和基础 frontier | 真实 fulltext/vector receiver、远端幂等 receipt、late-arrival 防护、minimum LSN |
| 3 | 部分完成 | SQLite `VACUUM INTO` snapshot、checksum/restore primitive、延后 WAL 截断 | 组合 checkpoint manifest、snapshot+WAL 重建、JSON importer、独立回收判定 |
| 4 | 部分完成 | OrderedKeyCodec、typed predicate、`IndexRow` cursor 接口 | prefix codec 边界、storage 内实体版本校验、完整 covering row |
| 5 | 部分完成 | edge index DDL/读写/cursor、基础 MVCC record | included columns；固定 snapshot、增量追平和 publish fence rebuild |
| 6 | 部分完成 | 版本化持久 manifest、`[lower,upper)` 路由/pruning、manifest handle 与延迟回收 | 将 manifest 接入物理 cursor/checkpoint；复用安全 rebuild 完成在线 split |
| 7 | 部分完成 | manifest 指标和针对性单元测试 | 全链路故障演练、旧路径清理和完整运维文档 |

## 2. 阶段 0：基线、类型与协议冻结（已完成）

### 修改

- 在 core 定义强类型 `CommitLsn`、`TargetId`、`IndexGeneration`、`ManifestEpoch`、`LeaseEpoch`、`OrderingKey`、`IdempotencyKey`，禁止裸 `u64` 在不同版本域之间传递。
- 定义 WAL intent/commit wire type、target mutation、lifecycle state、`IndexRow`、`IndexPredicate`、`PartitionSelector` 和 `IndexManifest` 的模块归属。
- 固定 API：写结果返回 `commit_lsn`；外部索引查询可传 `minimum_lsn`；等待超时、degraded frontier 和 dead-letter 使用独立错误类型。
- 建立 crash point、WAL 截断、target simulator、SQLite snapshot 损坏、MVCC snapshot 和 manifest fixture。

### 验收

- 类型与 wire format roundtrip、未知版本拒绝、checksum 失败测试通过。
- 明确 `CommitLsn` 是 commit record end LSN，MVCC timestamp 不是 LSN。
- `cargo fmt --check` 和全 workspace feature check 通过。

## 3. 阶段 1：本地提交到持久 outbox 的最小闭环（基本闭环，收尾中）

该阶段一次完成 WAL commit、target-specific intent、SQLite projection 和 claim 协议，避免出现“已删除旧投递但新队列尚未存在”的中间状态。transport 先使用确定性模拟器，阶段结束时真实写入已经只有 WAL → SQLite → claim 一条路径。

### 修改

- 在 transaction WAL 中实现 `append_transaction_batch`、batch checksum、commit record end LSN、durability fence 和 committed recovery。
- 显式事务与自动提交统一分配 transaction ID；storage 在同一 schema snapshot 下生成 data redo、native-index redo 和 target mutation intent。
- 在 graphdb-sync 引入 SQLite，建立 `events`、`commit_targets`、`idempotency`、`projection_state`、`target_state`、`generation_state`、`dead_letters` 和 migration 表。
- storage 将 committed intent 通过 core 类型投影给 materializer；事件插入、target ledger 和 `materialized_lsn` 在同一 SQLite 事务提交。
- 实现 generation gate、head-of-line claim、lease/fence、ack/retry/dead-letter/requeue 和模拟 transport dispatcher。
- 写 API 只在 WAL durability fence 与 MVCC visibility publish 完成后返回 `commit_lsn`。

### 删除

- 删除写成功后由 `SyncWrapper` 生成事件的职责、direct sync、发送后补 claim 和 transaction 整体 ack。
- 删除 JSON `PersistentOutbox` 运行路径、文件锁和全量重写；旧文件读取仅暂留为阶段 3 的一次性 importer。
- 删除 `txn_sequences`、`max_transaction_sequence` 和通用 vertex/edge payload 作为跨 target wire payload 的用法。

### 验收

当前已验证：WAL batch roundtrip、commit LSN、checksum、未提交 tail 不恢复、SQLite durability、frontier 不跨洞、stale lease 不能 ack、显式 commit/abort sink 和 storage 全量单元测试通过。以下验收仍未完成：真实 DDL 全链路、所有 crash point 注入、JSON importer 后删除旧 API，以及代码库中彻底清除兼容 direct/sequence 路径。

- 在 redo 前、intent 中间、commit record 中间、fsync 后和 visibility publish 前注入崩溃；恢复结果只能是完整未提交或完整已提交。
- 缺 intent、序号不连续、checksum 错误和截断 commit 不可恢复为已提交。
- SQLite materialize 重放不重复；同 ordering key 不越过 backoff/leased 头事件；stale lease 不能 ack。
- 从真实写 API 到模拟 target 完成闭环，代码中不存在 direct delivery。

## 4. 阶段 2：真实 transport、generation barrier 与无洞 frontier（未开始）

### 修改

- 实现 `(target, index_id)` lifecycle 状态机及 create/backfill/catch-up/activate/drain/drop barrier。
- 实现 fulltext `ApplyIndexBatch`，索引变化与 idempotency receipt 原子持久化。
- vector 采用能够拒绝旧 LSN 的持久 receiver；若当前 Qdrant API 不支持条件写，则使用 versioned point/tombstone，不声明普通 deterministic-ID upsert 为 exactly-once。
- 实现 `commit_targets` 连续推进算法，以及 target/index-generation 两级 frontier。
- 配置连接、写入、读取和一致性等待超时；实现 `minimum_lsn` 等待。
- dead-letter 阻塞 frontier；显式 skip 持久化 degraded range。

### 删除

- 删除只识别 `sync` target 的 routing、coordinator 当前元数据推导 mutation 和跨 target 共享确认。
- 删除任何以最大成功 event LSN 直接更新 watermark 的逻辑。

### 验收

- LSN 乱序完成时 frontier 不跨洞；无该 target event 的 commit 可被 ledger 安全跨越。
- generation 未 Active 时数据不可 claim；create/drop barrier 能阻挡所有 entity ordering key。
- 超时后旧请求晚到不能覆盖更高 LSN；fulltext receipt 与数据不存在分裂状态。
- 不同 target/index 的失败和等待互不错误确认。

## 5. 阶段 3：原子 checkpoint、outbox snapshot 与 WAL 回收（部分完成）

### 修改

- 使用 SQLite backup API 创建不可变 outbox snapshot，记录 snapshot LSN、大小和 checksum。
- checkpoint manifest 同时引用 storage snapshot、outbox snapshot 和 native-index manifests；临时文件 fsync 后原子发布 manifest并 fsync 目录。
- WAL cleanup 只使用已发布 manifest 的共同 safe LSN。
- 实现在线 SQLite 丢失、回退或损坏后从 outbox snapshot加剩余 WAL 重建。
- 以独占事务一次性导入旧 JSON event/marker，成功后改名为只读备份并记录 migration 完成。
- 实现 event、receipt、旧 snapshot 和旧 WAL segment 的独立回收判定。

### 删除

- 删除 JSON importer 之外的所有 JSON outbox API、旧 checkpoint marker、outbox sequence 字段和对应指标。
- 删除 checkpoint 依赖进程内 coordinator 状态的路径。

### 验收

当前已验证 SQLite snapshot 文件可原子生成、fsync 并通过 checksum 校验；WAL 截断已放到 snapshot 创建之后，并受 outbox safe LSN 限制。组合 manifest 的原子发布、损坏回退、在线 SQLite 丢失重建和旧 JSON 一次性导入仍未完成，因此本阶段不能视为完成。

- SQLite 在线文件删除且早期 WAL 已回收时，仍能由 outbox snapshot加剩余 WAL 恢复 pending event。
- outbox snapshot 或 manifest checksum 损坏时回退到上一有效 checkpoint。
- 任意 checkpoint crash point 不会发布半套 snapshot，也不会提前删除 WAL。
- JSON 导入失败不切换格式，成功后重启不会重复导入。

## 6. 阶段 4：OrderedKeyCodec、typed predicate 与统一 cursor（未开始）

### 修改

- 实现带版本的 `OrderedKeyCodec`，覆盖 null、bool、整数、浮点、decimal、bytes、UTF-8 binary collation、日期时间、升降序、null placement、复合 key、prefix upper bound 和 entity tie-breaker。
- planner 从 AST literal 到 `IndexPredicate` 始终保留 typed `Value`。
- storage cursor 返回 `IndexRow`，绑定 read timestamp 和 manifest handle，在 storage 内验证 index record 与实体版本。
- 实现 covering projection；`AllColumns` 仅代表 index key 与 included columns。

### 删除

- 删除 postcard 排序 key、字符串 predicate、扫描后 range/prefix 正确性过滤。
- 删除只返回 `Value` 的 index cursor和 query 层 stale-row 正确性过滤。
- 删除 `Option<String>` partition 和 vertex ID 字符串前缀过滤。

### 验收

- codec 字节序与语义顺序 property test 一致；encode/decode 和格式版本测试通过。
- 任意 read timestamp 的 index cursor 与同 snapshot 表扫描一致。
- covering 与回表的值、顺序、offset/limit 结果一致。

## 7. 阶段 5：edge index、included columns 与 generation rebuild（未开始）

### 修改

- vertex/edge 共用 index record、codec、cursor、GC 和 generation builder；`EdgeRef` 为 `(src, dst, edge_type, ranking)`。
- edge insert/delete/update 与表数据使用同一 MVCC timestamp 和 WAL transaction batch维护 index。
- 补齐 edge metadata、DDL、schema/storage reader、physical source 和 optimizer capability。
- included column 更新创建新版本并 tombstone 旧版本。
- 实现 `Building(snapshot_ts,start_lsn) -> CatchingUp -> publish fence -> Active` rebuild 协议及崩溃恢复。

### 删除

- 删除 edge index placeholder、假成功 DDL、未实现 cursor 前的 optimizer 选择和内部 edge ID identity。

### 验收

- vertex/edge index 在任意 snapshot 与表扫描一致。
- build 期间持续写入，发布后不丢 `start_lsn` 之后的更新。
- build、catch-up 和 publish 各 crash point 均恢复到旧 generation 或完整新 generation。

## 8. 阶段 6：manifest shard、split 与安全回收（部分完成）

### 实现审查（2026-07）

已完成带格式版本的 immutable manifest 原子落盘、无界端点的半开 shard range、按 key 和 query range 路由、epoch 单调发布、cursor 可持有的引用计数 handle，以及 reader 释放后才返回待回收文件的安全 primitive。该 primitive 只返回文件清单，实际删除仍由 checkpoint owner 在持久 fence 后执行。

在线 split 尚不能启用。当前 rebuild 会清空 active B-tree 后直接从调用方提供的 `vertices`/`edges` 重建，只记录 `Building -> Active`，没有 `snapshot_ts/start_lsn`、增量 change log、barrier LSN、publish fence 或崩溃恢复。基于该路径实现 split 会在并发写下丢更新，也会让读者观察到构建中的不完整索引。必须先补齐阶段 5 的 generation rebuild 协议，再把 manifest catalog 接入真实 cursor 和 checkpoint 发布。

### 修改

- 实现单 shard immutable `IndexManifest`、结构化 `PartitionSelector` 和引用计数 manifest handle。
- 实现 `[lower,upper)` shard 路由与 predicate pruning。
- split 复用 rebuild 增量追平与 publish fence，按完整 ordered key 边界构建新 generation。
- 持久化 manifest epoch 与 generation state；只在没有 reader handle 后回收旧 B-tree、锁和 checkpoint 文件。

### 删除

- 删除 split 后旧 generation 的写入路径和任何字符串约定分区。

### 验收

- split 前后结果、全局顺序和 covering 结果一致；范围查询只打开相交 shard。
- split 并发写和 crash 测试不丢更新、不重复可见记录。
- 长读持有旧 manifest 时文件不回收，handle 释放后可回收。

## 9. 阶段 7：端到端收尾（部分完成）

### 当前边界（2026-07）

manifest catalog 已暴露 active epoch/generation、active reader、retired generation、publish 和 reclaim 计数，并覆盖 range routing、持久 roundtrip、未知版本拒绝、epoch 发布和长读回收 fence 单元测试。全阶段收尾仍被前序未完成项阻塞：prefix cursor 仍使用扫描后解码过滤，covering row 尚未生成，rebuild/split 没有 crash-safe 增量追平，因此不能删除相应兼容路径或宣称通过 split/recovery 演练。

### 修改

- 增加 WAL 截断、SQLite 重建、RPC timeout/late arrival、frontier hole、barrier、rebuild/split 和多 checkpoint 回收端到端测试。
- 增加 target/index frontier lag、backlog、oldest event、retry、dead-letter、degraded skip、fence failure、generation、transport latency、materializer和snapshot lag 指标。
- 更新中文设计和运维文档，记录恢复、死信处置和一致性等待行为。

### 删除

- 删除迁移 adapter、废弃配置、旧 JSON reader、旧测试 fixture 和不再成立的 benchmark。
- 使用 `rg` 清除 `PersistentOutbox`、direct sync、transaction sequence、postcard index key、stale-row query filter、字符串 partition 和 placeholder edge index。

### 验收

- `cargo fmt --check`；
- `cargo clippy --all-targets --all-features`；
- `cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant`；
- 相关单元、集成、故障和端到端测试全部通过；
- 至少完整演练一次 WAL/outbox snapshot 恢复和一次并发写下的 index split/recovery。

## 10. 阶段间约束

- 阶段 1 未完成，不删除现有可恢复投递路径；阶段 1 合并时必须一次切换为 WAL → SQLite → claim。
- 阶段 2 未完成，不对外承诺 `minimum_lsn`、exactly-once 或 generation 可用。
- 阶段 3 未完成，不得仅因在线 SQLite 已 materialize 就回收唯一 WAL 副本。
- 阶段 4 未完成，不启用依赖字节顺序的 range scan 或 shard pruning。
- 阶段 5 未完成，不允许 optimizer 选择 native edge index。
- 阶段 6 未完成，不暴露物理 partition selector，也不执行在线 split。
