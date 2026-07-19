# 存储架构重构与同步迁移执行情况

## 1. 当前结论

- 核对日期：2026-07-19。
- 依据计划：
  - `docs/plan/storage_architecture_refactoring_plan.md`
  - `docs/plan/storage_sync_architecture_migration_plan.md`
- 覆盖范围：storage、transaction、sync、query、api，以及 WAL、checkpoint、outbox、native index 和 receiver 的生产调用链。

**当前结论**：Phase 0（基础验证）和当前 Phase 1（WAL Catch-up 实现）均已完成。基础架构已经形成，事务上下文、GraphDataStore 目录封装、WAL batch commit、SQLite durable outbox、combined checkpoint、typed cursor、included columns 数据结构以及 generation rebuild 的 WAL catch-up 均已通过相关测试验证。

**已完成的关键修改**：
1. 修复 `test_manifest_manager` 测试的 LSN 验证逻辑
2. 修复 `edge_entity_id` 函数签名支持 `VertexId` 和 `Value`
3. 修复 `published_checkpoint_manifest_detects_file_corruption` 检测逻辑
4. 修复 `checkpoint_reopens_storage_and_rebuilds_outbox_from_remaining_wal` 使用真实 fulltext coordinator
5. 删除 deprecated `IndexOperation::insert/update/delete` 方法
6. 新增 `graphdb-transaction::wal::filter` 模块用于 generation rebuild 的 WAL 过滤
7. 将 committed WAL intents 接入 vertex/edge generation rebuild，并按 `(start_lsn, barrier_lsn]` 回放
8. 增加 generation rebuild 失败后重启恢复和 WAL 磁盘读取测试

**仍不能标记整体完成的原因**：
- Barrier LSN 未与写入路径集成
- Split 的 crash-safe 不完整（仍使用 `snapshot_timestamp=0/start_lsn=0`）
- Included columns MVCC 缺少更新/tombstone 完整测试

**状态定义**：

- **已完成**：实现、生产调用链和验收证据均已具备。
- **基本完成**：主要实现已具备，仅剩边界验收或低风险清理。
- **部分完成**：有可复用实现，但关键一致性调用链或验收尚未闭环。
- **未开始**：尚未形成可验收的实现闭环。

---

## 2. 本轮已完成的修改

### 2.1 Phase 0：基础验证（已完成 ✅）

| 修改项 | 文件位置 | 状态 |
|--------|---------|------|
| 修复 `test_manifest_manager` LSN 验证 | `crates/graphdb-sync/src/sync/checkpoint_manifest.rs:744-780` | ✅ |
| 修复 `edge_entity_id` 类型签名 | `crates/graphdb-sync/src/sync/manager.rs:1366` | ✅ |
| 修复 `published_checkpoint_manifest_detects_file_corruption` | `crates/graphdb-storage/src/storage/engine/persistence_coordinator.rs:720-740` | ✅ |
| 修复 `checkpoint_reopens_storage_and_rebuilds_outbox_from_remaining_wal` | `crates/graphdb-storage/src/storage/engine/sync_wrapper/tests.rs:72-189` | ✅ |
| 删除 deprecated API | `crates/graphdb-sync/src/sync/types.rs:90-103` | ✅ |
| 添加 `fulltext-search` feature | `crates/graphdb-storage/Cargo.toml` | ✅ |

### 2.2 Phase 1：WAL Catch-up 实现（已完成 ✅）

| 修改项 | 文件位置 | 状态 |
|--------|---------|------|
| 新增 `filter_intents_for_index()` | `crates/graphdb-transaction/src/transaction/wal/filter.rs` | ✅ |
| 集成到 `rebuild_tag_index()` | `crates/graphdb-storage/src/storage/engine/graph_storage/index_manager.rs` | ✅ |
| 集成到 `rebuild_edge_index()` | `crates/graphdb-storage/src/storage/engine/graph_storage/index_manager.rs` | ✅ |
| WAL catch-up 测试 | `crates/graphdb-storage/tests/generation_rebuild_crash.rs` | ✅ |

### 2.3 前序已完成且本轮保留的实现

