# Storage 架构分阶段修改方案

## 1. 目标

本计划用于解决 `graphdb-storage` 当前的事务上下文、checkpoint/snapshot、数据目录锁管理、外部索引同步和公开 API 边界问题。

总体目标：

- 消除并发请求之间的事务上下文串扰。
- 使 checkpoint 和 snapshot 具备明确且可验证的一致性语义。
- 将跨表目录不变量和锁顺序收口到统一组件。
- 保证本地数据与外部索引事件最终一致。
- 缩小 storage 对外稳定接口。
- 保留现有 CSR、列存、编码、MVCC 和 compaction 实现。

本项目处于开发阶段，不要求保留旧接口兼容层。每个阶段完成后应直接删除被替代的接口和路径，避免长期双轨实现。

## 2. 实施原则

1. 正确性优先于性能优化。
2. 每个阶段先补失败测试，再修改实现。
3. 不同时大规模重写物理表和引擎编排层。
4. WAL、checkpoint 和 snapshot 的格式变更必须有明确版本。
5. 不通过线程局部变量保存事务上下文。
6. 不在多个模块中直接组合 `GraphDataStore` 的原始锁。
7. 每阶段完成后运行 storage 单元测试、集成测试和 workspace compile check。

## 3. 阶段概览

| 阶段 | 主题 | 优先级 | 前置条件 |
| --- | --- | --- | --- |
| 0 | 建立回归测试和架构基线 | P0 | 无 |
| 1 | 显式事务操作上下文 | P0 | 阶段 0 |
| 2 | 修复 snapshot 数据源 | P0 | 阶段 0 |
| 3 | 原子 checkpoint 协议 | P1 | 阶段 1、2 |
| 4 | GraphDataStore 目录封装与锁收口 | P1 | 阶段 1 |
| 5 | 外部索引 transactional outbox | P1 | 阶段 1、3 |
| 6 | API 收口和配置契约修正 | P2 | 阶段 1、4、5 |
| 7 | 性能与可观测性验证 | P2 | 阶段 1 至 6 |

阶段 1 和阶段 2 在完成阶段 0 后可以独立开发，但合并时必须分别完成全部验收测试。

## 4. 阶段 0：建立回归测试和架构基线

### 4.1 目标

在改动关键执行路径前，用测试固定当前正确行为，并让已知缺陷能够稳定复现。

### 4.2 修改内容

新增以下测试：

1. 两个并发事务共享同一个 `GraphStorage`，分别使用不同 read timestamp。
2. 两个请求交错设置、读取和清理事务上下文。
3. 创建 checkpoint 后立即创建 snapshot，验证 snapshot 内存在最新顶点、边、索引和用户数据。
4. checkpoint 写入中途注入错误，验证不会发布有效 checkpoint，也不会截断对应 WAL。
5. checkpoint 期间并发写入，验证恢复后数据既不丢失也不产生非法可见版本。
6. 外部索引同步失败，明确记录当前“本地成功、接口失败”的行为，作为阶段 5 的待修复测试。

增加测试用故障注入点，建议只在 `test-support` feature 下启用：

- checkpoint 创建后、数据 flush 前。
- 顶点 flush 后、边 flush 前。
- metadata 写入前。
- WAL truncate 前。
- outbox 投递前后。

### 4.3 验收标准

- 已知事务上下文和 snapshot 问题可以通过确定性测试复现。
- 已有 531 个 storage 测试继续通过。
- 故障注入代码不进入普通生产构建路径。

## 5. 阶段 1：显式事务操作上下文

### 5.1 目标

删除 storage 实例上的全局 `current_txn_context`，使每次读写明确归属于一个事务或自动提交操作。

### 5.2 设计

新增只读操作上下文，例如：

```rust
pub struct StorageOperationContext {
    pub transaction_id: Option<TransactionId>,
    pub read_timestamp: Timestamp,
    pub write_timestamp: Option<Timestamp>,
    pub read_only: bool,
}
```

具体字段应根据 transaction crate 的现有类型调整，避免重复维护事务状态。

