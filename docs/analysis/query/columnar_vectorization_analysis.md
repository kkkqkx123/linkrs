# 列式化/向量化/批量优化分析

> 基于当前代码库实际使用场景的分析，不考虑性能测试数据。
>
> **状态更新（M1/M2 落地后）**：本文 §1-§6 的"现状"与"结论"撰写于投影下推与扁平列落地之前，部分表述已过时——投影下推、扫描扁平属性列（`{var}.{prop}` 复合槽）、列式批求值、扫描边界扁平记录（`FlatVertexRecord`）、单份存储（取消主动 `materialize_columns`）、Filter 全选短路均已落地。落地后的方案细节与任务分解见 `docs/plan/scan-column-flattening-design.md` 与 `docs/plan/scan-column-flattening-execution-plan.md`；`evaluate_batch` 已删除。本文的"不必要"结论仅针对**完整列式化/typed column/SIMD/选择向量传播**，维持不变。

## 当前架构概览

linkrs 的查询引擎是一个 **混合架构**：

-   **存储层**（ColumnStore）是列式的：每个属性以独立的 `Column` 存储，固定宽度类型用平坦 `Vec<u8>`、变长类型用 offset 数组 + 数据 buffer。支持 RLE、字典、FSST、Bitpacking、ALP 等列式编码。
-   **查询执行层**（DataChunk）是行式的：`DataChunk.rows: Vec<Vec<Value>>`，每行是 29-variant `Value` enum 的堆分配 Vec。算子遍历 `chunk.rows`，逐行调用 `ExpressionEvaluator` 进行递归树遍历求值。
-   **图遍历层**：frontier 为 `VecDeque<TraversalItem>`，visited set 使用自适应 `HashSet`/`BitVec`。

## 实际使用场景

根据集成测试和 e2e 测试（`tests/e2e/social_network.rs`、`tests/integration_streaming_executor.rs`），典型查询包括：

```
-- 简单扫描过滤
MATCH (p:person) WHERE p.age > 28 RETURN p.name

-- 投影
MATCH (p:person) RETURN p.name, p.age

-- 单跳图遍历
MATCH (p:person)-[:friend]->(f:person) RETURN p.name, f.name

-- 多步遍历
GO 2 STEPS FROM 'p1' OVER friend YIELD friend.name

-- 聚合
MATCH (p:person) RETURN count(*), sum(p.salary)

-- 分组聚合
MATCH (p:person) RETURN p.city, count(*) GROUP BY p.city

-- 排序
MATCH (n:Person) RETURN n.name, n.age ORDER BY n.age

-- 等值连接（Hash Join）
MATCH (a:Person)-[:Knows]->(b:Person) RETURN a.name, b.name

-- DML（单条/批量写入）
INSERT VERTEX person(name, age) VALUES 'p1':('Alice',30), 'p2':('Bob',25)
```

**数据特征：**
- 数据集小（测试通常只有个位数顶点）
- 查询简单（2-4 个算子链）
- 表达式简单（属性访问、数值比较、逻辑运算）
- 图遍历深度浅（1-2 hop）
- 单节点部署，无分布式

## 逐项分析

### 1. 列式向量化 scan/filter/project

**现状：**
- `SourceOperator` 从 `ColumnStore` 读取数据时，先将每个 `Vertex`/`Edge` 解构成 `Vec<Value>` 行，然后组装成 `DataChunk`。
- Filter 逐行求值谓词，收集 `selected: Vec<usize>`，最后调用 `take_indices()` 移动选中行。
- Project 逐行逐表达式求值，构建新行。
- （已落地：扫描源输出扁平属性列（`{var}.{prop}` 复合槽），Filter/Project 走上列式批求值快路径；Filter 全选时直接透传 chunk。）

**分析：**
- 存储层已经是列式（`ColumnStore`），且**投影下推已落地**——query 只需 `name` 时只加载所需列（`ScanOptions.projection` + `get_projected_batch`）。
- `DataChunk` 的 `get_column()` 已接入列式求值路径（首次调用惰性物化并缓存）；源算子不再主动 `materialize_columns`，单份存储。
- 测试数据极小（< 100 行），行式遍历的开销在整体延迟中占比可忽略。
- **行式 → 列式转换本身有成本**：`Vec<Vec<Value>>` 转 `Vec<Vec<Value>>` 并无意义。

