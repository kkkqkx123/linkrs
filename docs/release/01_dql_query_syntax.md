# Linkrs 数据查询语言 (DQL)

## 概述

数据查询语言 (DQL) 用于从图数据库中检索数据，包括模式匹配、图遍历、索引查找、路径查找、全文检索、向量检索等功能，
并覆盖分组聚合、管道组合、集合操作、变量赋值等辅助子句与表达能力（见第 9、10 节）。

---

## 1. MATCH - 模式匹配查询

### 功能
使用图模式匹配查询节点和边。

### 语法结构
```cypher
MATCH <pattern> [, <pattern> ...] [WHERE <condition>] [RETURN <projection>] [ORDER BY <expression>] [LIMIT <n>] [SKIP <n>]
```

### 关键特性
- 支持节点模式: `(variable:Label {prop: value})`，标签支持多段写法 `:Person:Employee`
- 支持边模式: `-[variable:EdgeType {prop: value}]->`、`<-[...]-`、`-[...]-`（无向）；边类型可省略 `:`，如 `-[knows]->`
- 支持多边类型: `-[e:KNOWS|FOLLOWS]->`（`|` 仅用于边类型；节点多标签用 `:A:B` 形式）
- 支持路径模式: `(a)-[e]->(b)`
- 支持可变长边: `-[e*]->`、`-[e*2..5]->`、`-[e*..3]->`、`-[e*2]->`，同时支持方括号形式 `-[e*[2..5]]->`
- 支持路径语义修饰: `*TRAIL`（不重复经过边）、`*ACYCLIC`（不重复经过点）、`*SHORTEST`、`*ALL SHORTEST`、`*WEIGHTED(weight_prop)`（带权最短路径）
- 支持递归模式推导: `-[e*(v | WHERE ... | {node_proj}, {edge_proj})]->`（两个投影依次为节点投影、边投影）
- 支持谓词过滤（WHERE 可出现在 RETURN 之前，也可在 RETURN 之后追加第二个 WHERE，两者取 AND）
- 支持属性投影（RETURN 支持 `*`、DISTINCT、GROUP BY、HAVING、SAMPLE、ORDER BY、SKIP、LIMIT）
- 支持 OPTIONAL MATCH
- 支持多模式逗号连接（join），支持 join hint：`USING JOIN BINARY(left, right)` 或 `USING JOIN MULTIWAY(probe, build, ...)`
- MATCH 模式之后还支持 `DELETE VERTEX/EDGE` 子句与 `MERGE` 子句

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

> **路径语义说明**：`*TRAIL` 不重复经过边（顶点可重复），`*ACYCLIC`
> 不重复经过点。两者与 `*min..max` 范围可组合（`*1..3 TRAIL`）；
> 语义与范围都是可选的，**只写语义不写范围时为无界枚举**——路径数随
> 图稠密程度阶乘级增长，请按需加上界（如 `*1..5`）或依赖查询取消。
> **纯 `*` 不写语义时同样无界**：为保持枚举有限（无约束的 walk 在带环
> 图上不会终止），无界枚举按 trail 规则执行；带上界时（如 `*1..3`）
> 按 walk 规则执行，跳数上界保证终止。

---

## 2. GO - 图遍历查询

### 功能
从起始节点开始进行图遍历。

### 语法结构
```cypher
GO [<steps>] [STEP] FROM <vertex_id> [, <vertex_id> ...] [OVER <edge_type> [, <edge_type> ...] [REVERSELY | BIDIRECT]] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持指定遍历步数：目前仅支持单个整数字面量（缺省为 1）；`GO 2 TO 4 STEPS` 区间形式暂不支持（AST 已预留 Range/Variable 表示，解析层未接入；非整数字面量会静默按 1 步处理）
- 支持正向、反向（`REVERSELY` 或 `IN`）、双向（`BIDIRECT`）遍历
- OVER 子句可省略（遍历所有边类型），也可指定多个边类型（逗号分隔）
- 支持 `$$.tag.prop`（目标点）、`$^.tag.prop`（源点）、`$-.prop`（管道输入）引用表达式
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
- `TAG` / `EDGE` 关键字可显式声明目标类型；`ON` 子句也可整体省略（此时按名称解析：先尝试 tag，再尝试边类型）
- 支持复合条件
- 支持属性投影（YIELD 支持 DISTINCT、WHERE、ORDER BY、SKIP、LIMIT）
- 另有全文检索变体 `LOOKUP ON <schema> INDEX <index> WHERE '<query>'` 与向量检索变体 `LOOKUP VECTOR`（见第 6、7 节）

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
FETCH TAG <tag> <vertex_id> [, <vertex_id> ...]
FETCH EDGE <edge_type> <src_id> -> <dst_id> [@<rank>]
```

