# DDL 对齐遗留项处置方案

## 1. 背景与范围

DDL 定义对齐四阶段完成后，代码审查确认三项遗留问题与一项测试缺口。本文档给出完整问题清单与细化的修改方案，作为后续实施的依据。

处置原则（与项目约定对齐）：

- 正确性与防误用优先于兼容与性能。
- 无生产能力且无消费方的设施一律删除，不预留、不挂起。
- 文档是发布契约：做不到的能力要么实现，要么在文档中显式声明边界，不允许"可调用但静默失效"。
- 显式失败优于静默忽略；旧数据无兼容义务。

本文只处置既有能力的收尾，不引入文档未声明的新特性。

## 2. 问题清单

### 2.1 问题一：`alter_space_comment` 全链路孤立（存储 API + WAL 操作码 + 回放）

事实清单（行号为写作时位置）：

- SQL 面已无入口：`COMMENT ON` 与 `ALTER SPACE` 已在 DDL 对齐阶段整体移除；发布文档（`docs/release/03_ddl_schema_definition.md`）无任何"修改空间注释"语法，CREATE SPACE 仅在创建时接受 `comment`。
- 存储 trait：`linkrs-storage/src/client/traits_schema.rs:14` 声明 `alter_space_comment`。
- 实现链：`engine/graph_storage/schema_api.rs:26` → `engine/graph_storage/schema_writer.rs:195`（含 `append_schema_redo(WalOpType::AlterSpaceComment, ...)`）→ `linkrs-core/src/metadata/schema_manager.rs:298`（内存更新 + `SpaceUpdated` 事件）。
- 包装层实现：`engine/sync_wrapper/schema.rs:22`、`metrics.rs:474`、`test_mock.rs:406`、`linkrs-migration/src/executor/tests.rs:525`。
- WAL 定义：`linkrs-core/src/wal/types.rs:120` 变体（判别式 22）+ TryFrom 臂 + Display 臂；`wal/redo.rs:89` 的 `AlterSpaceCommentRedo`；`wal.rs:12` 再导出；`wal/traits.rs:88` 的回放 trait 方法。
- 回放链：`linkrs-storage/src/engine/transaction/recovery.rs:87` 分发 → `recovery/schema.rs:82` 执行；`linkrs-transaction/src/wal/recovery/replay.rs:309` 匹配臂 + `wal/recovery/tests.rs:76` mock 行。
- 测试：`linkrs-storage/src/engine/graph_storage/tests/schema_tests.rs:445` 直接调用该 trait 方法。

影响面：该能力无任何生产调用方，却横跨四个 crate 与 WAL 盘上格式，是纯粹的遗留设施。

判断：删除整条链。不补 `ALTER SPACE ... SET COMMENT` 新特性——文档无此契约，若未来需要应作为新特性整体立项（文档先行）。

### 2.2 问题二：TTL 过期数据无物理删除入口

事实清单：

- purge 已实现：`linkrs-storage/src/engine/graph_storage/ttl.rs:27`（顶点）与 `:60`（边），按 schema 扫描后走正常 delete 写路径（因此天然过 WAL、可被崩溃恢复）。
- 读路径已全量过滤：本 DDL 对齐补齐了三个 `*_with_schema` 公开读路径后，所有物化属性行的读路径均按 TTL 过滤。
- 生产入口缺失：`StorageAdmin`（`linkrs-storage/src/client/traits_admin.rs:32`）等对外 trait 未暴露 purge；无 SQL 命令；无 gRPC/HTTP；无后台任务。当前唯一调用方是 `linkrs-query/tests/ddl/tag_basic.rs:775` 的集成测试（直接持有 `GraphStorage`）。
- 现有先例：`repair_dangling_edges`（`traits_admin.rs:38`）同为"存储层维护能力、无 SQL/服务入口"，仅被 storage 自测使用；`StorageGcOps`（`traits_admin.rs:143`）提供了后台任务句柄的先例。
- 内存形态：purge 当前一次性把全部过期 vid 收集进 Vec 再逐个删除，数据量大时有内存尖峰。
- 测试缺口：边的 purge 只有存储单测，缺执行层集成测试（顶点两侧都有）。

