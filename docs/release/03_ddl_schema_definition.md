# GraphDB 数据定义语言 (DDL)

## 概述

数据定义语言 (DDL) 用于定义和管理图数据库的Schema，包括标签、边类型、索引等的创建、修改和删除。

---

## 1. CREATE TAG - 创建标签

### 功能
定义节点标签及其属性。

### 语法结构
```cypher
CREATE TAG [IF NOT EXISTS] <tag_name> (
    <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>']
    [, <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>'] ...]
    [, ttl_duration=<seconds>]
    [, ttl_col=<prop_name>]
)
```

### 关键特性
- 支持多种数据类型
- 支持IF NOT EXISTS
- 支持NOT NULL约束
- 支持DEFAULT默认值
- 支持COMMENT属性注释
- 支持TTL自动过期

### 支持的数据类型
| 类型 | 说明 |
|------|------|
| INT/INT8/INT16/INT32/INT64 | 整数类型 |
| FLOAT/DOUBLE | 浮点数类型 |
| STRING/VARCHAR/TEXT | 字符串类型 |
| BOOL/BOOLEAN | 布尔类型 |
| DATE | 日期类型 |
| TIMESTAMP | 时间戳类型 |
| DATETIME | 日期时间类型 |

### 约束说明
| 约束 | 语法 | 默认值 | 说明 |
|------|------|--------|------|
| NOT NULL | `prop: TYPE NOT NULL` | 未指定时默认可空 | 属性值不能为空，插入数据时必须提供值 |
| NULL | `prop: TYPE NULL` | ✅ **默认行为** | 属性值可为空，插入数据时可不提供值 |
| DEFAULT | `prop: TYPE DEFAULT <value>` | 未指定时无默认值 | 插入数据时如未提供值，自动使用默认值 |
| COMMENT | `prop: TYPE COMMENT 'text'` | 未指定时无注释 | 属性的描述说明，仅用于文档目的 |

#### 约束默认值详细说明

**NULL 约束（默认可空）**
- 当不指定 `NOT NULL` 或 `NULL` 时，属性**默认可空**（等同于 `NULL`）
- 示例：`name: STRING` 等价于 `name: STRING NULL`

**DEFAULT 约束（默认无默认值）**
- 当不指定 `DEFAULT` 时，属性**没有默认值**
- 插入数据时如未提供值且属性可为空，则填充 `NULL`
- 如属性有 `NOT NULL` 约束且无默认值，插入时必须提供值，否则会报错

**COMMENT 约束（默认无注释）**
- 当不指定 `COMMENT` 时，属性**没有注释**
- 注释仅用于文档说明，不影响数据存储和查询

#### 约束组合规则

| 场景 | 语法示例 | 插入行为 |
|------|----------|----------|
| 仅类型 | `age: INT` | 可空，无默认值，不提供值时填充 NULL |
| NOT NULL | `age: INT NOT NULL` | 非空，无默认值，**必须**提供值 |
| NOT NULL + DEFAULT | `age: INT NOT NULL DEFAULT 0` | 非空，有默认值，不提供值时使用默认值 0 |
| DEFAULT | `age: INT DEFAULT 0` | 可空，有默认值，不提供值时使用默认值 0 |
| NULL + DEFAULT | `age: INT NULL DEFAULT 0` | 可空，有默认值，不提供值时使用默认值 0 |

### TTL说明
TTL（Time To Live）用于自动清理过期数据。

**TTL 参数：**
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `ttl_duration` | INT | `0`（禁用TTL） | TTL持续时间（秒），0表示禁用 |
| `ttl_col` | STRING | 无 | 用于计算过期时间的属性名，必须是TIMESTAMP或INT类型 |

**TTL 默认行为：**
- 当不指定 `ttl_duration` 和 `ttl_col` 时，**TTL 默认禁用**
- 仅指定 `ttl_duration` 而不指定 `ttl_col` 时，TTL 不会生效（需要两者配合）
- 建议同时指定 `ttl_duration` 和 `ttl_col`，或都不指定

