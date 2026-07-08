# Neo4j Cypher DML - 数据操作语言

## 概述

DML（Data Manipulation Language）用于创建、修改和删除图中的数据，包括节点、关系和属性。

---

## 1. CREATE - 创建数据

### 1.1 创建节点

```cypher
// 创建单个节点
CREATE (n:Person)
RETURN n

// 创建带属性的节点
CREATE (p:Person {name: "Alice", age: 30})
RETURN p

// 创建带多个标签的节点
CREATE (p:Person:Employee {name: "Bob", id: 123})
RETURN p

// 创建多个节点
CREATE (a:Person {name: "Alice"}), (b:Person {name: "Bob"})
RETURN a, b

// 创建带列表属性的节点
CREATE (p:Person {name: "Alice", tags: ["developer", "manager"]})
RETURN p

// 创建带动态标签的节点
CREATE (greta:$(nodeLabels) {name: 'Greta Gerwig'})
RETURN greta.name, labels(greta)
```

### 1.2 创建关系

```cypher
// 创建节点并建立关系
CREATE (a:Person {name: "Alice"})-[r:FRIENDS_WITH]->(b:Person {name: "Bob"})
RETURN a, r, b

// 在已存在的节点之间创建关系
MATCH (a:Person {name: "Alice"}), (b:Person {name: "Bob"})
CREATE (a)-[r:FRIENDS_WITH]->(b)
RETURN r

// 创建带属性的关系
MATCH (a:Person {name: "Alice"}), (b:Person {name: "Bob"})
CREATE (a)-[r:FRIENDS_WITH {since: 2020, strength: 0.9}]->(b)
RETURN r

// 创建双向关系
MATCH (a:Person), (b:Person)
CREATE (a)-[:FRIENDS_WITH]-(b)
RETURN a, b

// 创建带动态关系类型的关系
CREATE ()-[r:$(relType)]->()
RETURN type(r)
```

### 1.3 创建路径

```cypher
// 创建完整路径
CREATE p = (a:Person {name: "Alice"})-[:FRIENDS_WITH]->(b:Person {name: "Bob"})
RETURN p

// 创建多跳路径
CREATE p = (a)-[:KNOWS]->(b)-[:KNOWS]->(c)
RETURN p
```

### 1.4 动态标签和关系类型

```cypher
// 使用参数动态设置标签和关系类型
CREATE (greta:$($nodeLabels) {name: 'Greta Gerwig'})
WITH greta
UNWIND $movies AS movieTitle
CREATE (greta)-[rel:$($relType)]->(m:Movie {title: movieTitle})
RETURN greta.name AS name, labels(greta) AS labels, type(rel) AS relType, collect(m.title) AS movies
```

---

## 2. MERGE - 匹配或创建

### 2.1 基本 MERGE

```cypher
// 匹配或创建节点
MERGE (p:Person {name: "Alice"})
RETURN p

// 匹配或创建关系
MATCH (a:Person {name: "Alice"}), (b:Person {name: "Bob"})
MERGE (a)-[:FRIENDS_WITH]->(b)
RETURN a, b

// MERGE 完整路径（不推荐，可能导致意外结果）
MERGE (a:Person {name: "Alice"})-[:FRIENDS_WITH]->(b:Person {name: "Bob"})
RETURN a, b
```

### 2.2 ON CREATE - 仅在创建时执行

```cypher
// 创建时设置属性
MERGE (p:Person {name: "Keanu Reeves"})
ON CREATE SET p.created = timestamp()
RETURN p

// 创建时设置多个属性
MERGE (p:Person {name: "Alice"})
ON CREATE
  SET p.created = timestamp(),
      p.source = "import"
RETURN p
```

### 2.3 ON MATCH - 仅在匹配时执行

```cypher
// 匹配时更新属性
MERGE (p:Person {name: "Keanu Reeves"})
ON MATCH SET p.lastSeen = timestamp()
RETURN p

// 匹配时增加计数
MERGE (p:Person {name: "Alice"})
ON MATCH SET p.visitCount = p.visitCount + 1
RETURN p
```

### 2.4 ON CREATE 和 ON MATCH 组合

```cypher
// 同时处理创建和匹配情况
MERGE (keanu:Person {name: 'Keanu Reeves'})
ON CREATE
  SET keanu.created = timestamp()
ON MATCH
  SET keanu.lastSeen = timestamp()
RETURN keanu.name, keanu.created, keanu.lastSeen

// 复杂场景
MERGE (p:Person {email: "alice@example.com"})
ON CREATE
  SET p.name = "Alice",
      p.created = datetime()
ON MATCH
  SET p.lastLogin = datetime(),
      p.loginCount = p.loginCount + 1
RETURN p
```

---

## 3. SET - 设置属性和标签