影响：TTL 只能"逻辑过期"不能物理回收，存储随时间无限增长；发布文档 TTL 第 5 条承诺"过期清理接口需要被显式调用"，但该接口对用户不可达，契约悬空。

判断：补 SQL 维护语句入口（方案见 3.2），顺带做有界批量改造与边集成测试。

### 2.3 问题三：TTL 在纯拓扑快路径与计数路径上不过滤

事实清单：

- 以下快路径读 CSR 原始邻接、不物化属性，无法判定 TTL：
  - `linkrs-query/src/executor/streaming/operators/graph_operator/expand_frontier.rs:108` 与 `:411`：多跳中间跳，identifier-only（模块注释明确说明此设计）。
  - `graph_operator/expand_row_path.rs:83`：row-path 逐深度回放，非末跳为纯 ID。
  - `source_operator/neighbors.rs:133-139`：GetNeighbors 源算子批量取邻接 ID，`edge_types` 传空表示全部边类型。
  - `graph_operator/expand_frontier.rs:640`：`out_degree_batch` 度统计尾。
- 计数类读路径不判定 TTL（均不物化 `ttl_col` 属性）：`count_vertices_by_tag`（`reader/vertex.rs:169`，基于 `approximate_id_hole_stats` 的近似值）与 `count_edges_by_type`（`reader/edge.rs:769`，基于可见性门的精确行计数），被优化器统计收集器（`optimizer/stats/collector.rs:174`、`:607`）与迁移流程使用；两者都会把过期行计入。
- 文档已将边界写清（`docs/release/03_ddl_schema_definition.md` TTL 工作原理第 3 条）。

影响：边类型配置 TTL 后，多跳遍历、度统计、GET NEIGHBORS 会经过期边，结果与"过期即不存在"的语义不一致；统计类数值含过期行（已声明）。

判断：拓扑快路径按"TTL 激活则降级到物化路径"修复；计数路径保持现状（理由见 3.3）。

### 2.4 问题四（测试缺口）：边 TTL 的执行层覆盖不足

事实清单：

- `tests/ddl/tag_basic.rs` 有顶点 TTL 读过滤 + purge 集成测试。
- `tests/ddl/edge_basic.rs` 有边 TTL 创建与读过滤集成测试。
- 边 purge 只有存储单测（`graph_storage/ttl.rs` 内）。

判断：随问题二的验收一并补齐（3.2 的测试清单包含）。

## 3. 细化修改方案

### 3.1 问题一：删除 `alter_space_comment` 全链路

目标态：存储层不再存在"修改空间注释"能力；WAL 不再有该操作码；全仓无残留引用。

改动清单（按 crate）：

- linkrs-storage
  - `client/traits_schema.rs:14`：删除 trait 方法。
  - `engine/graph_storage/schema_api.rs:26`：删除实现。
  - `engine/graph_storage/schema_writer.rs:195`：删除函数（连带其中的 `append_schema_redo` 调用）。
  - `engine/sync_wrapper/schema.rs:22`、`metrics.rs:474`、`test_mock.rs:406`：删除各自实现/mock。
  - `engine/transaction/recovery/schema.rs:82`：删除回放函数；`engine/transaction/recovery.rs:87`：删除分发条目。
  - `engine/graph_storage/tests/schema_tests.rs:445`：删除该断言段（"创建时 comment 生效"已由其他用例覆盖；此处针对的是已删除的修改路径）。
- linkrs-core
  - `metadata/schema_manager.rs:298`：删除方法。
  - `wal/types.rs`：删除 `AlterSpaceComment = 22` 变体、TryFrom 臂、Display 臂；判别式 22 留空，**不重新编号**后续变体。
  - `wal/redo.rs:89`：删除 `AlterSpaceCommentRedo`；`wal.rs:12`：从再导出列表移除。
  - `wal/traits.rs:88`：删除回放 trait 方法。
