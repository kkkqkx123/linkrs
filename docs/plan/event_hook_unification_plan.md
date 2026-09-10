# Event Hook 统一化后续方案（修订版）

前置工作（已落地）：`EventSubscriptions` 快照后求值、filter panic 隔离、
`SubscriptionGuard` + `clear()`、`dispatch_detailed`，C-API 四类回调的
`catch_unwind` 隔离。见 `docs/plan/event_hook_remediation.md`（该文档明确把
统一总线列为非目标），本文是其后续，只覆盖当时有意 scope 外的四项重构。

本文是对初版的修订，纳入了对初版设计的评审意见，主要修正：
`T2c BudgetWarning` 不再挂靠 commit 马甲、`T3b` 进度改为独立钩子而非
`SessionEvent` 变体、`T3a` 明确双 Session 层级与双 ID 体系、`T4` 加实验门控。

## 现状（问题定位）

1. 同一事件枚举被拆到多个注册表，订阅者必须注册多次且事件关联断裂：
   - `SchemaChangeEvent`：`SchemaManager` 与 `IndexManager` 各持一个
     `EventSubscriptions`（`crates/graphdb-core/src/metadata/schema_manager.rs`、
     `index_manager.rs`）；`GraphStorageContext::register_schema_callback`
     （`crates/graphdb-storage/src/engine/graph_storage/context/schema.rs`）
     被迫向两处双注，且只有注册没有注销，重复投递无法取消。
   - `SessionEvent`：`QueryManager`
     （`crates/graphdb-query/src/query_manager.rs`，发 query 半）与
     `GraphSessionManager`
     （`crates/graphdb-server/src/session/session_manager.rs`，发 session 半）
     各持一个，订阅者收不全。注释虽写“单枚举双发射源”，注册表仍分裂。
   - `TransactionEvent`：`TransactionManager`
     （`crates/graphdb-transaction/src/manager.rs`）用 `commit_callbacks` +
     `rollback_callbacks` 两个注册表承载同一个枚举，`BudgetWarning` 挂靠
     commit 通道，名实不符。
2. 无中央订阅入口：`GraphDatabase`（`crates/graphdb-api/src/embedded/database.rs`，
   内层 `Arc<GraphDatabaseInner<S>>`）是天然的挂载点，但今天各 manager 各自为政，
   嵌入式用户想“订阅全库事件”要逐个 manager 注册。
3. 无中断原语：`QueryManager::kill_query` 只改 `QueryInfo` 状态标记，不触及执行器
   的 `CancelToken`（`crates/graphdb-query/src/executor/streaming/runtime.rs`）。
   现实比初版描述更复杂，存在两套并行的查询身份体系：
   `QueryManager(i64)` vs `QueryRegistry(QueryId u64 + CancelToken)`，
   目前只有 `registry -> qm.kill_query` 单向桥（`runtime.rs:cancel_with_reason`），
   缺反向桥。且存在两个 Session 层级：嵌入式 `api::Session`
   （`crates/graphdb-api/src/embedded/session.rs`）与服务端 `ClientSession`
   （`crates/graphdb-server/src/client/`），中断设计必须明确作用在哪一层。
   对照 ladybug `ClientContext::interrupted: AtomicBool + interrupt()`，本项目缺
   连接级一键中断。
4. 无流水线扩展点：对照 ladybug 的
   `TransformerExtension / BinderExtension / PlannerExtension / MapperExtension`，
   本项目 parser/binder/planner/mapper 各阶段无 trait 钩子，外部无法扩展语法或
   改写计划（UDF 注册不算流水线钩子，见 `docs/plan/udf_extension_design.md`）。

## 任务清单

### T2 合并同枚举分裂注册表（先做，是 T1 的前提）

不先合并，T1 的总线 id 映射要处理双注特例，故 T2 优先。

- T2a Schema：采用共享 `Arc<EventSubscriptions<SchemaChangeEvent>>` 方案
  （不采用“`IndexManager` 调 `SchemaManager` 发射”方案，后者会引入
  `core` 内部反向依赖且改动 6 处发射点，风险更高）。
  `SchemaManager` 新增 `shared_schema_callbacks()` 访问器与
  `with_shared_schema_callbacks()` 构造器，`IndexManager` 同理；
  `GraphStoragePersistent` 创建两者时只建一个共享 `Arc` 并同时注入。
  `GraphStorageContext::register_schema_callback` 改为单注直通
  （只向共享注册表注一次），并新增 `unregister_schema_callback` +
  `schema_callback_count` 以补齐注销能力。
  `register_schema_callback / unregister_schema_callback / schema_callback_count`
  方法名保持兼容。
