# 聚合操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的聚合与阻塞类操作（Aggregate / GroupBy / TopN / Distinct /
Window 等），包括其语义、物理 spec、执行器实现与内存管理。
本文档与 2026-11-01 代码现状对齐。

聚合属于**阻塞算子**：必须完整读取全部输入后才能产出结果，与流式算子（Filter、Project、
Expand 等）的逐批推进模式不同。

---

## 一、逻辑层与物理 spec

规划层为 `RETURN count(*) GROUP BY x`、`ORDER BY ... LIMIT n`、`DISTINCT`、窗口函数等
生成对应逻辑节点，落地为 `BlockingSpec`
（`crates/linkrs-query/src/executor/streaming/operators/spec/blocking.rs`，
经 `spec.rs` shim 统一 re-export），与执行器状态枚举一一对应。

---

## 二、物理层 BlockingOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/blocking.rs`

`BlockingOperator` 由 `from_spec(BlockingSpec, MemoryBudget, SlotLayout)` 实例化，
状态枚举 `BlockingOperatorKind` 共 13 种变体：

| 变体 | 说明 | 关键字段 |
|------|------|----------|
| `Sort` | 全量排序 | `sort_expressions`、`sort_directions` |
| `Aggregate` | **Hash 聚合**（核心路径） | `group_by_expressions`、`aggregate_functions: (AggregateFunction, args)`、`output_col_names` |
| `GroupBy` | 仅分组不聚合 | `group_by_expressions` |
| `PartialAggregate` / `FinalAggregate` | **两阶段聚合**（分区并行形态）：先局部聚合再全局合并 | 同 `Aggregate` |
| `TopN` | 排序 + 截断（Sort+Limit 的合并优化产物，只保留堆顶 N 行） | `n`、排序键 |
| `Distinct` | 去重 | — |
| `WindowFunction` / `Window` | 窗口函数 | `window_exprs`、`partition_by_exprs`、`order_by_exprs` |
| `Materialize` | 物化输入供复用（如 CTE 工作表） | — |
| `DataCollect` | 数据收集（DML 结果收集等） | — |
| `RollUpApply` | GO 语句多跳结果上卷 | `rollup_expressions` |

每个变体都携带独立的 `*State`（`Option<...State>`）与 `MemoryTracker`。

---

## 三、执行机制

### 3.1 Hash 聚合流程

```
1. 逐批消费输入，计算 group_by_expressions 得到分组键
2. 以分组键哈希定位聚合组，对每个 aggregate_functions 做 step 累积（AggregateState）
3. 输入耗尽后，对每组执行 finalize，按 output_col_names 产出结果行
```

无 GROUP BY 时退化为单组全局聚合。

### 3.2 两阶段聚合

`PartialAggregate` → （数据交换/重分区）→ `FinalAggregate`：
局部先聚合压缩数据量，全局阶段只合并部分聚合结果，
为多分区并行执行预留的形态（单机下亦可减少shuffle数据量）。

### 3.3 TopN 优化

`ORDER BY ... LIMIT n` 在优化阶段合并为 `TopN`：以固定大小堆只维护前 N 行，
内存 O(n) 而非 O(全量行数)。`tests/e2e/optimizer.rs` 与 EXPLAIN 测试对此做回归保护。

### 3.4 内存管理

所有阻塞变体均持 `MemoryTracker`（受 `MemoryBudget` 预算约束）；
`Sort`/`Aggregate` 等超限时走外部溢出（spill）路径，
与 Join/Set 算子共用 `executor/base` 的预算基础设施。

---

## 四、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **阻塞执行算子** | `crates/linkrs-query/src/executor/streaming/operators/blocking.rs` |
| **物理 BlockingSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/blocking.rs` |
| **聚合函数定义** | `crates/linkrs-core/src/types/operators.rs`（`AggregateFunction`） |
| **内存预算** | `crates/linkrs-query/src/executor/base/`（`MemoryBudget` / `MemoryTracker`） |
| **聚合/TopN 回归测试** | `crates/linkrs-query/tests/e2e/optimizer.rs`、`tests/dql/optimizer.rs` |

---

## 五、总结

1. **阻塞算子统一管理**：13 种变体共享同一 `BlockingOperator` 骨架（spec 实例化 + 状态机 +
   内存追踪），而非每个算子独立实现；
2. **Hash 聚合为主路径**：分组键哈希定位 + step/finalize 两段式聚合函数接口；
3. **两阶段聚合就绪**：Partial/Final 变体为分区并行执行提供了现成形态；
4. **TopN/Distinct 等融合优化**：Sort+Limit 合并为 TopN，降低阻塞算子的内存与时间开销。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
