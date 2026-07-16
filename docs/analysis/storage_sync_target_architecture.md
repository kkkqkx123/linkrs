# 存储、索引与同步的目标架构

> 文档状态（2026-07）：目标架构已完成设计冻结；阶段 0 已完成，阶段 1 的本地提交闭环已接入真实写路径，阶段 3 已具备 SQLite 快照和共同 safe LSN 的基础能力；ordered codec、typed cursor 和 edge index 已有部分实现，manifest shard 路由与 reader 回收 fence 已落地 primitive。本文其余章节描述目标行为，不代表 transport、generation rebuild 或在线 shard split 已经完成。

## 1. 目标与不变量

系统以 WAL 中完成持久化的事务提交记录作为唯一提交事实，并由这份事实驱动本地数据、native index 和外部索引。必须始终满足以下不变量：

1. 数据 redo、native-index redo 和外部索引 intent 同属一个事务；只有完整且已持久化的 commit record 才使它们可见。
2. 本地提交不依赖 fulltext 或 vector 服务可用，外部 target 通过持久 outbox 最终追赶。
3. 每个 target 独立 claim、确认、重试和记录连续进度；一个 target 的结果不能改变另一个 target 的状态。
4. native index record 与表数据使用同一 MVCC write timestamp；storage cursor 在绑定的 read timestamp 下只返回正确版本。
5. WAL、outbox event、idempotency receipt 和 index generation 只能依据持久化边界回收。
6. 任意已经允许 WAL 回收的 pending intent，仍至少存在于一个可校验、可恢复的 outbox snapshot 中。
7. generation 构建、切换和删除都有持久状态机及 fence；固定 snapshot 构建期间发生的写入不能丢失。

`CommitLsn` 是提交事实、outbox、checkpoint 和外部投递进度的统一持久顺序轴，但不是系统中唯一的版本类型。`MvccTimestamp`、`SchemaVersion`、`IndexGeneration`、`ManifestEpoch` 和 `LeaseEpoch` 保持独立类型，并通过明确字段与 `CommitLsn` 关联，禁止相互替代。

## 2. 总体拓扑

```text
write transaction / auto-commit
        |
        | one WAL append batch + durability fence
        v
data redo + native-index redo + target intents + transaction commit
        |                                      |
        | committed replay                     | committed replay
        v                                      v
storage/native-index recovery          SQLite outbox projection
                                               |
                                      generation gate + claim
                                               |
                               +---------------+---------------+
                               |                               |
                       fulltext receiver                vector receiver
                       receipt + apply                  monotonic apply
                               |                               |
                               +---------------+---------------+
                                               |
                                    contiguous target frontier

checkpoint publication
        = storage snapshot
        + outbox snapshot
        + native-index manifests
        + atomic checkpoint manifest
```

写路径不允许在数据写入成功后临时生成同步事件。显式事务与自动提交都分配内部 `TransactionId`，在同一个 schema snapshot 下展开本地修改和目标专属 intent，并走同一提交协议。

## 3. WAL 提交协议

### 3.1 稳定记录

WAL wire type 位于 `graphdb-core`，不能引用 SQLite、RPC client、Qdrant client 或进程内 coordinator 类型：

```text
OutboxIntent {
    transaction_id,
    intent_sequence,
    target,
    index_id,
    index_generation,
    mutation_bytes,
    idempotency_key,
    ordering_key
}

TransactionCommit {
    transaction_id,
    intent_count,
    batch_checksum
}

TransactionAbort {
    transaction_id
}
```

规范 `CommitLsn` 是 `TransactionCommit` WAL record 的结束 LSN，由 WAL writer 在持有 append 锁时写入 header。payload 不重复保存自己尚未分配的 LSN。parser 从 header 得到 `CommitLsn`，并将它附加到恢复后的 redo 和 intent。

`mutation_bytes` 是在事务 schema snapshot 下展开的目标专属 mutation。历史提交不能在投递时重新查询当前 schema 或 index generation 来改变含义。

### 3.2 提交与可见性发布

一个事务使用专用的 `append_transaction_batch` 写入：

1. data redo；
2. native-index redo；
3. 序号从零连续排列的零个或多个 `OutboxIntent`；
4. 最后的 `TransactionCommit`。

writer 在锁内完成 LSN 分配、batch checksum、写入和 durability fence。group commit 可以合并 fsync，但每个事务只有在 `durable_lsn >= commit_lsn` 后才能发布 MVCC 可见性并向调用者返回成功。任何错误都不能让 API 返回一个未持久化的 `CommitLsn`。

