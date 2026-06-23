# GraphDB CLI E2E 测试设计方案

## 概述

本文档描述使用 graphdb-cli 进行端到端 (E2E) 测试的完整设计方案，包括测试命令范围、测试数据设计和执行流程。

## 测试目标

1. 验证 graphdb-cli 与 GraphDB 服务器的完整交互链路
2. 验证 EXPLAIN/PROFILE 等分析命令的正确性
3. 验证各类查询语句的执行结果
4. 验证元数据管理功能
5. 验证事务管理功能

## 测试命令范围

### 1. 连接与会话管理命令

| 命令                  | 说明             | 测试优先级 |
| --------------------- | ---------------- | ---------- |
| `\connect <space>`    | 连接到指定图空间 | P0         |
| `\disconnect`         | 断开当前会话     | P0         |
| `\conninfo`           | 显示连接信息     | P1         |
| `\show_spaces` / `\l` | 列出所有图空间   | P0         |

### 2. Schema 管理命令

| 命令                    | 说明           | 测试优先级 |
| ----------------------- | -------------- | ---------- |
| `\show_tags` / `\dt`    | 列出所有标签   | P0         |
| `\show_edges` / `\de`   | 列出所有边类型 | P0         |
| `\show_indexes` / `\di` | 列出所有索引   | P1         |
| `\describe <tag>`       | 描述标签结构   | P1         |
| `\describe_edge <edge>` | 描述边类型结构 | P1         |

### 3. 查询分析命令

| 命令                          | 说明               | 测试优先级 |
| ----------------------------- | ------------------ | ---------- |
| `\explain <query>`            | 显示查询执行计划   | P0         |
| `\explain analyze <query>`    | 显示并执行查询计划 | P0         |
| `\explain format=dot <query>` | DOT 格式输出计划   | P1         |
| `\profile <query>`            | 执行并分析查询性能 | P0         |

### 4. 事务管理命令

| 命令        | 说明         | 测试优先级 |
| ----------- | ------------ | ---------- |
| `\begin`    | 开始事务     | P0         |
| `\commit`   | 提交事务     | P0         |
| `\rollback` | 回滚事务     | P0         |
| `\txstatus` | 查看事务状态 | P1         |

### 5. 输出格式命令

| 命令            | 说明             | 测试优先级 |
| --------------- | ---------------- | ---------- |
| `\format <fmt>` | 设置输出格式     | P1         |
| `\timing`       | 切换执行时间显示 | P1         |
| `\x`            | 切换垂直显示模式 | P1         |

### 6. 变量与脚本命令

| 命令                  | 说明         | 测试优先级 |
| --------------------- | ------------ | ---------- |
| `\set <name> <value>` | 设置变量     | P1         |
| `\unset <name>`       | 删除变量     | P1         |
| `\i <file>`           | 执行脚本文件 | P1         |

## GQL 查询语句测试范围

### 1. DDL 语句

```cypher
-- 图空间管理
CREATE SPACE test_space (vid_type=STRING)
USE test_space
DROP SPACE test_space

-- 标签管理
CREATE TAG person(name: STRING, age: INT, email: STRING)
CREATE TAG company(name: STRING, founded: INT)
ALTER TAG person ADD (address: STRING)
DROP TAG person

-- 边类型管理
CREATE EDGE friend(degree: FLOAT, since: TIMESTAMP)
CREATE EDGE works_at(position: STRING, since: DATE)
DROP EDGE friend

-- 索引管理
CREATE TAG INDEX idx_person_name ON person(name)
CREATE EDGE INDEX idx_friend_since ON friend(since)
DROP INDEX idx_person_name
```

### 2. DML 语句

```cypher
-- 插入顶点
INSERT VERTEX person(name, age, email) VALUES "p1": ("Alice", 30, "alice@example.com")
INSERT VERTEX person(name, age) VALUES "p2": ("Bob", 25), "p3": ("Charlie", 35)
INSERT VERTEX company(name, founded) VALUES "c1": ("TechCorp", 2010)

-- 插入边
INSERT EDGE friend(degree, since) VALUES "p1" -> "p2" @0: (0.8, "2020-01-01")
INSERT EDGE works_at(position, since) VALUES "p1" -> "c1" @0: ("Engineer", "2020-06-01")

-- Cypher 风格创建
CREATE (n:Person {name: 'David', age: 28})
CREATE (a:Person {name: 'Alice'})-[:KNOWS {since: '2020-01-01'}]->(b:Person {name: 'Bob'})

-- 更新数据
UPDATE VERTEX "p1" SET person.age = 31
UPDATE EDGE "p1" -> "p2" @0 OF friend SET degree = 0.9

-- 删除数据
DELETE VERTEX "p3"
DELETE EDGE "p1" -> "p2" @0 OF friend
```