- linkrs-transaction
  - `wal/recovery/replay.rs:309`：删除匹配臂与 import；`wal/recovery/tests.rs:76`：删除 mock 行与 import。
- linkrs-migration
  - `executor/tests.rs:525`：删除 mock 实现。

WAL 兼容处理：

- `WAL_VERSION`（`wal/types.rs:14`，当前 2）提升到 3。旧版 WAL 文件在头部校验（`WalFileHeader::is_valid`，`types.rs:344`）即失败，启动时显式报错，不做部分回放。
- 不重新编号后续操作码：重新编号会改变所有后续操作的盘上含义且无收益；留空 22 后，旧文件中操作码为 22 的记录即使绕过头部校验也会在 `TryFrom<u8>` 处失败（`InvalidOpType`），同样是显式失败。
- 项目无旧数据兼容义务，上述行为符合"显式失败优于静默忽略"。

验收标准：

- `cargo check --workspace` 零告警零错误。
- 全仓 grep `AlterSpaceComment` / `alter_space_comment` 零残留。
- WAL 回放相关测试（linkrs-transaction、linkrs-storage recovery）通过。

明确不做：

- 不补 `ALTER SPACE ... SET COMMENT`（无文档契约；未来如需要，作为新特性整体立项，文档先行）。
- 不为"未来可能的注释修改"保留存储 API 或 WAL 操作码。

### 3.2 问题二：新增 `PURGE EXPIRED` 维护语句

入口选型：

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| SQL 维护语句 | 显式、可测、与 CHECKPOINT / KILL QUERY 同层；文档有归属章节 | 需打通 parser 到 executor 全链 | 采用 |
| gRPC / HTTP RPC | 服务侧自然 | CLI 与嵌入式不可达；需改 proto | 本期不做，文档声明仅 SQL 入口 |
| 后台定时清扫 | 全自动 | 隐藏副作用、时序不确定、与"显式调用"契约冲突、引入后台线程 | 不做 |

语法（建议）：

```
PURGE EXPIRED [TAG <tag_name> | EDGE <edge_name>] IN <space_name>
```

- 省略 `TAG`/`EDGE` 时，清理该 space 内全部配置了 TTL 的标签与边类型。
- `IN <space_name>` 必填，避免误清当前空间以外的数据；space 不存在显式报错。
- 目标 schema 不存在显式报错。目标存在但未配置 TTL：不报错，`removed_rows` 为 0（与"未配置即无过期数据"一致）。

结果集：列 `space_name` / `schema_name` / `schema_type`（`TAG`、`EDGE`）/ `removed_rows`（bigint）。每个被扫描的 TTL schema 一行（含 0 行），便于观测；space 无 TTL schema 时结果集为空。

分层改动：

- 存储层（linkrs-storage）
  - `client/traits_admin.rs` 的 `StorageAdmin` 新增 `purge_expired(space) -> StorageResult<Vec<TtlPurgeEntry>>`；`TtlPurgeEntry { schema_name, is_edge, removed }` 定义在 `traits_admin.rs`（与 `StorageStats` 同址）。实现放 `engine/graph_storage/ttl.rs`：枚举 `list_tags` / `list_edge_types`，按 `has_valid_ttl` 过滤后调用既有 `purge_expired_vertices` / `purge_expired_edges`。
  - `engine/sync_wrapper/admin.rs`、`metrics.rs`、`test_mock.rs` 同步实现，形态与 `repair_dangling_edges` 完全同构。
  - 内存改造：purge 内部从"一次性收集全部过期 vid"改为有界批量（每累积 N 个执行一次删除，N 取常量 1024），消除内存尖峰；保持单 schema 顺序删除语义。
  - 不新增 WAL 操作码：删除走既有 `DeleteVertex` / `DeleteEdge` 记录，恢复链路零改动。
