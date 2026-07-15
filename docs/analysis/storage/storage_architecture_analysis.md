# Storage 包架构分析

## 1. 分析结论

当前 `graphdb-storage` 的总体技术方向合理，顶点列存、边双向 CSR、分段冻结与合并、MVCC、WAL 和压缩编码已经形成较完整的存储能力。物理存储层成熟度较高，模块依赖也符合项目的 crate DAG。

但引擎编排层仍存在会影响正确性的边界问题，尤其是共享事务上下文、checkpoint 与 snapshot 的数据一致性、多个数据目录的原子更新，以及本地存储和外部索引同步之间的一致性。因此，当前实现适合作为开发阶段的单机存储引擎基础，但尚不宜将并发事务和备份恢复能力视为生产级实现。

综合评价约为 **6.5/10**：

- 物理数据结构和单表生命周期设计良好。
- 顶层接口已开始按能力拆分，方向正确。
- 测试数量和覆盖面较好。
- 跨请求事务状态和跨组件提交协议需要优先重构。

## 2. 当前架构

```text
StorageReader / StorageWriter / StorageSchemaOps / StorageAdmin
                            │
                       GraphStorage
                            │
                  GraphStorageContext
              ┌─────────────┼─────────────┐
              │             │             │
       GraphDataStore   Persistence     Runtime
              │        WAL/Checkpoint  Txn/GC/Freeze
       ┌──────┴──────┐      /Snapshot
       │             │
  VertexTable     EdgeStore
  ColumnStore     CSR/Segment/MVCC
```

主要层次如下：

1. `storage/client.rs` 定义读、写、Schema、认证、维护和恢复等能力接口。
2. `GraphStorage` 是对外门面，负责实现各类 storage trait。
3. `GraphStorageContext` 聚合数据表、Schema、索引、版本管理、持久化和后台任务。
4. `GraphDataStore` 保存顶点表、边表、标签映射和边标签反向索引。
5. `VertexTable` 使用列存、ID 映射和时间戳管理顶点数据。
6. `EdgeStore` 使用出入方向 CSR、冻结段、属性表和 MVCC 管理边数据。
7. `PersistenceCoordinator` 编排 WAL、flush、checkpoint 和 snapshot。

## 3. 合理的设计

### 3.1 crate 依赖方向清晰

`graphdb-storage` 直接依赖 `graphdb-core`、`graphdb-transaction` 和 `graphdb-sync`，没有依赖 query 或 api。存储层不感知查询计划、HTTP 或 gRPC 协议，符合项目设定的依赖方向。

### 3.2 物理存储结构符合图负载

顶点使用列存，边使用双向 CSR，是适合本地单机图数据库的选择：

- CSR 能降低邻接遍历的随机访问成本。
- 独立的出边和入边结构适合方向查询。
- mutable CSR 与 frozen segment 兼顾增量写入和稳定读取。
- segment merge、tombstone 和 MVCC 支持时间可见性与空间回收。
- 列压缩已经进入表的 flush/load 流程，而不是停留在独立编码工具层。

### 3.3 顶层接口进行了能力拆分

当前已拆分 `StorageReader`、`StorageWriter`、`StorageSchemaOps`、`StoragePersistenceOps`、`StorageRecoveryOps` 和 `StorageGcOps`，再由 `StorageClient` 聚合。相比单一巨型接口，这种设计更利于 mock、指标装饰和同步装饰。

### 3.4 实现细节大多保持 crate 内可见

cache、edge、encoding、engine 和 index 等模块大多使用 `pub(crate)`，对外主要暴露门面、能力接口和少量必要类型。整体封装方向正确。

### 3.5 测试基础较好

分析期间执行：

```shell
cargo test -p graphdb-storage --tests
```

结果为：

- 481 个单元测试通过。
- 50 个集成测试通过。
- 总计 531 个测试通过，无失败。
- 存在 1 个未使用变量警告。

测试覆盖 CSR、列编码、MVCC、压缩、compaction、并发数据操作、WAL 恢复和完整生命周期。需要注意，现有事务隔离并发测试通过 `Arc<Mutex<GraphStorage>>` 串行访问 storage，没有覆盖共享事务上下文的真实并发竞态。