恢复器只有在以下条件同时成立时才接受事务：commit record 完整、batch checksum 正确、intent 数量相等、intent sequence 连续。未提交 redo 不能成为恢复后可见数据。`TransactionAbort` 只用于显式终止和提前释放恢复状态，不是可见性来源。

## 4. Outbox 投影、checkpoint 与恢复

### 4.1 SQLite 数据模型

SQLite 开启 WAL 和 full synchronous，作为 WAL committed intent 的可恢复投影。至少包含：

- `events`：target、index/generation、mutation、commit LSN、ordering key、状态、重试和 lease；
- `commit_targets`：每个 committed LSN 对每个 target 的事件总数、已应用数和终结状态；
- `idempotency`：`(target, idempotency_key)` 唯一约束及回收 LSN；
- `projection_state`：`materialized_lsn`、source checkpoint 和 schema version；
- `target_state`：连续 `applied_lsn`、运行状态和最后错误；
- `generation_state`：index lifecycle 状态、barrier LSN 和 generation gate；
- `dead_letters`：不可恢复错误及人工处置记录；
- `schema_migrations`：数据库结构迁移状态。

materializer 按 commit 边界工作，在一个 SQLite 事务中插入该 commit 的全部事件和 target ledger，再推进 `materialized_lsn`。重复 replay 由 commit LSN 和 idempotency 唯一约束消除。没有某 target mutation 的 commit 也能通过 ledger 明确跨越，不能依靠猜测跳过 LSN 空洞。

### 4.2 claim、顺序与 fencing

claim 使用 `BEGIN IMMEDIATE`。一个事件只有满足以下条件才能被领取：

1. 它是该 ordering key 最早的未终结事件；头事件处于 backoff 时，后续事件也必须等待；
2. 对应 generation gate 为 `Active`，或它本身是当前允许执行的 lifecycle event；
3. 未被有效 lease 持有，且 `next_attempt_at` 已到达。

claim 写入 `lease_owner`、`lease_until` 和递增的 `lease_epoch`。ack、retry、dead-letter 和 requeue 都必须携带 `(event_id, lease_owner, lease_epoch)` 条件。lease fencing 只保护本地队列状态，不能替代远端幂等或单调版本检查。

系统只有 claim 后发送这一条投递路径，不保留 direct send、发送后补 claim 或按 transaction 一次确认全部 target 的旁路。

### 4.3 outbox snapshot 与 WAL 回收

在线 SQLite 文件不是允许 WAL 回收的唯一副本。每次 storage checkpoint 按以下顺序发布：

1. 选择 `safe_lsn`，等待 SQLite `materialized_lsn >= safe_lsn`；
2. 通过 SQLite backup API 生成一致、不可变的 outbox snapshot，并 fsync 文件和目录；
3. 写入 storage/native-index snapshot 和 manifests；
4. 原子发布 checkpoint manifest，记录 storage LSN、outbox snapshot LSN、文件名、大小和 checksum；
5. 只有 manifest 发布成功后，才删除不晚于 `safe_lsn` 的 WAL segment 和更旧但不再引用的 snapshot。

在线 SQLite 丢失时，从最近有效 outbox snapshot 恢复，再 replay 该 snapshot LSN 之后的 committed WAL intent。这样即使 target 尚未确认，WAL 也可回收而 pending event 不会丢失。

outbox event 和 idempotency receipt 的回收还要求：对应 target 连续 frontier 已越过 event LSN、没有未完成 lifecycle barrier、至少一个仍保留的 checkpoint/outbox snapshot 不再依赖该记录。dead-letter 默认不是成功应用，不能自动推进 frontier 或触发回收。

## 5. Target mutation、barrier 与连续 watermark

### 5.1 Mutation

```text
IndexMutation {
    target,
    index_id,
    index_generation,
    entity_ref,
    operation: Upsert | Delete,
    document_or_vector,
    commit_lsn,
    idempotency_key,
    ordering_key
}
```

同一 `(target, index_id, entity_ref)` 严格顺序投递，不同 key 可以并发。index lifecycle 使用 `(target, index_id)` 级持久状态机：

```text
Creating -> Backfilling -> CatchingUp -> Active -> Draining -> Dropped
                               |                       |
                               +------ Failed --------+
```

generation 未 `Active` 时，其数据 mutation 不可 claim。切换时先建立 barrier，追平到 barrier LSN，再原子切换 active generation。drop 先进入 `Draining`，等待或明确废弃所有前序事件，再删除远端资源。

