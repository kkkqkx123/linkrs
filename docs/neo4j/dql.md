# Neo4j Cypher DQL - 数据查询语言

## 概述

Cypher 是 Neo4j 的声明式查询语言，专为属性图数据库设计。DQL（Data Query Language）用于查询和检索图中的数据。

---

## 1. MATCH 子句

### 1.1 基本模式匹配

```cypher
// 匹配具有特定标签的节点
MATCH (n:Person)
RETURN n

// 匹配具有特定属性的节点
MATCH (p:Person {name: "Alice"})
RETURN p

// 匹配节点之间的关系
MATCH (a:Person)-[r:FRIENDS_WITH]->(b:Person)
RETURN a, r, b

// 匹配双向关系
MATCH (a:Person)-[:FRIENDS_WITH]-(b:Person)
RETURN a, b
```

### 1.2 路径匹配

```cypher
// 匹配固定长度的路径
MATCH (a:Person)-[:FRIENDS_WITH*2]->(b:Person)
RETURN a, b

// 匹配可变长度的路径
MATCH (a:Person)-[:FRIENDS_WITH*1..5]->(b:Person)
RETURN a, b

// 匹配任意长度的路径
MATCH (a:Person)-[:FRIENDS_WITH*]->(b:Person)
RETURN a, b
```

### 1.3 可选匹配 OPTIONAL MATCH

```cypher
// 可选匹配，如果找不到则返回 null
MATCH (p:Person {name: "Alice"})
OPTIONAL MATCH (p)-[:FRIENDS_WITH]->(friend)
RETURN p.name, friend.name

// 多个可选匹配
MATCH (p:Person {name: "Alice"})
OPTIONAL MATCH (p)-[:FRIENDS_WITH]->(friend)
OPTIONAL MATCH (p)-[:WORKS_WITH]->(colleague)
RETURN p.name, friend.name, colleague.name
```

---

## 2. WHERE 子句

### 2.1 基本过滤条件

```cypher
// 等于
MATCH (p:Person)
WHERE p.name = "Alice"
RETURN p

// 不等于
MATCH (p:Person)
WHERE p.name <> "Alice"
RETURN p

// 比较运算符
MATCH (p:Person)
WHERE p.age >= 18
RETURN p

// 逻辑运算符
MATCH (p:Person)
WHERE p.age >= 18 AND p.name STARTS WITH 'A'
RETURN p.name

MATCH (p:Person)
WHERE p.age >= 18 OR p.age < 10
RETURN p

MATCH (p:Person)
WHERE NOT p.name = "Alice"
RETURN p
```

### 2.2 字符串匹配

```cypher
// 以指定字符串开头
MATCH (p:Person)
WHERE p.name STARTS WITH 'A'
RETURN p

// 以指定字符串结尾
MATCH (p:Person)
WHERE p.name ENDS WITH 'e'
RETURN p

// 包含指定字符串
MATCH (p:Person)
WHERE p.name CONTAINS 'li'
RETURN p

// 正则表达式
MATCH (p:Person)
WHERE p.name =~ 'A.*'
RETURN p
```

### 2.3 范围查询

```cypher
// IN 列表
MATCH (p:Person)
WHERE p.name IN ["Alice", "Bob", "Charlie"]
RETURN p

// BETWEEN
MATCH (p:Person)
WHERE p.age BETWEEN 18 AND 65
RETURN p

// 空值检查
MATCH (p:Person)
WHERE p.email IS NULL
RETURN p

MATCH (p:Person)
WHERE p.email IS NOT NULL
RETURN p
```

---

## 3. RETURN 子句

### 3.1 基本返回

```cypher
// 返回节点
MATCH (n:Person)
RETURN n

// 返回特定属性
MATCH (n:Person)
RETURN n.name, n.age

// 返回关系
MATCH (a:Person)-[r:FRIENDS_WITH]->(b:Person)
RETURN a, r, b

// 返回路径
MATCH p = (a:Person)-[:FRIENDS_WITH]->(b:Person)
RETURN p
```

### 3.2 别名

```cypher
MATCH (p:Person)
RETURN p.name AS personName, p.age AS personAge
```

### 3.3 返回所有

```cypher
MATCH (p:Person)
RETURN *
```

### 3.4 排序 ORDER BY

```cypher
// 升序排序
MATCH (n:Person)
RETURN n.name, n.age
ORDER BY n.age ASC

// 降序排序
MATCH (n:Person)
RETURN n.name, n.age
ORDER BY n.age DESC

// 多字段排序
MATCH (n:Person)
RETURN n.name, n.age, n.height
ORDER BY n.age DESC, n.height ASC
```

### 3.5 分页 SKIP 和 LIMIT