## 4. 主要架构问题

### 4.1 P0：事务上下文是 storage 实例级全局状态

`GraphStorageRuntime` 使用单个共享字段保存当前事务：

```rust
current_txn_context: Arc<RwLock<Option<Arc<TransactionContextInfo>>>>
```

读写时间戳通过该字段隐式取得。API 服务会在查询执行前设置事务上下文，并在 RAII guard 析构时清空它。

当两个请求并发使用同一个 `GraphStorage` 时，可能发生：

1. 事务 A 设置上下文 A。
2. 事务 B 将其覆盖为上下文 B。
3. 事务 A 后续操作读取到 B 的时间戳或事务 ID。
4. 事务 A 结束时清空 B 的上下文。

这不是单纯的封装问题，而是事务隔离正确性问题。`RwLock` 只能保证字段访问没有数据竞争，不能保证请求与上下文之间的逻辑归属。

不应通过线程局部变量修复，因为异步请求可能跨线程调度。事务上下文应作为调用参数显式传递，或通过事务绑定的 storage handle 持有。

### 4.2 P0：snapshot 复制的数据源不是刚生成的 checkpoint

checkpoint 将数据写入：

```text
checkpoint_<sequence>/data
```

但 checkpoint 完成后创建 snapshot 时，传给 `SnapshotManager` 的是持久化配置中的主 `data_dir`。`SnapshotManager` 随后直接复制该目录。

结果是：

- snapshot 的元数据记录了当前 checkpoint sequence 和 WAL LSN。
- snapshot 内容却可能来自更旧的主数据目录，甚至为空。
- API 仍可能返回 `snapshot_created = true`。

snapshot 应以刚完成的 checkpoint 数据目录为源，或直接基于不可变 checkpoint 创建快照。

### 4.3 P1：GraphDataStore 暴露原始锁和容器

`GraphDataStore` 分别保存并暴露以下锁：

- `vertex_tables`
- `edge_tables`
- `vertex_label_names`
- `edge_label_names`
- `vertex_label_counter`
- `edge_label_counter`
- `edge_label_index`

调用方直接取得 `RwLock<HashMap<...>>` 后自行组合操作。这使 `GraphDataStore` 更接近一个贫血数据容器，而不是维护不变量的数据目录。

影响包括：

- 多个映射之间的修改不能原子提交。
- 锁顺序散落在 schema、undo、recovery、persistence 等模块中。
- `edge_tables` 与 `edge_label_index` 等派生结构可能短暂或永久不一致。
- 后续新增锁时容易形成锁顺序反转和死锁。

例如删除顶点类型需要依次修改标签名、顶点表、相关边表和边标签反向索引，当前没有单一领域操作保证其整体一致性。

### 4.4 P1：checkpoint 缺少明确的一致性和发布协议

当前 checkpoint 流程大致为：

1. 读取当前 WAL LSN。
2. 在 checkpoint manager 中创建 checkpoint 记录。
3. 更新 WAL checkpoint sequence。
4. 依次写出顶点表、边表、索引和用户数据。
5. 写 checkpoint metadata。
6. 截断 WAL。

主要问题：

- 顶点表、边表和索引分别加锁并依次写出，没有统一逻辑快照屏障。
- 并发写入可能使不同组件对应不同时间点。
- flush 失败可能留下部分目录或已推进的 checkpoint sequence。
- `PersistenceState` 只在正常返回路径恢复为 `Idle`。
- 有效 checkpoint 没有通过临时目录加原子 rename 发布。

checkpoint 应采用“冻结逻辑快照、写临时目录、fsync、原子发布、最后截断 WAL”的协议。

### 4.5 P1：外部索引同步与本地写入不是原子的

`SyncWrapper` 先执行本地写入，再调用外部全文或向量索引同步。如果同步失败，方法返回错误，但本地写入已经成功。

这会产生以下语义问题：

- 调用者看到失败，但数据库实际已修改。
- 调用者重试可能产生重复数据或重复事件。
- 本地数据和外部索引可能永久不一致。
- batch 同步过程中失败时可能只发送部分事件。