### 5.2 无洞进度

`target_state.applied_lsn` 定义为该 target 已完成的最大连续 commit 边界，不是最大成功 event LSN。ack 先更新 event 和 `commit_targets.applied_count`，随后循环推进所有已经完整终结的连续 commit。不同 index 需要独立 read-your-writes 时，额外维护 `(target, index_id, generation)` frontier；target 总 frontier 仅用于全 target 管理和回收。

dead-letter 会停止相关 frontier。管理员只能选择修复并 requeue，或执行显式 `Skip`；后者必须持久化一致性缺口并让等待接口返回 degraded 错误，不能伪装成正常 applied。

查询的 `minimum_lsn` 等待相应 index frontier；达到边界才执行，否则超时返回一致性等待错误。

## 6. Transport 正确性

fulltext receiver 必须在同一个原子持久化事务中应用 index mutation 和 target-local receipt，或使用等价的可证明恢复协议，再返回确认。

vector transport 不能仅依靠确定性 point ID。超时请求可能在新请求之后到达，因此必须满足单调 LSN 应用：

- 优先使用支持 compare-and-set/version precondition 的 receiver；
- 若 Qdrant 不能原子拒绝旧 LSN，则在其前面部署持久 receiver；
- 或采用 versioned point ID，并由查询只选择最大 LSN，delete 写入 versioned tombstone。

不具备单调保护时，只能声明 at-least-once 并依赖 reconciliation，不能声称 exactly-once 或 read-your-writes。所有 transport 都配置连接、写入、读取和一致性等待超时；调用超时只表示结果未知，必须以同一 idempotency key 重试。

## 7. Native index

### 7.1 OrderedKeyCodec

排序 key codec 与 payload serde 分离，格式带版本号。它必须定义 null、bool、有符号/无符号整数、浮点、decimal、bytes、UTF-8 字符串、日期时间、升降序、null placement、复合字段边界和 entity tie-breaker。

整数使用保持顺序的 big-endian 变换；浮点使用 IEEE total-order 变换并明确 NaN 策略；字符串依据固定 binary collation 使用可转义终止编码，并提供可计算 prefix upper bound。任何 locale collation 都必须拥有独立 codec/version，不能依赖运行环境。

### 7.2 MVCC record 与 cursor

index record 包含完整 ordered key、entity ref、MVCC create/delete timestamp 和 included columns。key 或 included column 更新创建新 record 并 tombstone 旧 record，二者和表数据使用同一 write timestamp及同一 WAL transaction batch。

storage cursor 创建时绑定 read timestamp 和 immutable manifest handle，负责 index record 可见性及实体版本一致性，返回：

```text
IndexRow =
    RowId(VertexId | EdgeRef)
    Covering { entity_ref, columns }
```

请求列完全被 key 与 included columns 覆盖时返回 `Covering`；否则返回已验证索引版本的 `RowId` 并统一回表。query 层回表可以取列，但不能作为过滤 stale index record 的正确性补丁。range/prefix scan 只能使用 codec 边界。

`EdgeRef` 使用 `(src, dst, edge_type, ranking)`。vertex 和 edge 共用 codec、MVCC record、cursor、GC、checkpoint 和 generation 基础设施。

## 8. Generation rebuild、shard split 与回收

每个 index 的 `IndexManifest` 不可变并带 `ManifestEpoch`。一个 generation 包含若干不重叠的 `[lower, upper)` shard，每个 shard 有独立 B-tree、锁和 checkpoint 文件。cursor 持有 manifest 的引用计数 handle；旧 generation 只有在 manifest 已安全发布且没有 reader handle 后才能删除。

rebuild 和 split 使用同一追平协议：

1. 持久化 `Building { snapshot_ts, start_lsn }`；
2. 从固定 snapshot 构建新 generation；
3. 从 WAL/index change log 回放 `start_lsn` 之后的增量；
4. 建立短暂 publish fence，确定 `barrier_lsn`；
5. 追平到 barrier，fsync 新 generation 和 manifest；
6. 原子发布新 manifest，再解除 fence；
7. fence 后写入只路由到新 generation；旧 generation 只服务已捕获的 reader。

如果采用构建期间双写，仍必须持久化双写开始 LSN、处理崩溃恢复，并在发布前验证两代已追平。不能只基于固定 snapshot 构建后直接切换。

## 9. 模块边界

