# GraphDB 数据查询语言 (DQL)

## 概述

数据查询语言 (DQL) 用于从图数据库中检索数据，包括模式匹配、图遍历、索引查找、路径查找等功能。

---

## 1. MATCH - 模式匹配查询

### 功能
使用图模式匹配查询节点和边。

### 语法结构
```cypher
MATCH <pattern> [WHERE <condition>] [RETURN <projection>] [ORDER BY <expression>] [LIMIT <n>] [SKIP <n>]
```

### 关键特性
- 支持节点模式: `(variable:Label {prop: value})`
- 支持边模式: `-[variable:EdgeType {prop: value}]->`
- 支持路径模式: `(a)-[e]->(b)`
- 支持谓词过滤
- 支持属性投影
- 支持排序和分页
- 支持 OPTIONAL MATCH

### 示例
```cypher
-- 基本匹配
MATCH (p:Person {name: 'Alice'})-[:FRIEND]->(f)
WHERE f.age > 25
RETURN p.name, f.name
ORDER BY f.age DESC
LIMIT 10

-- 可选匹配
OPTIONAL MATCH (p:Person)-[:KNOWS]->(f)
RETURN p.name, f.name
```

---

## 2. GO - 图遍历查询

### 功能
从起始节点开始进行图遍历。

### 语法结构
```cypher
GO <steps> FROM <vertex_id> OVER <edge_type> [REVERSELY] [BIDIRECT] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持指定遍历步数
- 支持正向、反向、双向遍历
- 支持边类型过滤
- 支持条件过滤
- 支持属性投影

### 示例
```cypher
-- 单步遍历
GO 1 STEP FROM "player100" OVER follow

-- 多步遍历
GO 2 TO 4 STEPS FROM "123" OVER follow REVERSELY
WHERE target.age > 18
YIELD target.name, target.age
```

---

## 3. LOOKUP - 基于索引查找

### 功能
使用索引快速查找节点或边。

### 语法结构
```cypher
LOOKUP ON <tag_or_edge> WHERE <condition> [YIELD <properties>]
```

### 关键特性
- 利用索引加速查询
- 支持标签和边类型
- 支持复合条件
- 支持属性投影

### 示例
```cypher
LOOKUP ON person WHERE person.name == "Alice"
YIELD person.name, person.age
```

---

## 4. FETCH - 获取数据

### 功能
根据ID获取节点或边的详细信息。

### 语法结构
```cypher
FETCH PROP ON <tag> <vertex_id> [, <vertex_id> ...]
FETCH PROP ON <edge_type> <src_id> -> <dst_id> [@<rank>]
```

### 关键特性
- 支持批量获取节点属性
- 支持获取边属性
- 支持指定边rank

### 示例
```cypher
-- 获取顶点属性
FETCH PROP ON person "101", "102", "103"

-- 获取边属性
FETCH PROP ON follow "101" -> "102" @0
```

---

## 5. FIND PATH - 路径查找

### 功能
查找两个节点之间的路径，支持带权最短路径。

### 语法结构
```cypher
FIND <SHORTEST|ALL> PATH [WITH LOOP] [WITH CYCLE] FROM <src_id> TO <dst_id> OVER <edge_type> [WHERE <condition>] [UPTO <steps> STEPS] [WEIGHT <weight_expr>]
```

### 关键特性
- 支持最短路径查找
- 支持所有路径查找
- 支持带权最短路径（使用Dijkstra或A*算法）
- 路径顶点唯一性是默认行为（路径中不重复访问同一顶点）
- 支持显式允许自环边（A->A）
- 支持显式允许回路（路径中重复访问顶点）
- 支持路径长度限制
- 支持条件过滤

### 环路控制选项

| 选项 | 默认 | 说明 |
|-----|------|------|
| `WITH CYCLE` | 无 | 允许路径中重复访问顶点（如 A->B->C->A） |
| `WITH LOOP` | 无 | 允许自环边（A->A），用于时序数据或List属性存储 |

### 区别说明
- `WITH CYCLE`: 控制路径中是否允许重复访问顶点（如 A->B->C->A）
- `WITH LOOP`: 控制是否允许自环边（A->A），不影响路径顶点唯一性检测
- 两个选项独立工作，可以同时使用
- 默认情况下，路径顶点唯一且自环边被去重

### 权重表达式
- `ranking`: 使用边的ranking字段作为权重
- `<property_name>`: 使用指定属性作为权重（如 `weight`, `distance`, `cost`）
- 省略WEIGHT子句: 使用无权图（BFS算法）

### 算法选择
- **无权图**: 使用双向BFS算法，时间复杂度最优 O(b^(d/2))
- **带权图**: 使用Dijkstra算法，支持多对多最短路径
- **带权图+启发式**: 使用A*算法，适合单对单且有空间信息的场景

### 示例
```cypher
-- 无权最短路径（默认去重自环边）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow

