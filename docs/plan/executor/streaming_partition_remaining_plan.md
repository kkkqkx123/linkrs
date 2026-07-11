# Streaming 分区执行后续实施计划

> 制定日期：2026-07-11  
> 前置文档：`streaming_reliability_plan.md`、`executor_architecture.md`  
> 范围：已完成 P0–P4 后，继续将 streaming 从“语义正确的单线程分区执行”演进为“可由优化器选择、可观测、可并行的物理执行系统”。

## 一、当前基线与结论

已完成的能力：

| 项目 | 当前状态 | 实现边界 |
| --- | --- | --- |
| 生命周期、取消、资源释放 | 已完成 | 执行/流式/Drop 路径共享 teardown，失败保留原错误 |
| 分区 profile 身份 | 已完成 | profile key 含分区身份，避免同一 plan node 相互覆盖 |
| `Gather::Concatenate` | 已完成 | 逐分区拼接，并校验 schema 一致性 |
| `Gather::MergeSort` | 已完成 | 局部排序后多路归并，支持全局排序 limit |
| Aggregate / Dedup / Limit | 已完成 | `Local × N → Gather → GlobalOperator`，语义优先 |
| Join | 已完成 | 左右输入分别 Gather 后执行一个全局 Join，跨分区键可正确匹配 |
| 生产执行入口 | 已完成 | materialized 与流式入口均消费显式 `PartitionSpec` |

当前实现的正确性优先于并行度：所有跨分区全局语义在一个 global operator 中完成。因此它不是 hash/range shuffle，也不是 broadcast join；不能将当前形态宣传为并行 join。

最重要的结构性限制是 factory 按根节点类型选择物理形态。它能处理单个 global root，但不能自然组合例如 `Limit(Sort(Scan))`、`Aggregate(Dedup(Scan))`、`Sort(Join(...))` 等多个 exchange 边界。后续工作必须先消除这个限制，再增加性能优化。

## 二、设计原则与非目标

### 2.1 原则

1. `PartitionSpec` 只能由有明确数据范围来源的规则生成；禁止猜测 `0..u32::MAX`。
2. 分区布局、exchange 类型和 local/global 边界属于物理计划，而非 executor factory 的隐式推断。
3. 每一次优化都必须可回退到单树执行，并在 `EXPLAIN` 中说明原因。
4. 在并行前完成共享内存预算、取消传播、错误汇聚和 profile 语义；不得通过“每个线程一个独立预算”绕过全局上限。
5. 不为尚无语义证明的算子复制局部树。未知节点必须显式单节点 fallback 或报不支持。

### 2.2 本轮非目标

- 不恢复旧 pipeline 模块，也不在现有 Volcano tree 外再引入仅作分类用途的 DAG。
- 不实现磁盘 spill；预算不足必须返回可诊断错误。
- 不把 Gather 伪装成 shuffle/broadcast。
- 不在物理计划和线程安全边界未确定前启用 Rayon 或任意 worker pool。

## 三、P5：分区物理计划与自动选择

### 3.1 问题

当前 `ExecutionPlan` 虽有 `PartitionSpec`，但没有描述：

- 哪个 scan 使用哪些 range；
- 哪些子树为 local；
- 哪个位置要 Gather、MergeSort、Shuffle 或 Broadcast；
- global 算子如何串联。

这导致 factory 只能从 root 反向特判，且优化器不会自动填充 `PartitionSpec`。

### 3.2 修改方案

1. 在 `planning/plan/` 新增物理分区描述，建议命名为 `PartitionedPhysicalPlan`，不修改逻辑 `PlanNodeEnum` 的语义。

   ```text
   PartitionedPhysicalPlan
   ├── Local { partition_spec, logical_subtree }
   ├── Gather { input, mode }
   ├── MergeSort { input, keys, limit }
   ├── Global { logical_subtree, inputs }
   ├── Shuffle { input, keys, partition_spec }       // P7 启用
   └── Broadcast { input, partition_spec }           // P7 启用
   ```

   第一版可只允许 `Local → Gather/MergeSort → Global` 的树形组合；不必提前实现 Shuffle/Broadcast 数据通道。

2. 将 `StreamingQueryExecutor::from_partitioned_plan_node()` 替换为接收上述物理计划的入口。builder 只负责把一段已经标记的逻辑子树构造成 executor，不再决定 Gather 应插在何处。

