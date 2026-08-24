# 向量引擎与 Qdrant 集成分析与改进方案

> 状态：分析 + 实施方案（2026-08-24）。所有结论均经代码逐点核实，
> 并修正了早期探索中的两处误判（见 §2.1 注记）。
>
> 关联：`docs/plan/vector_local_engine_plan.md`（总方案）及 phase_b/c 系列；
> 本文与其重叠的条目仅做引用，不重复实施。

---

## 一、集成架构现状

### 1.1 总体拓扑

```
                     ┌─ crates/vector-search ──► LocalVectorEngine（进程内）
VectorBackend 枚举 ──┤   （crates/graphdb-sync/src/sync/backend.rs:23-29）
                     └─ crates/vector-client ──► VectorManager ──► Qdrant gRPC/HTTP
```

- **vector-search** 是零 graphdb 依赖的叶子 crate：精确扫描 + IVFFlat
  （无 HNSW），自有五文件 mmap 存储（meta/vectors/keys/payloads/wal），
  WAL-first 提交（fsync + crc32 + 幂等重放 + meta 计数对账），ArcSwap
  无锁读快照，墓碑率 >20% 触发 compaction（临时文件 + rename 原子替换）。
  IVF 索引是可丢弃的派生物：损坏即静默降级精确扫描。
- **vector-client** 是纯 Qdrant 客户端（vendored proto + tonic-build 自生成
  stub + 手写 REST），crate 内 `VectorEngine` trait 仅有 Qdrant 实现；
  **本地引擎刻意不走该 trait**，所有共享类型以 `vector-search/types.rs`
  为单一来源，client 仅 `pub use` 转发。
- 引擎选择在启动时按 `[vector] engine = local|qdrant` 配置分派
  （crates/graphdb-server/src/startup.rs:84-134）；feature 门控沿 DAG
  透传：`vector`（默认，纯本地）/ `vector-qdrant`（叠加网络栈）。

### 1.2 查询体系（最完整）

- 专属语句族：`CREATE/DROP VECTOR INDEX`、`SEARCH VECTOR … WITH …
  THRESHOLD/WHERE/LIMIT/OFFSET/YIELD`、`MATCH VECTOR`、`LOOKUP VECTOR`
  （`crates/graphdb-query/src/query/parser/ast/vector.rs`）。
- 规划器把 WHERE 谓词递归编译为 `VectorFilter` 下推（AND→must、OR→should、
  NOT→must_not），不支持的形式显式报错
  （`query/planning/vector_planner.rs` convert_where_clause_to_filter）。
- 执行算子经 `futures::executor::block_on(coordinator.search_with_options(..))`
  桥接异步协调器
  （`query/executor/streaming/operators/vector_operator.rs:266,349,418,494,543`）。

### 1.3 存储体系（双轨制）

- 向量属性值本体走列存主存：`VECTOR(n)` 归入变宽列，f32 小端内联，
  随 checkpoint 持久化（`storage/vertex/column_store.rs:84-86,361-369,733`），
  WAL 恢复支持三种向量类型串（recovery.rs:1043-1076）；不可作主键。
  CSR 与向量无关。
- 向量索引完全旁路：local 自管目录 / Qdrant 远端。
  graphdb-transaction crate 内无向量特判。

### 1.4 同步体系（最深）

生产接线存在且已核实（此处修正既有 phase_c 文档 §1.3 的过时结论——
该文档撰写时产生侧尚不存在，现 manager.rs 已补齐）：

1. 写顶点时 sync_wrapper 将属性变更 stage 为 intent，并按活跃 target 复制
   （fulltext/vector 各一份，`graphdb-sync/src/sync/manager.rs:138-154`）；
2. 提交经 WAL durability fence 后物化进 durable SQLite outbox
   （`storage/engine/sync_wrapper.rs:142-221`）;
3. finalize 在提交线程**同步触发一次** `retry_outbox_sync`
   （sync_wrapper.rs:211），后台另有 5s 兜底轮询（manager.rs:848-853）
   与指数退避重试 / 死信队列（manager.rs:412-436）；
4. 消费端按 `target=="vector"` 分发到 `apply_vector_mutation`
   （manager.rs:484-489,688-833，feature `"vector"` 门控，双后端通用），
   `VectorReceiver` 以 LSN + idempotency key 幂等去重
   （收据持久化 vector_receiver_state.bin）。

