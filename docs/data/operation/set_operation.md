# 集合操作（UNION / INTERSECT / EXCEPT）分析文档

## 概述

本文档分析 Linkrs 查询模块中的集合操作算子，包括其语义、物理 spec、执行器实现与
去重/缓冲/落盘机制。本文档与 2026-11-01 代码现状对齐。

---

## 一、语义类型

集合操作将两个同构子计划的行集按集合语义合并：

| 语义 | 说明 |
|------|------|
| `UNION` | 并集，**去重** |
| `UNION ALL` | 并集，不去重 |
| `INTERSECT` | 交集，去重 |
| `EXCEPT` | 左侧有、右侧没有的行，去重 |
| `MINUS` | 同 `EXCEPT`（别名语法） |

---

## 二、物理层 SetSpec 与执行算子

### 2.1 SetSpec

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/set.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::SetSpec`）

```rust
pub enum SetSpec {
    Union,
    UnionAll,
    Intersect,
    Except,
    Minus,
}
```

### 2.2 SetOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/set_operator.rs`

`SetOperator` 的状态枚举 `SetOperatorKind` 共 5 种变体，与 spec 一一对应：

| 变体 | 说明 | 关键状态 |
|------|------|----------|
| `Union` | 并集去重 | `seen_rows: HashSet<String>` 记录已输出行的序列化键；`left_consumed` 标记左输入耗尽后转读右输入 |
| `UnionAll` | 并集不去重 | 先排空左输入再排空右输入，无额外状态 |
| `Intersect` | 交集 | `left_rows` 缓冲左侧行、`right_rows: HashSet<String>` 记录右侧行键；两侧均缓冲完毕后一次性求交输出（`left_buffered`/`right_buffered`/`emitted` 三相状态） |
| `Except` | 差集 | `exclude_rows: HashSet<String>` 物化右侧行键作为排除表，逐行过滤左输入 |
| `Minus` | 差集（别名） | 同 `Except` |

### 2.3 执行流程与去重机制

```
Union:      读左输入 → 每行序列化为键，未见于 seen_rows 则输出并登记
            → 左耗尽后同样处理右输入
UnionAll:   直接依次转发左右输入
Intersect:  缓冲左侧行 + 右侧行键 → 双方就绪后输出两侧都存在的键（去重）
Except:     先物化右侧行键为排除表 → 左输入逐行过滤输出
```

去重以**整行序列化键**（`HashSet<String>`）实现，即按全列值判定重复，
与 SQL 集合语义一致。

### 2.4 内存管理与落盘

`Union`/`Intersect`/`Except`/`Minus` 均携带 `MemoryTracker` 受内存预算约束；
内存超限时经 `spill_with_manager` 分区落盘（spill），并提供
`spilled_bytes` / `spilled_rows` / `spill_count` 观测接口
（机制与 Join 算子的 Grace 落盘同源）。

---

## 三、关联算子

- `intersect.rs` / `intersect_build.rs`：独立的存在性检测辅助结构（半连接风格的交集构建），
  供优化后形态复用；
- 阻塞语义：除 `UnionAll` 外，集合算子均为**阻塞算子**（需完整读取一侧或两侧输入），
  与聚合、排序同属 `BlockingOperator` 家族的管理范围。

---

## 四、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **Set 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/set_operator.rs` |
| **物理 SetSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/set.rs` |
| **交集辅助** | `crates/linkrs-query/src/executor/streaming/operators/intersect.rs`、`intersect_build.rs` |
| **阻塞算子管理** | `crates/linkrs-query/src/executor/streaming/operators/blocking.rs` |

---

## 五、总结

1. **五种语义齐全**：UNION / UNION ALL / INTERSECT / EXCEPT / MINUS，
   MINUS 作为 EXCEPT 的别名语法保留；
2. **行级序列化去重**：以整行键的 HashSet 实现去重与排除，语义严格对齐 SQL；
3. **缓冲三相推进**：Intersect 采用"左缓冲 → 右登记 → 求交输出"的三相状态机；
4. **内存可控可观测**：MemoryTracker + 落盘溢出 + spilled_* 指标，
   与 Join/聚合共用同一套有界内存基础设施。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