**TTL 工作原理：**
1. 数据插入时，记录 `ttl_col` 指定属性的值作为基准时间
2. 当 `当前时间 > 基准时间 + ttl_duration` 时，数据被视为过期
3. 过期数据会在后台自动清理（或在查询时被过滤）

**TTL 使用场景：**
- 会话数据自动清理（如用户登录令牌）
- 临时数据过期删除（如验证码、缓存数据）
- 日志数据定期归档（如操作日志保留30天）

### 示例
```cypher
-- 基础创建
CREATE TAG IF NOT EXISTS person(name: STRING, age: INT, created_at: TIMESTAMP)

-- 带约束创建
CREATE TAG Person(
    id: INT NOT NULL COMMENT '主键ID',
    name: STRING NOT NULL DEFAULT 'unknown' COMMENT '姓名',
    age: INT DEFAULT 0 COMMENT '年龄',
    email: STRING NULL COMMENT '邮箱'
)

-- 带TTL创建（数据1年后自动过期）
CREATE TAG Session(
    token: STRING NOT NULL,
    user_id: INT NOT NULL,
    created_at: TIMESTAMP NOT NULL,
    ttl_duration=31536000,
    ttl_col=created_at
)
```

---

## 2. CREATE EDGE - 创建边类型

### 功能
定义边类型及其属性。

### 语法结构
```cypher
CREATE EDGE [IF NOT EXISTS] <edge_type> (
    <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>']
    [, <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>'] ...]
    [, ttl_duration=<seconds>]
    [, ttl_col=<prop_name>]
)
```

### 关键特性
- 支持多种数据类型
- 支持IF NOT EXISTS
- 支持NOT NULL约束
- 支持DEFAULT默认值
- 支持COMMENT属性注释
- 支持TTL自动过期

> **注意：** 约束默认值与 CREATE TAG 相同：属性默认可空，无默认值时插入 NULL，TTL 默认禁用。

### 示例
```cypher
-- 基础创建
CREATE EDGE IF NOT EXISTS follow(degree: FLOAT, since: TIMESTAMP)

-- 带约束创建
CREATE EDGE WORKS_AT(
    since: DATE NOT NULL COMMENT '入职日期',
    department: STRING DEFAULT 'unknown' COMMENT '部门',
    active: BOOL DEFAULT true COMMENT '是否在职'
)

-- 带TTL创建（数据30天后自动过期）
CREATE EDGE TempRelation(
    data: STRING,
    expire_at: TIMESTAMP NOT NULL,
    ttl_duration=2592000,
    ttl_col=expire_at
)
```

---

## 3. Schema 自动创建（Cypher DML 触发）

### 功能
当使用 Cypher 风格的 `CREATE` 数据语句时，如果指定的标签或边类型不存在，系统会自动推断并创建对应的 Schema。

### 触发条件
- 使用 `CREATE (n:Label {...})` 创建节点时，如果 `Label` 不存在
- 使用 `CREATE ()-[:Type {...}]->()` 创建边时，如果 `Type` 不存在

### 自动推断规则

#### 数据类型推断
| 属性值示例 | 推断的数据类型 | 说明 |
|------------|----------------|------|
| `'Alice'` | STRING | 字符串值 |
| `30` | INT64 | 整数值 |
| `30.5` | DOUBLE | 浮点数值 |
| `true` / `false` | BOOL | 布尔值 |
| `datetime()` | DATETIME | 日期时间函数 |
| `date()` | DATE | 日期函数 |
| `timestamp()` | TIMESTAMP | 时间戳函数 |

#### Schema 特性
| 特性 | 自动创建行为 | 说明 |
|------|--------------|------|
| 属性约束 | 默认可空（NULL） | 不添加 NOT NULL 约束 |
| 默认值 | 无默认值 | 不设置 DEFAULT 值 |
| 注释 | 无注释 | 不添加 COMMENT |
| TTL | 禁用 | 不设置 ttl_duration 和 ttl_col |

### 示例