### 3.1 设置属性

```cypher
// 设置单个属性
MATCH (p:Person {name: "Alice"})
SET p.age = 31
RETURN p

// 设置多个属性
MATCH (p:Person {name: "Alice"})
SET p.age = 31, p.city = "New York"
RETURN p

// 使用参数设置属性
MATCH (p:Person {name: "Alice"})
SET p += {age: 31, city: "New York"}
RETURN p

// 基于计算设置属性
MATCH (p:Person)
SET p.upperName = toUpper(p.name)
RETURN p

// 条件设置
MATCH (p:Person)
SET p.ageGroup = CASE
  WHEN p.age < 18 THEN "Minor"
  WHEN p.age < 65 THEN "Adult"
  ELSE "Senior"
END
RETURN p
```

### 3.2 设置标签

```cypher
// 添加单个标签
MATCH (p:Person {name: "Alice"})
SET p:VIP
RETURN p

// 添加多个标签
MATCH (p:Person {name: "Alice"})
SET p:VIP:Premium
RETURN p

// 使用参数动态添加标签
MATCH (p:Person {name: "Alice"})
SET p:$($newLabel)
RETURN labels(p)

// 替换所有标签（删除原有标签，只保留新标签）
MATCH (p:Person {name: "Alice"})
SET p:Premium
RETURN p
```

### 3.3 列表属性操作

```cypher
// 追加到列表
MATCH (p:Person {name: "Alice"})
SET p.tags = p.tags + ["newTag"]
RETURN p

// 从列表移除
MATCH (p:Person {name: "Alice"})
SET p.tags = p.tags - ["oldTag"]
RETURN p

// 替换列表
MATCH (p:Person {name: "Alice"})
SET p.tags = ["tag1", "tag2", "tag3"]
RETURN p
```

---

## 4. REMOVE - 移除属性和标签

### 4.1 移除属性

```cypher
// 移除单个属性
MATCH (p:Person {name: "Alice"})
REMOVE p.age
RETURN p

// 移除多个属性
MATCH (p:Person {name: "Alice"})
REMOVE p.age, p.city
RETURN p
```

### 4.2 移除标签

```cypher
// 移除单个标签
MATCH (p:Person:VIP {name: "Alice"})
REMOVE p:VIP
RETURN p

// 移除多个标签
MATCH (p:Person:VIP:Premium {name: "Alice"})
REMOVE p:VIP, p:Premium
RETURN p
```

---

## 5. DELETE - 删除数据

### 5.1 删除节点

```cypher
// 删除节点（必须先删除所有关系）
MATCH (n:Person {name: "Charlie"})
MATCH (n)-[r]-()
DELETE r
DELETE n

// 使用 DETACH DELETE 删除节点及其所有关系
MATCH (n:Person {name: "Charlie"})
DETACH DELETE n
```

### 5.2 删除关系

```cypher
// 删除特定关系
MATCH (a:Person {name: "Alice"})-[r:FRIENDS_WITH]->(b:Person {name: "Bob"})
DELETE r
RETURN a, b

// 删除所有某种类型的关系
MATCH ()-[r:FRIENDS_WITH]->()
DELETE r
```

### 5.3 删除路径

```cypher
// 删除路径
MATCH p = (n:Person {name: "Charlie"})-[*]->()
DELETE p
```

### 5.4 条件删除

```cypher
// 删除满足条件的节点
MATCH (n:Person)
WHERE n.age < 18
DETACH DELETE n

// 删除孤立节点
MATCH (n)
WHERE NOT (n)--()
DELETE n
```

---

## 6. FOREACH - 批量操作

```cypher
// 批量更新属性
MATCH (p:Person)
WHERE p.name IN ["Alice", "Bob", "Charlie"]
FOREACH (n IN collect(p) | SET n.updated = true)

// 批量设置标签
MATCH (p:Person)
WHERE p.age > 65
FOREACH (n IN collect(p) | SET n:Senior)

// 批量删除属性
MATCH (p:Person)
FOREACH (n IN collect(p) | REMOVE n.tempProperty)
```

---

## 7. LOAD CSV - 从 CSV 导入数据

### 7.1 基本导入

```cypher
// 从 CSV 文件导入（带表头）
LOAD CSV WITH HEADERS FROM 'file:///users.csv' AS row
CREATE (:User {id: row.UserID, name: row.UserName})

// 从 CSV 文件导入（无表头）
LOAD CSV FROM 'file:///users.csv' AS row
CREATE (:User {id: row[0], name: row[1]})

// 从远程 URL 导入
LOAD CSV FROM 'https://data.neo4j.com/bands/artists.csv' AS row
MERGE (:Artist {name: row[1], year: toInteger(row[2])})
```

### 7.2 动态列映射

