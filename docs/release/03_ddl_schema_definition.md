# Linkrs 数据定义语言 (DDL)

## 概述

数据定义语言 (DDL) 用于定义和管理图数据库的Schema，包括图空间、标签、边类型、索引、序列、宏、类型别名等的创建、修改和删除。

---

## 1. CREATE TAG - 创建标签

### 功能
定义节点标签及其属性。

### 语法结构
```cypher
CREATE TAG [IF NOT EXISTS] <tag_name> (
    <prop_name>[: <prop_type>] [SERIAL] [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>']
    [, <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>'] ...]
    [, ttl_duration=<seconds>]
    [, ttl_col=<prop_name>]
)

-- 从查询结果定义标签
CREATE TAG [IF NOT EXISTS] <tag_name> AS (<query>)
```

### 关键特性
- 支持多种数据类型
- 支持IF NOT EXISTS
- 支持NOT NULL约束
- 支持DEFAULT默认值（支持字面量与常量函数调用，创建时即求值）
- 支持COMMENT属性注释
- 支持TTL自动过期
- 支持 `SERIAL` 类型修饰（自增整数，隐含 NOT NULL）
- 支持属性名后省略 `:` 与类型（省略时类型由绑定阶段处理）
- 支持 `AS (<query>)` 从子查询定义标签

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
| ARRAY\<T\> / STRUCT\<...\> | 复合类型（最大嵌套深度 16） |
| 用户定义类型别名 | 通过 CREATE TYPE 定义后可直接使用 |

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
- DEFAULT 值可以是字面量（含负数）或常量函数调用（如 `datetime()`），在创建 Schema 时即求值

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
| SERIAL | `id: SERIAL` | 自增整数，隐含 NOT NULL |

### TTL说明
TTL（Time To Live）用于自动清理过期数据。

**TTL 参数：**
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `ttl_duration` | INT | `0`（禁用TTL） | TTL持续时间（秒），0表示禁用 |
| `ttl_col` | STRING | 无 | 用于计算过期时间的属性名，必须是TIMESTAMP或INT类型 |

**TTL 默认行为：**
- 当不指定 `ttl_duration` 和 `ttl_col` 时，**TTL 默认禁用**
- `ttl_duration` 与 `ttl_col` 必须同时指定或同时省略，仅指定其一时直接报错
  （`TTL requires both ttl_duration and ttl_col to be specified together`），不会静默忽略
- `ttl_duration` 必须为正整数（秒）；`ttl_col` 必须引用已定义的属性，
  且该属性类型须为整数类型（`INT`/`INT64` 等）或 `TIMESTAMP`/`DATETIME`

**TTL 工作原理：**
1. 数据插入时，记录 `ttl_col` 指定属性的值作为基准时间（整数为 Unix 秒，`TIMESTAMP`/`DATETIME` 按其纪元秒换算）
2. 当 `当前时间 > 基准时间 + ttl_duration` 时，数据被视为过期
3. 过期数据在查询时被过滤（读路径按 Schema TTL 配置判定），并可通过存储层过期清理接口物理删除
4. `ttl_col` 为 `NULL`、缺失或类型不符的行永不过期；删除 `ttl_col` 属性会清除该 Schema 的 TTL 配置，
   重命名该属性会自动跟随新的属性名

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

-- SERIAL自增列
CREATE TAG Counter(id: SERIAL, name: STRING)

-- 带TTL创建（数据1年后自动过期）
CREATE TAG UserSession(
    sid: INT64 NOT NULL,
    token: STRING NOT NULL,
    user_id: INT NOT NULL,
    created_at: TIMESTAMP NOT NULL,
    ttl_duration=31536000,
    ttl_col=created_at
)

