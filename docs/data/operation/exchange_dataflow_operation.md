# 数据流 / 交换（Exchange）操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的数据交换与流控制操作（Exchange / Gather / Barrier 等），
它们不改变数据内容，只负责**在管道片段（fragment）之间搬运、重排与同步数据**，
是分区并行执行骨架的关键环节。本文档与 2026-11-01 代码现状对齐。

---

## 一、定位与作用

流式执行器将计划切分为多个管道片段：片段内部是可逐批推进的流水线（如
Scan → Filter → Project），片段边界则由 Exchange 算子衔接——它消费一个或多个
上游片段的输出，按策略重新组织后交给下游片段。
引擎级调度器以 morsel 风格的共享原子计数器动态分发分区任务
（见 `spec/exchange.rs` 模块注释）。

---

## 二、物理层 ExchangeSpec

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/exchange.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::ExchangeSpec`）

| 变体 | 说明 | 典型用途 |
|------|------|----------|
| `Concatenate { partition_count }` | 按分区顺序串接 N 路输出 | 多分区结果汇聚（Gather） |
| `MergeSort { sort_expressions, sort_directions, limit }` | 对**已排序**的 N 路输入做归并排序 | 各分区有序输出 TopK 合并，支持 `limit` 提前终止 |
| `RepartitionHash { num_partitions, hash_expressions, input/output_layout }` | 按键哈希把行路由到 N 个桶 | Hash Join / Hash Aggregate 对齐分区边界 |
| `Broadcast { num_consumers }` | 每行复制到全部消费者（按大小浅拷贝/深克隆） | 小构建侧广播给所有探测侧分区 |
| `Barrier` | 等待全部输入片段完成后才开始输出（纯同步，不重排数据） | 阻塞阶段排序，如构建侧先于探测 |
| `Materialize { child_count }` | 强制上游片段先完整物化 | 显式 spool / 管道断裂（pipeline break），向调度器与校验器发出断裂信号 |

---

## 三、ExchangeOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/exchange_operator.rs`

`ExchangeOperator` 由 `from_spec` 实例化，关键实现：

- `drain_and_partition`（`:468`）：排空上游并按 `RepartitionHash` 分桶；
- `compute_hash`（`:505`）：以 `hash_expressions` 计算行到分区的路由哈希；
- `next_merge_row`（`:545`）+ `compare_rows`（`:648`）：归并排序逐行取最小/最大行，
  尊重 `limit` 提前终止（测试 `test_merge_sort_honors_limit` 保障）；
- `fill_input` / `advance_input`：多输入游标推进；
- `validate_schema`（`:686`）：校验各输入布局一致。

单元测试覆盖哈希重分区、Broadcast/Barrier/Materialize 的状态构造与 MergeSort limit。

---

## 四、相邻的流控制算子

同目录下还有几个与数据流相关的算子（不属于 Exchange 但同属管道控制）：

| 算子 | 文件 | 作用 |
|------|------|------|
| `gather_operator.rs` | `operators/gather_operator.rs` | 汇聚多路输入（Concatenate 语义的算子化形态） |
| `shuffle_join_operator.rs` | `operators/shuffle_join_operator.rs` | 先重分区再做连接的复合算子 |
| `flatten.rs` | `operators/flatten.rs` | 嵌套/多值结果展平 |
| `copy.rs` | `operators/copy.rs` | 数据复制 |
| `sink_operator.rs` | `operators/sink_operator.rs` | 计划出口：结果行写出 |

与 `ExchangeSpec::Materialize` 对应的显式物化阻塞算子见聚合文档中的
`BlockingOperatorKind::Materialize`。

---

## 五、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **Exchange 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/exchange_operator.rs` |
| **物理 ExchangeSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/exchange.rs` |
| **Gather** | `crates/linkrs-query/src/executor/streaming/operators/gather_operator.rs` |
| **Shuffle Join** | `crates/linkrs-query/src/executor/streaming/operators/shuffle_join_operator.rs` |
| **Sink** | `crates/linkrs-query/src/executor/streaming/operators/sink_operator.rs` |
| **调度器（morsel 分发）** | 引擎级共享调度器（见 spec/exchange.rs 注释） |

---

## 六、总结

1. **六种交换策略**：Concatenate / MergeSort / RepartitionHash / Broadcast / Barrier /
   Materialize 覆盖汇聚、归并、重分区、广播、同步、物化断点全部片段衔接形态；
2. **为并行而生**：RepartitionHash 与 Broadcast 直接服务于 Hash Join/Hash Aggregate 的
   分区对齐与小表广播，是并行执行计划的标准组成部分；
3. **管道断裂显式化**：Materialize/Barrier 将阻塞语义向调度器与计划校验器显式声明；
4. **限流归并**：MergeSort 支持提前终止，配合分区有序扫描可低代价合并 TopK。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
