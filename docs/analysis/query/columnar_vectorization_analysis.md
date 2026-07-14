# 列式化/向量化/批量优化分析

> 基于当前代码库实际使用场景的分析，不考虑性能测试数据。

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

**分析：**
- 存储层已经是列式（`ColumnStore`），但扫描时**未做投影下推**——即使 query 只需 `name`，也会加载所有属性。
- `DataChunk` 已有 `get_column()` / `column_ref()` API，但**零调用点**——属于死代码。
- 测试数据极小（< 100 行），行式遍历的开销在整体延迟中占比可忽略。
- **行式 → 列式转换本身有成本**：`Vec<Vec<Value>>` 转 `Vec<Vec<Value>>` 并无意义。

**结论：当前阶段不必要。**
- 投影下推到存储层的收益更大（减少 I/O），比列式 DataChunk 更紧迫。
- 在 1024 行的 chunk size 下，SIMD 利用率有限。
- 零用户报告行式处理为瓶颈。

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
- `evaluate_batch()` 名义上是批处理，实际是 `expressions.iter().map(|e| evaluate(e, ctx))`——逐个表达式依次求值，没有批处理。
- 上下文通过 `SlotLayout.name_to_slot` HashMap 做名称解析，没有 slot 级快速路径。
- 算子循环：对每行创建 `ValueRowContext`/`BorrowedRowContext`，然后对每行每表达式调用 `evaluate()`。

**分析：**
- 表达式求值的开销主要包括：(1) 递归遍历 dispatch，(2) `Value` 枚举构造/析构，(3) 字符串名称查找。
- 图数据库查询的表达式通常简单（`p.age > 28` 或 `a + b`），不是 OLAP 风格的长表达式链。
- 向量化表达式求值（如按列求值、批量 dispatch）对于简单表达式收益有限，但会大幅提升代码复杂度。

**结论：当前阶段不必要。**
- 行式求值在表达式简单的场景下足够。
- 向量化表达式求值通常和列式 DataChunk 绑定，单一引入无意义。
- 这是典型的「过早优化」——没有 profile 证明表达式求值是瓶颈。

### 4. 批量存储属性读写 (Batch Storage Property Fetch/Write)

**现状：**
- `VertexCursor::next_batch(batch_size) -> Vec<Vertex>` 返回批量顶点对象。
- `PropertyBatchReader` trait 提供 `read_vertex_props(&[VertexId], &[String]) -> Vec<Vec<Value>>`——从列式 ColumnStore 读取指定 ID 的指定属性。
- 但 `SourceOperator` 的 `StorageScanVertices` 没有使用 `PropertyBatchReader`——它调用 `cursor.next_batch()` 获取完整 `Vertex`，然后逐一解构为 `Vec<Value>`。

**分析：**
- 这是**当前最值得优化的点**：`next_batch()` 已经批量获取顶点，但没有利用列式 ColumnStore 做投影下推。
- 如果 `SourceOperator` 支持列投影（只读 `name` 和 `age` 两列），可以减少属性读取和 Value boxing。
- 测试中的多属性场景：`CREATE TAG person(name: STRING, age: INT, city: STRING, salary: FLOAT)`，投影通常只取 1-3 列——浪费大量无意义加载。

**结论：部分必要，但不是列式化，而是投影下推。**
- 将 `PropertyBatchReader` 接入扫描路径，让 `SourceOperator` 支持仅读取所需列。
- 这和「列式 DataChunk」是两个不同概念——投影下推行式 DataChunk 同样适用。
- 实现成本低、收益确定、不破坏现有算子接口。

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
| 选择向量 | 不必要 | `Vec<usize>` + `take_indices()` 在当前场景下足够 |
| 向量化表达式 | 不必要 | 简单表达式场景，ROI 低 |
| **投影下推** | **建议做** | 利用已有 `PropertyBatchReader`，减少列读取 |
| 批量存储写入 | 不必要 | 当前 DML 负载轻 |
| 细粒度存储 Morsel | 不必要 | 单线程场景，并行无意义 |
| 紧凑 visited set | 不必要 | 已有自适应 BitVec 实现 |
| 紧凑 frontier | 不必要 | 空间换时间，紧凑化反而增加 I/O |

**最值得做的唯一优化：投影下推到存储层。**

具体来说：
1. 扩展 `SourceOperator` 支持 `projected_columns: Option<Vec<String>>`
2. 在扫描时调用 `PropertyBatchReader::read_vertex_props(ids, &projected_columns)` 而非构建完整 `Vertex`
3. 跳过非投影列的属性加载和 Value boxing
4. 其他算子无需修改（行式 DataChunk 不变）

其余的列式化/向量化/批量优化，应严格执行 M6 策略——**等 profile 证明瓶颈后再引入**。
