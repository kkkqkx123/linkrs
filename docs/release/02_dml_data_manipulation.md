# GraphDB 数据操作语言 (DML)

## 概述

数据操作语言 (DML) 用于对图数据库中的数据进行增删改操作，包括插入、更新、删除、合并、批量导入等功能。

---

## 1. INSERT - 插入数据

### 功能
插入新的节点或边。

### 语法结构
```cypher
INSERT VERTEX [IF NOT EXISTS] <tag_name> [(<prop_list>)] [, <tag_name> [(<prop_list>)] ...] [VALUES] <vid>: [(<value_list>)] [: (<value_list>) ...] [, <vid>: ...]
INSERT VERTEX [IF NOT EXISTS] ON <tag_name> [, <tag_name> ...] [VALUES] <vid> [, <vid> ...]
INSERT EDGE [IF NOT EXISTS] <edge_type> [(<prop_list>)] [VALUES] <src_vid> -> <dst_vid> [@<rank>]: [(<value_list>)] [, <src_vid> -> <dst_vid> [@rank]: ...]
```

### 关键特性
- 支持批量插入节点
- 支持插入边
- 支持指定边rank
- **支持多标签节点插入** - 一个顶点可同时插入多个标签（值组用冒号分隔）
- **支持IF NOT EXISTS** - 仅在顶点/边不存在时插入
- 支持默认属性列表（空括号表示使用所有默认属性）
- 支持 `ON <tag>` 简写形式（不指定属性名列表）
- `VALUES` 关键字可省略
- 顶点 VID 和属性值均为表达式

### 示例
```cypher
-- 基本顶点插入
INSERT VERTEX person(name, age) VALUES "101": ("Alice", 25), "102": ("Bob", 30)

-- 带IF NOT EXISTS的插入（幂等操作）
INSERT VERTEX IF NOT EXISTS person(name, age) VALUES "101": ("Alice", 25)

-- 多标签顶点插入
INSERT VERTEX person(name, age), employee(title, department) VALUES "103": ("Charlie", 35): ("Engineer", "Tech")

-- ON简写形式
INSERT VERTEX ON person VALUES "104"

-- 边插入
INSERT EDGE follow(degree) VALUES "101" -> "102" @0: (0.8)

-- 带IF NOT EXISTS的边插入
INSERT EDGE IF NOT EXISTS follow(degree) VALUES "101" -> "102" @0: (0.8)

-- 批量边插入
INSERT EDGE follow(degree) VALUES "101" -> "102" @0: (0.8), "102" -> "103" @0: (0.9)

-- 空属性列表
INSERT EDGE follow() VALUES "101" -> "102": ()
```

---

## 2. CREATE - 创建数据（Cypher风格）

### 功能
使用Cypher风格语法创建节点和边，提供更直观、灵活的图数据操作方式。

### 语法结构
```cypher
-- 创建节点（属性可选）
CREATE (<variable>:<Label> [{<prop>: <value>, ...}])
CREATE (:<Label> [{<prop>: <value>, ...}])  -- 无变量

-- 创建边（属性可选）
CREATE (<src>)-[:<EdgeType> [{<prop>: <value>, ...}]]->(<dst>)
CREATE (<src>)-[:<EdgeType>]-(<dst>)   -- 无向边
CREATE (<src>)<-[:<EdgeType>]-(<dst>)  -- 反向边

-- 创建路径（节点+边）
CREATE (<var1>:<Label1> [{props}])-[:<EdgeType> [{props}]]->(<var2>:<Label2> [{props}])

-- 创建多个模式
CREATE <pattern1>, <pattern2>, ...
```

### 关键特性

| 特性 | 说明 | 示例 |
|------|------|------|
| **Cypher风格语法** | 与Neo4j兼容的语法 | `CREATE (n:Person {name: 'Alice'})` |
| **Schema自动推断** | 自动创建不存在的Tag和Edge Type | 创建节点时自动创建Person标签 |
| **多标签支持** | 一个节点可以有多个标签 | `CREATE (n:Person:Employee {...})` |
| **变量绑定** | 可在后续引用创建的节点 | `CREATE (n)-[:KNOWS]->(m)` |
| **可选属性** | 节点和边可以没有属性 | `CREATE (n:Person)` |
| **批量创建** | 支持一次创建多个模式 | `CREATE (a), (b), (c)` |
| **路径创建** | 同时创建节点和关系 | `CREATE (a)-[:KNOWS]->(b)` |