### 关键特性
- 支持一次获取多个顶点属性（逗号分隔的 id 列表）
- `*` 表示获取所有 tag 的属性（仅顶点形式；边形式使用 `*` 会直接报错）
- 支持获取边属性
- 支持指定边 rank
- 支持属性投影
- 不支持 `FETCH PROP ON <tag> <id> <prop_list>` 指定属性列表的写法（语法保留但解析后被丢弃）

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
FIND <SHORTEST | ALL> PATH [WITH LOOP] [WITH TRAIL] [WITH WALK] FROM <src_id> [, <src_id> ...] [TO <dst_id>] [OVER <edge_type> [, <edge_type> ...] [REVERSELY | BIDIRECT]] [UPTO <steps> STEP] [WEIGHT <weight_property>] [WHERE <condition>] [YIELD <properties>]
```

### 关键特性
- 支持最短路径查找（`SHORTEST`，缺省）
- 支持所有路径查找（`ALL`）
- 支持带权最短路径（`WEIGHT <prop>`，Dijkstra 算法）；`WEIGHT ranking` 使用边 ranking 字段，其余按属性名读取（缺失/非数值权重按 1.0，负值截断为 0.0，与 MATCH `*WEIGHTED` 规则一致）；每个 (src, dst) 对返回一条最廉价路径
- `WEIGHT` 仅支持 `FIND SHORTEST PATH`；与 `FIND ALL PATH` 组合会在绑定阶段报错
- 支持路径长度限制（`UPTO <steps> STEP`，`STEPS` 亦可）；省略时默认最大 10 步
- 支持多边类型路径（`OVER <type> [, <type> ...]`）：端点标签由全部所列边类型的 schema 分方向推导，任一角色无法收敛到唯一标签时报显式错误
- `TO` 子句可省略；目前 `TO` 后仅支持单个目标点表达式，不支持逗号分隔的 id 列表
- OVER 子句可省略（遍历所有边类型，方向缺省为正向）
- 支持条件过滤与属性投影（WHERE 在路径结果之上过滤）

### 环路控制选项

| 选项 | 默认 | 说明 |
|-----|------|------|
| `WITH WALK` | 无 | 允许路径中重复访问顶点与边（如 A->B->C->A），自环边随之自然合法 |
| `WITH TRAIL` | 无 | 边不可重复（平行边因类型/rank 区分而互不相同），顶点可重复（如 A->B->A->B） |
| `WITH LOOP` | 无 | 仅放行自环边（A->A，作为路径末端扩展），其余顶点重复仍被拒绝 |

### 区别说明
- 三个选项与标准路径分类一一对应：缺省即 path（顶点唯一、自环剪除，`VertexUnique`）、`WITH LOOP` → `AllowSelfLoop`、`WITH TRAIL` → `EdgeUnique`、`WITH WALK` → `Free`；越宽松的选项优先
- 这三个选项仅对 `FIND ALL PATH` 生效；`FIND SHORTEST PATH` 忽略它们（回路不可能产生更短路径）
- 默认情况下，路径顶点唯一
- `WITH WALK`/`WITH TRAIL` 与 MATCH 侧 `*`（无界时按 trail 枚举）的重复语义一致；`WITH TRAIL` 与 `*TRAIL` 同义

### 算法选择
- **无权图**: 使用最短路径算子（BFS 类扩展）
- **带权图**: `FIND SHORTEST PATH ... WEIGHT <prop>` 使用 Dijkstra 算法；MATCH 可变长模式 `*WEIGHTED(prop)` 使用同一权重规则

### 示例
```cypher
-- 无权最短路径（默认去重自环边）
FIND SHORTEST PATH FROM "101" TO "201" OVER follow

-- 所有路径查询（UPTO 省略时默认 10 步）
FIND ALL PATH FROM "101" TO "201" OVER follow UPTO 5 STEP

-- 允许自环边（其余顶点重复仍被拒绝）
FIND ALL PATH WITH LOOP FROM "player100" TO "player200" OVER temp UPTO 5 STEP

-- 边不可重复、顶点可重复（标准 trail 语义，与 *TRAIL 同义）
FIND ALL PATH WITH TRAIL FROM "player100" TO "player200" OVER follow UPTO 5 STEP