```cypher
// 跳过前 2 条，返回 2 条
MATCH (n)
ORDER BY n.name
SKIP 2
LIMIT 2
RETURN collect(n.name) AS names

// 使用 OFFSET（GQL 标准语法）
MATCH (n)
ORDER BY n.name DESC
OFFSET 3
LIMIT 2
RETURN collect(n.name) AS names
```

---

## 4. WITH 子句

### 4.1 链式查询

```cypher
// WITH 用于连接多个查询阶段
MATCH (p:Person)
WHERE p.age > 18
WITH p
MATCH (p)-[:FRIENDS_WITH]->(friend)
RETURN p.name, friend.name

// WITH 进行中间聚合
MATCH (p:Person)-[:FRIENDS_WITH]->(friend)
WITH p, count(friend) AS friendCount
WHERE friendCount > 5
RETURN p.name, friendCount
```

### 4.2 聚合函数

```cypher
// COUNT - 计数
MATCH (p:Person)
RETURN count(p) AS totalPersons

MATCH (p:Person)-[:FRIENDS_WITH]->()
RETURN p.name, count() AS friendCount

// SUM - 求和
MATCH (p:Person)
RETURN sum(p.age) AS totalAge

// AVG - 平均值
MATCH (p:Person)
RETURN avg(p.age) AS averageAge

// MIN - 最小值
MATCH (p:Person)
RETURN min(p.age) AS youngestAge

// MAX - 最大值
MATCH (p:Person)
RETURN max(p.age) AS oldestAge

// COLLECT - 收集为列表
MATCH (p:Person)
RETURN collect(p.name) AS names

// 分组聚合
MATCH (p:Person)-[:FRIENDS_WITH]->(friend)
RETURN p.name, collect(friend.name) AS friends
```

---

## 5. UNION 和 UNION ALL

### 5.1 UNION（去重合并）

```cypher
// 合并两个查询结果，自动去重
MATCH (p:Person)
RETURN p.name AS name
UNION
MATCH (m:Movie {title: "The Matrix"})
<-[:ACTED_IN]-(a:Person)
RETURN a.name AS name
```

### 5.2 UNION ALL（保留重复）

```cypher
// 合并两个查询结果，保留重复项
MATCH (p:Person)
RETURN p.name AS name
UNION ALL
MATCH (m:Movie {title: "The Matrix"})
<-[:ACTED_IN]-(a:Person)
RETURN a.name AS name
```

### 5.3 不同顺序的 UNION

```cypher
RETURN 'val' as one, 'val' as two
UNION
RETURN 'val' as two, 'val' as one

RETURN 'val' as one, 'val' as two
UNION ALL
RETURN 'val' as two, 'val' as one
```

---

## 6. 路径查询函数

### 6.1 shortestPath - 最短路径

```cypher
// 查找两个节点之间的最短路径
MATCH (p1:Person {name: "Alice"}), (p2:Person {name: "Bob"})
MATCH p = shortestPath((p1)-[:FRIENDS_WITH*]-(p2))
RETURN p
```

### 6.2 allShortestPaths - 所有最短路径

```cypher
// 查找两个节点之间的所有最短路径
MATCH (p1:Person {name: "Alice"}), (p2:Person {name: "Bob"})
MATCH p = allShortestPaths((p1)-[:FRIENDS_WITH*]-(p2))
RETURN p
```

### 6.3 GQL 标准路径选择器

```cypher
// ALL SHORTEST - 所有最短路径
MATCH (n:Person {name: 'Alice'}), (m:Person {name: 'Bob'})
MATCH p = ALL SHORTEST (n)-[:FRIENDS_WITH*]-(m)
RETURN p

// ANY SHORTEST - 任意一条最短路径
MATCH (n:Person {name: 'Alice'}), (m:Person {name: 'Bob'})
MATCH p = ANY SHORTEST (n)-[:FRIENDS_WITH*]-(m)
RETURN p

// SHORTEST k - 前 k 条最短路径
MATCH (n:Person {name: 'Alice'}), (m:Person {name: 'Bob'})
MATCH p = SHORTEST 3 (n)-[:FRIENDS_WITH*]-(m)
RETURN p

// SHORTEST k GROUPS - 按组的最短路径
MATCH (n:Person {name: 'Alice'}), (m:Person {name: 'Bob'})
MATCH p = SHORTEST 2 GROUPS (n)-[:FRIENDS_WITH*]-(m)
RETURN p
```

---

## 7. 模式推导（Pattern Comprehension）

```cypher
// 从模式推导列表
MATCH (p:Person {name: "Alice"})
RETURN [ (p)-[:FRIENDS_WITH]->(f) | f.name ] AS friendNames

// 带条件的模式推导
MATCH (p:Person {name: "Alice"})
RETURN [ (p)-[:FRIENDS_WITH]->(f) WHERE f.age > 18 | f.name ] AS adultFriends
```

