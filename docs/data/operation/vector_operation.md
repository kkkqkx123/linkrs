# 向量类型操作分析文档

本文档描述 Linkrs 项目中向量（vector）类型支持的全部操作。
本文档与 2026-11-01 代码现状对齐。

## 1. 核心向量值操作

### 1.1 构造

| 操作 | 说明 | 位置 |
|-----------|-------------|----------|
| `VectorValue::dense(data: Vec<f32>)` | 创建稠密向量 | `crates/linkrs-core/src/value/vector.rs:20` |
| `VectorValue::sparse(indices: Vec<u32>, values: Vec<f32>)` | 创建稀疏向量（仅存非零值） | `crates/linkrs-core/src/value/vector.rs:24` |

### 1.2 类型访问

| 操作 | 说明 | 返回 | 位置 |
|-----------|-------------|---------|----------|
| `dimension()` | 获取向量维度 | `usize` | `crates/linkrs-core/src/value/vector.rs:30` |
| `nnz()` | 获取非零元素个数 | `usize` | `crates/linkrs-core/src/value/vector.rs:40` |
| `as_dense()` | 获取稠密数据引用 | `Option<&[f32]>` | `crates/linkrs-core/src/value/vector.rs:56` |
| `into_dense()` | 消耗自身并返回稠密数据 | `Option<Vec<f32>>` | `crates/linkrs-core/src/value/vector.rs:48` |
| `to_dense()` | 转换为稠密表示 | `Vec<f32>` | `crates/linkrs-core/src/value/vector.rs:84` |

### 1.3 类型判断

| 操作 | 说明 | 返回 | 位置 |
|-----------|-------------|---------|----------|
| `is_sparse()` | 是否为稀疏向量 | `bool` | `crates/linkrs-core/src/value/vector.rs:74` |
| `is_dense()` | 是否为稠密向量 | `bool` | `crates/linkrs-core/src/value/vector.rs:79` |

### 1.4 校验

| 操作 | 说明 | 返回 | 位置 |
|-----------|-------------|---------|----------|
| `validate_dimension(expected: usize)` | 校验维度是否匹配 | `Result<(), VectorError>` | `crates/linkrs-core/src/value/vector.rs:64` |

### 1.5 数学运算

| 操作 | 说明 | 返回 | 位置 |
|-----------|-------------|---------|----------|
| `dot(other: &VectorValue)` | 计算点积 | `Result<f32, VectorError>` | `crates/linkrs-core/src/value/vector.rs:115` |
| `l2_norm()` | 计算 L2 范数（欧几里得范数） | `f32` | `crates/linkrs-core/src/value/vector.rs:168` |
| `cosine_similarity(other: &VectorValue)` | 计算余弦相似度 | `Result<f32, VectorError>` | `crates/linkrs-core/src/value/vector.rs:176` |

### 1.6 工具操作

| 操作 | 说明 | 返回 | 位置 |
|-----------|-------------|---------|----------|
| `estimated_size()` | 估算内存占用（字节） | `usize` | `crates/linkrs-core/src/value/vector.rs:101` |

## 2. SQL 内置函数操作

向量函数可在查询语句中使用，用于向量计算与转换。

**文件路径**: `crates/linkrs-query/src/executor/expression/functions/builtin/vector.rs`

### 2.1 相似度 / 距离函数

| 函数 | 说明 | 参数 | 返回 |
|----------|-------------|------------|---------|
| `cosine_similarity(vec1, vec2)` | 计算余弦相似度 | 2 个向量 | `Float` |
| `dot_product(vec1, vec2)` | 计算点积 | 2 个向量 | `Float` |
| `euclidean_distance(vec1, vec2)` | 计算欧几里得距离 | 2 个向量 | `Float` |
| `manhattan_distance(vec1, vec2)` | 计算曼哈顿距离 | 2 个向量 | `Float` |
| `array_cosine_similarity(...)` | 数组版余弦相似度 | 数组 | `Float` |
| `array_distance(...)` | 数组版距离 | 数组 | `Float` |