**结论：当前阶段不必要。**（完整列式 DataChunk 不做；投影下推与扫描扁平列已落地，见 plan 文档。）

### 2. 有效性位图 (Validity Bitmap) 和选择向量 (Selection Vector)

**现状：**
- Filter 使用 `Vec<usize>` 作为选择向量，然后 `take_indices()` 物理移动行。
- 整个引擎没有 `BitVec` 选择向量（除 `VisitedSet` 外）。
- Null 通过 `Value::Null` 变体表示，没有列级有效性位图。

**分析：**
- `Vec<usize>` 选择向量在过滤率低时（保留多数行）代价较高——需要分配 indices Vec 并逐行 memcpy。
- `BitVec` 选择向量传递可以延迟物化，但当前算子**不支持延迟读取**——每个算子直接消费 `chunk.rows`。
- 有效性位图只在列式存储中有意义——行式 `Vec<Value>` 中 `Value::Null` 的开销是 1 个变体 tag。

**结论：当前阶段不必要。**
- 引入选择向量需要重写所有算子接口（current `DataChunk → DataChunk`），工作量远大于收益。
- 没有列式处理，有效性位图无意义。
- 测试中过滤条件简单，`Vec<usize>` 足够。

### 3. 向量化表达式求值 (Vectorized Expression Evaluation)

**现状：**
- `ExpressionEvaluator` 使用递归树遍历解释执行。
- 批处理语义由 `DataChunk::evaluate_expressions` / `evaluate_expression` 承担：支持 Literal/Variable/Parameter/Unary/Binary/TypeCast/Property-on-Variable 的列式批求值（`eval_with_cache`），其余表达式回退逐行递归求值；快路径命中/未命中由 `ColumnarStats` 计数（可观测）。
- 上下文通过 `SlotLayout.name_to_slot` HashMap 做名称解析，没有 slot 级快速路径。
- 算子循环：对每行创建 `ValueRowContext`/`BorrowedRowContext`，然后对每行每表达式调用 `evaluate()`。

**分析：**
- 表达式求值的开销主要包括：(1) 递归遍历 dispatch，(2) `Value` 枚举构造/析构，(3) 字符串名称查找。
- 图数据库查询的表达式通常简单（`p.age > 28` 或 `a + b`），不是 OLAP 风格的长表达式链。
- 向量化表达式求值（如按列求值、批量 dispatch）对于简单表达式收益有限，但会大幅提升代码复杂度。

**结论：当前阶段不必要。**（列式批求值快路径已落地，typed/SIMD 内核不做。）

### 4. 批量存储属性读写 (Batch Storage Property Fetch/Write)

**现状：**
- `VertexCursor::next_batch(batch_size) -> Vec<Vertex>` 返回批量顶点对象；扁平变体 `next_flat_batch -> Vec<FlatVertexRecord>` 跳过 `Vertex`/`HashMap` 装箱（已落地）。
- `PropertyBatchReader` trait 提供 `read_vertex_props(&[VertexId], &[String]) -> Vec<Vec<Value>>`——从列式 ColumnStore 读取指定 ID 的指定属性。
- `SourceOperator` 的 `StorageScanVertices` 已通过 `ScanOptions.projection` + `get_projected_batch` 做存储侧投影下推（已落地）。

**分析：**
- 存储侧投影下推已实现：只读取投影列，减少属性读取和 Value boxing。
- 测试中的多属性场景：`CREATE TAG person(name: STRING, age: INT, city: STRING, salary: FLOAT)`，投影通常只取 1-3 列——无意义的列加载已消除。

**结论：已落地。**（投影下推 + 扫描边界扁平记录；下一步按决策门验证是否需要 typed column。）

### 5. 细粒度存储 Morsel

**现状：**
- 默认 batch size 1024（`ScanOptions.batch_size`）。
- `GraphVertexCursor` 每次返回 `Vec<Vertex>`，按 `internal_id` 顺序扫描。
- 并行通过 `ExchangeOperator` / `GatherOperator` + `PartitionBatch` 实现 morsel 级并行——工作线程从 `AtomicUsize` 领取分区索引。