---

## 8. 列表操作

```cypher
// 列表长度
MATCH (p:Person)
RETURN size(p.tags) AS tagCount

// 列表切片
MATCH (p:Person)
RETURN p.names[0..3] AS firstThreeNames

// 列表推导
MATCH (p:Person)
RETURN [n IN p.names | toUpper(n)] AS upperNames

// 过滤列表
MATCH (p:Person)
RETURN [n IN p.names WHERE n STARTS WITH 'A'] AS aNames

// 列表包含检查
MATCH (p:Person)
WHERE "Alice" IN p.names
RETURN p
```

---

## 9. 常用函数

### 9.1 字符串函数

```cypher
// 转大写
MATCH (p:Person)
RETURN toUpper(p.name)

// 转小写
MATCH (p:Person)
RETURN toLower(p.name)

// 字符串长度
MATCH (p:Person)
RETURN size(p.name)

// 子字符串
MATCH (p:Person)
RETURN substring(p.name, 0, 3)

// 连接字符串
MATCH (p:Person)
RETURN p.firstName + " " + p.lastName AS fullName

// 修剪空格
MATCH (p:Person)
RETURN trim(p.name)

// 左修剪
MATCH (p:Person)
RETURN ltrim(p.name)

// 右修剪
MATCH (p:Person)
RETURN rtrim(p.name)

// 分割字符串
MATCH (p:Person)
RETURN split(p.tags, ",") AS tagList
```

### 9.2 数学函数

```cypher
// 四舍五入
MATCH (p:Person)
RETURN round(p.score)

// 向上取整
MATCH (p:Person)
RETURN ceil(p.score)

// 向下取整
MATCH (p:Person)
RETURN floor(p.score)

// 绝对值
MATCH (p:Person)
RETURN abs(p.score)

// 随机数
RETURN rand()

// 平方根
MATCH (p:Person)
RETURN sqrt(p.area)
```

### 9.3 时间函数

```cypher
// 当前时间戳
RETURN timestamp()

// 当前日期
RETURN date()

// 当前时间
RETURN time()

// 当前日期时间
RETURN datetime()

// 日期属性
MATCH (e:Event)
RETURN e.date.year, e.date.month, e.date.day

// 日期计算
RETURN date() + duration({days: 7}) AS nextWeek
```

### 9.4 类型转换函数

```cypher
// 转整数
RETURN toInteger("42")

// 转浮点数
RETURN toFloat("3.14")

// 转字符串
RETURN toString(42)

// 转布尔值
RETURN toBoolean("true")
```

### 9.5 节点和关系函数

```cypher
// 获取节点标签
MATCH (n)
RETURN labels(n)

// 获取关系类型
MATCH ()-[r]->()
RETURN type(r)

// 获取节点 ID
MATCH (n)
RETURN id(n)

// 获取关系 ID
MATCH ()-[r]->()
RETURN id(r)

// 获取所有属性
MATCH (n)
RETURN properties(n)

// 获取属性键
MATCH (n)
RETURN keys(n)
```

---

## 10. 存在性子句

```cypher
// EXISTS 检查属性是否存在
MATCH (p:Person)
WHERE EXISTS(p.email)
RETURN p

// EXISTS 检查模式是否存在
MATCH (p:Person)
WHERE EXISTS((p)-[:FRIENDS_WITH]->())
RETURN p.name
```

---

## 11. CASE 表达式

```cypher
// 简单 CASE
MATCH (p:Person)
RETURN p.name,
       CASE p.age
         WHEN < 18 THEN "Minor"
         WHEN >= 18 AND < 65 THEN "Adult"
         ELSE "Senior"
       END AS ageGroup

// 通用 CASE
MATCH (p:Person)
RETURN p.name,
       CASE
         WHEN p.age < 18 THEN "Minor"
         WHEN p.age >= 18 AND p.age < 65 THEN "Adult"
         ELSE "Senior"
       END AS ageGroup
```

---

## 参考文档

- [Neo4j Cypher Manual 25](https://neo4j.com/docs/cypher-manual/25/)
- [MATCH 子句](https://neo4j.com/docs/cypher-manual/25/clauses/match/)
- [WHERE 子句](https://neo4j.com/docs/cypher-manual/25/clauses/where/)
- [RETURN 子句](https://neo4j.com/docs/cypher-manual/25/clauses/return/)
- [WITH 子句](https://neo4j.com/docs/cypher-manual/25/clauses/with/)
- [UNION 子句](https://neo4j.com/docs/cypher-manual/25/clauses/union/)