-- 顶点与边均可重复（标准 walk 语义）
FIND ALL PATH WITH WALK FROM "player100" TO "player200" OVER follow UPTO 5 STEP
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
- 短语查询依赖底层全文引擎（tantivy）对查询串内引号的解释，解析层不区分短语 token
- 前缀、通配符、模糊搜索等无解析层语法，仅在查询串内依赖 tantivy 查询语法支持
- 支持评分排序
- 支持结果高亮
- 支持分页（ORDER BY / LIMIT 省略方向时默认 ASC）

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
| `score` / `score()` | 相关性评分（裸标识符 `score`、`matched_fields` 会自动按零参函数处理） |
| `matched_fields` | 匹配的字段 |
| `highlight(<field>)` | 高亮显示 |
| `*` | 所有字段 |

> 除 `SEARCH` 外，解析层还支持两种全文检索形式：
> - `LOOKUP ON <schema> INDEX <index> WHERE '<query>'`
> - `MATCH '<pattern>' WHERE FULLTEXT_MATCH(<field>, '<query>')`

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
- `WITH` 子句为必需，支持三种查询形式（`parameter = $name` 与 `param = $name` 等价）：
  - `vector = [0.1, 0.2, ...]`：浮点数组字面量，维度必须与索引定义一致
  - `text = '...'`：文本查询（先嵌入再检索）
  - `param = $name`：参数化查询
- 支持阈值过滤（`THRESHOLD`）
- 支持属性过滤（`WHERE`）
- 支持排序（省略方向时默认 ASC）、分页
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
LOOKUP VECTOR <schema_name> <index_name> WITH vector = [...] | text = '...' | param = $<name> [YIELD <fields> | RETURN <fields>] [LIMIT <n>]
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
- 支持扩展步数限制：规范形式 `GET SUBGRAPH <n> STEPS FROM ...`，也支持 `GET SUBGRAPH STEPS <n> FROM ...`（`STEP`/`STEPS` 均可，且步数必须在 `FROM` 之前）；两者都省略时默认 1 步
- `WITH PROP` / `WITH EDGE` 目前仅完成语法解析，绑定与执行阶段尚未接入（不影响返回内容）
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
RETURN [* | <expression> [AS <alias>]] [, ...] [DISTINCT] [GROUP BY <keys>] [HAVING <condition>] [SAMPLE <n>] [ORDER BY <expression> [ASC|DESC]] [SKIP <n>] [LIMIT <n>]
```

> RETURN 可作为独立语句使用（如 `RETURN 1 + 1`），GROUP BY 另支持 `ROLLUP` / `CUBE` / `GROUPING SETS`。SKIP 与 LIMIT 可按任意顺序书写。

### 9.2 YIELD 子句
```cypher
YIELD [DISTINCT] <expression> [AS <alias>] [, ...] [WHERE <condition>] [ORDER BY <expression>] [SKIP <n>] [LIMIT <n>] [SAMPLE <n>]
```

#### 关键特性
- 支持属性投影和表达式计算，支持 `*`
- YIELD 子句语法支持 DISTINCT 去重
- 支持 WHERE 条件过滤（在投影后过滤）
- 支持 ORDER BY、SKIP、LIMIT、SAMPLE
- 可作为独立语句使用（注意：独立 YIELD 语句当前实现会丢弃 DISTINCT 与 SAMPLE，仅作为后缀子句时完整生效）
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
WITH [DISTINCT] <expression> [AS <alias>] [, ...] [WHERE <condition>] [ORDER BY ...] [SKIP <n>] [LIMIT <n>]
WITH <name> AS (<query>) [, <name2> AS (<query2>) ...]      -- CTE
WITH RECURSIVE <name> AS (<query>) [, ...]                  -- 递归 CTE
```

> WITH 支持表达式投影（含 DISTINCT / WHERE / ORDER BY / SKIP / LIMIT），也支持定义普通与递归 CTE，且可作为独立语句使用。

### 9.7 UNWIND 子句
```cypher
UNWIND <expression> AS <variable>
```

> UNWIND 可作为独立语句使用，也可作为管道阶段。

### 9.8 管道 (`|`) 与集合操作

任意查询语句后可接管道后缀，对中间结果继续加工：

```cypher
<statement> | WHERE <condition>
<statement> | YIELD <properties>
<statement> | RETURN <properties>
<statement> | WITH <projection>
<statement> | UNWIND <expression> AS <variable>
<statement> | GROUP BY <keys> [YIELD <aggregates>]
<statement> | COLLECT <expression> [AS <alias>] [, ...]
```

> 除 `| WHERE` 与 `| COLLECT` 外，`RETURN` / `WITH` / `UNWIND` / `GROUP BY` 作为管道阶段时 `|` 可省略（直接跟在上一条语句之后）。

支持集合操作（直接跟在语句之后，无需 `|`）：

