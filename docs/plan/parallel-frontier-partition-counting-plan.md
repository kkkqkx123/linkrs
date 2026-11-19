# 并行计数前沿与分区元数据计数设计

**日期**：2026-11-19  
**问题来源**：`docs/issue/linkrs-remaining-parity-issues.md` 第 19、20 节；`docs/plan/linkrs-remaining-parity-implementation-plan.md` 阶段二之后。  
**状态**：后续阶段设计，本文件不含实施代码，不重新定义已闭环问题。

## 1. 背景与目标

阶段二已经把 `join_two_hop` 与 `variable_path_count` 从秒级压到个位数毫秒：当前四轮中位数分别约为 6.3–6.8 ms 与 6.7–7.2 ms，`EXPLAIN ANALYZE` 中 `ExpandAll` 约 7.8 ms，顶点扫描约 0.6 ms。当前 `Parallel Workers` 对这两个用例仍显示 `requested=16, actual=1`，因为并行形状闸门明确排除了 `count_only` 展开。

本阶段的目标不是继续微调单次邻接读取，而是把已经证明正确的标量计数前段放进现有并行执行通道，并在更后续用分区元数据计数取代可证明等价的种子扫描。两个方向按顺序实施：

1. **并行计数前沿**：把每个分区的种子扫描与批量度数调用并行化，局部只产出整数计数，最终复用现有 `COUNT → SUM(_expand_count)` 合并。
2. **分区元数据计数**：当查询形状和存储自证信息足够强时，直接从每个数据域分区取边数或顶点数，不再扫描种子，也不构造计数前沿。

目标用例保持 `join_two_hop`、`variable_path_count` 与九条纯计数；`group_by_count`、`distinct_limit` 作为回归门。期望并行前沿相对当前单线程再取得可复现收益；元数据计数只放行前置可证明的形状，其余形状保持当前路径。

## 2. 参考实现分析

### 2.1 取证边界

当前工作树没有 `ref/neug` 与 `ref/ladybug` 源码目录；devs 上只有 Ladybug 0.22.3 与 Neug 0.3.1 的 wheel 和 Python 包。以下分析以仓库内此前基于这些源码写成的 `docs/research/query/neug_query_statements.md`、`docs/research/query/ladybug_query_statements.md`、`docs/research/query/query_comparison_analysis.md`、`docs/research/csr_neug_vs_current.md`，以及 `docs/issue/count-slow-vs-ladybug.md`、`docs/plan/count-slow-vs-ladybug.md` 的实测结论为依据。wheel 只用于确认接口与版本，不据此推断未公开的算子实现细节。

### 2.2 Neug 可参考的部分

- **CSR 视图而非边对象**：研究文档记录 Neug 的 `GenericView`/`NbrList` 以零拷贝区间暴露邻接，`ImmutableNbr`/`MutableNbr` 在槽位内保存邻居与数据，迭代按读时间戳判定可见性，有序前缀还能二分。可借鉴的是“一次解析 schema、按区间读拓扑、避免构造通用边对象”的读形态；Linkrs 已有 `neighbor_dst_ids_batch`、`out_degree_batch` 与 `neighbor_degree_batch`，下一阶段应让分区任务复用这些原语，而不是在查询层重建一套邻接迭代器。
- **单类型/多类型区分**：Neug 用 `EdgeStrategy{kNone,kSingle,kMultiple}` 决定 CSR 类别，研究文档也记录了当前项目在单表单向下可跳过去重集合。可借鉴的是把“分区内是否需要去重”作为显式形状判断，而不是让每个 worker 都无条件建哈希集合。
- **同快照读语义**：Neug 的可见性由读时间戳统一约束。Linkrs 已由绑定执行上下文提供同一快照；并行 worker 必须共享该快照，不因分片而各自取新的时间戳。
- **不参考的部分**：Neug 的行内属性、模板化 CSR 变体和逐顶点写锁与 Linkrs 的拓扑/属性分离、外部版本权威和多核只读模型冲突，不能局部移植。既有问题文档也注明 Neug 的基准是 AP 单线程 wall-clock，只作参照，不能把它的并行调度当作性能证据。

### 2.3 Ladybug 可参考的部分