#### 自动创建标签
```cypher
-- 创建节点，自动创建 Person 标签
CREATE (n:Person {name: 'Alice', age: 30, salary: 50000.50})

-- 自动创建的 Schema:
-- CREATE TAG Person(
--   name: STRING,
--   age: INT64,
--   salary: DOUBLE
-- )
```

#### 自动创建边类型
```cypher
-- 创建边，自动创建 KNOWS 边类型
CREATE (a)-[:KNOWS {since: '2020-01-01', degree: 0.8}]->(b)

-- 自动创建的 Schema:
-- CREATE EDGE KNOWS(
--   since: STRING,
--   degree: DOUBLE
-- )
```

#### 自动创建多标签
```cypher
-- 创建多标签节点
CREATE (n:Person:Employee {name: 'Bob', department: 'Engineering'})

-- 自动创建两个标签:
-- CREATE TAG Person(name: STRING)
-- CREATE TAG Employee(name: STRING, department: STRING)
```

### 注意事项

1. **类型推断的局限性**
   - 所有字符串都推断为 STRING，不会自动使用 VARCHAR
   - 所有整数都推断为 INT64，不会使用 INT8/INT16/INT32
   - 如需更精确的类型控制，请使用 DDL 预先定义 Schema

2. **约束缺失**
   - 自动创建的属性都是可空的
   - 不会自动设置默认值
   - 不会自动添加 NOT NULL 约束
   - 如需约束，请使用 ALTER TAG/EDGE 修改

3. **性能考虑**
   - Schema 自动创建需要额外的元数据操作
   - 大批量数据导入时，建议先使用 DDL 创建 Schema
   - 自动创建适合交互式查询和小批量数据操作

4. **命名规范**
   - 自动创建的 Schema 名称与 Cypher 语句中的标签/边类型名称一致
   - 遵循标识符命名规范（区分大小写）

### 与显式 DDL 的对比

| 特性 | Schema 自动创建 | 显式 DDL |
|------|-----------------|----------|
| 使用场景 | 交互式查询、快速原型 | 生产环境、大批量导入 |
| 类型控制 | 自动推断 | 精确指定 |
| 约束支持 | 仅默认可空 | 完整约束支持 |
| 性能 | 稍慢（需元数据操作） | 更快（无运行时创建） |
| 灵活性 | 高 | 中 |

### 最佳实践

1. **开发阶段**：可以使用 Schema 自动创建快速迭代
2. **测试阶段**：建议使用显式 DDL 确保 Schema 稳定性
3. **生产阶段**：
   - 使用显式 DDL 预先定义所有 Schema
   - 禁用 Schema 自动创建（如支持该配置）
   - 使用版本控制管理 Schema 变更

---

## 4. CREATE SPACE - 创建图空间

### 功能
创建图空间（数据库实例）。

### 语法结构
```cypher
CREATE SPACE [IF NOT EXISTS] <space_name> [(vid_type=<type>, partition_num=<n>, replica_factor=<n>, comment="<text>")]
```

### 关键特性
- 支持IF NOT EXISTS
- 可配置VID类型（INT64, FIXEDSTRING32等）
- 可配置分区数
- 可配置副本因子
- 可添加注释

### 示例
```cypher
-- 基本创建
CREATE SPACE IF NOT EXISTS test_space

-- 带参数创建
CREATE SPACE test_space(vid_type=FIXEDSTRING32, partition_num=10, replica_factor=3, comment="测试空间")
```

---

## 5. CREATE INDEX - 创建索引

### 功能
在标签或边类型上创建索引。

### 语法结构
```cypher
CREATE INDEX [IF NOT EXISTS] <index_name> ON <tag_or_edge_name> (<prop_list>)
```

### 示例
```cypher
CREATE INDEX IF NOT EXISTS idx_person_name ON person(name)
CREATE INDEX idx_follow_degree ON follow(degree)
```

---

## 6. CREATE FULLTEXT INDEX - 创建全文索引

### 功能
在标签或边类型的文本属性上创建全文索引，支持高效的文本搜索功能。

