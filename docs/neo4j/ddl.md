# Neo4j Cypher DDL - 数据定义语言

## 概述

DDL（Data Definition Language）用于定义和管理数据库结构，包括索引、约束、数据库等。

---

## 1. 索引管理

### 1.1 CREATE INDEX - 创建索引

#### 1.1.1 基本索引语法

```cypher
// 旧语法（仍支持）
CREATE INDEX ON :Person(name)

// 新语法（推荐）
CREATE INDEX FOR (n:Person) ON (n.name)

// 带索引名称
CREATE INDEX person_name_index FOR (n:Person) ON (n.name)
```

#### 1.1.2 B-Tree 索引（默认）

```cypher
// 创建 B-Tree 索引（默认类型）
CREATE INDEX FOR (n:Person) ON (n.name)

// 显式指定 B-Tree 选项
CREATE INDEX FOR (n:Person) ON (n.name)
OPTIONS {
  indexProvider: 'btree-1.0'
}

// 带配置的 B-Tree 索引
CREATE INDEX FOR (n:Person) ON (n.name)
OPTIONS "{ btree-option: btree-value }"
```

#### 1.1.3 TEXT 索引（文本索引）

```cypher
// 创建文本索引（用于全文搜索）
CREATE TEXT INDEX person_name_text_index FOR (n:Person) ON (n.name)

// 文本索引适用于字符串前缀和子串搜索
CREATE TEXT INDEX node_text_index_nickname FOR (n:Person) ON (n.nickname)
```

#### 1.1.4 POINT 索引（空间索引）

```cypher
// 创建点索引（用于空间数据）
CREATE POINT INDEX person_location_index FOR (n:Person) ON (n.location)

// 带空间配置的点索引
CREATE POINT INDEX `node_point_index_name` FOR (n:`Person`) ON (n.`sublocation`) 
OPTIONS {
  indexConfig: {
    `spatial.cartesian-3d.max`: [1000000.0, 1000000.0, 1000000.0],
    `spatial.cartesian-3d.min`: [-1000000.0, -1000000.0, -1000000.0],
    `spatial.cartesian.max`: [1000000.0, 1000000.0],
    `spatial.cartesian.min`: [-1000000.0, -1000000.0],
    `spatial.wgs-84-3d.max`: [180.0, 90.0, 1000000.0],
    `spatial.wgs-84-3d.min`: [-180.0, -90.0, -1000000.0],
    `spatial.wgs-84.max`: [180.0, 90.0],
    `spatial.wgs-84.min`: [-180.0, -90.0]
  }
}
```

#### 1.1.5 RANGE 索引（范围索引）

```cypher
// 创建范围索引（用于数值和日期范围查询）
CREATE RANGE INDEX FOR (n:Person) ON (n.age)

// 范围索引适用于数值、日期和时间类型
CREATE RANGE INDEX FOR (e:Event) ON (e.date)
CREATE RANGE INDEX FOR (p:Product) ON (p.price)
```

#### 1.1.6 复合索引

```cypher
// 创建复合索引（多属性索引）
CREATE INDEX FOR (n:Person) ON (n.lastName, n.firstName)

// 复合索引优化多条件查询
CREATE INDEX person_name_age_index FOR (n:Person) ON (n.name, n.age)
```

#### 1.1.7 关系属性索引

```cypher
// 为关系属性创建索引
CREATE INDEX FOR ()-[r:PURCHASED]-() ON (r.date)
CREATE INDEX relationship_date_index FOR ()-[r:LIKES]-() ON (r.since)
```

### 1.2 SHOW INDEXES - 显示索引

```cypher
// 显示所有索引
SHOW INDEXES

// 显示文本索引
SHOW TEXT INDEXES

// 显示点索引
SHOW POINT INDEXES

// 显示范围索引
SHOW RANGE INDEXES

// 显示 B-Tree 索引
SHOW BTREE INDEXES

// 显示特定名称的索引
SHOW INDEXES WHERE name = 'person_name_index'

// 显示特定标签的索引
SHOW INDEXES WHERE labelsOrTypes = ['Person']
```

### 1.3 DROP INDEX - 删除索引

```cypher
// 删除指定名称的索引
DROP INDEX person_name_index

// 如果存在则删除
DROP INDEX person_name_index IF EXISTS

// 删除所有文本索引
SHOW TEXT INDEXES
YIELD name
CALL {
  WITH name
  DETACH DELETE name
} IN TRANSACTIONS
```

### 1.4 索引使用示例

```cypher
// 创建索引后，查询会自动使用
CREATE INDEX FOR (n:Person) ON (n.name)

// 以下查询将使用索引
MATCH (p:Person {name: "Alice"})
RETURN p

// 范围查询使用 RANGE 索引
CREATE RANGE INDEX FOR (n:Person) ON (n.age)
MATCH (p:Person)
WHERE p.age > 18 AND p.age < 65
RETURN p

// 文本搜索使用 TEXT 索引
CREATE TEXT INDEX FOR (n:Person) ON (n.name)
MATCH (p:Person)
WHERE p.name CONTAINS 'ali'
RETURN p
```