提供两种可选入口，推荐事务绑定 handle：

```rust
let store = storage.bind(operation_context);
store.get_vertex(...)?;
store.insert_vertex(...)?;
```

自动提交操作由 storage/transaction manager 创建一次性 context，而不是在每个底层方法中隐式生成不相关的时间戳。

### 5.3 修改范围

- `storage/client.rs`
- `storage/engine/graph_storage/context`
- `storage/engine/graph_storage/reader.rs`
- `storage/engine/graph_storage/writer.rs`
- `storage/engine/graph_storage/cursor_impl.rs`
- `storage/engine/sync_wrapper`
- `graphdb-query` 的 storage 调用入口
- `graphdb-api` 的请求执行入口

删除：

- `StorageTransactionContextOps`
- `get_transaction_context`
- `set_transaction_context`
- `GraphStorageRuntime::current_txn_context`
- API 层的 `TransactionContextGuard`

### 5.4 测试要求

- 同一 storage 上至少 8 个并发事务使用不同 timestamp，互不串扰。
- 一个事务结束不得影响其他事务的 context。
- cursor 在整个生命周期中保持创建时的 read timestamp。
- `SyncWrapper` 取得的是当前操作 context 中的 transaction ID。
- 自动提交与显式事务的可见性分别测试。

### 5.5 验收标准

- storage 共享对象中不存在请求级可变事务状态。
- storage 操作的 timestamp 来源可以从方法参数或绑定 handle 静态追踪。
- 并发事务回归测试通过。

## 6. 阶段 2：修复 snapshot 数据源

### 6.1 目标

保证 snapshot 内容与其记录的 checkpoint sequence 和 WAL LSN 完全对应。

### 6.2 修改内容

将 checkpoint 产出的不可变目录作为 snapshot 唯一数据源：

```text
checkpoint_<sequence>/
  checkpoint.meta
  data/
```

`PersistenceCoordinator::create_checkpoint` 应把已完成的 checkpoint 路径传给 `SnapshotManager`。不得继续传入主 `config.data_dir`。

如果 snapshot 与 checkpoint 位于同一文件系统，可优先考虑 hard link 或 reflink；不支持时再递归复制。无论采用哪种方式，都不能直接引用仍可变的主数据目录。

### 6.3 测试要求

- 主 data 目录为空时，checkpoint 后创建的 snapshot 仍包含全部最新数据。
- 主 data 目录故意保持旧版本时，snapshot 必须来自新 checkpoint。
- 从 snapshot 独立恢复后，顶点、边、索引、Schema 和用户数据一致。
- snapshot metadata 中的计数、checkpoint sequence 和 LSN 与内容匹配。

### 6.4 验收标准

- snapshot 创建路径不再读取主可变 data 目录。
- snapshot 可以作为独立恢复源完成全量恢复。
- 不存在只验证目录或统计、不验证内容的 snapshot 测试。

## 7. 阶段 3：原子 checkpoint 协议

### 7.1 目标

建立清晰的 checkpoint 状态机，确保失败 checkpoint 不可见，成功 checkpoint 可独立恢复，并且 WAL 只在安全点截断。

### 7.2 目标流程

```text
Capture timestamp and WAL LSN
            │
Freeze or pin logical snapshot
            │
Write checkpoint_<seq>.tmp
            │
Write and fsync data + metadata
            │
Atomic rename to checkpoint_<seq>
            │
Publish checkpoint sequence
            │
Truncate WAL up to safe LSN
```

### 7.3 修改内容

1. checkpoint manager 在数据发布成功前不得将新 sequence 视为有效。
2. 所有文件先写入临时目录。
3. metadata 应包含格式版本、timestamp、LSN、文件清单、大小和校验和。
4. 对文件和目录执行必要的 sync。
5. 使用同文件系统原子 rename 发布。
6. WAL truncate 必须发生在原子发布和 sequence 持久化之后。
7. 启动时清理未发布的 `.tmp` checkpoint。
8. 使用 RAII state guard，所有错误路径都恢复 coordinator 状态。

### 7.4 一致性方案

推荐复用 MVCC timestamp：