- `graphdb-core`：稳定 ID、LSN、entity ref、WAL wire type、target mutation wire type；
- `graphdb-transaction`：WAL append batch、durability fence、commit/abort parser 和 committed recovery；
- `graphdb-storage`：schema snapshot 下展开 intent、本地数据/native index、checkpoint 编排和 committed replay；
- `graphdb-sync`：SQLite projection、snapshot、claim、dispatcher、transport、frontier 和观测；
- `graphdb-query`：typed predicate、`IndexRow` 消费和最小 frontier 等待。

为保持依赖 DAG，`graphdb-sync` 不解析 `graphdb-transaction` 的内部 recovery type。storage/recovery 将 `graphdb-core` 中的 committed intent 通过抽象投影接口推给 sync。transaction crate 不依赖 SQLite、coordinator 或 transport client。

## 10. 观测与禁止路径

持久指标至少包括：每 target/index frontier lag、backlog、最老事件年龄、retry、dead-letter、degraded skip、lease fence failure、generation 状态、transport latency、materializer lag、snapshot LSN 和最后成功 checkpoint。管理接口读取持久状态，不以 coordinator 内存值冒充恢复边界。

迁移完成后禁止保留：

- 数据写成功后 best-effort enqueue；
- JSON 或内存 coordinator 作为 committed intent 权威来源；
- 投递时根据当前 schema 临时推导 target mutation；
- target 共享 ack 或最大成功 LSN watermark；
- direct send 与 claim send 双路径；
- 无 generation gate 的 lifecycle event；
- 仅靠确定性 point ID 防止旧写覆盖新写；
- postcard 排序 key、字符串化 predicate、扫描后正确性过滤；
- `Option<String>` 分区选择或 vertex ID 字符串前缀伪分区；
- 未经增量追平直接发布 rebuild/split generation。

## 11. 当前实现进度与边界

### 11.1 已落地能力

- `graphdb-core` 已提供 `CommitLsn`、target/generation/lease/order/idempotency 等强类型，以及 WAL intent、commit、abort 和 index mutation wire type。
- `graphdb-transaction` 已实现事务批量追加、batch checksum、commit record end LSN、durability fence 和 committed-only recovery。WAL writer 在创建新 segment 时延续逻辑 LSN，避免重启后跨文件乱序。
- 显式事务和自动提交都通过内部 transaction ID 进入同一个 commit sink。GraphStorage 不再抢先提交带有同步 intent 的自动事务。
- `graphdb-sync` 已建立 SQLite outbox schema，支持 generation gate、ordering key head-of-line、lease fencing、ack/retry/dead-letter、幂等 materialize 和无洞 target frontier。
- 写路径的 intent 先暂存于事务内存状态，commit 后再由 WAL committed fact 投影到 SQLite；dispatcher 只从 SQLite claim 后投递。
- SQLite 已提供 `VACUUM INTO` 一致快照、文件 fsync、checksum 校验和 verified restore primitive。
- checkpoint WAL 截断已延后到快照发布后，并在存在 outbox marker 时取 storage LSN 与 outbox safe LSN 的共同边界。

### 11.2 尚未完成的目标能力

- 旧 `PersistentOutbox` 类型和少量兼容 API 仍存在，尚未完成一次性 JSON importer、迁移标记和最终删除。
- fulltext/vector 仍使用现有 coordinator/transport 适配层，尚未实现远端 receipt 与 mutation 的同事务持久化，也未完成 vector 的持久化单调 LSN 拒绝协议。
- checkpoint manifest 尚未同时原子引用 storage snapshot、outbox snapshot 和 native-index manifests；SQLite 丢失后的 snapshot 加剩余 WAL 自动重建流程仍需接入启动恢复。
- `minimum_lsn` 等待接口、显式 degraded skip、完整 lifecycle barrier 和 target/index 双层 frontier 尚未对外稳定开放。
- `OrderedKeyCodec`、typed predicate、`IndexRow` cursor 和 edge native index 已有实现，但 prefix 仍存在扫描后过滤，covering record 与 storage 内实体版本校验尚未闭环。
- immutable manifest、结构化 shard range、路由/pruning、引用计数 handle 和延迟回收 primitive 已实现；尚未接入物理 cursor/checkpoint，在线 split 仍等待 crash-safe rebuild/catch-up/publish fence。

因此，当前实现可以声明“本地 WAL → SQLite outbox → claim 的 at-least-once 基础闭环”，不能声明所有 target 的 exactly-once、在线 rebuild/split 安全发布或完整 read-your-writes guarantee。