### 语法结构
```cypher
CREATE FULLTEXT INDEX [IF NOT EXISTS] <index_name> ON <tag_or_edge_name> (<field_list>)
[ENGINE = {BM25 | INVERSEARCH}]
[OPTIONS (key=value, ...)]
```

### 全文检索引擎类型

| 引擎 | 说明 | 适用场景 |
|------|------|----------|
| `BM25` | 基于概率相关性的文本排名算法 | 通用文本搜索，文档检索 |
| `INVERSEARCH` | 倒排索引引擎 | 快速关键词匹配，高频查询 |

### 全文索引选项

| 选项 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `k1` | FLOAT | 1.5 | BM25 词频调节参数 |
| `b` | FLOAT | 0.75 | BM25 文档长度调节参数 |
| `analyzer` | STRING | default | 分词器名称 |
| `store_original` | BOOL | true | 是否存储原文 |

### 关键特性
- 支持多种文本搜索查询类型
- 支持布尔查询（MUST, SHOULD, MUST_NOT）
- 支持短语搜索
- 支持前缀和通配符搜索
- 支持模糊搜索
- 支持评分排序
- 支持高亮显示

### 示例
```cypher
-- 创建基础全文索引（默认使用BM25引擎）
CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON Article(content)

-- 创建指定引擎的全文索引
CREATE FULLTEXT INDEX idx_news_title ON News(title, summary) ENGINE = BM25

-- 创建带选项的全文索引
CREATE FULLTEXT INDEX idx_product_desc ON Product(description)
OPTIONS (k1=1.2, b=0.8, analyzer=standard)
```

---

## 7. CREATE VECTOR INDEX - 创建向量索引

### 功能
在标签或边类型的向量属性上创建向量索引，支持向量相似度搜索。

### 语法结构
```cypher
CREATE VECTOR INDEX [IF NOT EXISTS] <index_name> ON <tag_or_edge_name> (<field>)
WITH (vector_size=<dimension>, distance={COSINE | EUCLID | DOT})
[HNSW (m=<value>, ef_construction=<value>)]
[QUANTIZATION (type={SQ8 | PQ16}, ratio=<value>)]
```

### 向量索引参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `vector_size` | INT | 是 | 向量维度大小 |
| `distance` | STRING | 是 | 距离度量方式 |

### 距离度量方式

| 方式 | 说明 | 适用场景 |
|------|------|----------|
| `COSINE` | 余弦相似度 | 文本嵌入、图像特征 |
| `EUCLID` | 欧氏距离 | 坐标数据、推荐系统 |
| `DOT` | 点积相似度 | 归一化向量 |

### HNSW 参数（可选）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `m` | INT | 16 | 每层最大连接数 |
| `ef_construction` | INT | 200 | 构建时动态列表大小 |

### 量化参数（可选）

| 参数 | 类型 | 说明 |
|------|------|------|
| `type` | STRING | 量化类型（SQ8, PQ16） |
| `ratio` | FLOAT | 压缩比 |

### 关键特性
- 支持多种距离度量
- 支持向量相似度搜索
- 支持阈值过滤
- 支持属性过滤
- 支持批量操作
- 支持与 Qdrant 集成

### 示例
```cypher
-- 创建基础向量索引（余弦距离）
CREATE VECTOR INDEX IF NOT EXISTS idx_doc_embedding ON Document(embedding)
WITH (vector_size=768, distance=COSINE)

-- 创建带 HNSW 参数的向量索引
CREATE VECTOR INDEX idx_article_vector ON Article(content_vector)
WITH (vector_size=1024, distance=COSINE)
HNSW (m=32, ef_construction=300)

-- 创建带量化参数的向量索引
CREATE VECTOR INDEX idx_product_vector ON Product(feature_vector)
WITH (vector_size=512, distance=EUCLID)
QUANTIZATION (type=PQ16, ratio=0.5)
```

---

## 8. ALTER TAG - 修改标签

### 功能
修改标签定义。

### 语法结构
```cypher
ALTER TAG <tag_name> ADD (<prop_name>: <prop_type> [, <prop_name>: <prop_type> ...])
ALTER TAG <tag_name> DROP (<prop_name> [, <prop_name> ...])
ALTER TAG <tag_name> CHANGE (<old_prop> <new_prop>: <prop_type>)
```