---

## 2. 约束管理

### 2.1 CREATE CONSTRAINT - 创建约束

#### 2.1.1 唯一约束（旧语法）

```cypher
// 旧语法（仍支持）
CREATE CONSTRAINT ON :User(email) ASSERT IS UNIQUE

// 旧语法 - 属性存在且唯一
CREATE CONSTRAINT ON :Person(ssn) ASSERT EXISTS
```

#### 2.1.2 唯一约束（新语法）

```cypher
// 新语法（推荐）
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.email IS UNIQUE

// 带约束名称
CREATE CONSTRAINT person_email_unique FOR (p:Person) REQUIRE p.email IS UNIQUE

// 多属性唯一约束
CREATE CONSTRAINT person_name_unique FOR (p:Person) REQUIRE (p.firstName, p.lastName) IS UNIQUE
```

#### 2.1.3 NOT NULL 约束

```cypher
// 创建非空约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.name IS NOT NULL

// 带约束名称
CREATE CONSTRAINT person_name_not_null FOR (p:Person) REQUIRE p.name IS NOT NULL

// 多属性非空约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE (p.firstName, p.lastName) IS NOT NULL
```

#### 2.1.4 存在约束

```cypher
// 创建属性存在约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.email IS NOT NULL

// 关系类型存在约束
CREATE CONSTRAINT FOR ()-[r:PURCHASED]-() REQUIRE r.date IS NOT NULL
```

#### 2.1.5 节点键约束

```cypher
// 节点键约束（组合唯一 + 非空）
CREATE CONSTRAINT FOR (p:Person) REQUIRE (p.country, p.vat) IS NODE KEY
```

#### 2.1.6 关系唯一约束

```cypher
// 关系唯一性约束
CREATE CONSTRAINT FOR ()-[r:LIKES]-() REQUIRE r.id IS UNIQUE
```

#### 2.1.7 带选项的约束

```cypher
// 带配置的约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.email IS UNIQUE
OPTIONS "{ btree-option: btree-value }"
```

### 2.2 SHOW CONSTRAINTS - 显示约束

```cypher
// 显示所有约束
SHOW CONSTRAINTS

// 显示特定类型的约束
SHOW UNIQUE CONSTRAINTS
SHOW NOT NULL CONSTRAINTS
SHOW NODE KEY CONSTRAINTS

// 显示特定标签的约束
SHOW CONSTRAINTS WHERE labelsOrTypes = ['Person']

// 显示特定名称的约束
SHOW CONSTRAINTS WHERE name = 'person_email_unique'

// 显示特定属性的约束
SHOW CONSTRAINTS WHERE properties = ['email']
```

### 2.3 DROP CONSTRAINT - 删除约束

```cypher
// 删除指定名称的约束
DROP CONSTRAINT person_email_unique

// 旧语法删除约束
DROP CONSTRAINT ON :User(email)

// 如果存在则删除
DROP CONSTRAINT person_email_unique IF EXISTS
```

---

## 3. 数据库管理

### 3.1 CREATE DATABASE - 创建数据库

```cypher
// 创建数据库
CREATE DATABASE myDatabase

// 如果不存在则创建
CREATE DATABASE myDatabase IF NOT EXISTS

// 创建数据库并指定选项
CREATE DATABASE myDatabase
OPTIONS {
  existingData: 'use',
  existingDataPath: '/path/to/data'
}

// 创建只读数据库
CREATE DATABASE myReadOnlyDatabase
OPTIONS {
  read_only: true
}
```

### 3.2 SHOW DATABASES - 显示数据库

```cypher
// 显示所有数据库
SHOW DATABASES

// 显示特定数据库
SHOW DATABASE myDatabase

// 显示当前数据库
SHOW CURRENT DATABASE

// 显示数据库详情
SHOW DATABASE myDatabase VERBOSE
```

### 3.3 DROP DATABASE - 删除数据库

```cypher
// 删除数据库
DROP DATABASE myDatabase

// 如果存在则删除
DROP DATABASE myDatabase IF EXISTS
```

### 3.4 数据库操作

```cypher
// 启动数据库
START DATABASE myDatabase

// 停止数据库
STOP DATABASE myDatabase

// 切换到数据库
USE myDatabase
```

---

## 4. 别名管理

### 4.1 CREATE ALIAS - 创建别名

```cypher
// 创建数据库别名
CREATE ALIAS myAlias FOR DATABASE myDatabase

// 创建外部数据库别名
CREATE ALIAS externalDb FOR DATABASE AT 'neo4j://remote-server:7687/myDatabase'
```

### 4.2 SHOW ALIASES - 显示别名

```cypher
// 显示所有别名
SHOW ALIASES FOR DATABASE

// 显示特定别名
SHOW ALIAS myAlias FOR DATABASE
```