- **计数直达存储**：`docs/issue/count-slow-vs-ladybug.md` 记录 Ladybug 的同语义纯计数直接在 CSR/元数据上数边，亚毫秒返回，且不受变量绑定影响。Linkrs 的元数据计数和阶段二的融合度数读取是同类思路，下一阶段应继续减少中间行数，而不是把计数变回行流水线。
- **线程配置与可观测性**：wheel 接口暴露 `CALL threads=N` 与 PROFILE，说明线程数是会话级可观测配置。Linkrs 已有 `ParallelConfig`、共享调度器和 `EXPLAIN ANALYZE` 并行字段；新阶段应补齐“请求/实际分区数、每个局部计数、回退原因”，而不是新增第二套报告格式。
- **ALGO 扩展的边界**：两份研究文档都把 PageRank、WCC、SCC、k-core 归入扩展函数，说明算法图与 MATCH 计数属于不同任务。本阶段不借图算法函数形态，只借“算法输入是投影后的拓扑视图、输出是规约后的标量/小表”的执行边界。
- **不参考的部分**：Ladybug 的投影图伪库、`CREATE GRAPH/USE GRAPH` 和 Kùzu 特定 DSL 不在 Linkrs 当前架构内；不能为了对齐计数而引入多图命名空间。

### 2.4 可借鉴项与落地关系

| 参考设计 | 可借鉴点 | Linkrs 落点 | 不移植原因 |
|---|---|---|---|
| Neug CSR 区间视图 | 按拓扑区间批量读、避免边对象 | 复用 `neighbor_degree_batch` 与其分区切片 | Linkrs 已有冷热分离 CSR 和外部版本权威 |
| Neug 单/多类型策略 | 显式区分是否需要去重 | 形状闸门记录 resolved edge type 数 | 去重仍由存储原语负责 |
| Ladybug 计数直达 | 跳过行物化，局部只保留标量 | 分区任务返回整数，最终 `SUM` | 不改变计数语义 |
| Ladybug 线程/PROFILE | 线程配置与执行统计同屏可见 | 扩展现有 `EXPLAIN ANALYZE` 字段 | 不新增报告通道 |

## 3. 设计原则

- 正确性优先：只放行可证明等价的窄形状；任何 TTL、属性谓词、路径语义、采样、`OPTIONAL MATCH`、多聚合 FILTER 都保持当前串行路径。
- 复用原模块：并行调度、`Exchange`、有界缓冲、`PartialAggregate`/`FinalAggregate` 和 `SUM(_expand_count)` 重写都沿用现有实现，不新增替代执行器。
- 局部状态最小化：计数前沿的局部结果只有整数；分区元数据计数的局部结果只有边数/顶点数。不做分区局部 group map，也不做局部去重集合。
- 同一快照：所有 worker 共享绑定的读时间戳、pending gate 和事务上下文；不建立跨调用可见性缓存。
- 默认关闭：新增放行开关与既有灰度位并列，生产默认不变。

## 4. 阶段一：并行计数前沿

### 4.1 放行形状

从计划根向下只穿过纯 `Project` 与恒真 `Filter`，只接受无分组、无 `DISTINCT`、无聚合 FILTER、全部为 `COUNT` 的聚合。其下是带 `count_only` 注解的 `ExpandAll`，且只接受：

- `step_limits` 为 `[2]` 或 `[1,2]`，或单步 `step_limit = 1`；
- 无边/目的端属性需求、无展开过滤、无字面种子；
- 路径语义为 `None` 或 `Walk`，`include_empty_paths` 关闭；
- `closed_loop` 注解成立，存储端在运行时再次验证 schema 闭合；
- 底层是带标签的顶点扫描，且扫描范围可由现有分区机制切分。

TTL 不在计划期判断，由执行期检查：任一相关边类型启用 TTL 时该分区回退到物化行数统计，不返回不完整计数。

### 4.2 执行模型

分区边界仍由现有 `ScanVertices` 的 `shard_slice`/顶点 ID 区间生成，每个分区任务只拿自己的种子片段。任务内部按批调用现有批量度数原语：单步用 `out_degree_batch`，固定二跳与 `1..2` 用 `neighbor_degree_batch`。每个分区处理完所有种子后产出一条 `_expand_count` 行，内容的整数等于该分区的局部计数。

`Exchange(Concatenate)` 按分区序号串联这些计数行，最终聚合仍使用既有 `SUM(_expand_count)` 重写。局部计数是整数，不做交换律以外的假设，也不依赖 worker 完成顺序；分区序号只决定稳定输出顺序，便于 PROFILE 对照。

有界缓冲沿用 `max_buffered_chunks`，计数行很小，但需要计入 typed 字节记账；背压时 worker 暂停领取新 morsel。取消或错误仍经既有 `PartitionHandle` 传播，整批任务停止，不保留部分结果。

### 4.3 不生效时

任一条件不满足时，计划不产生分区树，`EXPLAIN` 保持当前单 `ExpandAll` 形状。运行时 TTL 或 schema 自证失败时，分区任务回退到当前物化行数统计，但最终合并方式不变；回退原因必须出现在 `EXPLAIN ANALYZE` 中。

