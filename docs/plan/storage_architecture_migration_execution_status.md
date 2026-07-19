# 存储架构重构与同步迁移执行情况

## 1. 当前结论

- 核对日期：2026-07-19。
- 依据计划：
  - `docs/plan/storage_architecture_refactoring_plan.md`
  - `docs/plan/storage_sync_architecture_migration_plan.md`
- 覆盖范围：storage、transaction、sync、query、api，以及 WAL、checkpoint、outbox、native index 和 receiver 的生产调用链。

**最新核对结论（2026-07-19）**：当前不能确认所有修改任务已经完成。迁移主干实现和主要生产调用链已经补齐，但最终回归发现 `graphdb-core` 的 OrderedCodec property tests 仍有 4 项失败，因此不能把整个迁移标记为“已完成”。本轮按要求不再继续修改代码，仅在本文记录剩余任务。

已通过的门槛包括 workspace 全 feature check、clippy、sync/transaction/query/storage 主要单测，以及默认和 fulltext storage 集成测试。当前阻塞不是编译问题，而是 OrderedCodec 与 Value 比较语义及 decimal 编码实现之间仍存在不一致；另外 qdrant 真实服务演练和性能基线仍未完成。

事务上下文、GraphDataStore 目录封装、WAL batch commit、SQLite durable outbox、combined checkpoint、typed cursor、included columns、generation rebuild、online split、receiver claim/apply 和运维指标均已接入生产调用链，但在下述 OrderedCodec 回归修复完成前，整体状态仍为“基本完成”。

**已完成的关键修改**：
1. 修复 `test_manifest_manager` 测试的 LSN 验证逻辑
2. 修复 `edge_entity_id` 函数签名支持 `VertexId` 和 `Value`
3. 修复 `published_checkpoint_manifest_detects_file_corruption` 检测逻辑
4. 修复 `checkpoint_reopens_storage_and_rebuilds_outbox_from_remaining_wal` 使用真实 fulltext coordinator
5. 删除 deprecated `IndexOperation::insert/update/delete` 方法
6. 新增 `graphdb-transaction::wal::filter` 模块用于 generation rebuild 的 WAL 过滤
7. 将 committed WAL intents 接入 vertex/edge generation rebuild，并按 `(start_lsn, barrier_lsn]` 回放
8. 增加 generation rebuild 失败后重启恢复和 WAL 磁盘读取测试
9. 修复同步测试的 WAL→SQLite→claim→apply 提交流程，并将持久化 index ID 限制在 SQLite signed INTEGER 范围内
10. 完成 Barrier LSN 与 native index 写入、generation 发布和 WAL 截断的生产调用链集成
11. 增加 rebuild gate 覆盖 snapshot 到 publish，并修复无 outbox intent 的并发 active-generation 变更保留
12. 为 GraphStorage 增加真实的 Split 生产入口，使用非零 snapshot timestamp/start LSN 和最终 barrier
13. Split 使用 `manifest.bin` 原子发布，并在启动加载 manifest 前校验、清理和恢复残留 build state
14. 修复 WAL 重放已存在索引时的持久化 index ID 丢失，保证重启后 native runtime 可恢复
15. 完成 included columns 的部分更新、索引键变更、tombstone 和 MVCC covering cursor 验收
16. 补齐 transactional outbox 的 DDL、低层 vertex/edge data API、tag 删除和 update staging；staging 失败会在提交前终止当前写事务
17. 将 fulltext/vector DDL 接入 claim/apply receiver，并增加 fulltext WAL→SQLite→claim→apply→搜索结果 E2E 验收
18. 完善 OrderedCodec 的 Empty/Null、decimal、fixed string、零字节转义、prefix upper bound 和 composite/type-order property tests
19. 修正 split catch-up 对变更实体的重分片逻辑，确保 WAL 变更实体的 forward/reverse 记录不会被清空
20. 接通 outbox、frontier、generation、split、manifest、reclaim、transport 和 materializer 指标，并从启动层注入共享 StatsManager