-- 带权最短路径（使用weight属性）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow WEIGHT weight

-- 带权最短路径（使用ranking字段）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow WEIGHT ranking

-- 所有路径查询
FIND ALL PATH FROM "101" TO "201" OVER follow UPTO 5 STEPS

-- 带权所有路径查询
FIND ALL PATH FROM "101" TO "201" OVER follow WEIGHT distance UPTO 10 STEPS

-- 允许自环边（用于时序数据查询）
FIND ALL PATH WITH LOOP FROM "player100" TO "player200" OVER temp UPTO 5 STEPS

-- 允许回路（路径中可重复访问顶点）
FIND ALL PATH WITH CYCLE FROM "player100" TO "player200" OVER follow UPTO 5 STEPS

-- 同时允许自环边和回路
FIND ALL PATH WITH LOOP WITH CYCLE FROM "player100" TO "player200" OVER follow UPTO 5 STEPS
```

---

## 6. SEARCH - 全文检索

### 功能
对全文索引进行文本搜索。

### 语法结构
```cypher
SEARCH ON <index_name> [('<query>')]
[YIELD <field> [, <field> ...]]
[WHERE <condition>]
[ORDER BY <field> [ASC | DESC]]
[LIMIT <n>]
[OFFSET <n>]
```

### 关键特性
- 支持简单文本查询
- 支持布尔查询（MUST, SHOULD, MUST_NOT）
- 支持短语搜索
- 支持前缀和通配符搜索
- 支持模糊搜索
- 支持评分排序
- 支持结果高亮
- 支持分页

### 查询类型

| 类型 | 示例 | 说明 |
|------|------|------|
| 简单查询 | `'database'` | 搜索包含 "database" 的文档 |
| 字段查询 | `title:'graph'` | 在 title 字段搜索 "graph" |
| 多字段查询 | `title:'graph' OR content:'database'` | 多字段组合查询 |
| 布尔查询 | `title:'graph' AND content:'database'` | 布尔组合查询 |
| 短语查询 | `'"graph database"'` | 搜索短语 "graph database" |
| 前缀查询 | `'data*'` | 搜索以 "data" 开头的词 |
| 模糊查询 | `'databases~'` | 模糊搜索 "databases" |
| 范围查询 | `score:[0.5 TO 0.9]` | 分数在 0.5-0.9 之间 |

### YIELD 可用字段

| 字段 | 说明 |
|------|------|
| `<field>` | 索引字段 |
| `score()` | 相关性评分 |
| `highlight(<field>)` | 高亮显示 |
| `matched_fields()` | 匹配的字段 |

### 示例
```cypher
-- 简单搜索
SEARCH ON idx_article_content('database')

-- 字段搜索
SEARCH ON idx_news_title('graph')
YIELD title, content, score()

-- 带过滤和排序
SEARCH ON idx_product_desc('laptop')
WHERE score() > 0.5
ORDER BY score() DESC
LIMIT 10