### 3. DQL 查询语句

```cypher
-- MATCH 模式匹配
MATCH (p:person) RETURN p.name, p.age
MATCH (p:person {name: 'Alice'}) RETURN p
MATCH (p:person)-[:friend]->(f:person) RETURN p.name, f.name
MATCH (p:person)-[:friend*1..3]->(f:person) RETURN p.name, f.name

-- GO 图遍历
GO 1 STEP FROM "p1" OVER friend YIELD friend.name
GO 2 STEPS FROM "p1" OVER friend REVERSELY YIELD friend.name
GO 1 TO 3 STEPS FROM "p1" OVER friend BIDIRECT YIELD friend.name

-- LOOKUP 索引查找
LOOKUP ON person WHERE person.name == "Alice" YIELD person.name, person.age
LOOKUP ON friend WHERE friend.degree > 0.5 YIELD friend.degree

-- FETCH 属性获取
FETCH PROP ON person "p1", "p2" YIELD person.name, person.age
FETCH PROP ON friend "p1" -> "p2" @0 YIELD friend.degree

-- FIND PATH 路径查找
FIND SHORTEST PATH FROM "p1" TO "p3" OVER friend
FIND ALL PATH FROM "p1" TO "p3" OVER friend UPTO 5 STEPS
FIND SHORTEST PATH FROM "p1" TO "p3" OVER friend WEIGHT degree
```

### 4. EXPLAIN/PROFILE 语句

```cypher
-- EXPLAIN 查询计划
EXPLAIN MATCH (p:person) RETURN p.name
EXPLAIN FORMAT = DOT MATCH (p:person)-[:friend]->(f) RETURN p.name, f.name
EXPLAIN GO 2 STEPS FROM "p1" OVER friend

-- PROFILE 性能分析
PROFILE MATCH (p:person)-[:friend*2]->(f) RETURN count(f)
PROFILE GO 3 STEPS FROM "p1" OVER friend YIELD friend.name
```

## 测试数据设计

### 社交网络场景

#### 1. Schema 定义

```cypher
-- 创建测试空间
CREATE SPACE e2e_social_network (vid_type=STRING)
USE e2e_social_network

-- 创建标签
CREATE TAG person(
    name: STRING NOT NULL,
    age: INT,
    email: STRING,
    city: STRING,
    created_at: TIMESTAMP DEFAULT now()
)

CREATE TAG company(
    name: STRING NOT NULL,
    industry: STRING,
    founded_year: INT,
    headquarters: STRING
)

-- 创建边类型
CREATE EDGE friend(
    degree: FLOAT DEFAULT 0.5,
    since: DATE,
    trust_level: INT
)

CREATE EDGE works_at(
    position: STRING,
    since: DATE,
    salary_range: STRING
)

CREATE EDGE lives_in(
    since: DATE,
    address: STRING
)

-- 创建索引
CREATE TAG INDEX idx_person_name ON person(name)
CREATE TAG INDEX idx_person_age ON person(age)
CREATE TAG INDEX idx_person_city ON person(city)
CREATE EDGE INDEX idx_friend_since ON friend(since)
```

#### 2. 测试数据设计思路

**数据规模**

- 人员: 20个顶点
- 公司: 5个顶点
- 朋友关系: 30条边 (形成社交网络结构)
- 工作关系: 15条边 (人员与公司关联)
- 居住关系: 20条边 (人员与城市关联)

**数据生成策略**

- **人员数据**: 使用常见英文名，年龄分布在24-36岁之间，覆盖4个城市(北京、上海、深圳、广州)
- **公司数据**: 涵盖不同行业(科技、软件、云服务、咨询、互联网)，成立年份2008-2016
- **朋友关系**: 使用随机图生成算法，确保网络连通性，边的degree属性在0.6-0.9之间模拟亲密度
- **工作关系**: 每人关联一家公司，职位和薪资范围根据年龄和经验合理分布
- **居住关系**: 每人关联一个城市，与工作地点可相同或不同，模拟真实场景

