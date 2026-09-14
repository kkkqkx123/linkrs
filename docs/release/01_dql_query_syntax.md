# GraphDB 数据查询语言 (DQL)

## 概述

数据查询语言 (DQL) 用于从图数据库中检索数据，包括模式匹配、图遍历、索引查找、路径查找、全文检索、向量检索等功能。

---

## 1. MATCH - 模式匹配查询

### 功能
使用图模式匹配查询节点和边。

### 语法结构
```cypher
MATCH <pattern> [, <pattern> ...] [WHERE <condition>] [RETURN <projection>] [ORDER BY <expression>] [LIMIT <n>] [SKIP <n>]
```

### 关键特性
- 支持节点模式: `(variable:Label {prop: value})`
- 支持边模式: `-[variable:EdgeType {prop: value}]->`、`<-[...]-`、`-[...]-`（无向）
- 支持多边类型: `-[e:KNOWS|FOLLOWS]->`
- 支持路径模式: `(a)-[e]->(b)`
- 支持可变长边: `-[e*]->`、`-[e*2..5]->`、`-[e*..3]->`、`-[e*2]->`
- 支持路径语义修饰: `*TRAIL`（不重复经过边）、`*ACYCLIC`（不重复经过点）、`*SHORTEST`、`*ALL SHORTEST`、`*WEIGHTED(weight_prop)`（带权最短路径）
- 支持递归模式推导: `-[e*(v | WHERE ... | {proj}, {proj})]->`
- 支持谓词过滤
- 支持属性投影（RETURN 支持 DISTINCT、HAVING、SAMPLE、ORDER BY、SKIP、LIMIT）
- 支持 OPTIONAL MATCH
- 支持多模式逗号连接（join），支持 join hint

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

-- 可变长遍历（不重复经过边）
MATCH (a:Person)-[e:KNOWS*1..3 TRAIL]->(b)
RETURN a.name, b.name

-- 带权最短路径
MATCH (a)-[e:ROUTE*SHORTEST]->(b)
RETURN a, b
```

---

## 2. GO - 图遍历查询

### 功能
从起始节点开始进行图遍历。

### 语法结构
```cypher
GO [<steps>] [STEP] FROM <vertex_id> [, <vertex_id> ...] [OVER <edge_type> [, <edge_type> ...] [REVERSELY | BIDIRECT]] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持指定遍历步数（单个整数，缺省为 1；不支持 `GO 2 TO 4 STEPS` 区间形式）
- 支持正向、反向（`REVERSELY` 或 `IN`）、双向（`BIDIRECT`）遍历
- OVER 子句可省略（遍历所有边类型），也可指定多个边类型
- 支持条件过滤
- 支持属性投影（YIELD 支持 DISTINCT、WHERE、ORDER BY、SKIP、LIMIT、SAMPLE）
- 支持管道后缀（`| WHERE ...`、`| YIELD ...` 等）与集合操作（UNION / INTERSECT / MINUS）

### 示例
```cypher
-- 单步遍历
GO FROM "player100" OVER follow

-- 指定步数遍历
GO 2 STEP FROM "player100" OVER follow

-- 反向遍历并投影
GO FROM "player100" OVER follow REVERSELY
WHERE $$.player.age > 18
YIELD $$.player.name, $$.player.age
```

---

## 3. LOOKUP - 基于索引查找

### 功能
使用索引快速查找节点或边。

### 语法结构
```cypher
LOOKUP ON [TAG | EDGE] <tag_or_edge> [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 利用索引加速查询
- `TAG` / `EDGE` 关键字可显式声明目标类型，也可省略（在绑定阶段解析）
- 支持复合条件
- 支持属性投影（YIELD 支持 DISTINCT、WHERE、ORDER BY、SKIP、LIMIT）
- 另有向量检索变体 `LOOKUP VECTOR`（见第 7 节）

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
FETCH [PROP] ON [<tag> | *] <vertex_id> [, <vertex_id> ...] [YIELD <properties>]
FETCH [PROP] ON <edge_type> <src_id> -> <dst_id> [@<rank>] [YIELD <properties>]
```

### 关键特性
- 支持批量获取节点属性
- `*` 表示获取所有 tag 的属性
- 支持获取边属性
- 支持指定边 rank
- 支持属性投影

### 示例
```cypher
-- 获取顶点属性
FETCH PROP ON person "101", "102", "103"

-- 获取所有 tag 的顶点属性
FETCH PROP ON * "101"

-- 获取边属性
FETCH PROP ON follow "101" -> "102" @0
```

---

## 5. FIND PATH - 路径查找

### 功能
查找两个节点之间的路径，支持带权最短路径。