- T2b Session：`QueryManager` 与 `GraphSessionManager` 的
  `session_callbacks` 字段类型统一为 `Arc<EventSubscriptions<SessionEvent>>`，
  各新增 `new_with_shared()` 构造器 + `shared_session_callbacks()` 访问器 +
  `set_shared_session_callbacks(&mut self, …)`（字段是 `Arc`，存量实例需
  `&mut` 才能换源；`new()` 行为不变，仍建 fresh 注册表，避免破坏存量调用方）。
  共处一进程的装配点负责注入同一共享 `Arc`；无法共处时由 T1 HookBus 做双注
  扇出兜底。注释写清“单枚举双发射源”。
- T2c Transaction：合并 `commit_callbacks` + `rollback_callbacks` 为单个
  `txn_callbacks: Arc<EventSubscriptions<TransactionEvent>>`。
  对外提供三类入口，语义诚实、不伪装：
  - `register_txn_callback(_filtered)`：统一入口，不过滤，收全量；
  - `register_commit_callback(_filtered)`：兼容马甲，内置 filter 只放行
    `Committed | CommitDurableButUnfinalized`；
  - `register_rollback_callback(_filtered)`：兼容马甲，内置 filter 只放行
    `Aborted`；
  - `register_budget_warning_callback(_filtered)`：`BudgetWarning` 独立入口，
    不再挂靠 commit 通道。
  `emit_commit_event / emit_rollback_event / emit_budget_warning_event` 均走
  同一注册表。`*_callback_count` 口径统一为注册表总数并在文档中注明
  （不再按 filter 口径分别统计，避免误导）。内部 stats 订阅改为走统一注册表
  的 filtered 订阅。`drain_context_budget_warnings` 改走
  `emit_budget_warning_event`。

验收：各 manager 现有回调单测通过；新增“一次注册收全量”测试
（schema 建表+建索引只注一次即收到两类；txn 提交/回滚各收到一次；
`BudgetWarning` 经独立入口可达）；`GraphStorageContext` 双注代码删除。

### T1 中央 HookBus（订阅门面，不搬数据）

- 在 `graphdb-api`（embedded database 层，与各 manager 同可见）新增 `HookBus`：
  持有指向各 manager 注册表的 `Arc`/引用，按事件类型提供统一
  `subscribe_* / unsubscribe` 入口，内部转发到对应叶子注册表并合并返回
  `SubscriptionId`（需维护“总线 id → (manager, 叶子 id)”映射以支持统一注销，
  映射表需线程安全，注销需幂等）。
- `GraphDatabaseInner` 构造时装配 `HookBus`，对外暴露 `GraphDatabase::hooks()`。
- 叶子 `dispatch` 路径零改动：不搬事件数据、不引入异步 channel，保持同步
  inline + best-effort 语义（与已落地的底座一致）。
- 范围说明：`HookBus` 只聚合嵌入式可见的 manager
  （schema / storage / txn；query/session 侧 `QueryManager` 不在
  `GraphDatabaseInner` 内，其聚合待执行器装配点明确后再接）。
  服务端 `GraphSessionManager` 侧的聚合由 server 层另行装配，不在本任务内。
  C-API veto hook（`commit_hook` 否决语义）明确排除在外，文档中写清
  “通知 vs 决策”二分。

验收：嵌入式单测一次订阅收到 schema + txn 两类事件；`unsubscribe` 后两类均止；
`cargo test -p graphdb-api` 通过。

### T3 连接级中断 + 进度钩子（对标 ladybug）

- T3a 中断：分两层明确语义。
  - 嵌入式层：在 `Session`（`crates/graphdb-api/src/embedded/session.rs`）
    新增 `AtomicBool interrupted + interrupt() / is_interrupted() /
    clear_interrupt()`；`Session::execute*` 入口检查该标志，置位时直接返回
    interrupted 错误（合作式取消的第一道门）。
  - 执行器层：`QueryManager::kill_query` 在持有 `QueryRegistry` 映射时同步
    `cancel(QueryId, CancelReason::Killed)`，补上缺失的反向桥，使 kill 真正停掉
    执行中查询而非仅改状态。`QueryManager i64` 与 `QueryRegistry QueryId u64`
    的映射关系由装配点（`set_query_manager / set_query_registry` 处）负责登记，
    本任务先做“有映射则联动取消、无映射则仅改状态”的渐进语义，不强求一次性
    统一两套 ID。
  - 服务端 `ClientSession::kill_query` 保持现状转发，最终经由执行器层联动。