**数据特点**

- 包含多跳路径(用于GO和FIND PATH测试)
- 存在环路(用于测试遍历深度控制)
- 属性值分布均匀(用于索引和过滤测试)
- 边类型多样(用于JOIN和模式匹配测试)

### 电商场景 (扩展测试)

#### 1. Schema 定义

```cypher
-- 创建测试空间
CREATE SPACE e2e_ecommerce (vid_type=STRING)
USE e2e_ecommerce

-- 创建标签
CREATE TAG user(
    user_id: STRING NOT NULL,
    username: STRING,
    email: STRING,
    register_date: DATE,
    level: INT DEFAULT 1
)

CREATE TAG product(
    product_id: STRING NOT NULL,
    name: STRING,
    category: STRING,
    price: DOUBLE,
    stock: INT DEFAULT 0
)

CREATE TAG order(
    order_id: STRING NOT NULL,
    total_amount: DOUBLE,
    status: STRING,
    create_time: TIMESTAMP,
    pay_time: TIMESTAMP
)

-- 创建边类型
CREATE EDGE placed(
    order_time: TIMESTAMP,
    ip_address: STRING
)

CREATE EDGE contains(
    quantity: INT,
    unit_price: DOUBLE,
    discount: DOUBLE DEFAULT 0.0
)

CREATE EDGE views(
    view_time: TIMESTAMP,
    duration_seconds: INT
)
```

#### 2. 测试数据设计思路

**数据规模**

- 用户: 100 个顶点
- 商品: 200 个顶点
- 订单: 500 个顶点
- 下单关系: 500 条边 (用户与订单关联)
- 包含关系: 2000 条边 (订单与商品关联，平均每单4件商品)
- 浏览关系: 5000 条边 (用户浏览商品记录)

**数据生成策略**

- **用户数据**: 随机生成用户名和邮箱，注册日期分布在最近2年，用户等级1-5随机分布
- **商品数据**: 覆盖10个品类(电子产品、服装、食品等)，价格区间10-10000元，库存0-1000随机
- **订单数据**: 订单状态包括待支付、已支付、已发货、已完成、已取消，金额根据商品自动计算
- **下单关系**: 每个用户平均5个订单，下单时间集中在最近1年
- **包含关系**: 根据订单金额和商品单价合理分配商品数量
- **浏览关系**: 每个用户平均浏览50个商品，停留时间5-300秒随机

**数据特点**

- 大量多对多关系(用于JOIN性能测试)
- 时间序列数据(用于时序查询测试)
- 数值范围广泛(用于聚合和排序测试)
- 状态枚举值(用于分组和过滤测试)

## 测试用例设计

### 测试套件 1: 基础连接与 Schema 管理

```
TC-001: 连接服务器并列出空间
  1. 执行: \connect default
  2. 验证: 连接成功
  3. 执行: \show_spaces
  4. 验证: 返回空间列表

TC-002: 创建图空间并切换
  1. 执行: CREATE SPACE e2e_test (vid_type=STRING)
  2. 验证: 创建成功
  3. 执行: USE e2e_test
  4. 验证: 切换成功

TC-003: 创建标签和边类型
  1. 执行: CREATE TAG person(name: STRING, age: INT)
  2. 验证: 创建成功
  3. 执行: \show_tags
  4. 验证: 包含 person
  5. 执行: CREATE EDGE friend(degree: FLOAT)
  6. 验证: 创建成功
  7. 执行: \show_edges
  8. 验证: 包含 friend
```

### 测试套件 2: 数据操作

```
TC-004: 插入顶点数据
  1. 执行: INSERT VERTEX person(name, age) VALUES "p1": ("Alice", 30)
  2. 验证: 插入成功
  3. 执行: FETCH PROP ON person "p1"
  4. 验证: 返回正确数据

TC-005: 插入边数据
  1. 执行: INSERT VERTEX person(name, age) VALUES "p2": ("Bob", 25)
  2. 执行: INSERT EDGE friend(degree) VALUES "p1" -> "p2" @0: (0.8)
  3. 验证: 插入成功
  4. 执行: FETCH PROP ON friend "p1" -> "p2" @0
  5. 验证: 返回正确数据

TC-006: Cypher 风格创建
  1. 执行: CREATE (n:Person {name: 'Charlie', age: 35})
  2. 验证: 创建成功
  3. 执行: MATCH (p:person) RETURN count(p)
  4. 验证: 返回正确计数
```