3. `PartitionSpec::try_new()` 取代可 panic 的构造路径，校验：非空、range 非空、按 start 严格排序、无重叠；保留允许不连续范围的能力。分区布局需携带来源（顶点 ID、边 ID、索引范围）与可选 snapshot/version，防止计划缓存跨数据布局复用。

4. 新增 `PartitioningPlanner`，输入为根计划、执行模式、统计信息和配置，输出 `Option<PartitionedPhysicalPlan>`：

   - 仅扫描范围可从 storage metadata 明确取得、预估行数超过阈值、且树中没有未支持节点时选择分区；
   - 统计缺失、范围未知、写操作、事务边界、递归图遍历、volatile expression 时返回 `None`；
   - 分区数取 `min(config.max_partitions, available_ranges, cpu_hint)`，首版默认上限为 1，只有显式配置才大于 1；
   - 计划缓存 key 必须包含 partitioning 配置与布局版本，或缓存中不保存物理 partition 结果。

5. `EXPLAIN`/`PROFILE` 显示：是否分区、range、exchange、fallback 原因和预估行数。API 的执行模式 reason 复用该决策文本。

### 3.3 验收标准

- `Limit(Sort(Scan))`、`Aggregate(Dedup(Scan))`、`Sort(Join(...))` 能表达为明确的多边界物理计划，而不是 root 特判。
- 不含 `PartitionSpec`、统计缺失、或算子不支持时稳定回退单树，结果不变。
- 非法 range 与过时布局不会进入执行器。
- 同一逻辑计划在不同 partition 配置下不会错误命中同一物理缓存项。

## 四、P6：全局算子的局部化与内存模型

### 4.1 Aggregate 与 GroupBy

当前 Aggregate 在 Gather 后全量执行，语义正确但放大 global 内存和 CPU。不能直接复用已有 Aggregate 作为 partial/final：`AVG` 至少需要 `(sum, count)`，`COUNT` 与 `SUM` 的输入/输出类型也不同。

修改方案：

1. 新建显式 accumulator state API，而不是以 `Value` 最终结果作为 partial state：
   `Count(u64)`、`Sum(NumericAccumulator)`、`Min(Option<Value>)`、`Max(Option<Value>)`、`Avg { sum, count }`。
2. 增加 `PartialAggregate` 与 `FinalAggregate` 物理节点；partial 输出 group key + typed accumulator state，final 负责 merge 与最终格式化。
3. 第一批仅支持 `COUNT`、`SUM`、`MIN`、`MAX`、`AVG` 和无 grouping/grouping 两种情况。每个函数需定义空输入、NULL、整数溢出和浮点语义。
4. `COLLECT`、`DISTINCT aggregate`、percentile、median、mode、方差类函数继续走 Gather 后单节点，直到有独立的 merge state 与测试。
5. Hash group table 按 entry/capacity 增量记账；final/close 释放精确预留字节。

### 4.2 Distinct、TopN、Limit

1. Distinct 改为 `LocalDistinct → GlobalDistinct`。row key 不再使用 `format!("{:?}")`，改为稳定的 typed row key/编码；local 与 global hash set 都按容量计账。
2. TopN 改为 `LocalTopN(N) → MergeSort(limit=N)`；无排序的 Limit 仅允许全局 limit，避免宣称无序输入具有稳定顺序。
3. OFFSET 维持 global-only；排序 `OFFSET + LIMIT` 必须在 merge 完成后应用。
4. 对每个优化添加“分区结果等于单树结果”属性测试，覆盖空分区、重复值、NULL、单个大 chunk 与多 chunk。

### 4.3 验收标准

- 小预算下 partial/final Aggregate、两级 Distinct、TopN 均在上限前失败，`MemoryBudget::current()` 最终归零。
- AVG、NULL COUNT、跨分区同 group、跨分区重复行与单树结果一致。
- profile 可区分 local peak、exchange peak 与 global peak，查询总 peak 语义明确为执行期间共享预算的峰值。

## 五、P7：真正的 Join Exchange

### 5.1 当前限制

P4 的双 Gather Join 会把两侧完整数据送到一个 global Join；其结果正确，但没有利用分区并行，也可能使 build side 超出单个 global operator 的内存预算。

### 5.2 修改方案

先实现单线程、可物化验证的 exchange，再考虑并发：

