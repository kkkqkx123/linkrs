# Vector Type Operations Analysis

This document describes all operations supported by the vector type in GraphDB.

## 1. Core Vector Value Operations

### 1.1 Construction

| Operation | Description | Location |
|-----------|-------------|----------|
| `VectorValue::dense(data: Vec<f32>)` | Create a dense vector | `src/core/value/vector.rs:21` |
| `VectorValue::sparse(indices: Vec<u32>, values: Vec<f32>)` | Create a sparse vector (only non-zero values) | `src/core/value/vector.rs:26` |

### 1.2 Type Access

| Operation | Description | Returns | Location |
|-----------|-------------|---------|----------|
| `dimension()` | Get vector dimension | `usize` | `src/core/value/vector.rs:31` |
| `nnz()` | Get number of non-zero elements | `usize` | `src/core/value/vector.rs:41` |
| `as_dense()` | Get reference to dense data | `Option<&[f32]>` | `src/core/value/vector.rs:57` |
| `into_dense()` | Consume and return dense data | `Option<Vec<f32>>` | `src/core/value/vector.rs:49` |
| `to_dense()` | Convert to dense representation | `Vec<f32>` | `src/core/value/vector.rs:85` |

### 1.3 Type Checking

| Operation | Description | Returns | Location |
|-----------|-------------|---------|----------|
| `is_sparse()` | Check if sparse vector | `bool` | `src/core/value/vector.rs:75` |
| `is_dense()` | Check if dense vector | `bool` | `src/core/value/vector.rs:80` |

### 1.4 Validation

| Operation | Description | Returns | Location |
|-----------|-------------|---------|----------|
| `validate_dimension(expected: usize)` | Validate dimension matches expected | `Result<(), VectorError>` | `src/core/value/vector.rs:65` |

### 1.5 Mathematical Operations

| Operation | Description | Returns | Location |
|-----------|-------------|---------|----------|
| `dot(other: &VectorValue)` | Compute dot product | `Result<f32, VectorError>` | `src/core/value/vector.rs:116` |
| `l2_norm()` | Compute L2 norm (Euclidean norm) | `f32` | `src/core/value/vector.rs:169` |
| `cosine_similarity(other: &VectorValue)` | Compute cosine similarity | `Result<f32, VectorError>` | `src/core/value/vector.rs:177` |

### 1.6 Utility Operations

| Operation | Description | Returns | Location |
|-----------|-------------|---------|----------|
| `estimated_size()` | Estimate memory usage in bytes | `usize` | `src/core/value/vector.rs:102` |

## 2. SQL Function Operations

Vector functions can be used in SQL queries for vector computation and transformation.

### 2.1 Similarity Functions

| Function | Description | Parameters | Returns | Location |
|----------|-------------|------------|---------|----------|
| `cosine_similarity(vec1, vec2)` | Compute cosine similarity | 2 vectors | `Float` | `src/query/executor/expression/functions/builtin/vector.rs:16` |
| `dot_product(vec1, vec2)` | Compute dot product | 2 vectors | `Float` | `src/query/executor/expression/functions/builtin/vector.rs:17` |
| `euclidean_distance(vec1, vec2)` | Compute Euclidean distance | 2 vectors | `Float` | `src/query/executor/expression/functions/builtin/vector.rs:19` |
| `manhattan_distance(vec1, vec2)` | Compute Manhattan distance | 2 vectors | `Float` | `src/query/executor/expression/functions/builtin/vector.rs:21` |

### 2.2 Property Functions

| Function | Description | Parameters | Returns | Location |
|----------|-------------|------------|---------|----------|
| `dimension(vector)` | Get vector dimension | 1 vector | `Int` | `src/query/executor/expression/functions/builtin/vector.rs:24` |
| `l2_norm(vector)` | Compute L2 norm | 1 vector | `Float` | `src/query/executor/expression/functions/builtin/vector.rs:26` |
| `nnz(vector)` | Get non-zero element count | 1 vector | `Int` | `src/query/executor/expression/functions/builtin/vector.rs:28` |

### 2.3 Transformation Functions

| Function | Description | Parameters | Returns | Location |
|----------|-------------|------------|---------|----------|
| `normalize(vector)` | Normalize to unit length | 1 vector | `Vector` | `src/query/executor/expression/functions/builtin/vector.rs:30` |

## 3. Vector Index Operations

### 3.1 DDL Statements