### 测试套件 3: 查询语句

```
TC-007: MATCH 基础查询
  1. 执行: MATCH (p:person) RETURN p.name, p.age
  2. 验证: 返回所有人员
  3. 验证: 结果包含 name 和 age 列

TC-008: MATCH 带条件查询
  1. 执行: MATCH (p:person) WHERE p.age > 28 RETURN p.name
  2. 验证: 只返回年龄大于28的人员

TC-009: MATCH 路径查询
  1. 执行: MATCH (p:person)-[:friend]->(f:person) RETURN p.name, f.name
  2. 验证: 返回所有朋友关系

TC-010: GO 遍历查询
  1. 执行: GO 1 STEP FROM "p1" OVER friend YIELD friend.name
  2. 验证: 返回 p1 的直接朋友

TC-011: LOOKUP 索引查询
  1. 执行: LOOKUP ON person WHERE person.name == "Alice" YIELD person.age
  2. 验证: 返回 Alice 的年龄

TC-012: FIND PATH 路径查找
  1. 执行: FIND SHORTEST PATH FROM "p1" TO "p3" OVER friend
  2. 验证: 返回最短路径
```

### 测试套件 4: EXPLAIN/PROFILE 分析

```
TC-013: EXPLAIN 基础查询计划
  1. 执行: \explain MATCH (p:person) RETURN p.name
  2. 验证: 返回查询计划树
  3. 验证: 包含 ScanVertices 节点
  4. 验证: 包含 Project 节点

TC-014: EXPLAIN 带索引查询
  1. 先创建索引: CREATE TAG INDEX idx_name ON person(name)
  2. 执行: \explain LOOKUP ON person WHERE person.name == "Alice"
  3. 验证: 计划包含 IndexScan 节点

TC-015: EXPLAIN 连接查询
  1. 执行: \explain MATCH (p:person)-[:friend]->(f:person) RETURN p.name, f.name
  2. 验证: 计划包含 Join 节点
  3. 验证: 显示依赖关系

TC-016: EXPLAIN FORMAT=DOT
  1. 执行: \explain format=dot MATCH (p:person) RETURN p.name
  2. 验证: 返回 DOT 格式输出
  3. 验证: 可被 Graphviz 解析

TC-017: PROFILE 性能分析
  1. 执行: \profile MATCH (p:person)-[:friend*2]->(f) RETURN count(f)
  2. 验证: 返回查询结果
  3. 验证: 显示执行时间统计
  4. 验证: 显示各阶段耗时

TC-018: EXPLAIN GO 语句
  1. 执行: \explain GO 2 STEPS FROM "p1" OVER friend
  2. 验证: 返回遍历计划
  3. 验证: 显示 Traverse 节点

TC-019: EXPLAIN FIND PATH
  1. 执行: \explain FIND SHORTEST PATH FROM "p1" TO "p3" OVER friend
  2. 验证: 返回路径查找计划
  3. 验证: 显示 ShortestPath 节点
```

### 测试套件 5: 事务管理

```
TC-020: 基础事务提交
  1. 执行: \begin
  2. 执行: INSERT VERTEX person(name, age) VALUES "tx1": ("TX_Test", 20)
  3. 执行: \commit
  4. 执行: FETCH PROP ON person "tx1"
  5. 验证: 数据存在

TC-021: 事务回滚
  1. 执行: \begin
  2. 执行: INSERT VERTEX person(name, age) VALUES "tx2": ("Rollback", 25)
  3. 执行: \rollback
  4. 执行: FETCH PROP ON person "tx2"
  5. 验证: 数据不存在

TC-022: 事务中查询
  1. 执行: \begin
  2. 执行: MATCH (p:person) RETURN count(p)
  3. 验证: 返回正确计数
  4. 执行: \commit
```

### 测试套件 6: 输出格式与变量

