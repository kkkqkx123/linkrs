# Typed 分区局部聚合设计

日期：2026-11-19
对应问题：`docs/issue/linkrs-remaining-parity-issues.md`
当前基线：`perf-data/parity/results/2026-11-18/stage8-final/`

## 背景与结论

有序并行展开已经把固定门中位数从 `distinct_limit` 305 / 313 ms、`group_by_count` 283 / 306 ms 降到 139 / 130 ms 与 128 / 149 ms，九条纯计数无回归。PROFILE 显示 `requested=16, actual=12`，并行段 wall time 约 125 ms、work time 约 1.2 s；每个分区内的源扫描约 55–62 ms，`ExpandAll` 约 90–99 ms，局部聚合约 100 ms，缓冲峰值 11 块 / 1.31 MB，无 spill。

当前并行前段仍保持 `rows: 0` 的 typed 路径，但 `PartialAggregate` 入口按兼容性原则把 typed-only chunk 重建为行视图，再走通用累加器。分组计数在每批 chunk 上都经历一次 typed → row 转置与逐行 `Value` 分配，这是分区内剩余的直接可控成本。`Distinct` 的 blocking 算子本身已支持单列整数 typed 键，无需再引入分区局部去重状态。

上一轮并行度扫描说明 16 个 worker 只落到 12 个实际分区，经济门按默认 `min_rows_per_partition` 截断了分区数；继续提高 worker 请求数前，应先收紧分区内的 typed 消费路径，并用同一 worker 数做对照，避免把配置效应误算成算子收益。

## 目标

- 让分组计数在分区局部状态上直接消费 typed 列，不再为每个输入 chunk 重建行视图。
- 保持串行与并行结果逐格一致：NULL 分组、重复边、空组、计数参数 NULL 性与最终排序全部不变。
- 与阶段八基线比较，`group_by_count` 的端到端中位数再下降至少 15%；`distinct_limit` 与九条纯计数不回归。
- 复用既有内存预算和最终合并路径，不新增第二套聚合实现；若实测无收益，撤回 typed 局部状态复杂度而不是保留备用分支。

## 设计

### 共享 typed 分组计数状态

把现行 `Aggregate` 中的固定宽度分组计数能力抽到 `blocking/aggregate_operator/` 的共享模块，由 `AggregateState` 与 `PartialAggregateState` 共同持有。状态只保存 `TypedGroupKey` 到计数的映射、键类型和每键记账字节数，键类型限定为非 NULL 的 `I64`/`I32`/`Bool`。

现有的键类型识别、键读取、计数参数 NULL 位图、每键预算预留与降级转移逻辑统一到该模块，避免在局部聚合和最终聚合中各写一份。最终聚合继续用现有 accumulator 合并接口，`PartialAggregate` 产出的行格式不变。

### PartialAggregate 的直接消费

局部聚合在收到 typed-only chunk 时，先按窄形状检查：单列变量分组键、单个 `Count` 函数、无聚合 FILTER、无 selection、键列宽度与 typed 长度一致且属于支持的固定宽度类型。满足时直接从 typed 列读取分组键与计数参数的 NULL 位图，累计到局部 typed 状态；不满足时把当前 typed 状态转移到通用 group map，再走原有逐行路径。

`count(h)` 在 rowless Project 中替换为非空单位列的行为保持不变：typed 路径只依据该列的 NULL 位图决定是否计数，不改变计数值来源。可空分组键、字符串键、多列分组、聚合 FILTER 和 `SUM(_expand_count)` 形状一律走通用路径。

### 降级与内存契约

typed 状态与通用状态共用 `MemoryTracker`，按新键数量和与通用路径相同的每键字节数预留。任一 chunk 不满足窄形状时，先把 typed 计数以 `Count` accumulator 的形式转移到通用 group map，并保留已记账内存，再继续处理该 chunk；混合 chunk 不得分别计数后简单相加。

本阶段不新增局部 spill 机制：局部状态超限沿用当前错误路径，不做静默截断或隐式降级到无预算状态。最终合并仍使用既有 `FinalAggregate` 的 accumulator 与 spill 管理，不绕过 run 合并规则。