### 关键特性
- 支持添加属性
- 支持删除属性
- 支持重命名属性
- 支持修改属性类型

### 示例
```cypher
ALTER TAG person ADD (email: STRING, phone: STRING)
ALTER TAG person DROP (temp_field)
ALTER TAG person CHANGE (old_name new_name: STRING)
```

---

## 9. ALTER EDGE - 修改边类型

### 功能
修改边类型定义。

### 语法结构
```cypher
ALTER EDGE <edge_type> ADD (<prop_name>: <prop_type> [, <prop_name>: <prop_type> ...])
ALTER EDGE <edge_type> DROP (<prop_name> [, <prop_name> ...])
ALTER EDGE <edge_type> CHANGE (<old_prop> <new_prop>: <prop_type>)
```

### 关键特性
- 支持添加属性
- 支持删除属性
- 支持重命名属性
- 支持修改属性类型

### 示例
```cypher
ALTER EDGE follow ADD (note: STRING)
ALTER EDGE follow DROP (old_field)
```

---

## 10. DROP TAG - 删除标签

### 功能
删除标签定义。

### 语法结构
```cypher
DROP TAG [IF EXISTS] <tag_name> [, <tag_name> ...]
```

### 关键特性
- 支持IF EXISTS
- 支持批量删除
- 级联删除相关数据

### 示例
```cypher
DROP TAG IF EXISTS person, company
```

---

## 11. DROP EDGE - 删除边类型

### 功能
删除边类型定义。

### 语法结构
```cypher
DROP EDGE [IF EXISTS] <edge_type> [, <edge_type> ...]
```

### 关键特性
- 支持IF EXISTS
- 支持批量删除
- 级联删除相关数据

### 示例
```cypher
DROP EDGE IF EXISTS follow, like
```

---

## 12. DROP SPACE - 删除图空间

### 功能
删除图空间。

### 语法结构
```cypher
DROP SPACE [IF EXISTS] <space_name>
```

### 示例
```cypher
DROP SPACE IF EXISTS test_space
```

---

## 13. DROP INDEX - 删除索引

### 功能
删除索引。

### 语法结构
```cypher
DROP INDEX [IF EXISTS] <index_name> [ON <space_name>]
DROP TAG INDEX [IF EXISTS] <index_name> [ON <space_name>]
DROP EDGE INDEX [IF EXISTS] <index_name> [ON <space_name>]
```

### 示例
```cypher
DROP INDEX IF EXISTS idx_person_name
DROP TAG INDEX idx_person_name ON test_space
```

---

## 14. DROP FULLTEXT INDEX - 删除全文索引

### 功能
删除全文索引。

### 语法结构
```cypher
DROP FULLTEXT INDEX [IF EXISTS] <index_name>
```

### 示例
```cypher
DROP FULLTEXT INDEX IF EXISTS idx_article_content
```

---

## 15. DROP VECTOR INDEX - 删除向量索引

### 功能
删除向量索引。

### 语法结构
```cypher
DROP VECTOR INDEX [IF EXISTS] <index_name>
```

### 示例
```cypher
DROP VECTOR INDEX IF EXISTS idx_doc_embedding
```

---

## 16. DESC/DESCRIBE - 描述对象

### 功能
显示标签、边类型或用户的定义。

### 语法结构
```cypher
DESCRIBE TAG <tag_name> [IN <space_name>]
DESCRIBE EDGE <edge_type> [IN <space_name>]
DESCRIBE SPACE <space_name>
```

### 关键特性
- 显示属性列表
- 显示属性类型
- 显示索引信息

### 示例
```cypher
DESCRIBE TAG person
DESCRIBE EDGE follow
DESCRIBE SPACE test_space
```

---

## 17. SHOW - 显示信息

### 功能
显示数据库中的各种信息。

### 语法结构
```cypher
SHOW SPACES
SHOW TAGS
SHOW EDGES
SHOW INDEXES
SHOW FULLTEXT INDEXES
SHOW VECTOR INDEXES
```