因此**读己之写在正常路径下成立**（提交后同步 drain）；残余缺口仅为
失败重试窗口（≤5s + 退避）与 batch_size 上限。

- Space 级共享 collection：collection 名 `space_{space_id}`，同 space 所有
  (tag, field) 共享物理集合，靠 payload `group_id="{tag}_{field}"`
  过滤隔离（`sync/vector_sync.rs:126-144,482-485`）。
- Embedding 子系统（OpenAI 兼容 provider，默认 Ollama）仅在 qdrant
  同步路径接入（startup.rs:360-378），未接入查询语句管道。

## 二、设计评价

### 2.1 合理之处

1. **依赖方向干净**：叶子引擎 crate + feature 门控 + 类型单一来源；
   enum 双后端分派避免 async-trait object，符合"最小化 dyn"约定。
2. **本地引擎崩溃安全完整**：WAL-first + 幂等重放 + 对账三层防御，
   recovery/storage 测试覆盖真实崩溃窗口；索引可降级，可用性优先。
3. **同步链路复用通用 outbox**（generation/backfill/frontier/死信），
   幂等接收器防崩溃重放重复投递；提交后同步 drain 保住读己之写。

> 注记：分析过程中修正两处早期误判——(a) 图写入→向量的自动传播在生产侧
> 存在（stage_intent 按 target 复制，非"没有产生方"）；(b) 提交后有同步
> 投递触发（非纯 5s 轮询）。

### 2.2 问题清单（按可行动性排序）

| # | 问题 | 证据 | 定性 |
|---|------|------|------|
| Q1 | 查询执行器用 `futures::executor::block_on` 桥接异步协调器。算子运行在专用查询线程，无 tokio 上下文：本地引擎（纯同步 CPU）恰好能工作；Qdrant 路径的超时定时器/IO 反应器依赖 tokio runtime，存在挂起风险。同一模式在 fulltext_operator 重复出现 | vector_operator.rs:266,349,418,494,543；fulltext_operator.rs 多处 | 可立即修 |
| Q2 | 两处调用点无条件以 HNSW 构造 `CollectionConfig`，本地引擎静默忽略 index_type/hnsw_config——配置语义误导，未来分支逻辑易引入隐性 bug | graphdb-api/src/api/core/vector_api.rs:79-88；graphdb-sync/src/sync/vector_sync.rs:480 | 可立即修（即 phase_c C1） |
| Q3 | `drop_vector_index` 只删逻辑索引注册表，物理 collection 永久保留：本地后端磁盘空间泄漏；qdrant 侧集合残留 | vector_sync.rs:352-369 | 可修（本地优先） |
| Q4 | `MATCH VECTOR` 的 pattern 被 executor 以 `..` 解构忽略，仅产出 (id, score)，无向量候选→顶点回查→图遍历闭环；YIELD 子句解析未消费；Text/Parameter 形式在建计划期报 `CapabilityUnavailable`（embedding 未接入语句管道） | vector_operator.rs:528-537,40；ddl.rs:758-779 | 大特性，登记后续 |
| Q5 | Space 共享 collection 导致同 space 不同 (tag,field) 无法有不同维度/度量（CollectionConfigConflict）；group_id 过滤在本地引擎是 post-filter 全扫（payload index no-op） | vector_sync.rs:124-135,324-337；backend.rs:111-120 | 架构决策，登记后续 |
| Q6 | 本地引擎每事务双 fsync（wal.bin + meta.bin）；IVF post-filter 高选择性谓词下仅一次 nprobe 翻倍补救；HNSW 语法参数对 local 静默无效（Q2 修复后语义澄清） | storage.rs:397-414,1056-1081 | 性能优化，需基准支撑，登记 |

## 三、分阶段实施方案

原则：小步、每步可编译可回归；代码注释仅描述意图（不引用本方案编号）。

### 阶段一：查询执行器的运行时感知异步桥接（对应 Q1）——已完成（2026-08-24）