### Distinct 与最终阶段

`Distinct` 继续走现有 blocking 算子：rowless typed chunk 由 `Exchange` 按分区序号透传，`Distinct` 用单列整数 typed 键集合完成全局去重，最终 `TopN`/`Limit` 只在合并后执行。本阶段不为 `Distinct` 增加分区局部集合，避免重演目的端唯一化在小基数场景的回退。

### 并行度校准

固定门继续以 16 worker 为默认对照；另跑一组降低 `min_rows_per_partition` 的对照，验证实际分区数能否从 12 提升到 16 以及收益是否来自分区数而非算子改动。配置只在 benchmark 命令中覆盖，不修改生产默认值。

若 typed 局部聚合落地后 `ExpandAll` 仍占主要 wall time，再单独立项审计分区内邻接读取；在该审计完成前不修改目的端读取接口，不把属性内联进 CSR，也不建立跨调用可见性缓存。

## 实施顺序

- 抽出共享 typed 分组计数模块，并让现有 `Aggregate` 保持等价行为；先跑聚合相关单测。
- 给 `PartialAggregateState` 增加 typed 状态，接入直接消费与降级转移。
- 补 typed/通用对照单测：固定宽度键、NULL 键、NULL 计数参数、混合 chunk、多次降级、预算超限。
- 扩展端到端等价测试：并行与串行在分组计数、重复边和空组下逐格一致；PROFILE 确认局部聚合前保持 `rows: 0`。
- 在 devs 上做 release 编译、完整正确性门、两轮固定门与 worker 校准对照；无收益则撤回本次代码改动。

## 正确性不变量

- 同一查询快照内的 MVCC 与 TTL 判定不变，typed 路径不读取跨调用缓存。
- NULL 分组键必须保持独立分组；可空 typed 列不进入固定宽度 fast path。
- 计数参数 NULL 性只影响计数值，不改变分组是否存在。
- 降级转移保留已记账内存和已累计计数，不重复计数、不丢失分组。
- 分区序号决定 `Exchange` 输出顺序；最终排序与 LIMIT 仍在全局合并后执行。
- 并行 worker 只做只读存储访问，不绕过事务暂存、pending gate 或权限检查。

## 观测与验收

- PROFILE 中 `PartialAggregate` 前一行保持 `rows: 0`，局部聚合输出行数与串行路径分组数一致；每分区 spill 计数保持为 0 或与串行路径一致。
- `EXPLAIN ANALYZE` 继续报告 `actual_workers > 1`、wall/work time 与缓冲峰值；worker 校准对照报告实际分区数。
- devs release 编译通过；DQL 319/319、streaming lib 414/414、`parallel_partition_execution` 全绿。
- 固定门两轮对照阶段八：`distinct_limit` 139 / 130 ms 与 `group_by_count` 128 / 149 ms 的对应项需有可复现改善且无回归；九条纯计数维持 0.14–0.60 ms。
- 原始日志与 JSON 落在 `perf-data/parity/results/2026-11-18/` 下对应阶段目录。

## 风险与应对

- **内存放大**：每个分区的 typed 键集合与通用 group map 可能同时短暂存在。降级转移立即清空 typed 状态，内存只按当前权威状态记账；预算超限直接报错。
- **语义漂移**：typed 键读取忽略 NULL 位图会把 NULL 错误并入占位值。窄形状检查必须排除 nullable 列，并用 NULL 键单测锁定。
- **收益不明确**：分区内 typed 消费只占 wall time 的一部分。用同 worker 数对照和 PROFILE 双重判定；达不到门槛就撤回，不保留双路径复杂度。
- **并行度混淆**：worker 数变化会带来计划形状变化。先固定 worker 数比较算子改动，再单独做分区数校准。

## 不做

- 不修改 CSR 布局、边身份、外部 MVCC 权威或目的端读取接口。
- 不引入跨调用可见性缓存，不把属性永久内联进拓扑。
- 不为 `Distinct` 增加分区局部集合或提前执行 TopN / LIMIT。
- 不改变生产环境默认并行配置；灰度开关继续默认关闭。
- 不在本阶段处理图算法、无方向元数据计数或存储持久化等独立问题。