### 示例
```cypher
SHOW SPACES
SHOW TAGS
SHOW EDGES
SHOW FULLTEXT INDEXES
SHOW VECTOR INDEXES
```

---

## 18. SHOW CREATE - 显示创建语句

### 功能
显示对象的完整创建语句（DDL），便于查看对象定义或迁移数据。

### 语法结构
```cypher
SHOW CREATE SPACE <space_name>
SHOW CREATE TAG <tag_name>
SHOW CREATE EDGE <edge_type>
SHOW CREATE INDEX <index_name>
```

### 关键特性
- 显示完整的CREATE语句
- 包含所有属性定义
- 包含约束条件（NOT NULL, DEFAULT等）
- 包含TTL配置
- 包含注释信息

### 示例
```cypher
-- 查看图空间创建语句
SHOW CREATE SPACE test_space

-- 查看标签创建语句
SHOW CREATE TAG Person

-- 查看边类型创建语句
SHOW CREATE EDGE KNOWS

-- 查看索引创建语句
SHOW CREATE INDEX idx_person_name
```

### 返回结果
```
+------------------------------------------------------------------------+
| create_statement                                                       |
+------------------------------------------------------------------------+
| CREATE TAG IF NOT EXISTS Person(                                       |
|     id: INT NOT NULL COMMENT '主键ID',                                 |
|     name: STRING NOT NULL DEFAULT 'unknown' COMMENT '姓名',            |
|     age: INT DEFAULT 0 COMMENT '年龄',                                 |
|     created_at: TIMESTAMP,                                             |
|     ttl_duration=31536000,                                             |
|     ttl_col=created_at                                                 |
| )                                                                      |
+------------------------------------------------------------------------+
```

---

## 功能汇总表

### 支持的特性

| 功能 | CREATE TAG | CREATE EDGE | 说明 |
|------|------------|-------------|------|
| IF NOT EXISTS | ✅ | ✅ | 避免重复创建错误 |
| NOT NULL | ✅ | ✅ | 非空约束 |
| DEFAULT | ✅ | ✅ | 默认值 |
| COMMENT | ✅ | ✅ | 属性注释 |
| TTL | ✅ | ✅ | 自动过期 |

### 默认值汇总

| 特性 | 默认值 | 说明 |
|------|--------|------|
| **NULL 约束** | `NULL`（可空） | 不指定时属性默认可空 |
| **DEFAULT 约束** | 无 | 不指定时无默认值，插入 NULL |
| **COMMENT 约束** | 无 | 不指定时无注释 |
| **TTL** | 禁用 | 不指定 `ttl_duration` 时 TTL 禁用 |
| **IF NOT EXISTS** | 无 | 不指定时重复创建会报错 |

### 完整示例

```cypher
-- 创建一个完整的用户标签
CREATE TAG IF NOT EXISTS User(
    user_id: INT NOT NULL COMMENT '用户ID',
    username: STRING NOT NULL COMMENT '用户名',
    email: STRING NOT NULL DEFAULT '' COMMENT '邮箱',
    age: INT NULL DEFAULT 0 COMMENT '年龄',
    status: STRING DEFAULT 'active' COMMENT '状态',
    created_at: TIMESTAMP NOT NULL COMMENT '创建时间',
    updated_at: TIMESTAMP COMMENT '更新时间',
    ttl_duration=31536000,
    ttl_col=created_at
);

-- 创建关注关系边
CREATE EDGE IF NOT EXISTS FOLLOWS(
    follow_id: INT NOT NULL COMMENT '关注ID',
    source_user: INT NOT NULL COMMENT '关注者ID',
    target_user: INT NOT NULL COMMENT '被关注者ID',
    created_at: TIMESTAMP NOT NULL COMMENT '关注时间',
    degree: DOUBLE DEFAULT 1.0 COMMENT '关系程度',
    ttl_duration=0
);

-- 查看创建语句
SHOW CREATE TAG User;
SHOW CREATE EDGE FOLLOWS;
```