### 2.2 属性函数

| 函数 | 说明 | 参数 | 返回 |
|----------|-------------|------------|---------|
| `dimension(vector)` | 获取向量维度 | 1 个向量 | `Int` |
| `l2_norm(vector)` | 计算 L2 范数 | 1 个向量 | `Float` |
| `nnz(vector)` | 获取非零元素个数 | 1 个向量 | `Int` |

### 2.3 转换函数

| 函数 | 说明 | 参数 | 返回 |
|----------|-------------|------------|---------|
| `normalize(vector)` | 归一化为单位长度 | 1 个向量 | `Vector` |
| `array_value(...)` | 数组到向量/数组的转换 | 数组 | `Vector` |

## 3. 向量索引操作

### 3.1 DDL 语句

**文件路径**: `crates/linkrs-query/src/parser/ast/vector.rs`

| 语句 | 说明 | 位置 |
|-----------|-------------|----------|
| `CREATE VECTOR INDEX` | 创建向量索引 | `crates/linkrs-query/src/parser/ast/vector.rs:19` |
| `DROP VECTOR INDEX` | 删除向量索引 | `crates/linkrs-query/src/parser/ast/vector.rs:67` |
| `SHOW VECTOR INDEX` | 列出向量索引 | `crates/linkrs-query/src/parser/ast/vector.rs:75` |
| `DESCRIBE VECTOR INDEX` | 查看向量索引详情 | `crates/linkrs-query/src/parser/ast/vector.rs:83` |

### 3.2 索引配置（`VectorIndexConfig`）

| 参数 | 说明 | 类型 |
|-----------|-------------|------|
| `vector_size` | 向量维度 | `usize` |
| `distance` | 距离度量（Cosine、Euclidean、Dot） | `VectorDistance` |
| `hnsw_m` | HNSW 参数 M（可选） | `Option<usize>` |
| `hnsw_ef_construct` | HNSW 参数 ef_construct（可选） | `Option<usize>` |
| 量化配置（`QuantizationKind` / `CompressionRatioKind`） | 可选量化与压缩比设置 | — |

### 3.3 距离度量（`VectorDistance`）

| 度量 | 说明 |
|--------|-------------|
| `Cosine` | 余弦相似度 |
| `Euclidean` | 欧几里得距离 |
| `Dot` | 点积 |

## 4. 向量检索操作

### 4.1 检索语句

**文件路径**: `crates/linkrs-query/src/parser/ast/vector.rs`

| 语句 | 说明 | 位置 |
|-----------|-------------|----------|
| `SEARCH VECTOR` | 按相似度检索向量 | `crates/linkrs-query/src/parser/ast/vector.rs:94` |
| `MATCH ... WHERE vector` | MATCH 携带向量条件 | `crates/linkrs-query/src/parser/ast/vector.rs:154` |
| `LOOKUP ...` | LOOKUP 携带向量检索 | `crates/linkrs-query/src/parser/ast/vector.rs:171` |

### 4.2 查询类型（`VectorQueryType`）

| 类型 | 说明 | 示例 |
|------|-------------|---------|
| `Vector` | 直接给定向量数据 | `[0.1, 0.2, 0.3]` |
| `Text` | 文本查询（配置了嵌入服务时自动向量化） | `"search text"` |
| `Parameter` | 参数引用 | `$query_vector` |

### 4.3 基于嵌入的文本检索

当 `config.toml` 中配置了 `[vector.embedding]` 时，Linkrs 会自动：
1. 识别 `SEARCH VECTOR` 语句中的文本查询
2. 调用嵌入服务将文本转换为向量
3. 使用生成的向量执行相似度检索

**配置嵌入服务后的自动向量化示例**：
```sql
-- 如果 [vector.embedding] 已配置，以下查询会自动将文本转为向量
SEARCH VECTOR ON my_vector INDEX
QUERY "machine learning tutorial"
LIMIT 10
```