1. 定义 `PartitionedBatches { schema, buckets: Vec<Vec<DataChunk>> }` 以及 schema/row-key 规则。所有 bucket 共享 query runtime 和 memory budget。
2. `HashShuffle` 根据 join key 的稳定 typed hash 将两侧数据放入相同 bucket；NULL key 的处理必须与当前 Join 语义一致并单独测试。
3. 逐 bucket 构建 `HashJoin`，输出按 bucket 顺序 concatenate。第一批仅启用 `HashInnerJoin` 与 `HashLeftJoin`；Right/Full/Semi 的 unmatched 语义另行设计和验证。
4. `Broadcast` 只允许右侧估算大小低于 `broadcast_max_bytes`。右侧先 materialize 一次，随后为每个左侧 bucket 创建只读引用/可重放输入；禁止重复读取 storage。若估算或实际预留超过阈值，回退 HashShuffle 或 GlobalGather。
5. planner 的 join 策略依次为：co-partitioned（未来）→ broadcast 小右表 → hash shuffle → global gather fallback。所有选择写入 EXPLAIN。
6. 交换缓冲、hash table 和 broadcast materialization 必须纳入共享预算；任一 bucket 失败时取消其余 bucket，并关闭全部树。

### 5.3 验收标准

- 交叉分区的 HashInner/HashLeft 与单树结果一致，包含重复 key、NULL、空 build/probe side。
- broadcast 右侧只扫描一次，结果与 HashShuffle 一致。
- 预算不足、某 bucket 错误、取消和提前断连时，所有 bucket 资源释放且错误不被覆盖。

## 六、P8：并行执行与背压

### 6.1 前置条件

只有 P5–P7 完成且 storage/client/executor tree 满足 `Send + Sync` 或拥有明确线程封送层时才开始。当前 `Box<StreamingExecutor>`、storage client 与部分 state 是否可跨线程，必须先用编译期断言和 API 审计确认。

### 6.2 修改方案

1. 引入小型、受配置限制的 worker executor；任务粒度是 local tree 或 shuffle bucket，不是每一行。禁止无界 spawn。
2. 每个任务返回 `Result<PartitionOutput, QueryError>`；协调器持有唯一的 cancel token，首个错误触发 cancel，等待/回收其他任务，再返回首个执行错误。
3. `Gather::Concatenate` 保持稳定 partition order；`MergeSort` 仍由协调器单点多路归并；不得因并行导致无排序查询承诺新顺序。
4. 使用有界 channel 或 bounded output queue 实施背压；容量按 chunk 数配置并计入内存预算。客户端断连使 coordinator cancel 所有 worker。
5. profile 分为 wall time、CPU work time、queue wait time。总 wall time 不累加 worker 耗时；总 rows 求和，peak 为查询期间共享预算峰值。

### 6.3 验收标准

- 结果与单线程分区/单树一致，且 Concatenate、MergeSort 顺序稳定。
- 阻塞消费者、取消、worker panic/错误不会遗留线程、channel 或 budget 预留。
- 基准显示在足够大的 scan/filter/partial aggregate 上有可重复收益；小查询自动保持单线程以避免调度开销。

## 七、P9：完成标准与提交顺序

| 提交 | 内容 | 依赖 |
| --- | --- | --- |
| 1 | `PartitionSpec` 校验、布局来源/version、物理计划数据结构 | 当前 P0–P4 |
| 2 | `PartitioningPlanner`、配置、EXPLAIN 和缓存隔离 | 提交 1 |
| 3 | 物理计划 builder，移除 factory 根节点特判 | 提交 1–2 |
| 4 | partial/final Aggregate、两级 Distinct、TopN 及内存测试 | 提交 3 |
| 5 | 单线程 HashShuffle 与受限 Broadcast Join | 提交 3–4 |
| 6 | 并发 worker、背压、并行 profile 与基准 | 提交 5 |

每个提交至少包含：单树等价测试、失败/取消/预算释放测试，以及一条 materialized 或 SSE/gRPC 入口测试。任何尚未被物理计划表达的算子都应保持单节点 fallback，不得通过复制局部树“尝试并行”。

## 八、优先级

1. P5（物理计划与自动选择）是下一项必须完成的工作；没有它，分区能力仍依赖调用方手工提供 `PartitionSpec`。
2. P6 中的 accumulator 与两级 Distinct 是降低 Gather 瓶颈的最高收益项。
3. P7 先做可验证的单线程 exchange，再做 P8 并行；这能将数据分布语义与线程调度风险分离。