## 5. 阶段二：分区元数据计数

### 5.1 前置证明

元数据计数比并行前沿更强：它跳过种子扫描。放行前必须同时满足：

- 查询为单一边类型、单方向、单标签端点、无谓词、无分组去重的纯计数；
- 存储能给出该空间/边类型的可信顶点 ID 域与布局版本，且分区边界与边表分片一致；
- 同一快照下，每条可见边或顶点只落入一个分区；`Both` 方向只有在端点同标签且定义确定性归属后才可考虑；
- TTL 与采样关闭，边类型/标签不存在时返回零而不是报错。

任一前置缺失即回退到阶段一的并行计数前沿或当前串行路径，不做部分元数据替换。

### 5.2 执行与合并

每个分区调用存储层的分区计数原语，返回该分区在快照下的可见边数或顶点数。原语必须是纯读，复用绑定的读时间戳与 pending gate；不把全局 `count_edges_by_type` 当作分区结果直接切分，除非其快照语义和分片归属已核验。

最终聚合仍复用 `SUM(_expand_count)`，因此从并行前沿到元数据计数只是替换局部计数的来源，不改变计划末端的数值契约。`EXPLAIN` 必须显示元数据计数来源、实际分区数与每个分区的计数值，便于和前沿版本逐例对照。

## 6. 配置与可观测

在 `ParallelConfig` 增加两个默认关闭的放行位：`allow_parallel_count_frontier` 与 `allow_partition_metadata_count`，与 `allow_pure_count_chain`、`allow_global_aggregate`、`allow_ordered_parallel_expand` 并列；`config.toml` 与服务透出同步补齐。 worker 数、最大分区数和最小每分区行数沿用现有配置，不新增第二套经济门。

`EXPLAIN ANALYZE` 至少补充：

- requested / actual workers 与 partitions；
- 每个分区的局部计数、是否走元数据或前沿；
- backpressure 下缓冲块与字节峰值；
- 串行回退原因（形状、TTL、schema 自证、worker 数、预算）。

这些字段只用于展示，不参与代价决策。

## 7. 正确性不变量与测试

- 整数结果与串行路径逐例完全相等；空输入仍输出单行零。
- 自环、平行边、悬空端点按边出现次数计数，不因分区边界重复或丢失。
- 分区输出顺序稳定；`SUM` 不受完成顺序影响；不提前执行 `TopN`/`LIMIT`。
- 并行 worker 只做只读访问，不绕过事务暂存、TTL、pending gate 或权限检查。
- 分区元数据计数必须有存储自证数据域和布局版本，缺失时回退而非猜测。

测试分层：优化器形状闸门正反例；执行层单分区/多分区整数合并与取消；存储层分区计数与全局计数对照；DQL 端到端数值与 `EXPLAIN` 标记；`parallel_partition_execution` 与 partitioning 单测全绿。

## 8. 实施顺序与验收

1. 先把 `count_only` 形状接入现有有序并行闸门，跑通单步纯计数；保持多跳与范围形状关闭。
2. 放开 `[2]` 与 `[1,2]` 的融合度数前沿，要求每个分区复用 `neighbor_degree_batch`，不新增邻接读取接口。
3. 补 `EXPLAIN ANALYZE` 的分区计数与回退原因，之后再做数值和性能对照。
4. 单独论证并实现分区元数据计数；论证未完成前不启用该放行位。
5. devs 上 release 编译、DQL、streaming lib、`parallel_partition_execution` 与 partitioning 单测全绿；固定门按形状 × 放行开关 × worker 数跑矩阵，并用不受分区影响的边计数作对照，区分真实收益与机器漂移。
6. 若 `work_time` 随 worker 数上升或结果不一致，按既有封顶结论缩小并行度或关闭对应放行位，不通过放宽形状换取数字。

回滚按模块进行：关闭配置开关即可停用新形状；若需要代码回滚，只还原配置、形状闸门、分区链装配与原语调用四处原模块，无需数据迁移。

## 9. 不做

- 不做跨分片事务、动态重分区或在线迁移。
- 不为局部计数引入 group map、去重集合或近似算法。
- 不修改 CSR 布局、边身份、外部 MVCC 权威，不把属性内联进拓扑，不建立跨调用可见性缓存。
- 不把 Linkrs 专用方言用例计入三方数值对比；本阶段关注同引擎串行/并行等价与性能。
- 不在本阶段实现 PageRank、WCC、k-core、SCC、Louvain；算法图属于后续独立阶段。