**未配置嵌入服务时，需要手动提供向量**：
```sql
-- 未配置嵌入服务时，需要手动提供向量
SEARCH VECTOR ON my_vector INDEX
QUERY [0.1, 0.2, 0.3, ...]
LIMIT 10
```

### 4.4 检索选项

| 选项 | 说明 |
|--------|-------------|
| `threshold` | 相似度分数下限 |
| `limit` | 最大返回条数 |
| `offset` | 结果偏移量 |
| `order by` | 按分数排序（asc/desc） |
| `where` | 按分数比较过滤 |
| `yield` | 指定返回列（`VectorYieldClause`） |

### 4.5 比较运算符

用于 WHERE 子句中的分数过滤：

| 运算符 | 说明 |
|----------|-------------|
| `=` | 等于 |
| `!=` | 不等于 |
| `<` | 小于 |
| `<=` | 小于等于 |
| `>` | 大于 |
| `>=` | 大于等于 |

## 5. HTTP API 操作

所有向量操作均可通过 HTTP API 访问（前缀 `/v1`）。

**路由文件**: `crates/linkrs-server/src/http/router.rs`；处理器: `crates/linkrs-server/src/http/handlers/vector.rs`

### 5.1 索引管理

| 端点 | 方法 | 说明 |
|----------|--------|-------------|
| `/v1/vector/indexes` | `POST` | 创建向量索引 |
| `/v1/vector/indexes` | `GET` | 列出所有向量索引 |
| `/v1/vector/indexes/:space_id/:tag_name/:field_name` | `GET` | 获取索引详情 |
| `/v1/vector/indexes/:space_id/:tag_name/:field_name` | `DELETE` | 删除向量索引 |
| `/v1/vector/indexes/rebuild` | `POST` | 重建向量索引 |
| `/v1/vector/indexes/clear` | `POST` | 清空向量索引 |

### 5.2 向量操作

| 端点 | 方法 | 说明 |
|----------|--------|-------------|
| `/v1/vector/search` | `POST` | 向量相似度检索 |
| `/v1/vector/:space_id/:tag_name/:field_name/:point_id` | `GET` | 按 ID 获取向量 |
| `/v1/vector/:space_id/:tag_name/:field_name/count` | `GET` | 统计向量数量 |
| `/v1/vector/scroll` | `POST` | 滚动遍历向量点 |
| `/v1/vector/payload` | `PUT` | 设置负载（payload） |
| `/v1/vector/payload/fields` | `PUT` | 设置负载字段 |
| `/v1/vector/payload/delete` | `POST` | 删除负载 |

## 6. 错误类型

**文件路径**: `crates/linkrs-core/src/value/vector.rs:264`

| 错误 | 说明 |
|-------|-------------|
| `DimensionMismatch { expected, actual }` | 向量维度不匹配 |
| `InvalidSparseIndices` | 稀疏向量索引非法 |
| `OutOfBounds { index, dimension }` | 下标越界 |
| `InvalidOperation(String)` | 非法向量操作 |

## 7. 总结

### 向量类型支持矩阵

| 类别 | 操作 |
|-----------|------------|
| **核心值** | dense, sparse, dimension, nnz, dot, l2_norm, cosine_similarity |
| **SQL 函数** | cosine_similarity, dot_product, euclidean_distance, manhattan_distance, dimension, l2_norm, nnz, normalize, array_value, array_cosine_similarity, array_distance |
| **索引** | CREATE / DROP / SHOW / DESCRIBE，Cosine / Euclidean / Dot 度量，HNSW 参数与量化配置 |
| **检索** | SEARCH VECTOR、MATCH、LOOKUP，支持过滤 / yield |
| **API** | HTTP 端点（前缀 `/v1`）覆盖索引管理、检索、点读取、计数、scroll 与负载管理 |

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
