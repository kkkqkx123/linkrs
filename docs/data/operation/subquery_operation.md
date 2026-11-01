# 子查询 / Apply 操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的子查询执行操作（Apply 算子族），包括其语义、逻辑层形式、
物理算子实现，以及与去关联（decorrelation）优化的关系。
本文档与 2026-11-01 代码现状对齐。

---

## 一、子查询的语义类型

子查询出现在两类位置：

1. **标量/存在性子查询**：`WHERE EXISTS (...)`、`WHERE x IN (SELECT ...)`、标量表达式子查询；
2. **模式子查询**：图查询中模式片段作为可复用子计划（`PatternApply`）。

按关联性分为**非关联子查询**（右侧可独立物化，执行一次）与**关联子查询**
（右侧依赖外层行，逐行绑定执行）。

---

## 二、逻辑层形式

规划层为子查询生成 Apply 类节点，物理 spec 定义于
`crates/linkrs-query/src/executor/streaming/operators/spec/apply.rs`：

```rust
pub enum ApplySpec {
    Apply { kind: ApplyKind, correlated_columns: Vec<String> },
    PatternApply { hash_keys, probe_keys, anti },
    CorrelatedApply { sub_plan: Arc<PhysicalPlan>, anti },
    RollUpApply { compare_columns, collect_column },
}

pub enum ApplyKind {
    Standard, // 每个外层行与右侧全部匹配行连接
    Semi,     // 存在性：右侧有匹配即保留外层行
    Anti,     // 反存在性：右侧无匹配才保留
    Single,   // 标量子查询：右侧多行匹配即报错
    All,      // 每个外层行展开全部匹配
}
```

---

## 三、物理层 Apply 执行算子

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/apply_operator.rs`
（spec：`spec/apply.rs`，经 `spec.rs` shim re-export）

`ApplyOperator` 的状态枚举 `ApplyOperatorKind` 共 4 种变体：

| 变体 | 说明 | 关键机制 |
|------|------|----------|
| `Apply` | 通用 Apply（Standard/Semi/Anti/Single/All 五种 `ApplyKind`） | 右侧物化一次（`materialize_right`），按 `correlated_columns` 做键匹配（`matching_rows`/`keys_match`）；Semi/Anti 只消费右侧存在性 |
| `PatternApply` | 模式子查询，等值键哈希匹配 | `hash_keys`/`probe_keys` + `anti`（NOT EXISTS 语义） |
| `CorrelatedApply` | **关联子查询逐行执行** | 右子树是根为 `Argument` 源的自包含 `PhysicalPlan`；每行将外层行绑定为关联帧（`correlated_bindings`）；子执行器首次物化后经 `reset()` 复用 |
| `RollUpApply` | 聚合上卷（GO 语句多跳结果按 `compare_columns` 分组收集） | `collect_column` 聚合收集列 |

### 3.1 执行流程（CorrelatedApply）

```
1. open()：实例化右侧子计划
2. 每个外层行：
   a. correlated_bindings 将外层行绑定为关联帧（剥离参数映射后缓存）
   b. 子执行器 reset() 后重新执行，物化本轮右侧结果
   c. 按语义产出：Standard/All 连接、Semi 存在性、Anti 反存在性、Single 唯一性校验
3. close()：释放子执行器
```

内存均受 `MemoryTracker` 约束；输出列布局按计划中的输出布局对齐。

---

## 四、去关联优化（Decorrelation）

**文件路径**: `crates/linkrs-query/src/optimizer/cost_based/subquery_unnesting.rs` +
`crates/linkrs-query/src/optimizer/heuristic/decorrelation.rs`

关联子查询的逐行执行代价是 O(外层行数 × 右侧行数)，优化器将其改写为等价 Join 形式
（见 Join 文档第六节）：

- `EXISTS`/`IN` 类关联子查询 → `SemiJoin`/`AntiJoin`（或带 `anti` 的 HashJoin 形式）；
- 可提取等值关联键时 → 物理 `JoinSpec::HashJoin`，一次性物化构建侧；
- 无法去关联的子查询（如含非等值关联、限定的标量子查询）保留 CorrelatedApply 逐行执行。

启发式批处理中的子查询宿主规则位于
`optimizer/heuristic/batch/subquery_host.rs`（对 Apply 类节点的访问与改写注册）。

---

## 五、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **Apply 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/apply_operator.rs` |
| **物理 ApplySpec / ApplyKind** | `crates/linkrs-query/src/executor/streaming/operators/spec/apply.rs` |
| **子查询去关联（CBO）** | `crates/linkrs-query/src/optimizer/cost_based/subquery_unnesting.rs` |
| **去关联（启发式）** | `crates/linkrs-query/src/optimizer/heuristic/decorrelation.rs` |
| **子查询宿主规则** | `crates/linkrs-query/src/optimizer/heuristic/batch/subquery_host.rs` |
| **关联子查询规划** | `crates/linkrs-query/src/planning/`（SubPlan 组合，见 plan_combiner.rs） |

---

## 六、总结

1. **四种物理变体**：通用 Apply、PatternApply（哈希模式匹配）、CorrelatedApply（逐行关联）、
   RollUpApply（聚合上卷），覆盖标量/存在性/模式/分组四类子查询场景；
2. **五种连接语义**：Standard/Semi/Anti/Single/All 显式枚举，Single 提供标量子查询的多行报错；
3. **关联帧机制**：CorrelatedApply 以 `Argument` 为根的自包含子计划 + 每行绑定帧执行，
   子执行器只物化一次并复用；
4. **优先去关联**：优化器尽量将关联子查询改写为 Semi/Anti/Hash Join，仅保留无法改写部分
   走逐行执行。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