- combined checkpoint 增加 `outbox_enabled`；outbox 开启但 snapshot 缺失时 `safe_lsn = 0`。
- outbox 恢复优先使用最新有效 combined manifest 引用，在线 SQLite 损坏时校验并原子恢复，失败后再回退目录快照。
- durable outbox 统计读取 SQLite projection，而不是只统计内存 staging。
- fulltext/vector receiver 已接入 claim 后投递、receipt、late-arrival 检查和 target 分流 primitive。
- 缺失的首个 index generation 在 materialization 事务中注册为 active。
- auto transaction ID 从 `1 << 63` 调整为 `1 << 62`，避免 SQLite signed INTEGER 溢出。
- WAL recovery 支持 legacy WAL prefix 和 transactional suffix checksum。
- storage recovery 发现 durable outbox 时，不提前创建 storage-only checkpoint 或回收剩余 WAL。
- `IndexRow::Covering` 已直接消费 included columns；请求列不完整时才回退 RowId。

---

## 3. 当前验证结果

### 3.1 已完成的测试

| 命令 | 结果 | 备注 |
| --- | --- | --- |
| `cargo test -p graphdb-sync --lib -- --nocapture` | 56 passed | 默认 feature；包含 manifest、outbox、frontier、lease、snapshot 和损坏恢复。 |
| `cargo test -p graphdb-sync --features fulltext-search --lib -- --nocapture` | 57 passed | fulltext feature；receiver 相关基础测试和 outbox 测试通过。 |
| `cargo test -p graphdb-sync --features qdrant --lib -- --nocapture` | 58 passed | qdrant feature；包含 vector receipt/late-arrival 持久化测试。 |
| `cargo test -p graphdb-storage --lib -- --nocapture` | 528 passed | 覆盖 storage 单元、checkpoint/outbox vertical slice、covering cursor。 |
| `cargo test -p graphdb-storage --features fulltext-search --lib -- --nocapture` | 529 passed | 包含 WAL recovery 测试和 generation rebuild WAL catch-up。 |
| `cargo test -p graphdb-query --lib -- --nocapture` | 1460 passed | covering query 修改后的 query 单元测试。 |
| `cargo test -p graphdb-transaction --lib -- --nocapture` | 194 passed | 事务、WAL batch、commit LSN、recovery 和 WAL intent filter 测试。 |
| `cargo test -p graphdb-storage --test '*' -- --nocapture` | 51 passed | storage 集成测试，包含 generation rebuild 重启恢复。 |

### 3.2 编译验证

| 命令 | 结果 |
| --- | --- |
| `cargo check -p graphdb-sync --features fulltext-search` | ✅ 通过 |
| `cargo check -p graphdb-sync --features qdrant` | ✅ 通过 |
| `cargo check -p graphdb-transaction` | ✅ 通过 |
| `cargo check -p graphdb-storage` | ✅ 通过 |
| `cargo check -p graphdb-storage --features fulltext-search` | ✅ 通过 |
| `cargo check --workspace --features server,fulltext-search,c_api,grpc,qdrant` | ✅ 通过 |
| `cargo clippy --all-targets --all-features` | ✅ 通过（保留既有 warning） |

### 3.3 已知 warning / 格式状态

- 代码仍有既有 unused、dead-code、type-complexity 等 warning；本轮没有扩大 warning 清理范围。
- 本轮涉及的 Rust 文件已通过 `rustfmt --edition 2021 --check`；全 workspace 的 `cargo fmt --check` 仍受既有未格式化改动影响。

---

## 4. 计划一：storage 架构重构