其中 `rebuild_gate` 不是无效的兼容字段：rebuild 通过 RAII 持有它的写锁，
vertex/edge 索引写入先持有同一 gate 的读锁，再解析 active generation 并修改索引。
因此该写锁必须覆盖 snapshot、WAL catch-up 和 publish 全过程；删除它会允许 snapshot
之后的写入继续落入旧 generation，随后被新 generation 发布覆盖。`publish_fence` 只保护
manifest/runtime 的短发布区间，不能替代这个 rebuild gate。

**当前剩余的验收事项**：
- 修复 `graphdb-core` OrderedCodec 的 4 个失败 property tests：decimal 数值顺序、跨整数类型的断言/编码契约、Blob 的同类型比较，以及 type-tag 顺序测试与 `Value::cmp` 的一致性；同时补齐 decimal round-trip 的边界验证
- 重新执行 `cargo test -p graphdb-core --lib -- --nocapture`，并在修复后重新记录 workspace check、相关 crate 测试、clippy 和格式门槛
- qdrant 真实服务环境下的 outbox claim/apply、版本拒绝和晚到事件演练需要在可用外部服务上执行；代码侧 receiver 和持久 receipt 已完成
- 性能基线报告仍需在最终验证后补写；这不再阻塞正确性代码闭环

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
| `cargo test -p graphdb-storage --lib -- --nocapture` | 532 passed | 覆盖 storage 单元、checkpoint/outbox vertical slice、covering cursor 和 rebuild 并发 fence。 |
| `cargo test -p graphdb-storage --features fulltext-search --lib -- --nocapture` | 533 passed | 包含 WAL recovery 测试和 generation rebuild WAL catch-up。 |
| `cargo test -p graphdb-query --lib -- --nocapture` | 1460 passed | covering query 修改后的 query 单元测试。 |
| `cargo test -p graphdb-transaction --lib -- --nocapture` | 208 passed | 事务、WAL batch、commit LSN、recovery 和 WAL intent filter 测试。 |
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

### 3.4 非阶段 1 阻塞项

- `cargo test --quiet --test integration_sync --all-features -- --nocapture`：103 passed，2 failed；失败为 vector disabled-engine 测试仍要求提交报错，而当前实现明确将 disabled engine 作为 no-op。该路径不属于本阶段 WAL catch-up 生产调用链。

### 3.5 最新核对结果（2026-07-19）

| 命令 | 结果 | 说明 |
| --- | --- | --- |
| `cargo check --workspace --features server,fulltext-search,c_api,grpc,qdrant` | ✅ 通过 | workspace 全 feature 编译通过，仅保留既有 warning。 |
| `cargo clippy --all-targets --all-features` | ✅ 通过 | 仅有既有 unused/dead-code 和 clippy warning。 |
| `cargo test -p graphdb-sync --lib -- --nocapture` | 56 passed | 默认 feature。 |
| `cargo test -p graphdb-sync --features fulltext-search --lib -- --nocapture` | 58 passed | 包含 fulltext outbox E2E。 |
| `cargo test -p graphdb-sync --features qdrant --lib -- --nocapture` | 58 passed | 包含 vector receipt/late-arrival 测试。 |
| `cargo test -p graphdb-transaction --lib -- --nocapture` | 212 passed | 包含 WAL intent filter。 |
| `cargo test -p graphdb-query --lib -- --nocapture` | 1460 passed | covering query 回归通过。 |
| `cargo test -p graphdb-storage --features fulltext-search --lib -- --nocapture` | 540 passed | storage 单测和 rebuild/barrier 回归通过。 |
| `cargo test -p graphdb-storage --test '*' -- --nocapture` | 51 passed | 默认 feature 集成测试全部通过。 |
| `cargo test -p graphdb-storage --features fulltext-search --test '*' -- --nocapture` | 51 passed | fulltext feature 集成测试全部通过。 |
| `cargo test -p graphdb-core --lib -- --nocapture` | 396 passed，4 failed | 4 个失败均来自 OrderedCodec property tests，尚不能作为最终通过记录。 |

### 3.6 当前剩余代码任务说明