### Schema自动推断

当使用CREATE语句创建数据时，如果指定的标签或边类型不存在，系统会自动推断并创建Schema：

| 属性值类型 | 推断的Schema类型 | 示例 |
|------------|------------------|------|
| 字符串 | STRING | `{name: 'Alice'}` → `name: STRING` |
| 整数 | INT64 | `{age: 30}` → `age: INT64` |
| 浮点数 | DOUBLE | `{salary: 50000.50}` → `salary: DOUBLE` |
| 布尔值 | BOOL | `{active: true}` → `active: BOOL` |
| 日期时间 | DATETIME | `{created: datetime()}` → `created: DATETIME` |

**注意：**
- 自动创建的Schema属性默认可空（NULL）
- 不会自动设置默认值
- 不会自动添加NOT NULL约束
- 如需更精确的Schema控制，请使用DDL语句预先定义

### 与NGQL语法对比

| 操作 | Cypher语法 | NGQL语法 |
|------|------------|----------|
| 创建节点 | `CREATE (n:Person {name: 'Alice', age: 30})` | `INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)` |
| 创建边 | `CREATE (a)-[:KNOWS {since: '2020-01-01'}]->(b)` | `INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')` |
| 多标签 | `CREATE (n:Person:Employee {...})` | `INSERT VERTEX Person(...), Employee(...) VALUES 1:(...):(...)` |

### 示例

#### 基本节点创建
```cypher
-- 创建带属性的节点
CREATE (p:Person {name: 'Alice', age: 25})

-- 创建无属性的节点
CREATE (p:Person)

-- 创建多标签节点
CREATE (p:Person:Employee {name: 'Bob', department: 'Engineering'})

-- 创建无变量节点（匿名节点）
CREATE (:Person {name: 'Charlie'})
```

#### 边创建
```cypher
-- 创建带属性的边
CREATE (a)-[:KNOWS {since: '2020-01-01', degree: 0.8}]->(b)

-- 创建无属性的边
CREATE (a)-[:FRIEND]->(b)

-- 创建双向边
CREATE (a)-[:COLLEAGUE]-(b)

-- 创建反向边
CREATE (a)<-[:FOLLOWS]-(b)
```

#### 路径创建
```cypher
-- 创建节点和边（完整路径）
CREATE (a:Person {name: 'Alice'})-[:KNOWS {since: '2020-01-01'}]->(b:Person {name: 'Bob'})

-- 创建长路径
CREATE (a:Person)-[:KNOWS]->(b:Person)-[:WORKS_AT]->(c:Company)
```

#### 批量创建
```cypher
-- 创建多个节点
CREATE (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}), (c:Person {name: 'Charlie'})

-- 创建多个边
CREATE (a)-[:KNOWS]->(b), (b)-[:KNOWS]->(c), (c)-[:KNOWS]->(a)

-- 混合创建
CREATE 
  (a:Person {name: 'Alice'}),
  (b:Person {name: 'Bob'}),
  (a)-[:KNOWS {since: '2020-01-01'}]->(b)
```

#### 复杂属性
```cypher
-- 使用各种数据类型
CREATE (p:Person {
  name: 'Alice',
  age: 30,
  salary: 50000.50,
  is_active: true,
  created_at: datetime(),
  tags: ['engineer', 'leader']
})
```

---

## 3. UPDATE - 更新数据

### 功能
更新节点或边的属性。

### 语法结构
```cypher
UPDATE VERTEX <vid> SET <prop> = <value> [, <prop> = <value> ...] [WHERE <condition>] [YIELD <return_list>]
UPDATE EDGE OF <edge_type> FROM <src_vid> TO <dst_vid> [@<rank>] SET <prop> = <value> [, ...] [WHERE <condition>] [YIELD <return_list>]
UPDATE EDGE <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> SET <prop> = <value> [, ...] [WHERE <condition>] [YIELD <return_list>]
```