-- 布尔查询
SEARCH ON idx_article_content('title:'graph' AND content:'database'')
YIELD title, snippet(content, 200)

-- 短语搜索
SEARCH ON idx_book_content('"graph database"')
YIELD chapter, highlight(content)
```

---

## 7. VECTOR SEARCH - 向量检索

### 功能
对向量索引进行相似度搜索。

### 语法结构
```cypher
VECTOR SEARCH ON <index_name> (<query_vector>)
[LIMIT <n>]
[WITH THRESHOLD <min_score>]
[WHERE <filter_condition>]
```

### 关键特性
- 支持余弦相似度搜索
- 支持欧氏距离搜索
- 支持点积相似度搜索
- 支持阈值过滤
- 支持属性过滤
- 支持批量搜索

### 查询向量格式
- 浮点数数组：`[0.1, 0.2, 0.3, ...]`
- 向量维度必须与索引定义的 `vector_size` 一致

### 示例
```cypher
-- 基本向量搜索
VECTOR SEARCH ON idx_doc_embedding([0.1, 0.2, 0.3, ...])
LIMIT 10

-- 带阈值搜索
VECTOR SEARCH ON idx_article_vector([0.5, 0.3, 0.8, ...])
LIMIT 5
WITH THRESHOLD 0.8

-- 带属性过滤
VECTOR SEARCH ON idx_product_vector([0.1, 0.2, 0.3, ...])
WHERE category == "electronics"
LIMIT 10

-- 搜索并返回相似度分数
VECTOR SEARCH ON idx_image_feature([0.1, 0.2, 0.3, ...])
LIMIT 20
```

---

## 8. GET SUBGRAPH - 子图查询

### 功能
获取指定节点的子图结构。

### 语法结构
```cypher
GET SUBGRAPH [WITH EDGE] <steps> STEPS FROM <vertex_id> [, <vertex_id> ...] [OVER <edge_type>] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持指定起始节点
- 支持入边、出边、双向扩展
- 支持扩展步数限制
- 包含属性信息

### 示例
```cypher
GET SUBGRAPH 2 STEPS FROM "101", "102" OVER follow
```

---

## 9. 辅助子句

### 7.1 RETURN 子句
```cypher
RETURN <expression> [AS <alias>] [, ...] [DISTINCT]
```

### 7.2 YIELD 子句
```cypher
YIELD <expression> [AS <alias>] [, ...] [WHERE <condition>] [SKIP <n>] [LIMIT <n>]
```

#### 关键特性
- 支持属性投影和表达式计算
- 支持 WHERE 条件过滤（在投影后过滤）
- 支持 SKIP 和 LIMIT 分页
- 可作为独立语句使用
- 可与其他语句组合使用

#### 示例
```cypher
-- 基本YIELD
YIELD 1 + 1 AS result

-- YIELD带WHERE过滤
YIELD target.name, target.age WHERE target.age > 25

-- YIELD带分页
YIELD target.name SKIP 5 LIMIT 10

-- GO语句中使用YIELD带WHERE
GO FROM "player100" OVER follow YIELD target.name, target.age WHERE target.age > 25

-- LOOKUP语句中使用YIELD带WHERE
LOOKUP ON person WHERE person.age > 20 YIELD person.name, person.age WHERE person.name STARTS WITH 'A'
```

### 7.3 WHERE 子句
```cypher
WHERE <condition>
```

### 7.4 ORDER BY 子句
```cypher
ORDER BY <expression> [ASC|DESC] [, ...]
```

### 7.5 LIMIT 和 SKIP
```cypher
LIMIT <n>
SKIP <n>
```

### 7.6 WITH 子句
```cypher
WITH <expression> [AS <alias>] [, ...] [WHERE <condition>]
```

### 7.7 UNWIND 子句
```cypher
UNWIND <expression> AS <variable>
```
