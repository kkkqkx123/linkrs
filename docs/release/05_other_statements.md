# GraphDB 其他语句

## 概述

本文档包含不属于DQL、DML、DDL、DCL分类的其他实用语句，包括查询分析、会话管理、配置管理、变量赋值和集合操作等高级功能。

---

## 1. USE - 切换图空间

### 功能
切换到指定的图空间。

### 语法结构
```cypher
USE <space_name>
```

### 示例
```cypher
USE test_space
USE social_network
```

---

## 2. EXPLAIN - 查询计划

### 功能
显示查询的执行计划，用于性能分析和优化。

### 语法结构
```cypher
EXPLAIN [FORMAT = {TABLE | DOT}] <statement>
```

### 关键特性
- 显示查询执行计划
- 显示预计成本
- 显示使用的索引
- 不实际执行查询
- 支持 TABLE 和 DOT 两种输出格式

### 示例
```cypher
EXPLAIN MATCH (p:Person {name: 'Alice'}) RETURN p
EXPLAIN FORMAT = TABLE MATCH (p:Person) RETURN p
EXPLAIN FORMAT = DOT GO 2 STEPS FROM "101" OVER follow
```

---

## 3. PROFILE - 性能分析

### 功能
执行查询并收集实际的性能数据，用于深度性能分析。

### 语法结构
```cypher
PROFILE [FORMAT = {TABLE | DOT}] <statement>
```

### 关键特性
- 实际执行查询
- 收集执行时间和资源消耗
- 显示实际行数和成本
- 支持 TABLE 和 DOT 两种输出格式

### 示例
```cypher
PROFILE MATCH (p:Person)-[:FRIEND]->(f) RETURN count(f)
PROFILE FORMAT = DOT GO 3 STEPS FROM "player100" OVER follow
```

---

## 4. GROUP BY - 分组聚合

### 功能
对查询结果进行分组和聚合计算。

### 语法结构
```cypher
GROUP BY <expression> [, <expression> ...]
[YIELD <item> [, <item> ...]]
[HAVING <condition>]
```

### 关键特性
- 支持多字段分组
- 支持 YIELD 子句指定输出
- 支持 HAVING 子句过滤分组
- 可与管道操作符结合使用

### 示例
```cypher
-- 基础分组
GROUP BY category YIELD category, count(*) AS cnt

-- 带 HAVING 过滤
GROUP BY age YIELD age, count(*) AS cnt HAVING cnt > 5

-- 与管道结合
GO FROM "player100" OVER follow | GROUP BY $-.dst YIELD $-.dst, count(*) AS cnt
```

---

## 5. 管道操作符

### 功能
使用管道符 `|` 连接多个语句，将前一个语句的结果传递给后一个语句。

### 语法结构
```cypher
<statement1> | <statement2> | <statement3>
```

### 示例
```cypher
GO FROM "101" OVER follow | YIELD $^.follow._dst AS dst | GO FROM dst OVER like
```

---

## 6. 会话管理语句

### 6.1 SHOW SESSIONS - 显示会话列表

#### 功能
显示当前所有的会话信息。

#### 语法结构
```cypher
SHOW SESSIONS
```

#### 示例
```cypher
SHOW SESSIONS
```

### 6.2 SHOW QUERIES - 显示查询列表

#### 功能
显示当前正在执行的查询。

#### 语法结构
```cypher
SHOW QUERIES
```

#### 示例
```cypher
SHOW QUERIES
```

### 6.3 KILL QUERY - 终止查询

#### 功能
终止指定的查询。

#### 语法结构
```cypher
KILL QUERY <session_id>, <plan_id>
```

#### 示例
```cypher
KILL QUERY 123, 456
```

---

## 7. 配置管理语句

### 7.1 SHOW CONFIGS - 显示配置

#### 功能
显示系统配置信息。

#### 语法结构
```cypher
SHOW CONFIGS [<module>]
```

#### 参数说明
- `module`: 可选，指定模块名（GRAPH、STORAGE、META）