### 语法结构
```cypher
FIND <SHORTEST | ALL> PATH [WITH LOOP] [WITH CYCLE] FROM <src_id> [, <src_id> ...] [TO <dst_id> [, <dst_id> ...]] [OVER <edge_type> [, <edge_type> ...] [REVERSELY | BIDIRECT]] [UPTO <steps> STEP] [WEIGHT <weight_property>] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持最短路径查找（`SHORTEST`，缺省）
- 支持所有路径查找（`ALL`）
- 支持带权最短路径（`WEIGHT` 子句，Dijkstra 算法）
- 路径顶点唯一性是默认行为（路径中不重复访问同一顶点）
- 支持显式允许自环边（A->A）
- 支持显式允许回路（路径中重复访问顶点）
- 支持路径长度限制（`UPTO <steps> STEP`）
- `TO` 子句可省略（查找从起点出发的路径）
- OVER 子句可省略（遍历所有边类型）
- 支持条件过滤
- 支持属性投影

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

### 示例
```cypher
-- 无权最短路径（默认去重自环边）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow

-- 带权最短路径（使用weight属性）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow WEIGHT weight

-- 带权最短路径（使用ranking字段）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow WEIGHT ranking

-- 所有路径查询
FIND ALL PATH FROM "101" TO "201" OVER follow UPTO 5 STEP

-- 允许自环边（用于时序数据查询）
FIND ALL PATH WITH LOOP FROM "player100" TO "player200" OVER temp UPTO 5 STEP

-- 允许回路（路径中可重复访问顶点）
FIND ALL PATH WITH CYCLE FROM "player100" TO "player200" OVER follow UPTO 5 STEP

-- 同时允许自环边和回路
FIND ALL PATH WITH LOOP WITH CYCLE FROM "player100" TO "player200" OVER follow UPTO 5 STEP
```

---

## 6. SEARCH - 全文检索

### 功能
对全文索引进行文本搜索。

### 语法结构
```cypher
SEARCH [INDEX] <index_name> MATCH <query>
[YIELD <field> [, <field> ...]]
[WHERE <condition>]
[ORDER BY <expression> [ASC | DESC] [, ...]]
[LIMIT <n>]
[OFFSET <n>]
```

> 注意：索引名后必须跟 `MATCH` 关键字引出查询表达式。

### 关键特性
- 支持简单文本查询
- 支持字段查询（`field:'keyword'`）
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
| 短语查询 | `'"graph database"'` | 搜索短语 "graph database" |

> 多字段与布尔组合（AND / OR）查询取决于底层全文引擎（tantivy 查询语法）的支持情况。

### YIELD 可用字段

| 字段 | 说明 |
|------|------|
| `<field>` | 索引字段 |
| `score` / `score()` | 相关性评分 |
| `matched_fields` | 匹配的字段 |
| `highlight(<field>)` | 高亮显示 |
| `*` | 所有字段 |

### 示例
```cypher
-- 简单搜索
SEARCH INDEX idx_article_content MATCH 'database'

-- 字段搜索
SEARCH INDEX idx_news_title MATCH 'graph'
YIELD title, content, score

-- 带过滤和排序
SEARCH INDEX idx_product_desc MATCH 'laptop'
WHERE score() > 0.5
ORDER BY score() DESC
LIMIT 10

-- 短语搜索 + 高亮
SEARCH INDEX idx_book_content MATCH '"graph database"'
YIELD chapter, highlight(content)
```

---

## 7. 向量检索

向量检索包含三种语句形式：`SEARCH VECTOR`、`LOOKUP VECTOR` 和 `MATCH VECTOR`。

### 7.1 SEARCH VECTOR - 相似度搜索

#### 语法结构
```cypher
SEARCH VECTOR <index_name> WITH vector = [<float>, ...] | text = '<query>' | param = $<param_name>
[THRESHOLD <min_score>]
[WHERE <filter_condition>]
[ORDER BY <expression> [ASC | DESC]]
[LIMIT <n>]
[OFFSET <n>]
[YIELD <field> [, <field> ...] | RETURN <field> [, <field> ...]]
```

#### 关键特性
- `WITH` 子句为必需，支持三种查询形式：
  - `vector = [0.1, 0.2, ...]`：浮点数组字面量，维度必须与索引定义一致
  - `text = '...'`：文本查询（先嵌入再检索）
  - `param = $name`：参数化查询
- 支持阈值过滤（`THRESHOLD`）
- 支持属性过滤（`WHERE`）
- 支持排序、分页
- `YIELD` 与 `RETURN` 等价，支持 `*` 与别名

#### 示例
```cypher
-- 基本向量搜索
SEARCH VECTOR idx_doc_embedding WITH vector=[0.1, 0.2, 0.3]
LIMIT 10