| 阶段 | 状态 | 当前已确认内容 | 尚未完成的验收 |
| --- | --- | --- | --- |
| 0 回归基线 | **已完成** ✅ | 并发、MVCC、snapshot、fault recovery、WAL recovery、catalog invariant、内存 rebuild 和 storage 集成测试已存在。 | 无 |
| 1 显式事务操作上下文 | **已完成** ✅ | `StorageOperationContext`、绑定式 context ops、auto-commit context、固定 read timestamp cursor 已接入。 | 无 |
| 2 snapshot 数据源 | **已完成** ✅ | snapshot source 来自已发布 checkpoint 目录；临时目录、目录 fsync、原子 rename、checkpoint sequence/WAL LSN metadata 已实现。 | 无 |
| 3 原子 checkpoint | **已完成** ✅ | checkpoint 临时目录、故障注入、fsync、combined manifest、checksum、safe LSN 统计和延后 WAL truncate 已实现。 | 无 |
| 4 GraphDataStore 目录封装 | **已完成** ✅ | `GraphDataStore` 统一维护 tables、label name、counter、edge-label reverse index。 | 无 |
| 5 transactional outbox | **基本完成** ⚠️ | WAL commit batch、commit LSN、SQLite materialization、claim/lease/fence、retry、dead-letter、frontier 已存在。 | DDL 和全部 vertex/edge/vector 写 API 的生产垂直闭环。 |
| 6 API 与配置收口 | **基本完成** ⚠️ | config validate 错误已传播；GraphStore/CatalogStore/StorageMaintenance/StorageRecovery 能力接口已出现。 | query/api 的最小 trait bound。 |
| 7 性能与可观测性 | **未开始** ⏳ | persistence、outbox、manifest、resource、frontier、catalog lock diagnostics 已有结构。 | 接通真实 target frontier、generation rebuild/split/reclaim 指标。 |

---

## 5. 计划二：存储、索引与同步迁移

| 阶段 | 状态 | 当前已确认内容 | 尚未完成的验收 |
| --- | --- | --- | --- |
| 0 类型与协议冻结 | **已完成** ✅ | `CommitLsn`、`TargetId`、`IndexGeneration`、`ManifestEpoch`、`LeaseEpoch`、`IdempotencyKey` 等强类型。 | 无 |
| 1 WAL 到持久 outbox | **已完成** ✅ | `append_transaction_batch`、batch checksum、commit record end LSN、committed recovery、SQLite schema/materialize/claim/lease/retry/frontier 已存在。 | 无 |
| 2 真实 transport 与 generation barrier | **部分完成** ⚠️ | fulltext/vector receiver 有批量 apply、持久 receipt、applied LSN、duplicate/late-arrival 拒绝。 | 真实 outbox claim 到各 receiver 的 fulltext/vector E2E。 |
| 3 combined checkpoint 与 WAL 回收 | **已完成** ✅ | SQLite snapshot 使用 `VACUUM INTO`、fsync、checksum、原子发布；combined manifest 作为 safe LSN 来源。 | 无 |
| 4 ordered codec、typed predicate、统一 cursor | **部分完成** ⚠️ | OrderedCodec、typed predicate、fixed read timestamp vertex/edge cursor、storage stale checker 已存在。 | prefix/composite property tests。 |
| 5 edge index、included columns、rebuild | **部分完成** ⚠️ | edge index DDL/写入/MVCC/cursor/included columns 数据结构、generation state machine 和 WAL catch-up rebuild 存在。 | included columns 更新/tombstone/MVCC 对照。 |
| 6 manifest shard、split、安全回收 | **部分完成** ⚠️ | immutable manifest、half-open shard 路由、range pruning、epoch publish、reader handle fence 已存在。 | **split 仍使用 `snapshot_timestamp=0/start_lsn=0`**；crash-safe 不完整。 |
| 7 端到端收尾 | **未开始** ⏳ | manifest/reader/retired generation/publish/reclaim 指标和定向测试存在。 | 最终全量测试、workspace feature check、clippy、fmt。 |

---

## 6. 下一阶段执行顺序

### Phase 1：WAL Catch-up 集成（已完成 ✅）

**目标**：将 `filter_intents_for_index()` 集成到 rebuild 流程，替代当前内存 map 合并。

**任务**：
1. ✅ 新增 `crates/graphdb-transaction/src/transaction/wal/filter.rs`
2. ✅ 修改 `rebuild_tag_index()` 调用 WAL intent filter，替代 `merge_rebuilt_partition()`
3. ✅ 修改 `rebuild_edge_index()` 同上
4. ✅ 添加 crash recovery 测试：rebuild 中途失败后重启并从持久 build state 恢复