| Statement | Description | Location |
|-----------|-------------|----------|
| `CREATE VECTOR INDEX` | Create a vector index | `src/query/parser/ast/vector.rs:16` |
| `DROP VECTOR INDEX` | Drop a vector index | `src/query/parser/ast/vector.rs:36` |

### 3.2 Index Configuration

| Parameter | Description | Type |
|-----------|-------------|------|
| `vector_size` | Dimension of vectors | `usize` |
| `distance` | Distance metric (Cosine, Euclidean, Dot) | `VectorDistance` |
| `hnsw_m` | HNSW parameter M | `Option<usize>` |
| `hnsw_ef_construct` | HNSW parameter ef_construct | `Option<usize>` |

### 3.3 Distance Metrics

| Metric | Description |
|--------|-------------|
| `Cosine` | Cosine similarity distance |
| `Euclidean` | Euclidean distance |
| `Dot` | Dot product distance |

## 4. Vector Search Operations

### 4.1 DML Statements

| Statement | Description | Location |
|-----------|-------------|----------|
| `SEARCH VECTOR` | Search vectors with similarity | `src/query/parser/ast/vector.rs:48` |
| `MATCH ... WHERE vector` | MATCH with vector condition | `src/query/parser/ast/vector.rs:165` |
| `LOOKUP ...` | LOOKUP with vector search | `src/query/parser/ast/vector.rs:182` |

### 4.2 Query Types

| Type | Description | Example |
|------|-------------|---------|
| `Vector` | Direct vector data | `[0.1, 0.2, 0.3]` |
| `Text` | Text query (auto-embedded if embedding configured) | `"search text"` |
| `Parameter` | Parameter reference | `$query_vector` |

### 4.3 Embedding-Aware Text Search

When `[vector.embedding]` is configured in `config.toml`, GraphDB will automatically:
1. Detect text queries in `SEARCH VECTOR` statements
2. Call the embedding service to convert text to vector
3. Use the resulting vector for similarity search

**Example query with automatic embedding**:
```sql
-- 如果 [vector.embedding] 已配置，以下查询会自动将文本转为向量
SEARCH VECTOR ON my_vector INDEX
QUERY "machine learning tutorial"
LIMIT 10
```

**Without embedding config**, text queries require manual vector conversion:
```sql
-- 未配置嵌入服务时，需要手动提供向量
SEARCH VECTOR ON my_vector INDEX
QUERY [0.1, 0.2, 0.3, ...]
LIMIT 10
```

### 4.4 Search Options

| Option | Description |
|--------|-------------|
| `threshold` | Minimum similarity score |
| `limit` | Maximum results |
| `offset` | Result offset |
| `order by` | Sort by score (asc/desc) |
| `where` | Filter by score comparisons |

### 4.4 Comparison Operators

Used in WHERE clause for score filtering:

| Operator | Description |
|----------|-------------|
| `=` | Equal |
| `!=` | Not equal |
| `<` | Less than |
| `<=` | Less than or equal |
| `>` | Greater than |
| `>=` | Greater than or equal |

## 5. HTTP API Operations

All vector operations accessible via HTTP API:

### 5.1 Index Management

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/vector/indexes` | `POST` | Create vector index |
| `/vector/indexes/:space_id/:tag_name/:field_name` | `DELETE` | Drop vector index |
| `/vector/indexes/:space_id/:tag_name/:field_name` | `GET` | Get index info |
| `/vector/indexes` | `GET` | List all indexes |

### 5.2 Vector Operations

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/vector/search` | `POST` | Search vectors |
| `/vector/:space_id/:tag_name/:field_name/:id` | `GET` | Get vector by ID |
| `/vector/count/:space_id/:tag_name/:field_name` | `GET` | Count vectors |

## 6. Error Types

| Error | Description |
|-------|-------------|
| `DimensionMismatch` | Vector dimensions don't match |
| `InvalidSparseIndices` | Invalid sparse vector indices |
| `OutOfBounds` | Index out of bounds |
| `InvalidOperation` | Invalid vector operation |

## 7. Summary

### Vector Type Support Matrix

| Category | Operations |
|-----------|------------|
| **Core** | dense, sparse, dimension, nnz, dot, l2_norm, cosine_similarity |
| **SQL Functions** | cosine_similarity, dot_product, euclidean_distance, manhattan_distance, dimension, l2_norm, nnz, normalize |
| **Index** | CREATE, DROP, Cosine, Euclidean, Dot metrics |
| **Search** | SEARCH VECTOR, MATCH, LOOKUP with filtering |
| **API** | CRUD operations via HTTP |