- 查询层（linkrs-query）
  - parser：`dispatch.rs` 增加 `PURGE` 关键字分发（复用 `check_keyword`，按既有惯例 `PURGE`/`EXPIRED` 作为标识符关键字，无需改 lexer）；`admin.rs` 增加解析函数，参照 CHECKPOINT 的位置与形状；`IN <space>` 复用 `match_token(TokenKind::In)` 模式（同 `util_stmt_parser.rs:81`、`describe_parser.rs:21`、`user_parser.rs:356`）；space 名与 TAG/EDGE 名分别用 `expect_dcl_name` / `expect_identifier`。
  - AST：`ast/stmt/management.rs` 增加 `PurgeExpiredStmt { space_name, target: Option<PurgeTarget> }`（`PurgeTarget::{Tag,Edge}`）；`ast/stmt.rs` 增加变体、`category` 归 DDL、`kind` 名。
  - planner：`planner.rs` 路由列表、`maintain_planner.rs` arm（参照 `plan_checkpoint`，携带 space 与 target 参数）。
  - 分类：`binder/bind.rs` 与 `pipeline/prepared/classify.rs` 的只读缓存排除列表加项（PURGE 变更数据，不可入只读缓存）。
  - spec：`executor/streaming/operators/spec/ddl.rs` 的 `SpaceManageCommand` 增加 `PurgeExpired { space_name, target }`；`arena_builder/specs/ddl.rs` 增加构造臂。
  - executor：`schema_executor/space.rs` 增加臂，经 `exec_ddl` 闭包调用 `StorageAdmin::purge_expired`（带 target 时走 schema 级 purge），按 `TtlPurgeEntry` 组装结果集。
- 文档
  - `docs/release/05_other_statements.md` 新增"PURGE EXPIRED - 过期数据物理清理"章节：功能、语法结构、关键特性（无后台任务、按 schema 汇报、有界批量）、示例、错误形态。
  - `docs/release/03_ddl_schema_definition.md` TTL 工作原理第 5 条改写：声明物理删除入口为 `PURGE EXPIRED` 语句，仍无后台定时任务。
  - 在 05 章节注明"当前仅 SQL 入口"（本期不做服务端 RPC）。
- 测试
  - parser：合法形态两种（带/不带 target）；非法形态（缺 `IN`、未知 space、未知 TAG/EDGE、`TAG` 与 `EDGE` 并用）。
  - 执行：顶点 TTL 过期后 purge，物理删除且 `removed_rows` 正确（补 2.4 的边版本）；未配置 TTL 的 schema 返回 0 行；space 不存在报错。
  - 分类：PURGE 不出现在只读缓存可复用集合。

验收标准：上述测试全绿；`cargo check --workspace` 零告警；文档章节与实现逐条对应；`PURGE` 不在只读缓存集合。

### 3.3 问题三：拓扑快路径 TTL 感知降级

目标态：只要展开涉及的边类型（或该 space 全部边类型，当过滤器为空时）任一配置了 TTL，快路径让位物化路径，使结果与"过期即不存在"一致；无 TTL 时行为与性能不变。

存储层：

- `client/traits_reader.rs` 新增 `edge_types_ttl_active(space, edge_types) -> Result<bool, StorageError>`：`edge_types` 为空表示全部边类型；任一 `has_valid_ttl` 即为真。
- 实现放 `engine/graph_storage/reader/`（TTL 判定辅助旁）；`graph_storage.rs`、`engine/sync_wrapper/reader.rs`、`metrics.rs`、`test_mock.rs` 同步实现。
- 顶点 TTL 不影响本组路径：这些路径只遍历边拓扑，终点顶点随后物化时已有 TTL 过滤；文档在 TTL 第 3 条已说明仅边 TTL 与拓扑相关。

查询层：