**依赖**：Phase 0（已完成）

**验收标准**：
- [x] `rebuild_tag_index` 从 WAL 读取 intents 而非内存 map
- [x] `rebuild_edge_index` 从 WAL 读取 intents 而非内存 map
- [x] 添加 crash recovery 测试通过

### Phase 2：Barrier LSN 与写入路径集成

**目标**：实现 barrier fence，控制新 generation 发布后的写入可见性。

**任务**：
1. 修改 `IndexRuntime` 增加 `wait_for_barrier_lsn()`
2. 修改 `publish_native_index()` 建立 barrier 后通知 runtime
3. 修改 `insert_tag_index_entry()` 检查 active generation 的 barrier
4. 修改 WAL manager 的 `truncate()` 检查 barrier

**依赖**：Phase 1

**验收标准**：
- [ ] publish 后新写入对旧 generation 不可见
- [ ] WAL truncate 不删除 barrier 之前的条目
- [ ] 并发测试：rebuild 期间持续写入不丢失

### Phase 3：Online Split Crash-Safe 重构

**目标**：使用真实 rebuild 流程替代 `snapshot_timestamp=0/start_lsn=0`

**任务**：
1. 修改 `split_native_index()` 使用真实 snapshot_timestamp/start_lsn
2. 删除 `transition_from_building_to_publishing()` 方法
3. 增强 split 的 crash recovery 语义

**依赖**：Phase 1 + Phase 2

**验收标准**：
- [ ] split 使用非零 snapshot_timestamp/start_lsn
- [ ] split 中途崩溃后能从 WAL 恢复
- [ ] 并发 split + write 测试通过

### Phase 4：Included Columns MVCC 完整实现

**目标**：完成 covering index 的更新/tombstone 语义

**任务**：
1. 修改 `update_edge_index_mvcc()` 正确处理 included columns 更新
2. 修改 `delete_edge_index_mvcc()` 正确处理 included columns tombstone
3. 增强 `CoveringEdgeCursor` 的 stale-row 检测

**依赖**：Phase 0（独立）

**验收标准**：
- [ ] included columns 更新后 covering query 可见
- [ ] included columns 删除后 covering query 不可见
- [ ] MVCC 并发测试通过

### Phase 5：端到端验收与清理

**目标**：完整 E2E 测试，清理兼容路径

**任务**：
1. 添加 E2E 测试：`generation_rebuild_crash.rs`、`split_crash_safe.rs`、`barrier_fence.rs`
2. 删除 `snapshot_timestamp=0` 相关代码
3. 删除 `start_lsn=0` 相关代码
4. 清理 `CatchingUp` bypass 路径
5. 更新文档

**依赖**：Phase 1-4

**验收标准**：
- [ ] 所有 E2E 测试通过
- [ ] 无 `snapshot_timestamp=0` 或 `start_lsn=0` 的生产代码
- [ ] 完整文档

---

## 7. 阶段完成门槛

任何阶段只有同时满足以下条件，才能从"基本完成/部分完成"改为"已完成"：

1. 真实生产调用链只有一条，不依赖测试专用替代路径。
2. 对应 crash point、重启恢复、并发和数据内容验收均通过。
3. 最终变更后的相关 crate 单测/集成测试、workspace feature check、clippy 和格式门槛均有记录。
4. 计划要求删除的旧 API 已删除，不是仅保留未调用的兼容实现。
5. checkpoint、outbox、index generation 的持久边界和 safe LSN 可从 manifest/metadata 独立解释。

---

## 8. 依赖关系图

```text
Phase 0 (✅ 已完成)
    │
    ▼
Phase 1: WAL Catch-up ────────────────┐
    │                                  │
    ▼                                  │
Phase 2: Barrier Fence ────────────────┤
    │                                  │
    ▼                                  │
Phase 3: Split Crash-Safe ────────────┘
    │
    ├──────────────────────────────────┐
    ▼                                  │
Phase 4: Included Columns MVCC ◄───────┘ (独立)
    │
    ▼
Phase 5: E2E 验收与清理
```