- 新增 `graphdb-sync/src/sync/runtime.rs`：`block_on_ambient` 按环境
  分派（multi-thread 运行时内 `block_in_place`；current-thread 运行时内经
  瞬态线程驱动；无运行时时自建瞬态运行时），返回 `Result` 以符合项目
  禁 unwrap 规范；`SyncManager::execute_sync` 改为委托该助手；
- 新增 `graphdb-query/.../streaming/helpers/runtime_bridge.rs`：`wait(label, future)`
  把桥接失败与操作失败统一映射为带标签的 `QueryError`（归入 helpers
  公共层而非 operators——它是基础设施功能，不符合算子职责）；
- 替换 vector_operator.rs 5 处、fulltext_operator.rs 5 处裸 block_on。
- 验证：`rg "futures::executor::block_on" crates/graphdb-query/src` 零命中；
  graphdb-sync 单测 2 例新增运行时桥接用例通过；graphdb-query lib 1596 例通过。

### 阶段二：CollectionConfig 按后端条件构造（对应 Q2 = phase_c C1）——核实为已落地，无需改码

实施前核实发现两处调用点（vector_api.rs:79-96、vector_sync.rs:293-298）
均已按 `backend.is_local()` 条件化，`IndexType::HNSW`/`with_hnsw` 仅存于
qdrant 分支内。phase_c 文档撰写后代码已先行合入该清理。
验收项 `grep IndexType::HNSW crates/` 通过。

### 阶段三：本地后端逻辑索引删除时的物理集合回收（对应 Q3）——已完成（2026-08-24）

- 重写 `VectorSyncCoordinator::drop_vector_index`
  （graphdb-sync/src/sync/vector_sync.rs:352-404）：
  - 本地后端且物理集合存在时：若该 collection 下已无其他逻辑索引，
    调用引擎删除整个 collection 目录；否则按 group_id 过滤清除被删组的
    向量点，避免孤儿数据驻留共享集合；
  - 回收失败仅告警不阻断（逻辑删除已生效，重放场景由存在性守卫兜底）；
  - qdrant 后端保持原语义（远端资源生命周期另行评审）。
- 新增测试 `test_drop_reclaims_group_points_and_physical_collection`
  （crates/graphdb-sync/tests/vector_local_backend.rs）：覆盖双组共享集合、
  组清除、末组删除后目录回收、同名索引重建四个环节。
- 兼容性核实：outbox DropIndex 消费路径
  （manager.rs apply_vector_mutation）直接调用本方法，语义一致；
  幂等接收器防止崩溃重放重复投递。

### 阶段四：后续项登记（文档收尾）——本文档 §四 即登记产物

另登记一个实施中发现的存量问题（非本次改动引入）：graphdb-query 存量
测试代码存在 `StatisticsManager::space_version` 缺失的编译错误
（`cargo clippy -p graphdb-query --all-targets` 可复现，干净工作树同样存在），
属 analyze DDL 相关测试与优化器接口脱节，应在 CI 接入（phase_c C5）前修复。

## 四、明确不做（本期）

| 项 | 理由 |
|----|------|
| MATCH VECTOR pattern 闭环 | 需要新的 join 算子（向量候选→顶点回查→遍历）与语法扩展（LIMIT），独立特性 |
| Text/Parameter 向量查询 | embedding 服务接入语句管道涉及异步取向量、缓存与失败语义，独立评审 |
| gRPC 向量端点 | server 层占位 RPC，与本文主题解耦 |
| 每 (tag,field) 独立 collection | 涉及元数据迁移与配额权衡，需单独设计 |
| 双 fsync 合并 | 需要 benchmark 基线证明收益后再动持久化协议 |

## 五、验收标准

- [x] 阶段一后：`rg "futures::executor::block_on" crates/graphdb-query/src` 零命中；
      默认 feature 全量测试通过（graphdb-query 1596 例 / graphdb-sync 69 例 /
      vector-search 200 例全绿）；
- [x] 阶段二后：`IndexType::HNSW`/`with_hnsw` 仅出现在 qdrant 条件分支
      （核实为已落地）；
- [x] 阶段三后：新测试证明本地后端删光逻辑索引即回收物理 collection 目录；
- [x] 全程：`cargo check --workspace --features vector-qdrant` 编译干净；
      `cargo fmt --all -- --check` 通过；改动文件无新增 clippy 告警
      （存量告警与一处存量测试编译错误见 §三-阶段四）。