### 关键特性
- 支持条件更新（WHERE子句，`WHEN` 为同义关键字）
- 支持多属性更新
- 支持节点和边更新
- 支持表达式计算
- 边更新支持两种语法：`OF <edge_type> FROM <src> TO <dst>` 与短形式 `<src> -> <dst> OF <edge_type>`
- **支持YIELD子句** - 返回更新后的属性值

> 注意：`ON <tag>` 子句仅在 UPSERT 语句中支持，UPDATE 不支持。

### 示例
```cypher
-- 基本顶点更新
UPDATE VERTEX "101" SET age = 26, name = "Alice Smith"

-- 带条件的顶点更新
UPDATE VERTEX "101" SET age = age + 1 WHERE age > 20

-- 带YIELD的更新（返回更新后的值）
UPDATE VERTEX "101" SET age = 26 YIELD $^.person.name, $^.person.age

-- 边更新（OF ... FROM ... TO ... 语法）
UPDATE EDGE OF follow FROM "101" TO "102" SET degree = 0.9

-- 边更新（短形式语法）
UPDATE EDGE "101" -> "102" @0 OF follow SET degree = 0.9

-- 带条件的边更新
UPDATE EDGE "101" -> "102" @0 OF follow SET degree = degree + 0.1 WHERE degree < 1.0
```

---

## 4. UPSERT - 插入或更新数据

### 功能
如果数据存在则更新，不存在则插入（原子操作）。

### 语法结构
```cypher
UPSERT VERTEX <vid> [ON <tag_name>] [SET <prop> = <value> [, ...]] [WHERE <condition>] [YIELD <return_list>]
UPSERT VERTEX ON <tag_name> [SET <prop> = <value> [, ...]] WHERE id(vid) == <vid_value> [YIELD <return_list>]
UPSERT EDGE <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> [SET <prop> = <value> [, ...]] [WHERE <condition>] [YIELD <return_list>]
UPSERT EDGE ON <edge_type> [SET <prop> = <value> [, ...]] WHERE id(src) == <src_value> AND id(dst) == <dst_value> [@<rank>] [YIELD <return_list>]
```

### 关键特性
- **原子性操作** - 保证插入或更新的原子性
- 支持条件判断
- 支持ON子句指定标签/边类型
- 支持YIELD子句返回结果
- 支持可选 `WHEN` 子句（解析但语义上不影响插入或更新判定）
- 适用于需要幂等写入的场景

### 示例
```cypher
-- 顶点UPSERT（存在则更新，不存在则插入）
UPSERT VERTEX "101" SET name = "Alice", age = 25

-- 指定标签的UPSERT
UPSERT VERTEX "101" ON person SET name = "Alice", age = 25

-- ON tag + WHERE 指定 vid 的UPSERT
UPSERT VERTEX ON person SET age = 26 WHERE id(vid) == "101"

-- 带YIELD的UPSERT
UPSERT VERTEX "101" SET age = 26 YIELD $^.person.name, $^.person.age

-- 边UPSERT
UPSERT EDGE "101" -> "102" OF follow SET degree = 0.9

-- 带rank的边UPSERT
UPSERT EDGE "101" -> "102" @1 OF follow SET degree = 0.9 YIELD $^.follow.degree
```

---

## 5. DELETE - 删除数据

### 功能
删除节点、边或标签。

### 语法结构
```cypher
DELETE VERTEX <vertex_id> [, <vertex_id> ...] [WITH EDGE]
DELETE TAG <tag_name> [, <tag_name> ...] FROM <vid> [, <vid> ...]
DELETE TAG * FROM <vid> [, <vid> ...]
DELETE EDGE <edge_type> <src_vid> -> <dst_vid> [@<rank>] [, ...]
DELETE EDGE <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> [, ...]
DETACH DELETE <vertex_expr> [, <vertex_expr> ...]
```

### 关键特性
- 支持批量删除节点
- 支持删除边（两种语法：边类型在前，或 `OF <edge_type>` 后置）
- 支持指定边rank
- 支持级联删除关联边（WITH EDGE）
- 支持 Cypher 风格的 `DETACH DELETE`（等价于 WITH EDGE）
- **支持删除指定标签** - 从顶点移除特定标签而不删除顶点
- **支持通配符删除所有标签** - DELETE TAG *
- 支持在 MATCH 语句后直接接 DELETE 子句（见下文）
- 不指定WITH EDGE时保留边（产生悬挂边）