- checkpoint 获取固定 read timestamp。
- 各表按同一 timestamp 导出可见状态。
- 导出期间不得依赖不断变化的“当前时间”。
- 索引和用户数据必须具有相同 epoch，或在 metadata 中明确各自重放边界。

如果现有表 flush 无法按 timestamp 导出，则先通过 freeze 切换出不可变 segment，再让 checkpoint 只读取被冻结状态。

### 7.5 测试要求

- 在每个故障注入点模拟崩溃并重新打开数据库。
- 临时 checkpoint 不得被加载。
- 已发布 checkpoint 必须完整通过校验。
- checkpoint 失败时 WAL 不得被过早截断。
- 并发写入要么包含在 checkpoint 中，要么保留在 WAL 中，不能丢失。

### 7.6 验收标准

- 磁盘上不存在可见的部分 checkpoint。
- checkpoint 恢复不依赖主 data 目录中的偶然残留。
- coordinator 状态在所有错误路径下正确恢复。

## 8. 阶段 4：GraphDataStore 目录封装与锁收口

### 8.1 目标

让数据目录自身维护表、名称和反向索引的不变量，调用方不再直接取得多个原始锁。

### 8.2 设计

引入目录级领域 API，例如：

```rust
impl GraphDataCatalog {
    fn create_vertex_type(...);
    fn drop_vertex_type(...);
    fn create_edge_type(...);
    fn drop_edge_type(...);
    fn get_or_create_edge_partition(...);
    fn with_vertex_table(...);
    fn with_edge_table(...);
    fn snapshot_catalog(...);
}
```

目录负责维护：

- label name 与 label ID 的双向关系。
- 表注册与删除。
- edge label 与实际 edge table key 的反向索引。
- label counter。
- 固定锁顺序。

表内部继续使用自身锁或由目录提供短生命周期访问 guard，避免为单表操作长期持有全局写锁。

### 8.3 迁移顺序

1. 定义并记录全局锁顺序。
2. 为读取添加目录方法。
3. 为 create/drop 添加原子目录操作。
4. 迁移 schema engine。
5. 迁移 transaction undo/recovery。
6. 迁移 persistence 和 maintenance。
7. 删除 `vertex_tables()`、`edge_tables()` 等原始锁 accessor。

### 8.4 测试要求

- create/drop 顶点类型和边类型后校验全部目录不变量。
- undo 和 WAL recovery 后校验反向索引。
- 并发创建实际 edge partition 不得重复或遗漏索引项。
- 多线程 schema/data 混合压力测试不得死锁。
- 增加 `verify_invariants()`，仅用于测试和诊断。

### 8.5 验收标准

- engine 其他模块不能直接访问目录内部 HashMap/RwLock。
- 所有多映射更新都通过目录领域操作完成。
- 锁顺序集中记录并通过压力测试。

## 9. 阶段 5：外部索引 transactional outbox

### 9.1 目标

让本地事务提交和索引变更事件具备统一持久化边界，避免本地成功后同步失败导致返回语义和数据状态不一致。

### 9.2 设计

写入流程调整为：

```text
Build data mutation + index events
              │
Append transaction/WAL record
              │
Apply local mutation
              │
Commit
              │
Outbox consumer sends events
              │
Persist delivery progress
```

每个事件至少包含：

- transaction ID。
- transaction-local sequence。
- space ID。
- object type 和 operation type。
- object identifier。
- properties 或可重建变更的信息。
- 幂等键。

### 9.3 修改内容

- 将 `SyncWrapper` 从“写后同步装饰器”改为事件记录/投递组件。
- 本地写方法不因提交后的即时外部投递失败而返回本地写失败。
- 增加 outbox backlog、retry count、last delivered LSN 等指标。
- 明确事务 rollback 时如何丢弃未提交事件。
- 明确 checkpoint 和 WAL truncate 如何保留未投递事件。

### 9.4 测试要求

- 外部索引不可用时，本地事务仍按定义完成并保留事件。
- 重启后继续投递未完成事件。
- 重复投递不产生重复索引结果。
- batch 操作不会只永久投递前半部分。
- rollback 不得投递事件。
- 多事务事件顺序符合 transaction ID/LSN 定义。

