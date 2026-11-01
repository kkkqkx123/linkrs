# 扫描（Scan / Source）操作分析文档

## 概述

本文档分析 Linkrs 查询模块中的扫描与数据源操作，即执行计划的零输入入口算子：
从存储引擎（CSR、索引、游标）或缓冲区读出顶点/边数据，供上层算子消费。
本文档与 2026-11-01 代码现状对齐。

---

## 一、逻辑层扫描节点

规划层生成 GetVertices / GetNeighbors / 顶点边扫描等逻辑节点（见 Expand 文档中的遍历节点族），
扫描相关的关键属性在逻辑层即已确定：

- **投影裁剪**（`projected_properties`）：只取查询需要的属性列；
- **身份模式注解**（identity-only）：只需要实体 ID 时跳过每行属性映射；
- **谓词注解**：可下推到存储层的扫描谓词（`ScanPredicate` / `BoundIndexPredicate`）。

---

## 二、物理层 SourceSpec 与执行算子

### 2.1 SourceSpec

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/spec/source.rs`
（经 `spec.rs` shim 统一 re-export 为 `operators::spec::SourceSpec`）

### 2.2 SourceOperator 执行器

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/source_operator.rs`

执行器入口为 `SourceOperator`（`kind` + 运行时上下文 + 输出布局 + `OperatorConfig`），
状态枚举 `SourceOperatorKind` 共 13 种变体：

| 变体 | 说明 | 关键字段 |
|------|------|----------|
| `ScanVertices` | 缓冲区顶点扫描（行来自 spec，用于已物化输入） | `buffer`、`col_names` |
| `StorageScanVertices` | **存储层顶点扫描**（主路径，行来自存储游标 `VertexCursor`） | `space_name`、`limit`、`partition_range`、`shard_slice`、`projected_properties`、`identity_only`、`skip_rows`、`predicate`、`tag`、`semi_mask` |
| `ScanEdges` | 缓冲区边扫描 | `buffer`、`col_names` |
| `StorageScanEdges` | **存储层边扫描**（行来自 `EdgeCursor`） | `edge_type`、`shard_slice`、`predicate`、`identity_only` |
| `IndexScan` | **索引扫描**：带类型化谓词与投影（`BoundIndexPredicate` + `IndexProjection` + `IndexCursor`） | `index_name`、`index_id`、`tag`、`predicate`、`projection` |
| `GetVertices` | 按顶点 ID 取点（带 ID 缓存） | `tag`、`vertex_ids`、`cached_ids` |
| `GetEdges` | 按 (src, dst, type, rank) 取边 | `edge_type`、`src`、`dst`、`rank` |
| `GetNeighbors` | 对每个输入顶点遍历邻居（`NeighborScanState`） | `tag`、`direction` |
| `CteScan` | 递归 CTE 工作表扫描：`open()`/`reset()` 时对不动点算子已发布表做快照 | `cte_name`、`buffer` |
| `StandaloneValues` | 独立 DML VALUES 源（表达式在 `open()` 求值一次，`now()` 等易变表达式取执行时刻值） | `values` |
| `Argument` | 参数行源（单行） | — |
| `GetProp` | 零输入属性读取（常规场景优先用一元 GetV 算子） | `entity_slot`、`prop_names` |
| `Start` | 空起始源 | — |

### 2.3 关键执行机制

```
open()    —— 绑定运行时；存储型变体在此打开游标（VertexCursor/EdgeCursor/IndexCursor），
             CteScan 在此对工作表快照，StandaloneValues 在此求值一次表达式
next()    —— 逐批推进游标产出行；缓冲型变体按 current_index 推进 buffer
set_semi_mask() —— 由 Join/Expand 注入半掩码（semi_mask），扫描只解码指定内部 ID 集合，
             不再全表枚举（Join/Expand 下推的关键加速路径）
reset()   —— 重新打开游标 / 重置缓冲指针（支持算子复用与重执行）
```

---

## 三、SeqScan vs IndexScan 的选择

物理规划/优化器在顶点访问路径上做二选一：

- **SeqScan**（`StorageScanVertices`）：全表/分区枚举，谓词仅做下推过滤；
  适合无可用索引、选择性低的查询；
- **IndexScan**（`IndexScan`）：命中属性索引时按 `BoundIndexPredicate` 范围定位，
  由 `IndexCursor` 有序输出；适合高选择性谓词。

该选择由 CBO 按谓词选择性与索引可用性决定，`tests/dql/optimizer.rs` 中以
EXPLAIN 断言 `IndexScan`/`SeqScan`/`IndexJoin` 等计划形态做回归保护。

---

## 四、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **Source 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/source_operator.rs` |
| **物理 SourceSpec** | `crates/linkrs-query/src/executor/streaming/operators/spec/source.rs` |
| **索引谓词/投影/游标** | 谓词与投影结构同上（`BoundIndexPredicate` / `IndexProjection`），游标 trait 由存储层提供 |
| **索引选择测试** | `crates/linkrs-query/tests/dql/optimizer.rs` |
| **存储游标实现** | `crates/linkrs-storage/`（VertexCursor / EdgeCursor / IndexCursor） |

---

## 五、总结

1. **缓冲与存储双轨**：同一算子族覆盖"已物化输入回放"（ScanVertices/ScanEdges/CteScan）与
   "存储直读"（StorageScan*/IndexScan）两类数据来源；
2. **下推三层**：谓词（`predicate`）、投影（`projected_properties`）、ID 集合（`semi_mask`）
   均可从上层算子下推进存储层，避免无效 IO；
3. **分区/分片感知**：`partition_range` + `shard_slice` 使扫描可按分区切分，为并行执行留口；
4. **身份模式**：identity-only / skip_rows 两种轻量输出模式，跳过属性映射开销。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