**分析：**
- 当前 morsel 粒度是算子树的并行（Exchange/Gather 分发整个子计划），不是数据级的 morsel。
- 数据级 morsel 需要 `GraphVertexCursor` 支持按 range 断开（已经支持 `vertex_id_range`），以及中间算子支持局部后终止。
- 测试中数据量小，并行执行从未实际触发（pool size = 1）。

**结论：当前阶段不必要。**
- 仅在并行度 > 1 时有意义，当前使用场景全为单线程。
- 等用户数据规模增长到需要并行扫描时再引入。

### 6. 图遍历紧凑 frontier/visited 布局

**现状：**
- frontier：`VecDeque<TraversalItem>`，每个 item 包含 `VertexId` + `Vertex` + `depth` + `Option<Edge>`——堆分配。
- visited set：`VisitedSet`，稀疏阶段用 `HashSet<VertexId>`，稠密阶段（>64 且 range ≤ 1,000,000）自动切换为 `BitVec` + `HashSet` overflow。

**分析：**
- `TraversalItem` 包含完整的 `Vertex` 对象（图遍历时会缓存顶点数据），这是为了减少存储查询——有意识的空间换时间设计。
- 紧凑 frontier（如存储 `VertexId` 而非 `Vertex`）会引入额外的存储查询，降低遍历吞吐。
- `VisitedSet` 的 `BitVec` 自适应是**已有实现**——已足够好。
- 图遍历的瓶颈通常在存储 I/O（`get_node_edges()` 返回 `Vec<Edge>`）和算法本身，frontier 布局影响极小。

**结论：当前阶段不必要。**
- `VisitedSet` 已有自适应 BitVec 实现。
- Frontier 紧凑化带来的内存节省在测试数据规模下不可测量。
- 紧凑 frontier 会迫使每次 pop 时去存储层查询 vertex，增加 I/O。

## 综合结论

| 优化项 | 必要性 | 理由 |
|--------|--------|------|
| 列式 DataChunk | 不必要 | 行式在 1024 行/simple expr 下已足够；产生列式 → 行式转换成本 |
| Validity Bitmap | 不必要 | 与行式引擎绑定，需重写算子接口 |
| 选择向量 | 不必要 | `Vec<usize>` + `take_indices()`（含全选短路）在当前场景下足够 |
| 向量化表达式 | 不必要 | 简单表达式场景，ROI 低；列式批求值快路径已落地 |
| **投影下推** | **已落地** | `ScanOptions.projection` + `get_projected_batch`，减少列读取 |
| **扫描扁平属性列** | **已落地** | 复合槽 `{var}.{prop}`，Filter/Project 走列式快路径 |
| **扫描边界扁平记录** | **已落地** | `FlatVertexRecord` 跳过 Vertex/HashMap 装箱 |
| **单份存储** | **已落地** | 源算子取消主动物化，`get_column` 惰性缓存 |
| 批量存储写入 | 不必要 | 当前 DML 负载轻 |
| 细粒度存储 Morsel | 不必要 | 单线程场景，并行无意义 |
| 紧凑 visited set | 不必要 | 已有自适应 BitVec 实现 |
| 紧凑 frontier | 不必要 | 空间换时间，紧凑化反而增加 I/O |

**已落地的最有价值优化：投影下推 + 扫描扁平属性列（贯通列式快路径）。**

具体来说：
1. `SourceOperator` 支持 `projected_columns`（`ScanOptions.projection`），存储侧只读投影列（`get_projected_batch`）
2. 扫描源输出 layout 追加扁平属性列（`{var}.{prop}`），Filter/Project 的 `p.age` 命中列式 `Property` 分支
3. 存储边界 `next_flat_batch` 跳过 `Vertex`/`HashMap` 装箱；chunk 列缓存按需物化（单份存储）
4. 快路径命中率由 `ColumnarStats` 可观测，扁平列承诺有 debug 断言

完整的列式化/向量化/批量优化（typed column、SIMD、选择向量传播、Morsel），仍严格执行 M6 策略——**等 profile 证明瓶颈后再引入**（基准验证集 B1-B7 与决策门见 `docs/plan/fallback-and-typed-column-analysis.md` §6）。