#### 示例
```cypher
SHOW CONFIGS
SHOW CONFIGS GRAPH
SHOW CONFIGS STORAGE
```

### 7.2 UPDATE CONFIGS - 更新配置

#### 功能
更新系统配置值。

#### 语法结构
```cypher
UPDATE CONFIGS [<module>] <config_name> = <value>
```

#### 参数说明
- `module`: 可选，指定模块名（GRAPH、STORAGE、META）
- `config_name`: 配置项名称
- `value`: 配置值（支持表达式）

#### 示例
```cypher
UPDATE CONFIGS max_connections = 1000
UPDATE CONFIGS STORAGE cache_size = 1024
UPDATE CONFIGS wal_ttl = 86400
```

---

## 8. 变量赋值语句

### 功能
将查询结果赋值给变量，供后续使用。

### 语法结构
```cypher
$<variable_name> = <statement>
```

### 关键特性
- 变量名以 `$` 开头
- 可存储查询结果
- 可在管道中传递

### 示例
```cypher
$result = GO FROM "player100" OVER follow
$result = MATCH (n:Person) RETURN n
```

---

## 9. 集合操作语句

### 功能
对两个查询结果集进行集合操作。

### 语法结构
```cypher
<statement1> UNION [ALL] <statement2>
<statement1> INTERSECT <statement2>
<statement1> MINUS <statement2>
```

### 操作类型

| 操作符 | 说明 |
|--------|------|
| UNION | 并集，去重 |
| UNION ALL | 并集，保留重复 |
| INTERSECT | 交集 |
| MINUS | 差集 |

### 示例
```cypher
-- UNION
GO FROM "player100" OVER follow UNION GO FROM "player101" OVER follow

-- UNION ALL
GO FROM "player100" OVER follow UNION ALL GO FROM "player101" OVER follow

-- INTERSECT
GO FROM "player100" OVER follow INTERSECT GO FROM "player101" OVER follow

-- MINUS
GO FROM "player100" OVER follow MINUS GO FROM "player101" OVER follow
```

---

## 10. 表达式支持

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

---

## 11. 查询语句完整示例

### 11.1 复杂MATCH查询
```cypher
MATCH (p:Person {name: 'Alice'})-[:FRIEND*1..3]->(f:Person)
WHERE f.age > 25 AND f.city = 'Beijing'
RETURN f.name, f.age, count(*) AS friend_count
ORDER BY friend_count DESC
LIMIT 10
```

### 11.2 多语句管道
```cypher
GO 2 STEPS FROM "player100" OVER follow 
WHERE follow.degree > 0.5 
| YIELD follow._dst AS friend_id, follow.degree AS degree
| GO FROM friend_id OVER serve 
WHERE serve.start_year > 2020
| YIELD $^.serve._dst AS team_id, degree
```

### 11.3 使用UNWIND展开列表
```cypher
UNWIND [1, 2, 3] AS n
RETURN n * 2 AS doubled
```

### 11.4 使用WITH传递中间结果
```cypher
MATCH (p:Person)-[:FRIEND]->(f)
WITH p, count(f) AS friend_count
WHERE friend_count > 5
RETURN p.name, friend_count
ORDER BY friend_count DESC
```

### 11.5 使用EXPLAIN分析查询
```cypher
EXPLAIN FORMAT = DOT
MATCH (p:Person {name: 'Alice'})-[:FRIEND]->(f:Person)
WHERE f.age > 25
RETURN f.name, f.age
```

### 11.6 使用GROUP BY分组
```cypher
GO FROM "player100" OVER follow
| YIELD $-.dst AS friend_id
| GO FROM friend_id OVER serve
| GROUP BY $-.dst YIELD $-.dst AS team_id, count(*) AS serve_count
HAVING serve_count > 1
```

### 11.7 使用集合操作
```cypher
-- 查找共同好友
GO FROM "player100" OVER follow
INTERSECT
GO FROM "player101" OVER follow
```

### 11.8 使用变量赋值
```cypher
$friends = GO FROM "player100" OVER follow
GO FROM $friends.dst OVER serve
```