- `ExpandCtx` 增加 TTL 激活缓存字段：首次计算后按算子实例复用（边类型集合是计划期常量；空集合代表全部边类型）。
- `expand_frontier.rs:108` / `:411` 多跳中间跳：命中即返回 `ColumnarOutcome::Declined`，交回通用物化路径——复用现有降级机制，不新增分支逻辑。
- `expand_row_path.rs:83`：同样以 `Declined` 让位。
- `neighbors.rs:133` GetNeighbors 源算子：状态机首次进入 `Collecting` 时计算并存入算子状态；命中则改用 `get_node_edges`（已按 TTL 过滤）逐块取邻接 ID，保持既有分块与去重语义。
- `expand_frontier.rs:640` `out_degree_batch` 计数尾：命中则逐 seed 走 `get_node_edges` 后计数，度统计因此排除过期边。
- 性能护栏：判定是一次内存 schema 扫描，结果按算子缓存；无 TTL 部署仅多一次判定后恒定 false；既有 `neighbor_batch_bench` 复跑确认无回退。

`count_*` 计数路径：不修。二者都不物化属性，无法判定 TTL；`count_vertices_by_tag` 的定位是 O(1) 洞统计 Cardinality 估计，精确化等于放弃该路径，`count_edges_by_type` 虽为精确行计数但同样不解码属性。计数虚高只影响计划选择与统计展示，不影响查询结果正确性；文档已声明统计可能含过期行，保持不变。

测试：

- 存储单测：无 TTL / 部分边类型有 TTL / `edge_types` 为空三态。
- 集成：边 TTL 场景下多跳 MATCH、度统计展开、GET NEIGHBORS 三类均剔除过期边；非 TTL 场景既有 dql 全量回归不回退。

验收标准：新增测试全绿；全量回归零新增失败；bench 无回退。

## 4. 实施顺序与依赖

三项互不依赖，可任意顺序实施。建议按"风险从低到高、变更从删到增"排列：

1. 问题一：纯删除，风险最低，先行。同时完成 WAL 版本提升。
2. 问题三：行为修正，与 WAL 版本无冲突。
3. 问题二：新增语句与新文档章节，放在最后，可复用前两步清理后的干净基线。

## 5. 风险与回退

| 项 | 风险 | 缓解 |
|----|------|------|
| 问题一 | WAL 版本提升使旧 WAL 文件拒绝启动 | 开发期无兼容义务；启动即显式报错，不做部分回退；recovery 测试覆盖正常路径 |
| 问题一 | 遗漏 IDE 搜索不到的调用点 | 改动前后各跑一次 `cargo check --workspace` 全量确认 |
| 问题二 | 大表 purge 执行时长与内存 | 有界批量删除；按 schema 顺序处理；结果集逐行汇报进度 |
| 问题二 | purge 与并发读的一致性 | 删除走正常 MVCC 写路径；读路径本已过滤过期行，物理删除不改变语义 |
| 问题三 | 降级路径性能回退被误触发 | 按算子缓存判定；仅边 TTL 激活时降级；bench 复跑 |
| 问题三 | GetNeighbors 长生命周期状态机的缓存时机 | 在 `Collecting` 初始化处一次性计算并存入算子状态 |

## 6. 验证方式

1. `cargo check --workspace`：零告警零错误。
2. `cargo test -p linkrs-transaction -p linkrs-core -p linkrs-storage`：问题一为主。
3. `cargo test -p linkrs-query`：问题二、三；含 `tests/ddl` 与 dql 全量。
4. `neighbor_batch_bench` 复跑：问题三性能护栏。
5. 全仓 grep 校验：`AlterSpaceComment` / `alter_space_comment` 零残留；`PURGE EXPIRED` 文档章节与实现逐条对应。

## 7. 明确不做

- 不做 TTL 后台定时清扫任务：与"显式调用"契约冲突；如未来需要，应作为配置项并参照 `StorageGcOps` 的后台任务先例单独立项。
- 不做计数路径（`count_vertices_by_tag` / `count_edges_by_type`）的 TTL 精确化：二者本不物化属性，近似/行计数定位不变，文档已声明边界。
- 不补 `ALTER SPACE ... SET COMMENT`：无文档契约，作为新特性另行立项。
- 不为 PURGE 新增 WAL 操作码：删除走既有记录。
- 不重新编号 WAL 操作码判别式。
- 不为 PURGE 增加 gRPC / HTTP RPC：本期仅 SQL 入口，文档声明。