-- 从查询定义标签
CREATE TAG adult AS (MATCH (p:Person) WHERE p.age >= 18 RETURN p)
```

---

## 2. CREATE EDGE - 创建边类型

### 功能
定义边类型及其属性，可同时声明端点约束。

### 语法结构
```cypher
CREATE EDGE [IF NOT EXISTS] <edge_type> (
    <prop_name>: <prop_type> [SERIAL] [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>']
    [, <prop_name>: <prop_type> ...]
    [, ttl_duration=<seconds>]
    [, ttl_col=<prop_name>]
) [FROM <src_tag> TO <dst_tag>]

-- 从查询结果定义边类型
CREATE EDGE [IF NOT EXISTS] <edge_type> AS (<query>)
```

### 关键特性
- 支持多种数据类型与属性约束（与 CREATE TAG 相同）
- 支持TTL自动过期
- 支持 `FROM <src_tag> TO <dst_tag>` 端点约束（可选）
- 支持 `AS (<query>)` 从子查询定义边类型

> **注意：** 约束默认值与 CREATE TAG 相同：属性默认可空，无默认值时插入 NULL，TTL 默认禁用。

### 示例
```cypher
-- 基础创建
CREATE EDGE IF NOT EXISTS follow(degree: FLOAT, since: TIMESTAMP)

-- 带约束与端点约束创建
CREATE EDGE WORKS_AT(
    since: DATE NOT NULL COMMENT '入职日期',
    department: STRING DEFAULT 'unknown' COMMENT '部门',
    active: BOOL DEFAULT true COMMENT '是否在职'
) FROM person TO company

-- 带TTL创建（数据30天后自动过期）
CREATE EDGE TempRelation(
    data: STRING,
    expire_at: TIMESTAMP NOT NULL,
    ttl_duration=2592000,
    ttl_col=expire_at
)
```

---

## 3. CREATE SPACE - 创建图空间

### 功能
创建图空间（数据库实例）。`CREATE GRAPH` 为同义写法。

### 语法结构
```cypher
CREATE [SPACE | GRAPH] [IF NOT EXISTS] <space_name> [(vid_type=<type>, comment='<text>')] [WITH <key>=<value> ...]
```

### 关键特性
- 支持IF NOT EXISTS
- 可配置VID类型（默认 `INT64`，支持 `FIXEDSTRING(N)` 等）
- 可添加注释（`comment`，括号内或 `WITH comment='...'` 均可）
- `WITH key=value` 形式的扩展参数（当前仅 `comment` 被语义化处理）

> **注意：** 当前单节点版本不支持 `partition_num`、`replica_factor` 参数。

### 示例
```cypher
-- 基本创建
CREATE SPACE IF NOT EXISTS test_space

-- 带参数创建
CREATE SPACE test_space(vid_type=FIXEDSTRING(32), comment="测试空间")

-- GRAPH同义写法
CREATE GRAPH basketball
```

---

## 4. CREATE INDEX - 创建索引

### 功能
在标签或边类型上创建属性索引。

### 语法结构
```cypher
CREATE TAG INDEX [IF NOT EXISTS] <index_name> ON <tag_name> (<prop_list>)
CREATE EDGE INDEX [IF NOT EXISTS] <index_name> ON <edge_name> (<prop_list>)
CREATE INDEX [IF NOT EXISTS] <index_name> ON <tag_name> (<prop_list>)   -- 默认为TAG索引
```

### 示例
```cypher
CREATE TAG INDEX IF NOT EXISTS idx_person_name ON person(name)
CREATE EDGE INDEX idx_follow_degree ON follow(degree)
CREATE INDEX IF NOT EXISTS idx_person_age ON person(age)
```

---

## 5. CREATE FULLTEXT INDEX - 创建全文索引

### 功能
在标签或边类型的文本属性上创建全文索引（BM25 引擎），支持高效的文本搜索功能。

### 语法结构
```cypher
CREATE FULLTEXT INDEX [IF NOT EXISTS] <index_name> ON <tag_or_edge_name>
    (<field> [, <field> ...])
    ENGINE = BM25
```

> **注意：** `ENGINE = BM25` 当前为必填子句，且仅支持 BM25 引擎（默认参数 k1=1.5、b=0.75）。
> 字段级 `ANALYZER '<analyzer>'` / `BOOST <weight>` 与 `OPTIONS (key=value, ...)` 当前**不受支持**，
> 写出这些子句会在解析期直接报错，而不是静默忽略。

### 索引命名语义

全文索引按 `(space, schema, field)` 定位实际索引数据；`DROP FULLTEXT INDEX <name>` /
`SHOW FULLTEXT INDEX` 按声明时的索引名匹配。创建时传入的索引名会随索引元数据保存，
同一 `(space, schema, field)` 上的重复创建按索引名区分。

### 关键特性
- 支持多种文本搜索查询类型
- 支持短语搜索
- 支持前缀和通配符搜索
- 支持模糊搜索
- 支持评分排序
- 支持高亮显示

### 示例
```cypher
-- 创建基础全文索引
CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON Article(content) ENGINE = BM25

-- 多字段全文索引
CREATE FULLTEXT INDEX idx_news_title ON News(title, summary) ENGINE = BM25
```

### 维护全文索引（ALTER FULLTEXT INDEX）

```cypher
ALTER FULLTEXT INDEX <index_name> ADD FIELD <field>
ALTER FULLTEXT INDEX <index_name> DROP FIELD <field>
ALTER FULLTEXT INDEX <index_name> SET <key> = <value>
ALTER FULLTEXT INDEX <index_name> REBUILD
ALTER FULLTEXT INDEX <index_name> OPTIMIZE
```

> **注意：** `ADD FIELD` 暂不支持 `ANALYZER` 子句；各维护动作中当前仅 `OPTIMIZE` 真实执行，
> 其余动作在执行期明确报错而非静默丢弃，如需增删字段或改选项请使用 `DROP` + `CREATE` 重建。

多个动作可用逗号连接。示例：

```cypher
ALTER FULLTEXT INDEX idx_article_content DROP FIELD tags, REBUILD
```

---

## 6. CREATE VECTOR INDEX - 创建向量索引

### 功能
在顶点标签的向量属性上创建向量索引，支持向量相似度搜索。仅支持顶点标签（`TAG`），
不支持边类型：向量索引的点 ID、payload 与重建源均按顶点建模，对边类型名建索引会直接报错。

### 语法结构
```cypher
CREATE VECTOR INDEX [IF NOT EXISTS] <index_name> ON <tag_name> (<field>)
WITH (vector_size=<dimension>, distance={COSINE | EUCLIDEAN | DOT | MANHATTAN}
      [, hnsw_m=<value>] [, hnsw_ef_construct=<value>]
      [, quantization={SCALAR | BINARY | PRODUCT}] [, quantile=<0..1>]
      [, compression={x4|x8|x16|x32|x64}] [, always_ram=<bool>])
```

> **注意：** `WITH (...)` 子句为必需；`vector_size` 必填，其余可选。
> `compression` 仅在 `quantization=PRODUCT` 时有效，其他组合执行期直接报错；
> 非法 HNSW / 量化参数同样在执行期直接报错，不会静默采用无效配置。

### 向量索引参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `vector_size` | INT | 是 | 向量维度大小 |
| `distance` | STRING | 否（默认 COSINE） | 距离度量方式 |
| `hnsw_m` | INT | 否 | HNSW 每层最大连接数 |
| `hnsw_ef_construct` | INT | 否 | HNSW 构建时动态列表大小 |
| `quantization` | STRING | 否 | 量化类型：`scalar` / `binary` / `product` / `none` |
| `quantile` | FLOAT | 否 | 量化分位数，取值 (0,1]，需配合 quantization（仅标量量化读取该值） |
| `compression` | STRING | 否 | 压缩比 x4/x8/x16/x32/x64，仅配合 `quantization=PRODUCT`；其他量化类型下显式报错 |
| `always_ram` | BOOL | 否 | 量化数据是否常驻内存，需配合 quantization |

### 距离度量方式

| 方式 | 说明 | 适用场景 |
|------|------|----------|
| `COSINE` | 余弦相似度 | 文本嵌入、图像特征 |
| `EUCLIDEAN` | 欧氏距离 | 坐标数据、推荐系统 |
| `DOT` | 点积相似度 | 归一化向量 |
| `MANHATTAN` | 曼哈顿距离 | 稀疏特征 |

### 关键特性
- 支持多种距离度量
- 支持向量相似度搜索
- 支持阈值过滤
- 支持属性过滤
- 支持量化压缩（标量/二进制/乘积量化）
- 支持与 Qdrant 集成

### 示例
```cypher
-- 创建基础向量索引（余弦距离）
CREATE VECTOR INDEX IF NOT EXISTS idx_doc_embedding ON Document(embedding)
WITH (vector_size=768, distance=COSINE)

-- 创建带 HNSW 参数的向量索引
CREATE VECTOR INDEX idx_article_vector ON Article(content_vector)
WITH (vector_size=1024, distance=COSINE, hnsw_m=32, hnsw_ef_construct=300)

-- 创建带量化参数的向量索引
CREATE VECTOR INDEX idx_product_vector ON Product(feature_vector)
WITH (vector_size=512, distance=EUCLIDEAN,
      quantization=PRODUCT, compression=x16, always_ram=false)
```

### 索引命名语义

向量索引按 `(space, tag, field)` 定位实际索引数据；声明时的索引名会随索引元数据保存，
供 `SEARCH / LOOKUP / DROP VECTOR INDEX <name>` 反查回 `(space, tag, field)` 位置。
未知索引名在规划期即定位失败，执行期按语句是否带 `IF EXISTS` 报错或返回空结果。

---

## 7. CREATE SEQUENCE - 创建序列

### 功能
创建自增序列，用于生成唯一ID。

### 语法结构
```cypher
CREATE SEQUENCE [IF NOT EXISTS] <seq_name>
    [START=<n>] [INCREMENT=<n>] [MINVALUE=<n>] [MAXVALUE=<n>] [CYCLE | NOCYCLE]
```

### 示例
```cypher
CREATE SEQUENCE IF NOT EXISTS user_id_seq START=1 INCREMENT=1 NOCYCLE
```

---

## 8. CREATE MACRO / CREATE TYPE - 宏与类型别名

### 功能
创建可复用的表达式宏和类型别名。

### 语法结构
```cypher
CREATE MACRO [IF NOT EXISTS] <macro_name>(<param> [= <default_expr>] [, ...]) AS <expression>
CREATE TYPE [IF NOT EXISTS] <alias_name> AS <underlying_type | 别名>
```

### 示例
```cypher
-- 宏：带默认参数的表达式
CREATE MACRO full_name(first, last, sep = '-') AS first + sep + last

-- 类型别名（支持别名引用别名，环检测在计划期执行）
CREATE TYPE user_id AS INT64
CREATE TYPE account_id AS user_id
```

---

## 9. ALTER TAG / ALTER EDGE - 修改标签与边类型

### 功能
修改标签或边类型的属性定义、名称和端点约束。

### 语法结构
```cypher
ALTER TAG <tag_name> ADD (<prop_name>: <prop_type> [, ...])
ALTER TAG <tag_name> DROP (<prop_name> [, <prop_name> ...])
ALTER TAG <tag_name> CHANGE (<old_prop> <new_prop>: <prop_type> [, ...])
ALTER TAG <tag_name> RENAME TO <new_name>

ALTER EDGE <edge_type> ADD (...)
ALTER EDGE <edge_type> DROP (...)
ALTER EDGE <edge_type> CHANGE (...)
ALTER EDGE <edge_type> RENAME TO <new_name>
ALTER EDGE <edge_type> ADD FROM <src_tag> TO <dst_tag>     -- 增加端点约束
ALTER EDGE <edge_type> DROP FROM <src_tag> TO <dst_tag>    -- 移除端点约束
```

### 关键特性
- 支持添加、删除属性
- 支持属性重命名 + 类型修改（CHANGE）
- 支持标签/边类型重命名（RENAME TO）
- 边类型支持增加/移除端点约束（ADD/DROP FROM ... TO ...）
- ADD/DROP/CHANGE 子句可在一个语句中连续出现多个

> **注意：** `CHANGE` 的语义冻结为"仅改名"：新属性继承目标类型的可空性默认值
> （默认可空），不保留也不修改原属性的默认值、注释与可空性约束。
> 如需调整这些约束，请使用 `DROP` + `ADD` 显式表达。
> TAG 与 EDGE 的 `CHANGE` 均走重命名路径，行为一致。

### 示例
```cypher
ALTER TAG person ADD (email: STRING, phone: STRING)
ALTER TAG person DROP (temp_field)
ALTER TAG person CHANGE (old_name new_name: STRING)
ALTER TAG person RENAME TO user

ALTER EDGE follow ADD (note: STRING)
ALTER EDGE follow RENAME TO follows
ALTER EDGE follow ADD FROM person TO person
```

---

## 10. ALTER FULLTEXT INDEX - 修改全文索引

见第 5 节「维护全文索引」。

---

## 11. DROP - 删除对象

### 语法结构
```cypher
DROP TAG [IF EXISTS] <tag_name> [, <tag_name> ...]
DROP EDGE [IF EXISTS] <edge_type> [, <edge_type> ...]
DROP [SPACE | GRAPH] [IF EXISTS] <space_name>
DROP TAG INDEX [IF EXISTS] <index_name> [ON <space_name>]
DROP EDGE INDEX [IF EXISTS] <index_name> [ON <space_name>]
DROP INDEX [IF EXISTS] <index_name> [ON <space_name>]
DROP FULLTEXT INDEX [IF EXISTS] <index_name>
DROP VECTOR INDEX [IF EXISTS] <index_name>
DROP SEQUENCE [IF EXISTS] <seq_name>
DROP MACRO [IF EXISTS] <macro_name>
DROP TYPE [IF EXISTS] <alias_name>
```

### 关键特性
- TAG/EDGE 支持批量删除与 IF EXISTS；批量删除逐项执行、逐项汇报
  （`IF EXISTS` 下不存在的对象汇报 `not_exists`，否则整个语句报错）
- `DROP INDEX` 默认按 TAG 索引处理；`ON <space_name>` 可选
- `DROP SPACE` 与 `DROP GRAPH` 等价，支持 IF EXISTS
- 删除被引用的类型别名会报错（依赖检查）

### 示例
```cypher
DROP TAG IF EXISTS person, company
DROP EDGE IF EXISTS follow, like
DROP SPACE IF EXISTS test_space
DROP GRAPH IF EXISTS test_space
DROP TAG INDEX IF EXISTS idx_person_name
DROP FULLTEXT INDEX IF EXISTS idx_article_content
DROP VECTOR INDEX IF EXISTS idx_doc_embedding
DROP MACRO IF EXISTS full_name
DROP TYPE IF EXISTS account_id
```

---

## 12. DESC/DESCRIBE - 描述对象

### 功能
显示标签、边类型、图空间或用户的定义。

### 语法结构
```cypher
DESCRIBE TAG <tag_name> [IN <space_name>]
DESCRIBE EDGE <edge_type> [IN <space_name>]
DESCRIBE SPACE <space_name>
DESCRIBE USER <username>
```

### 示例
```cypher
DESCRIBE TAG person
DESC EDGE follow
DESCRIBE SPACE test_space
DESCRIBE USER root
```

---

## 13. SHOW - 显示信息

### 功能
显示数据库中的各种元信息。

### 语法结构
```cypher
SHOW SPACES
SHOW TAGS
SHOW EDGES
SHOW INDEXES
SHOW INDEX <index_name>
SHOW STATS
SHOW FULLTEXT INDEX [<index_name>]
SHOW USERS
SHOW ROLES
SHOW FUNCTIONS
SHOW GRAPHS
SHOW MACROS
SHOW EXTENSIONS
SHOW ATTACHED DATABASES
SHOW SESSIONS
SHOW QUERIES
SHOW CONFIGS [<module>]
SHOW HOSTS / SHOW PARTS
```

### 示例
```cypher
SHOW SPACES
SHOW TAGS
SHOW EDGES
SHOW FULLTEXT INDEX idx_article_content
SHOW CONFIGS database
```

---

## 14. SHOW CREATE - 显示创建语句

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
SHOW CREATE SPACE test_space
SHOW CREATE TAG Person
SHOW CREATE EDGE KNOWS
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
| DEFAULT | ✅ | ✅ | 默认值（字面量/常量函数） |
| COMMENT | ✅ | ✅ | 属性注释 |
| TTL | ✅ | ✅ | 自动过期 |
| SERIAL | ✅ | ✅ | 自增列 |
| RENAME | ✅ | ✅ | ALTER ... RENAME TO |
| AS 子查询 | ✅ | ✅ | 从查询结果定义 Schema |
| 端点约束 | - | ✅ | FROM ... TO ... |

### 默认值汇总

| 特性 | 默认值 | 说明 |
|------|--------|------|
| **NULL 约束** | `NULL`（可空） | 不指定时属性默认可空 |
| **DEFAULT 约束** | 无 | 不指定时无默认值，插入 NULL |
| **COMMENT 约束** | 无 | 不指定时无注释 |
| **TTL** | 禁用 | 不指定 `ttl_duration` 时 TTL 禁用 |
| **IF NOT EXISTS / IF EXISTS** | 无 | 不指定时重复创建/删除不存在对象会报错 |
| **VID 类型** | `INT64` | 创建 SPACE 时不指定 vid_type 时的默认值 |

### 完整示例

```cypher
-- 创建一个完整的用户标签
CREATE TAG IF NOT EXISTS User(
    user_id: SERIAL COMMENT '用户ID',
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
    created_at: TIMESTAMP NOT NULL COMMENT '关注时间',
    degree: DOUBLE DEFAULT 1.0 COMMENT '关系程度'
) FROM User TO User;

-- 查看创建语句
SHOW CREATE TAG User;
SHOW CREATE EDGE FOLLOWS;
```