### 9.5 验收标准

- 不再存在“storage 已修改但方法因同步错误返回失败”的路径。
- 所有已提交索引事件都可在崩溃后恢复。
- 投递具备幂等和可观测的重试机制。

## 10. 阶段 6：API 收口和配置契约修正

### 10.1 目标

形成清晰的逻辑存储、目录和维护接口，删除已被替代的运行时 setter 和不必要物理类型暴露。

### 10.2 修改内容

1. 将非法 `PropertyGraphConfig` 错误从构造函数传播给调用方。
2. 根据消费者拆分接口：
   - `GraphStore`
   - `CatalogStore`
   - `StorageMaintenance`
   - `StorageRecovery`
3. query 层只依赖读写和 catalog 能力。
4. api/server 初始化层才依赖 maintenance/recovery。
5. 删除顶层不必要的 `VertexTable`、tombstone、物理同步类型 re-export。
6. 对分页和 cursor 能力取消昂贵的静默默认实现，改为原生实现或明确 capability error。
7. 统一 `&mut self` 和内部 `Arc<RwLock<_>>` 的并发语义，避免接口表现为独占访问但实现实际共享。

### 10.3 测试要求

- 非法 freeze、flush、cache 配置在构造时失败。
- query crate 的编译依赖不包含 maintenance-only trait。
- cursor 测试验证 lazy 行为和固定 read timestamp。
- 大数据分页不会先物化全部结果。

### 10.4 验收标准

- 对外 API 不包含 transaction context setter。
- 物理表实现默认不跨 crate 暴露。
- 构造函数的 `StorageResult` 具有真实错误语义。
- 各消费者依赖最小能力接口。

## 11. 阶段 7：性能与可观测性验证

### 11.1 目标

确认正确性重构没有造成不可接受的回归，并让关键后台状态可诊断。

### 11.2 基准指标

至少记录以下基线和重构后结果：

- 单顶点/批量顶点写入吞吐。
- 单边/批量边写入吞吐。
- 一跳出边和入边遍历延迟。
- 并发读写吞吐与长尾延迟。
- checkpoint 时长、写入放大和暂停时间。
- snapshot 创建时长和额外磁盘空间。
- WAL recovery 吞吐。
- outbox backlog 和投递延迟。
- 各目录锁等待时间。

### 11.3 可观测性

增加：

- 当前 checkpoint phase、sequence 和 safe LSN。
- 上次 checkpoint/snapshot 成功和失败原因。
- 未清理临时目录数量。
- outbox pending/retry/oldest-event age。
- catalog invariant check failure。
- background freeze 和 compaction backlog。

### 11.4 验收标准

- 关键读路径无明显性能回退。
- checkpoint 和 snapshot 的暂停时间符合单机部署目标。
- 发生失败时能从日志和指标确定失败阶段及安全恢复点。
- 完整测试和基准报告写入 `docs/benchmark` 或 `docs/stat`。

## 12. 每阶段统一验证命令

```shell
cargo fmt --check
cargo test -p graphdb-storage --lib -- --nocapture
cargo test -p graphdb-storage --test '*' -- --nocapture
cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant
cargo clippy --all-targets --all-features
```

涉及 query 或 api 接口修改的阶段，还必须运行对应 crate 和端到端测试。

## 13. 完成定义

整个计划完成需同时满足：

1. 并发事务不再通过共享 storage 字段传递上下文。
2. snapshot 只从与其 metadata 对应的不可变 checkpoint 创建。
3. checkpoint 使用临时目录和原子发布，失败时不截断必要 WAL。
4. `GraphDataStore` 不再向外暴露原始容器锁。
5. 外部索引变更由可恢复、幂等的 outbox 投递。
6. 非法配置在构造阶段返回错误。
7. query/api 依赖最小 storage 能力接口。
8. 全部单元、集成、故障恢复和并发测试通过。
9. 关键性能指标相对基线处于可接受范围。
