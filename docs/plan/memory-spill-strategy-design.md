# 查询内存策略与 spill 代价落地设计

## 结论

`memory-overflow-capacity-design.md` 的“本期落地”已完成。本轮补齐其中“后续方向”的前两项：让计划期内存策略真正影响算子切换，并在代价模型中计入 spill 成本；同时补上受限内存下的回归。整图超内存不在此轮实施，单独记录在 `long-term/whole-graph-beyond-memory-design.md`。

## 问题边界

- 物理计划的 `MemoryPolicy::Spillable { threshold }` 过去只被校验和 EXPLAIN 读取，运行时不消费；阈值还是固定常量，与估算工作集无关。
- 聚合策略选择器只输出诊断说明，不驱动执行；代价模型只有排序按固定行数计外排 I/O，聚合、去重、哈希连接没有 spill 成本。
- 单查询预算已经通过请求上下文注入 `ExecutionContext`，但物理计划会被缓存；把请求预算原样写进计划或用原始预算值扩展缓存键，都会破坏缓存复用并放大键空间。

## 设计原则

- 计划只保存与请求无关的事实：估算工作集和“是否具备 spill 能力”。请求预算在算子实例化时再决定策略，缓存键保持不变。
- 切换只改变进入 spill 的时机，不改变 run 格式、配额、清理和结果语义。
- 不新增内存调度模块，扩展现有 `MemoryPolicy`、`OperatorConfig`、`MemoryTracker` 和代价计算器。

## 方案

### 计划属性

- `populate_estimated_rows` 提前到运行时属性生成之前；`derive_physical_properties` 用算子估算行数乘以统一行宽得到工作集，写入 `MemoryPolicy::Spillable.threshold`。无估算时退回原有默认阈值。
- Sort、Aggregate、GroupBy、WindowFunction、Window、Distinct、Materialize、DataCollect、RollUpApply 和 Grace 哈希连接标记为可 spill；Partial/FinalAggregate、TopN、局部 TopK 没有真实 spill 路径，保持 `RequiresBudget`。
- 因为计划只携带工作集估算，不带请求预算字节数，原计划缓存身份无需变更。

### 执行切换

- 新增 `SpillMode`：`InMemory`、`OnDemand`、`ExternalFromStart { local_limit }`。它由物化器按“工作集是否超过该算子的预算份额”选择，并经 `OperatorBase`、`OperatorConfig` 注入算子。
- `MemoryTracker` 增加本地上限。共享 `MemoryBudget` 仍是最终权威，本地上限只让算子提前走到已有的 spill 分支；哈希连接因此提前进入 Grace 分区，而不是先把共享预算耗尽。
- `BlockingOperator::from_spec_with_mode` 和 `JoinOperator::from_spec_with_mode` 承载新路径，原有 `from_spec` 保持按需模式，供不需要计划策略的调用方使用。
- 所有 spill 数据仍由 `SpillManager` 统一管理文件数、磁盘配额和清理；本次没有新增旁路落盘。
- 回归暴露并修复了 Distinct 落盘路径的两个问题：预算失败时要把当前块的剩余行写入分区 run，不能在消费阶段提前返回结束；本地上限不能拒绝触发 spill 的那一行，否则单超大行会报错。

### 代价模型

- `CostCalculator::estimate_spill_cost` 统一按“工作集减去预算份额”计算溢出字节，再按页数和读写遍数折算 I/O。行宽、页数和页面代价沿用 `CostModelConfig` 中已有的唯一来源。
- 排序按两遍读写计价；聚合与去重按两遍；Grace 哈希连接按四遍（构建与探测两侧各写读一次）。
- 增加带显式预算的代价入口，`AggregateStrategySelector` 在上下文给出 `memory_limit` 时使用它；未给出时仍使用配置阈值，保持诊断输出稳定。

### 回归覆盖

- 本地上限与预算份额选择的单元测试；
- 计划属性携带估算工作集的单元测试；
- spill 代价随工作集和遍数单调增长的单元测试；
- 外部启动排序与内存排序结果逐值一致，且确实产生 spill 指标；
- 单行输入在极小本地上限下不丢行；
- 四个查询并发执行时各自结果与预算隔离；
- 排序与去重在同一查询中同时落盘，结果与基线一致，且至少两个算子有 spill 计数。

耗时只作为基准脚本观测项，不作为 CI 硬断言。

## 边界与后续

- 聚合策略选择器仍以诊断说明为主，执行切换的权威来源是物理属性和请求预算；这避免了把会随预算变化的决策写进缓存计划。
- 磁盘配额与文件数耗尽的底层回归仍由 spill 模块单测覆盖；本轮新增的是端到端多算子与并发路径。
- 若将来需要整图超内存，按长期设计文档分阶段扩展存储层，不在查询侧叠加旁路机制。