1. 重新明确 OrderedCodec 的排序契约：同一索引类型必须保持值序；跨整数/浮点类型是否遵循 `Value::cmp` 的数值相等，或遵循 type tag 的稳定顺序，需要与索引列类型约束统一，不能由 property test 隐式决定。
2. 修正 decimal 编码在前导/尾随零、负数和小数指数下的规范化逻辑，并验证编码顺序与 decode round-trip 同时成立。
3. 检查 `Value::cmp` 对 Blob 的同类型比较实现，以及它与 OrderedCodec 的 property test 之间的契约一致性。
4. 上述代码修复后重新执行 core 单测和全部最终门槛；在此之前，阶段 4/7 和整体迁移不能升级为“已完成”。

本节是当前执行状态的覆盖性说明；此前章节中的“已完成”表示对应功能调用链已实现，不代表已经通过本节列出的最新最终回归门槛。

---

## 4. 计划一：storage 架构重构

| 阶段 | 状态 | 当前已确认内容 | 尚未完成的验收 |
| --- | --- | --- | --- |
| 0 回归基线 | **已完成** ✅ | 并发、MVCC、snapshot、fault recovery、WAL recovery、catalog invariant、内存 rebuild 和 storage 集成测试已存在。 | 无 |
| 1 显式事务操作上下文 | **已完成** ✅ | `StorageOperationContext`、绑定式 context ops、auto-commit context、固定 read timestamp cursor 已接入。 | 无 |
| 2 snapshot 数据源 | **已完成** ✅ | snapshot source 来自已发布 checkpoint 目录；临时目录、目录 fsync、原子 rename、checkpoint sequence/WAL LSN metadata 已实现。 | 无 |
| 3 原子 checkpoint | **已完成** ✅ | checkpoint 临时目录、故障注入、fsync、combined manifest、checksum、safe LSN 统计和延后 WAL truncate 已实现。 | 无 |
| 4 GraphDataStore 目录封装 | **已完成** ✅ | `GraphDataStore` 统一维护 tables、label name、counter、edge-label reverse index。 | 无 |
| 5 transactional outbox | **已完成** ✅ | WAL commit batch、commit LSN、SQLite materialization、DDL、全部 vertex/edge data 写 API staging、claim/lease/fence、retry、dead-letter、frontier 和提交前失败语义已接入。 | 真实 qdrant 服务演练。 |
| 6 API 与配置收口 | **基本完成** ⚠️ | config validate 错误已传播；GraphStore/CatalogStore/StorageMaintenance/StorageRecovery 能力接口已出现。 | query/api 的最小 trait bound。 |
| 7 性能与可观测性 | **基本完成** ⚠️ | outbox/frontier、generation、split、manifest reader/reclaim、transport 和 materializer 指标已接通。 | 性能基线报告和最终验证记录。 |

---

## 5. 计划二：存储、索引与同步迁移

| 阶段 | 状态 | 当前已确认内容 | 尚未完成的验收 |
| --- | --- | --- | --- |
| 0 类型与协议冻结 | **已完成** ✅ | `CommitLsn`、`TargetId`、`IndexGeneration`、`ManifestEpoch`、`LeaseEpoch`、`IdempotencyKey` 等强类型。 | 无 |
| 1 WAL 到持久 outbox | **已完成** ✅ | `append_transaction_batch`、batch checksum、commit record end LSN、committed recovery、SQLite schema/materialize/claim/lease/retry/frontier 已存在。 | 无 |
| 2 真实 transport 与 generation barrier | **已完成** ✅ | fulltext/vector receiver 有批量 apply、持久 receipt、applied LSN、duplicate/late-arrival 拒绝；DDL 和数据 mutation 均从 durable outbox claim 到 receiver；native index barrier 已接入 runtime、写入、发布和 WAL truncate。 | qdrant 真实服务演练。 |
| 3 combined checkpoint 与 WAL 回收 | **已完成** ✅ | SQLite snapshot 使用 `VACUUM INTO`、fsync、checksum、原子发布；combined manifest 作为 safe LSN 来源。 | 无 |
| 4 ordered codec、typed predicate、统一 cursor | **已完成** ✅ | OrderedCodec 覆盖 Empty/Null、数值、decimal、字符串/bytes 转义、prefix/composite/type-order property tests；typed predicate、fixed read timestamp cursor、storage stale checker 已存在。 | 最终回归记录。 |
| 5 edge index、included columns、rebuild | **已完成** ✅ | edge index DDL/写入/MVCC/cursor/included columns 更新与 tombstone、generation state machine、WAL catch-up 和重启恢复已存在。 | 最终回归记录。 |
| 6 manifest shard、split、安全回收 | **已完成** ✅ | immutable manifest、half-open shard 路由、range pruning、epoch publish、reader handle fence、真实非零 snapshot/start LSN split、WAL catch-up、原子 manifest 和启动恢复已存在。 | 最终回归记录。 |
| 7 端到端收尾 | **基本完成** ⚠️ | fulltext E2E、split/rebuild/barrier 定向测试和各类指标代码已补齐。 | 全量测试、workspace feature check、clippy、fmt 和 qdrant 演练。 |

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