```cypher
<statement> UNION [ALL] <statement>
<statement> INTERSECT <statement>
<statement> MINUS <statement>
```

| 操作符 | 说明 |
|--------|------|
| UNION | 并集，去重 |
| UNION ALL | 并集，保留重复 |
| INTERSECT | 交集 |
| MINUS | 差集 |

#### 示例
```cypher
-- 管道过滤
GO FROM "player100" OVER follow | WHERE follow.degree > 1 | YIELD target.name

-- 集合操作
MATCH (a:Person) RETURN a.name UNION MATCH (b:Team) RETURN b.name
```

### 9.9 GROUP BY 语句

#### 语法结构
```cypher
GROUP BY <expression> [, <expression> ...]
[YIELD <item> [, <item> ...]]
[HAVING <condition>]

-- 分组扩展
GROUP BY ROLLUP (<expression> [, ...])       [YIELD ...] [HAVING ...]
GROUP BY CUBE (<expression> [, ...])         [YIELD ...] [HAVING ...]
GROUP BY GROUPING SETS ((<expr>, ...), ...)  [YIELD ...] [HAVING ...]
```

#### 关键特性
- 支持多字段分组
- 支持 YIELD 子句指定输出（缺省时按分组列输出，别名为 `group_0`、`group_1`...）
- 支持 HAVING 子句过滤分组
- 支持 ROLLUP / CUBE / GROUPING SETS 分组扩展
- 可与管道操作符结合使用

> RETURN 子句内置的 `GROUP BY`（含 ROLLUP / CUBE / GROUPING SETS，见 9.1 节）
> 与管道阶段 `| GROUP BY`（见 9.8 节）之外的独立语句形态在此描述。

#### 示例
```cypher
-- 基础分组
GROUP BY category YIELD category, count(*) AS cnt

-- 带 HAVING 过滤
GROUP BY age YIELD age, count(*) AS cnt HAVING cnt > 5

-- ROLLUP 小计
GROUP BY ROLLUP (country, city) YIELD country, city, count(*) AS cnt

-- GROUPING SETS
GROUP BY GROUPING SETS ((country, city), (country)) YIELD country, city, count(*) AS cnt

-- 与管道结合
GO FROM "player100" OVER follow | GROUP BY $-.dst YIELD $-.dst, count(*) AS cnt
```

### 9.10 变量赋值

#### 语法结构
```cypher
$<variable_name> = <statement>     -- 结果集变量：存储查询结果
LET [$]<name> = <expression>       -- 会话变量：存储表达式求值结果
```

#### 关键特性
- 结果集变量：变量名以 `$` 开头，可存储查询结果供后续语句与管道引用
  （如 `GO FROM $friends.dst OVER serve`）
- 会话变量：`LET` 右侧是表达式（非语句），可引用其他会话变量 `$name` 与参数 `@name`
- 会话变量名必须是合法标识符（`[A-Za-z_][A-Za-z0-9_]*`），`$` 前缀可省略，作用域为当前会话

#### 示例
```cypher
-- 结果集变量
$result = GO FROM "player100" OVER follow
$friends = MATCH (n:Person) RETURN n
GO FROM $friends.dst OVER serve

-- 会话变量
LET threshold = 25
LET $limit = 10
```

---

## 10. 表达式速查

### 10.1 字面量
- 字符串: `'hello'`, `"world"`
- 整数: `123`, `-456`
- 浮点数: `3.14`, `-0.5`
- 布尔值: `true`, `false`
- NULL: `NULL`

### 10.2 属性访问
```cypher
$^.tag.prop        -- 访问源点属性
$$.tag.prop        -- 访问目标点属性
$-.prop            -- 访问边属性
variable.prop      -- 访问变量属性
```

### 10.3 运算符
| 类型 | 运算符 |
|------|--------|
| 算术 | `+`, `-`, `*`, `/`, `%` |
| 比较 | `=`, `==`, `!=`, `<>`, `<`, `>`, `<=`, `>=` |
| 逻辑 | `AND`, `OR`, `NOT`, `XOR` |
| 字符串 | `+` (连接) |
| 列表 | `IN` |

### 10.4 函数
| 类别 | 函数 |
|------|------|
| 聚合 | `count()`, `sum()`, `avg()`, `max()`, `min()`, `collect()` |
| 字符串 | `concat()`, `substring()`, `lower()`, `upper()`, `trim()` |
| 数学 | `abs()`, `round()`, `floor()`, `ceil()`, `sqrt()`, `pow()` |
| 时间 | `now()`, `timestamp()`, `date()`, `datetime()` |
| 图相关 | `id()`, `tags()`, `type()`, `src()`, `dst()`, `rank()` |