```
TC-023: 切换输出格式
  1. 执行: \format json
  2. 执行: MATCH (p:person) RETURN p.name LIMIT 1
  3. 验证: 返回 JSON 格式
  4. 执行: \format csv
  5. 执行: MATCH (p:person) RETURN p.name LIMIT 1
  6. 验证: 返回 CSV 格式

TC-024: 设置和使用变量
  1. 执行: \set min_age 25
  2. 执行: MATCH (p:person) WHERE p.age > :min_age RETURN p.name
  3. 验证: 返回年龄大于25的人员

TC-025: 执行脚本文件
  1. 准备: 创建 test.gql 文件包含多个语句
  2. 执行: \i test.gql
  3. 验证: 所有语句执行成功
```

## 测试执行流程

### 阶段 1: 环境准备

```bash
# 1. 启动 GraphDB 服务器
cargo run --release

# 2. 等待服务器就绪
curl http://localhost:8080/v1/health

# 3. 启动 graphdb-cli
./graphdb-cli --host 127.0.0.1 --port 8080
```

### 阶段 2: 数据准备

```bash
# 执行数据初始化脚本
\i e2e_init_data.gql
```

### 阶段 3: 执行测试

```bash
# 按顺序执行测试套件
\i e2e_test_suite_1.gql  # 基础连接与 Schema
\i e2e_test_suite_2.gql  # 数据操作
\i e2e_test_suite_3.gql  # 查询语句
\i e2e_test_suite_4.gql  # EXPLAIN/PROFILE
\i e2e_test_suite_5.gql  # 事务管理
\i e2e_test_suite_6.gql  # 输出格式与变量
```

### 阶段 4: 结果验证

```bash
# 验证数据完整性
MATCH (p:person) RETURN count(p)
MATCH ()-[f:friend]->() RETURN count(f)

# 验证索引
SHOW INDEXES
```

### 阶段 5: 清理环境

```bash
# 删除测试空间
DROP SPACE e2e_social_network
DROP SPACE e2e_ecommerce
DROP SPACE e2e_test
```

## 自动化测试脚本

### 使用 CLI 非交互模式

```bash
# 单次命令执行
graphdb-cli -h 127.0.0.1 -p 8080 -c "SHOW SPACES"

# 执行脚本文件
graphdb-cli -h 127.0.0.1 -p 8080 -f e2e_test_suite.gql

# 带变量执行
graphdb-cli -h 127.0.0.1 -p 8080 -v "test_space=e2e_test" -f test.gql
```

### 预期输出验证

每个测试用例应定义预期输出模式，使用正则匹配或精确匹配验证结果:

```
TEST: TC-001
COMMAND: SHOW SPACES
EXPECT:
  - status: success
  - contains: "e2e_test"

TEST: TC-013
COMMAND: \explain MATCH (p:person) RETURN p.name
EXPECT:
  - contains: "ScanVertices"
  - contains: "Project"
  - contains: "person"
```

## 性能基准测试

### EXPLAIN/PROFILE 性能指标

| 测试项            | 数据集规模     | 预期响应时间 | 实际响应时间 |
| ----------------- | -------------- | ------------ | ------------ |
| EXPLAIN 简单查询  | 100 顶点       | < 100ms      |              |
| EXPLAIN 复杂 JOIN | 1000 顶点      | < 200ms      |              |
| PROFILE 简单查询  | 100 顶点       | < 500ms      |              |
| PROFILE 多跳遍历  | 1000 顶点, 3跳 | < 2000ms     |              |
| FIND PATH         | 100 顶点       | < 1000ms     |              |

## 故障场景测试

### 错误处理验证

```
TC-ERR-001: 无效查询
  COMMAND: INVALID SYNTAX
  EXPECT: 返回语法错误信息

TC-ERR-002: 不存在的空间
  COMMAND: USE non_existent_space
  EXPECT: 返回空间不存在错误

TC-ERR-003: 不存在的标签
  COMMAND: MATCH (n:nonexistent) RETURN n
  EXPECT: 返回标签不存在错误

TC-ERR-004: 无效 EXPLAIN
  COMMAND: \explain INVALID QUERY
  EXPECT: 返回查询解析错误
```

## 文档维护

- 最后更新: 2026-04-27
- 版本: v1.0
- 维护者: GraphDB Team