更可靠的方式是 transactional outbox：本地变更与索引事件一起进入事务/WAL，提交后由可重试消费者异步投递，事件通过事务 ID 和序列号保证幂等与顺序。

### 4.6 P2：配置校验结果被忽略

`GraphStorageContext::new_with_config` 调用了 `config.validate()`，但通过 `let _ =` 丢弃校验错误。外层构造函数返回 `StorageResult`，却不会因非法配置失败。

这使接口契约与行为不一致，也可能让错误配置在后台 freeze、flush 或 compaction 时才暴露。

### 4.7 P2：公开 API 混合逻辑接口和物理实现

storage 顶层同时公开：

- 图数据读写接口。
- Schema 和认证接口。
- checkpoint、recovery 和 GC 控制接口。
- transaction context setter。
- `VertexTable`、MVCC tombstone 等物理实现类型。

接口能力虽已拆分，但稳定边界仍不够明确。建议最终形成：

- `GraphStore`：逻辑数据读写。
- `CatalogStore`：space、schema 和 index metadata。
- `StorageMaintenance`：flush、checkpoint、recovery 和 GC。
- `TransactionalGraphStore` 或事务绑定 handle：携带事务上下文。

物理表类型原则上保持 crate 内可见。

### 4.8 P2：部分默认实现可能隐藏昂贵退化

例如分页扫描默认实现先加载全部边再执行 `skip/take`，cursor 默认实现也可能先物化完整结果。这种 fallback 适合 mock 或简单实现，但对生产引擎容易形成不可见的内存和延迟退化。

能力不受支持时，显式返回 capability error 通常比静默退化更安全；生产实现则应提供真正的 lazy cursor 和下推分页。

## 5. 风险优先级

| 优先级 | 问题 | 主要影响 |
| --- | --- | --- |
| P0 | storage 全局事务上下文 | 并发事务串扰、错误时间戳、隔离性破坏 |
| P0 | snapshot 复制错误数据目录 | 备份内容过期、恢复点与元数据不一致 |
| P1 | checkpoint 缺少原子发布 | 部分 checkpoint、WAL 边界不清晰 |
| P1 | GraphDataStore 暴露原始锁 | 不变量破坏、锁顺序复杂、潜在死锁 |
| P1 | 本地写入与外部索引同步非原子 | 返回语义错误、索引不一致 |
| P2 | 配置校验结果丢弃 | 非法配置延迟失败 |
| P2 | API 边界过宽 | 耦合增加、演进成本上升 |
| P2 | 默认扫描静默物化 | 内存和延迟不可控 |

## 6. 推荐目标架构

```text
Request / Transaction
        │ explicit OperationContext
        ▼
TransactionalGraphStore ───────► CatalogStore
        │
        ▼
StorageEngine
  ├── DataCatalog
  │     ├── VertexTableRegistry
  │     └── EdgeTableRegistry
  ├── VersionedTables
  │     ├── VertexTable
  │     └── EdgeStore
  ├── CommitPipeline
  │     ├── WAL
  │     ├── Data mutations
  │     └── Outbox events
  └── PersistenceService
        ├── Flush
        ├── Atomic checkpoint
        └── Snapshot from checkpoint
```

核心原则：

1. 请求相关状态不保存在共享 storage 实例中。
2. 数据目录负责维护映射和反向索引的不变量。
3. checkpoint 是不可变且原子发布的恢复单元。
4. snapshot 只从已发布 checkpoint 派生。
5. 外部系统同步由持久化事件驱动，不参与本地写方法的同步返回路径。
6. 对外公开逻辑能力，不公开不必要的物理表实现。

## 7. 总结

当前 storage 包不是需要推倒重写。CSR、列存、编码、MVCC、segment merge 和 WAL 等底层能力可以保留并继续演进。重构重点应放在两条主线上：

1. 明确事务/请求上下文如何进入每一次存储操作。
2. 明确多个存储组件如何共同发布一个一致、可恢复的持久化状态。

优先修复这两条主线后，再收口锁管理、同步机制和公开 API，可以在保留现有物理存储投资的前提下显著提升整体可靠性。