### 示例
```cypher
-- 仅删除顶点，保留关联边（可能产生悬挂边）
DELETE VERTEX "101", "102"

-- 删除顶点及其所有关联边
DELETE VERTEX "101" WITH EDGE

-- Cypher风格级联删除
DETACH DELETE "101"

-- 删除指定标签（保留顶点和其他标签）
DELETE TAG employee FROM "101"

-- 删除多个标签
DELETE TAG employee, manager FROM "101", "102"

-- 删除所有标签（保留空顶点）
DELETE TAG * FROM "101"

-- 删除边
DELETE EDGE follow "101" -> "102" @0

-- 删除边（OF后置语法）
DELETE EDGE "101" -> "102" @0 OF follow

-- 批量删除边
DELETE EDGE follow "101" -> "102" @0, "102" -> "103" @0
```

### MATCH + DELETE

MATCH 语句支持直接跟随 DELETE 子句，删除匹配到的数据：

```cypher
-- 删除匹配到的顶点（级联删除边）
MATCH (p:Person {name: 'Alice'}) DELETE p

-- 删除匹配到的边
MATCH (a)-[e:KNOWS]->(b) DELETE e
```

### 悬挂边说明
- 当删除顶点时不使用WITH EDGE，与该顶点关联的边会变成悬挂边（边的起点或终点不存在）
- 悬挂边可以通过GO语句查询到边属性，但点属性为空
- MATCH语句不会返回悬挂边
- 可以使用存储层API检测和修复悬挂边

---

## 6. MERGE - 合并数据

### 功能
如果存在则更新，不存在则创建（基于模式匹配）。

### 语法结构
```cypher
MERGE (<variable>:<Label> {<prop>: <value>}) [ON CREATE SET <prop> = <value>] [ON MATCH SET <prop> = <value>]
MERGE EDGE|VERTEX ON <tag_or_edge_type> [SET <prop> = <value>] WHERE id(vid|src) == <value> [AND id(dst) == <value>]
```

### 关键特性
- 幂等操作
- 支持存在时更新（ON MATCH）
- 支持不存在时创建（ON CREATE）
- 避免重复数据
- 基于模式匹配，比UPSERT更灵活
- 支持 UPSERT 风格的 `MERGE VERTEX/EDGE ON ...` 语法
- MATCH 语句支持跟随 MERGE 子句（解析校验，不改变匹配结果）
- 模式后允许裸 `SET` 子句（解析校验）

### 示例
```cypher
-- 基本MERGE操作
MERGE (p:Person {name: 'Alice'})
ON MATCH SET p.last_seen = timestamp()
ON CREATE SET p.created_at = timestamp()

-- 带多个属性的MERGE
MERGE (p:Person {id: '101'})
ON MATCH SET p.name = 'Alice', p.updated = true
ON CREATE SET p.name = 'Alice', p.created = timestamp(), p.status = 'active'

-- MERGE边关系
MERGE (a:Person {name: 'Alice'})-[r:FRIEND {since: 2020}]->(b:Person {name: 'Bob'})
ON MATCH SET r.updated = timestamp()
ON CREATE SET r.created = timestamp()
```

---

## 7. SET - 设置属性

### 功能
设置或更新属性值（独立语句，也可作为 UPDATE/MERGE/UPSERT 的 SET 子句）。

### 语法结构
```cypher
SET <variable>.<prop> = <value> [, <variable>.<prop> = <value> ...]
```

### 关键特性
- 支持动态属性设置
- 支持表达式计算
- 支持批量设置
- 赋值左侧必须是属性路径（如 `p.age`）
- 支持属性增减操作

### 示例
```cypher
-- 基本属性设置
SET p.age = 26, p.updated = true

-- 表达式计算
SET p.age = p.age + 1, p.full_name = p.first_name + ' ' + p.last_name

-- 移除属性（设置为NULL）
SET p.temp_field = NULL
```

---

## 8. REMOVE - 移除属性

### 功能
移除节点或边的属性或标签（独立语句）。