```cypher
// 使用动态列名
LOAD CSV WITH HEADERS FROM 'file:///artists-with-headers.csv' AS line
CREATE (n:$(line.label) {name: line.Name})
```

### 7.3 复杂导入场景

```cypher
// 分割字符串为列表
LOAD CSV WITH HEADERS FROM 'https://data.neo4j.com/importing-cypher/movies.csv' AS row
MERGE (m:Movie {id: toInteger(row.movieId)})
SET
    m.title = row.title,
    m.imdbId = toInteger(row.movie_imdbId),
    m.languages = split(row.languages, '|'),
    m.genres = split(row.genres, '|')
RETURN
  m.title AS title,
  m.imdbId AS imdbId,
  m.languages AS languages,
  m.genres AS genres
LIMIT 5
```

### 7.4 完整数据集导入

```cypher
// 清空数据库
MATCH (n) DETACH DELETE n;

// 创建唯一约束
CREATE CONSTRAINT Person_tmdbId IF NOT EXISTS
FOR (p:Person) REQUIRE p.tmdbId IS UNIQUE;

CREATE CONSTRAINT Movie_movieId IF NOT EXISTS
FOR (m:Movie) REQUIRE m.movieId IS UNIQUE;

// 创建人员节点
LOAD CSV WITH HEADERS FROM 'https://data.neo4j.com/importing-cypher/persons.csv' AS row
MERGE (p:Person {tmdbId: toInteger(row.person_tmdbId)})
SET p.name = row.name, p.born = date(row.born);

// 创建电影节点
LOAD CSV WITH HEADERS FROM 'https://data.neo4j.com/importing-cypher/movies.csv' AS row
MERGE (m:Movie {id: toInteger(row.movieId)})
SET
    m.title = row.title,
    m.imdbId = toInteger(row.movie_imdbId),
    m.languages = split(row.languages, '|'),
    m.genres = split(row.genres, '|');

// 创建关系
LOAD CSV WITH HEADERS FROM 'https://data.neo4j.com/importing-cypher/acted_in.csv' AS row
MATCH (p:Person {tmdbId: toInteger(row.person_tmdbId)})
MATCH (m:Movie {id: toInteger(row.movieId)})
MERGE (p)-[r:ACTED_IN]->(m)
SET r.role = row.role;

// 设置额外标签
MATCH (p:Person)-[:ACTED_IN]->()
WITH DISTINCT p
SET p:Actor;
```

---

## 8. 批量操作最佳实践

### 8.1 使用 USINGLE 进行批量更新

```cypher
// 批量创建或更新
UNWIND [
  {name: "Alice", age: 30},
  {name: "Bob", age: 25},
  {name: "Charlie", age: 35}
] AS person
MERGE (p:Person {name: person.name})
SET p.age = person.age
RETURN p
```

### 8.2 分批处理大数据集

```cypher
// 使用 LIMIT 分批处理
MATCH (n:OldData)
WITH n LIMIT 1000
DETACH DELETE n
RETURN count(*)
```

---

## 9. 事务性操作

### 9.1 CALL 子查询中的事务

```cypher
// 在事务中执行子查询
CALL {
    MATCH (n:LargeDataset)
    DETACH DELETE n
} IN TRANSACTIONS OF 1000 ROWS

// 带并发和错误处理的事务
CALL {
    MATCH (n:Data)
    SET n.processed = true
} IN 4 CONCURRENT TRANSACTIONS
OF 500 ROWS
REPORT STATUS AS status
ON ERROR CONTINUE
```

---

## 10. 数据导入导出模式

### 10.1 APOC 导出（需要 APOC 插件）

```cypher
// 导出为 JSON
CALL apoc.export.json.all("export.json")

// 导出查询结果
CALL apoc.export.json.query(
  "MATCH (p:Person) RETURN p.name, p.age",
  "people.json"
)
```

### 10.2 数据备份模式

```cypher
// 创建数据快照
MATCH (n)
WITH collect(n) AS nodes
MATCH ()-[r]->()
WITH nodes, collect(r) AS rels
RETURN nodes, rels
```

---

## 参考文档

- [Neo4j Cypher Manual 25 - CREATE](https://neo4j.com/docs/cypher-manual/25/clauses/create/)
- [Neo4j Cypher Manual 25 - MERGE](https://neo4j.com/docs/cypher-manual/25/clauses/merge/)
- [Neo4j Cypher Manual 25 - SET](https://neo4j.com/docs/cypher-manual/25/clauses/set/)
- [Neo4j Cypher Manual 25 - DELETE](https://neo4j.com/docs/cypher-manual/25/clauses/delete/)
- [Neo4j Cypher Manual 25 - LOAD CSV](https://neo4j.com/docs/cypher-manual/25/clauses/load-csv/)