- T3b 进度：不采用“`SessionEvent` 加变体”方案（初版评审结论：高频行级事件
  会污染低频生命周期枚举的全部 `match`，且节流逻辑侵入热路径）。
  改为 `QueryManager` 上的独立轻量钩子：
  `register_progress_callback(callback, rows_interval) -> SubscriptionId`，
  由执行器在行数达到间隔时显式调用 `emit_progress`；默认不注册即零开销，
  无需全局阈值开关。`SessionEvent` 保持不变，避免全仓 `match` 爆炸。
- T3c C-API：新增 `graphdb_connection_interrupt(session)`（对标 ladybug
  `lbug_connection_interrupt`），置嵌入式 `Session` 中断标志；文档注明是合作式
  取消，长阻塞算子在下一个检查点停下。

验收：中断测试——`interrupt()` 后 `execute` 直接返回 interrupted 错误；
`kill_query` 在 registry 映射存在时联动取消 token；
进度测试——注册进度钩子后收到至少一次回调且结果正确，不注册时零开销；
现有 query/server 测试全过。

### T4 查询流水线扩展点（对标 ladybug 四件套，最后做）

- 新增 trait（放在 `graphdb-query`，避免循环依赖），首版保持最小并加实验门控
  （`experimental_pipeline_extensions` 模块 + 文档注明 unstable）：
  `BinderExtension::bind / PlannerExtension::plan`（首批真实接入），
  `ParserExtension::transform / MapperExtension::map` 留接口桩 + 单测证明
  可注册调用。每个 trait 单方法 + 默认返回 `None` 的提供方法，
  输入输出复用本项目已有 `Stmt / BoundStatement / SubPlan` 等类型。
- 注册表 `ExtensionRegistry`（`Arc` 共享，有序 Vec + `parking_lot::RwLock`）
  提供 `add_*_extension` + 有序取回；`Binder::bind` 入口与
  `PlannerEnum::plan_bound`（或等价规划入口）按序试扩展，
  扩展返回 `Some` 即采用、`None` 则走内置逻辑（与 ladybug 语义一致），
  扩展 `panic` 用 `catch_unwind` 隔离后回落内置逻辑。
- 第一批只接 `BinderExtension + PlannerExtension`（改写/拦截价值最大）。

验收：示例扩展可拦截特定语句并改写计划；扩展 panic 被隔离不破坏查询
（复用底座 `catch_unwind` 约定）；现有 planner/binder 测试全过。

## 实施顺序与依赖

T2 → T1 → T3 → T4。T2 是前提（不先合并，T1 的总线 id 映射要处理双注特例）；
T3 依赖执行器 `CancelToken` 现状调研（先确认 `kill_query` 与 token 的断点再动手，
本文已确认：缺的是 `QueryManager -> QueryRegistry` 反向桥）；
T4 独立但建议最后做（涉及面最广）。

## 非目标

- 不引入异步 channel / 后台分发线程，全部保持同步 inline 语义。
- 不做 BEFORE 否决型数据触发器（SQL TRIGGER 语义）；唯一否决点仍是 C-API
  `commit_hook`。
- 不动 `SyncManager` outbox 管道与 tantivy `WatchCallback`。
- 不改 `SubscriptionId = u64` 与已落地的 `EventSubscriptions` API。
- `HookBus` 不聚合服务端独有的 `GraphSessionManager` 注册表（server 层另行处理）。

## 风险

- T2c 合并后 `commit_callback_count` 口径变化：调用方若断言数量需同步更新，
  先 grep 确认仅统计/测试用途；本文已将口径统一为总数并文档注明。
- T2b 字段类型 `EventSubscriptions` → `Arc<EventSubscriptions>`：`Debug` 实现与
  构造调用点需同步更新，编译器会指引。
- T3 进度钩子频率：独立钩子 + 按行间隔触发，不注册即零开销；禁止在热路径无条件
  发射。
- T4 扩展 trait 一旦公开即成 API 承诺：首版方法签名尽量小（单方法 + 默认返回
  `None` 的提供方法）+ 实验模块门控，避免日后频繁 breaking。