### 4.3 DROP ALIAS - 删除别名

```cypher
// 删除别名
DROP ALIAS myAlias FOR DATABASE

// 如果存在则删除
DROP ALIAS myAlias FOR DATABASE IF EXISTS
```

---

## 5. 存储过程管理

### 5.1 SHOW PROCEDURES - 显示存储过程

```cypher
// 显示所有存储过程
SHOW PROCEDURES

// 显示特定存储过程
SHOW PROCEDURES YIELD name, signature, description
WHERE name CONTAINS 'apoc'
RETURN *
```

### 5.2 SHOW FUNCTIONS - 显示函数

```cypher
// 显示所有函数
SHOW FUNCTIONS

// 显示内置函数
SHOW BUILT IN FUNCTIONS

// 显示用户定义函数
SHOW USER FUNCTIONS

// 显示特定函数
SHOW FUNCTIONS YIELD name, signature, description
WHERE name CONTAINS 'apoc'
RETURN *
```

---

## 6. 事务管理

### 6.1 事务控制

```cypher
// 开始事务（在驱动程序中通常自动处理）
:begin

// 提交事务
:commit

// 回滚事务
:rollback
```

### 6.2 事务配置

```cypher
// 设置事务超时
:CLEAR
:config {autoCommit: false}

// 在子查询中管理事务
CALL {
    MATCH (n:LargeDataset)
    SET n.processed = true
} IN TRANSACTIONS OF 1000 ROWS
```

---

## 7. 模式管理

### 7.1 SHOW SCHEMA - 显示模式

```cypher
// 显示数据库模式
SHOW SCHEMA

// 显示标签
SHOW LABELS

// 显示关系类型
SHOW RELATIONSHIP TYPES

// 显示属性键
SHOW PROPERTY KEYS
```

### 7.2 模式分析

```cypher
// 分析节点标签分布
CALL db.labels()

// 分析关系类型分布
CALL db.relationshipTypes()

// 分析属性键
CALL db.propertyKeys()

// 获取数据库统计信息
CALL db.stats()
```

---

## 8. DDL 最佳实践

### 8.1 索引创建策略

```cypher
// 1. 先分析查询模式
EXPLAIN MATCH (p:Person {name: "Alice"}) RETURN p

// 2. 为频繁查询的属性创建索引
CREATE INDEX FOR (n:Person) ON (n.name)
CREATE INDEX FOR (n:Person) ON (n.email)

// 3. 为范围查询创建 RANGE 索引
CREATE RANGE INDEX FOR (n:Person) ON (n.age)

// 4. 为文本搜索创建 TEXT 索引
CREATE TEXT INDEX FOR (n:Person) ON (n.description)

// 5. 为空间查询创建 POINT 索引
CREATE POINT INDEX FOR (n:Location) ON (n.coordinates)
```

### 8.2 约束创建策略

```cypher
// 1. 为唯一标识符创建唯一约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.id IS UNIQUE
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.email IS UNIQUE

// 2. 为必填字段创建 NOT NULL 约束
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.name IS NOT NULL

// 3. 在导入数据前创建约束以确保数据质量
CREATE CONSTRAINT person_id_unique IF NOT EXISTS
FOR (p:Person) REQUIRE p.id IS UNIQUE
```

### 8.3 批量 DDL 操作

```cypher
// 使用条件创建避免错误
CREATE INDEX person_name_index IF NOT EXISTS FOR (n:Person) ON (n.name)
CREATE CONSTRAINT person_email_unique IF NOT EXISTS FOR (p:Person) REQUIRE p.email IS UNIQUE
```

---

## 9. 性能优化

### 9.1 索引选择器提示

```cypher
// 使用索引提示
MATCH (p:Person {name: "Alice"})
USING INDEX p:Person(name)
RETURN p

// 使用扫描提示
MATCH (p:Person)
USING SCAN p:Person
WHERE p.name STARTS WITH 'A'
RETURN p
```

### 9.2 索引统计信息

```cypher
// 查看索引使用情况
SHOW INDEXES YIELD name, type, state, populationPercent
RETURN *

// 查看查询计划
EXPLAIN MATCH (p:Person {name: "Alice"}) RETURN p
PROFILE MATCH (p:Person {name: "Alice"}) RETURN p
```

---

## 参考文档

- [Neo4j Cypher Manual 25 - Indexes](https://neo4j.com/docs/cypher-manual/25/indexes/)
- [Neo4j Cypher Manual 25 - Constraints](https://neo4j.com/docs/cypher-manual/25/constraints/)
- [Neo4j Cypher Manual 25 - CREATE INDEX](https://neo4j.com/docs/cypher-manual/25/clauses/create-index/)
- [Neo4j Cypher Manual 25 - CREATE CONSTRAINT](https://neo4j.com/docs/cypher-manual/25/clauses/create-constraint/)
- [Neo4j Operations Manual - Database Management](https://neo4j.com/docs/operations-manual/current/)