-- 带阈值、过滤和投影
SEARCH VECTOR idx_product_embedding WITH vector=[0.5, 0.3, 0.8]
WHERE price < 500 AND score > 0.5
ORDER BY price DESC
YIELD product_id, name, price
LIMIT 10
```

### 7.2 LOOKUP VECTOR - 定点向量查找

#### 语法结构
```cypher
LOOKUP VECTOR <schema_name> <index_name> WITH vector = [...] | text = '...' | param = $<name> [YIELD <fields>] [LIMIT <n>]
```

#### 示例
```cypher
LOOKUP VECTOR basketball idx_product_embedding WITH vector=[0.1, 0.2]
YIELD product_id
LIMIT 5
```

### 7.3 MATCH VECTOR - 模式 + 向量条件

#### 语法结构
```cypher
MATCH VECTOR '<pattern_string>' WHERE <field> vector = [...] | text = '...' | param = $<name> [THRESHOLD <min_score>] [YIELD <fields> | RETURN <fields>]
```

#### 示例
```cypher
MATCH VECTOR "(n:Person)" WHERE embedding vector=[0.1, 0.2] THRESHOLD 0.8
RETURN embedding
```

---

## 8. GET SUBGRAPH - 子图查询

### 功能
获取指定节点的子图结构。

### 语法结构
```cypher
GET SUBGRAPH [WITH PROP | WITH EDGE] [<steps> STEPS | STEPS <steps>] FROM <vertex_id> [, <vertex_id> ...] [OVER <edge_type> [, <edge_type> ...] [REVERSELY | BIDIRECT]] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持指定起始节点（多个）
- 支持入边、出边、双向扩展
- 支持扩展步数限制：规范形式 `GET SUBGRAPH <n> STEPS FROM ...`，也支持 `GET SUBGRAPH STEPS <n> FROM ...`；两者都省略时默认 1 步
- `WITH PROP` / `WITH EDGE` 控制返回内容
- 包含属性信息
- 支持条件过滤与属性投影

### 示例
```cypher
GET SUBGRAPH 2 STEPS FROM "101", "102" OVER follow
```

---

## 9. 辅助子句与组合能力

### 9.1 RETURN 子句
```cypher
RETURN <expression> [AS <alias>] [, ...] [DISTINCT] [HAVING <condition>] [SAMPLE <n>] [ORDER BY <expression> [ASC|DESC]] [SKIP <n>] [LIMIT <n>]
```

### 9.2 YIELD 子句
```cypher
YIELD [DISTINCT] <expression> [AS <alias>] [, ...] [WHERE <condition>] [ORDER BY <expression>] [SKIP <n>] [LIMIT <n>] [SAMPLE <n>]
```

#### 关键特性
- 支持属性投影和表达式计算
- 支持 DISTINCT 去重
- 支持 WHERE 条件过滤（在投影后过滤）
- 支持 ORDER BY、SKIP、LIMIT、SAMPLE
- 可作为独立语句使用
- 可与其他语句组合使用（GO / LOOKUP / FETCH / FIND PATH / GET SUBGRAPH 等语句的 YIELD 后缀）

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
LOOKUP ON person WHERE person.age > 20 YIELD person.name WHERE person.name STARTS WITH 'A'
```

### 9.3 WHERE 子句
```cypher
WHERE <condition>
```

### 9.4 ORDER BY 子句
```cypher
ORDER BY <expression> [ASC|DESC] [, ...]
```

### 9.5 LIMIT 和 SKIP
```cypher
LIMIT <n>
SKIP <n>
```

### 9.6 WITH 子句
```cypher
WITH <expression> [AS <alias>] [, ...] [WHERE <condition>]
```

### 9.7 UNWIND 子句
```cypher
UNWIND <expression> AS <variable>
```

### 9.8 管道 (`|`) 与集合操作

任意查询语句后可接管道后缀，对中间结果继续加工：

```cypher
<statement> | WHERE <condition>
<statement> | YIELD <properties>
<statement> | RETURN <properties>
<statement> | WITH <projection>
<statement> | UNWIND <expression> AS <variable>
<statement> | GROUP BY <keys> [YIELD <aggregates>]
```

支持集合操作：

```cypher
<statement> UNION [ALL] <statement>
<statement> INTERSECT <statement>
<statement> MINUS <statement>
```

#### 示例
```cypher
-- 管道过滤
GO FROM "player100" OVER follow | WHERE follow.degree > 1 | YIELD target.name

-- 集合操作
MATCH (a:Person) RETURN a.name UNION MATCH (b:Team) RETURN b.name
```