### 语法结构
```cypher
REMOVE <variable>.<prop> [, <variable>.<prop> ...]
REMOVE <variable>:<Label> [, <variable>:<Label> ...]
```

### 关键特性
- 支持移除属性
- 支持移除标签（与DELETE TAG类似）
- 支持批量操作
- 返回移除的属性/标签数量

### 示例
```cypher
-- 移除属性
REMOVE p.temp_field

-- 移除多个属性
REMOVE p.temp1, p.temp2, p.deprecated_field

-- 移除标签
REMOVE p:OldLabel

-- 同时移除属性和标签
REMOVE p.temp_field, p:OldLabel
```

---

## 9. COPY - 批量导入/导出

### 功能
从 CSV 文件批量导入顶点/边数据，或将数据导出到 CSV 文件。

### 语法结构
```cypher
COPY VERTEX <tag_name> FROM '<file_path>' [, '<file_path>' ...] [BY COLUMN] [WITH (HEADER [true|false] | NO HEADER [, DELIMITER '<char>'] [, BATCH_SIZE <n>] [, CSV])]
COPY EDGE <edge_type> FROM '<file_path>' [, ...] [BY COLUMN] [WITH (...)]
COPY <tag_name> FROM '<file_path>'   -- 省略VERTEX关键字，默认为顶点
COPY VERTEX|EDGE <name> TO '<file_path>' [WITH (...)]
```

### 关键特性
- 支持从单个或多个 CSV 文件导入
- `FROM` 导入 / `TO` 导出；`TO` 仅支持单个文件
- `BY COLUMN`：多文件按列合并导入（仅 FROM）
- 选项可放在括号内或直接罗列：
  - `HEADER true|false` 或 `NO HEADER`（默认有表头）
  - `DELIMITER '<char>'`（默认 `,`）
  - `BATCH_SIZE <n>`：批量提交大小
  - `CSV`：显式声明 CSV 格式
- 省略 `VERTEX`/`EDGE` 关键字时默认为顶点导入

### 示例
```cypher
-- 从CSV导入顶点（默认有表头）
COPY VERTEX person FROM '/data/person.csv'

-- 多文件导入，自定义分隔符与批次
COPY EDGE follow FROM '/data/follow_1.csv', '/data/follow_2.csv'
  WITH (HEADER true, DELIMITER '|', BATCH_SIZE 1000)

-- 按列合并多文件
COPY VERTEX person FROM '/data/a.csv', '/data/b.csv' BY COLUMN

-- 导出
COPY VERTEX person TO '/tmp/person_out.csv'
```

---

## DML 语句对比总结

| 语句 | 主要用途 | 幂等性 | 条件支持 | 返回结果 | 适用场景 |
|------|---------|--------|----------|----------|----------|
| **INSERT** | 插入新数据 | 可选(IF NOT EXISTS) | 无 | 插入数量 | 批量导入、初始化数据 |
| **UPDATE** | 更新已有数据 | 否 | WHERE | YIELD支持 | 属性修改、增量更新 |
| **UPSERT** | 插入或更新 | 是 | WHERE | YIELD支持 | 幂等写入、同步场景 |
| **DELETE** | 删除数据 | 否 | MATCH模式 | 删除数量 | 数据清理、标签移除 |
| **MERGE** | 匹配则更新，否则创建 | 是 | 模式匹配 | 无 | 复杂条件的数据合并 |
| **SET** | 属性设置 | 否 | 无 | 无 | 属性动态更新 |
| **REMOVE** | 移除属性/标签 | 否 | 无 | 移除数量 | 清理临时数据 |
| **COPY** | 批量导入/导出 | 否 | 无 | 导入/导出统计 | CSV数据搬迁 |

### 选择建议

- **需要幂等性**: 使用 `INSERT IF NOT EXISTS` 或 `UPSERT`
- **需要返回更新后数据**: 使用 `UPDATE ... YIELD` 或 `UPSERT ... YIELD`
- **需要删除标签但保留顶点**: 使用 `DELETE TAG` 或 `REMOVE :Label`
- **复杂条件的数据合并**: 使用 `MERGE` (基于模式匹配)
- **简单的属性更新**: 使用 `UPDATE` 或 `SET`
- **CSV 批量搬迁**: 使用 `COPY`