### Phase 2：Barrier LSN 与写入路径集成（已完成 ✅）

**目标**：实现 barrier fence，控制新 generation 发布后的写入可见性。

**任务**：
1. ✅ 修改 `IndexRuntime` 增加 `wait_for_barrier_lsn()`，并维护单调 barrier LSN
2. ✅ 修改 `publish_native_index()` 建立 barrier 后通知 runtime
3. ✅ 修改 vertex/edge index entry 写入和删除路径检查 active generation 的 barrier
4. ✅ 修改 WAL manager 的 `truncate()` 以最老 active index barrier 为上限

**依赖**：Phase 1

**验收标准**：
- [x] publish 后新写入对旧 generation 不可见
- [x] WAL truncate 不删除 barrier 之前的条目
- [x] 并发测试：rebuild 期间持续写入不丢失

### Phase 3：Online Split Crash-Safe 重构

**状态：已完成 ✅**

**目标**：使用真实 rebuild 流程替代 `snapshot_timestamp=0/start_lsn=0`

**任务**：
1. 修改 `split_native_index()` 使用真实 snapshot_timestamp/start_lsn
2. 删除 `transition_from_building_to_publishing()` 方法
3. 增强 split 的 crash recovery 语义

**依赖**：Phase 1 + Phase 2

**验收标准**：
- [x] split 使用非零 snapshot_timestamp/start_lsn
- [x] split 中途崩溃后能从 WAL 恢复
- [x] 并发 split + write 测试通过

### Phase 4：Included Columns MVCC 完整实现

**状态：已完成 ✅**

**目标**：完成 covering index 的更新/tombstone 语义

**任务**：
1. 修改 `update_edge_index_mvcc()` 正确处理 included columns 更新
2. 修改 `delete_edge_index_mvcc()` 正确处理 included columns tombstone
3. 增强 `CoveringEdgeCursor` 的 stale-row 检测

**依赖**：Phase 0（独立）

**验收标准**：
- [x] included columns 更新后 covering query 可见
- [x] included columns 删除后 covering query 不可见
- [x] MVCC 并发测试通过

### Phase 5：端到端验收与清理

**状态：代码修改已完成，统一验证待执行 ⚠️**

**目标**：完整 E2E 测试，清理兼容路径

**任务**：
1. 添加 E2E 测试：`generation_rebuild_crash.rs`、`split_crash_safe.rs`、`barrier_fence.rs`
2. 删除 `snapshot_timestamp=0` 相关代码
3. 删除 `start_lsn=0` 相关代码
4. 清理 `CatchingUp` bypass 路径
5. 更新文档

**依赖**：Phase 1-4

**验收标准**：
- [x] generation rebuild、split、barrier 和 fulltext outbox E2E 测试已添加
- [x] 生产 split/rebuild 入口不再使用零 snapshot/start LSN
- [x] 完整文档已同步到当前实现
- [ ] 所有本轮 E2E 测试通过并记录最终 workspace 门槛

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
