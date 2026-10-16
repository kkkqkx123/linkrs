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
    <prop_name>: <prop_type> [SERIAL] [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>']
    [, <prop_name>: <prop_type> [NOT NULL | NULL] [DEFAULT <value>] [COMMENT '<text>'] ...]
    [, ttl_duration=<seconds>]
    [, ttl_col=<prop_name>]
)

-- 从查询结果定义标签
-- 注意：该语法仅在嵌入式 / 会话 API 入口支持，通用 ExecuteQuery / gRPC / HTTP / CLI
-- 路径执行时直接报错（错误信息含 "is only supported through the session API"）。
CREATE TAG [IF NOT EXISTS] <tag_name> AS (<query>)
```

### 关键特性
- 支持多种数据类型
- 支持IF NOT EXISTS
- 支持NOT NULL约束
- 支持DEFAULT默认值（支持字面量与常量函数调用，常量函数在 DDL 语句**解析时**求值一次并冻结为常量值）
- 支持COMMENT属性注释
- 支持TTL自动过期
- 支持 `SERIAL` 类型修饰（自增整数，隐含 NOT NULL；不能指定 DEFAULT、不能作为主键，每个标签/边类型最多一列）
- 支持 `AS (<query>)` 从子查询定义标签（仅嵌入式 / 会话 API 入口；通用 DDL 执行路径显式报错）

### 支持的数据类型
| 类型 | 实际类型与位宽 | 说明 |
|------|------|------|
| SMALLINT/INT2 | SmallInt（16 位） | 小整数类型 |
| INT/INTEGER/INT4/INT16/INT32 | Int（32 位） | 整数类型。注意 `INT16`/`INT32` 按名称历史兼容均映射为 32 位 `Int`，需要 16 位请用 `SMALLINT`/`INT2` |
| BIGINT/INT8/INT64 | BigInt（64 位） | 长整数类型 |
| FLOAT/DOUBLE | Float / Double（IEEE-754） | 浮点数类型 |
| STRING/VARCHAR/TEXT | String（变长） | 字符串类型 |
| FIXED_STRING(n) | FixedString(n) | 定长字符串类型 |
| BOOL/BOOLEAN | Bool | 布尔类型 |
| DATE | Date | 日期类型 |
| TIME | Time | 时间类型 |
| TIMESTAMP | DateTime | 时间戳类型，`DATETIME` 的别名，两者等价 |
| DATETIME | DateTime | 日期时间类型 |
| ARRAY\<T\> / STRUCT\<...\> | 复合类型（最大嵌套深度 16） | 复合类型 |
| 用户定义类型别名 | 通过 CREATE TYPE 定义后可直接使用 | 指向某一具体类型的别名 |

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
- DEFAULT 值可以是字面量（含负数）或常量函数调用（如 `datetime()`）；常量函数在 DDL 语句**解析时**求值一次并冻结为常量值，
  之后所有插入行共享该值，不会每行重新取当前时间

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
| SERIAL | `id: SERIAL` | 自增整数，隐含 NOT NULL。SERIAL 列不能指定 DEFAULT，不能作为主键，且每个标签/边类型最多只能有一个 SERIAL 列 |

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
3. 过期数据在读取时被过滤：所有会物化属性行的读路径（点查、按标签/属性扫描、边查询、邻接查询、
   导出等）均按 Schema 的 TTL 配置判定并跳过过期行。纯拓扑快路径（仅取邻接 ID 的多跳中间跳、
   度统计展开、GET NEIGHBORS 等）会先判定所涉及的边类型是否配置了 TTL：若有，则降级到物化路径
   从而正确跳过过期边；未配置 TTL 时保持零物化快路径，行为与性能不变。`count_vertices_by_tag` /
   `count_edges_by_type` 计数路径不解码属性、不判定 TTL，计数值含尚未物理清除的过期行；
   开启后台清理或执行 `PURGE EXPIRED` 后收敛（与上游一致，统计收集器维持近似口径）。
   过期行的物理删除通过 `PURGE EXPIRED` 语句执行（语法见本文档第 15 节）
4. `ttl_col` 为 `NULL`、缺失或类型不符的行永不过期；删除 `ttl_col` 属性会清除该 Schema 的 TTL 配置，
   重命名该属性会自动跟随新的属性名
5. TTL 配置随 Schema 元数据持久化，重命名/删除属性时的联动规则见上；过期行的物理删除有两种形态：
   显式执行 `PURGE EXPIRED` 语句（语法见本文档第 15 节，立即可用），或开启后台清理后由
   存储按配置间隔自动执行同一清理（见本文档第 15 节"后台自动清理"与《07_configuration_reference.md》
   存储配置的 `ttl_purge_enabled` / `ttl_purge_interval_secs`）。后台清理默认关闭，
   开启后每轮清理该实例全部空间内所有配置了 TTL 的标签与边类型，与显式语句共用同一实现

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
-- 仅在嵌入式 / 会话 API 入口可执行；通用 SQL / gRPC / HTTP / CLI 路径报错。
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
> `SERIAL` 的限制同 CREATE TAG：不能指定 DEFAULT、不能作为主键、每个边类型最多一列。

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

> **注意：** 空间注释为**仅创建时可设**（无 `ALTER SPACE ... SET COMMENT`），标签/边类型的
> Schema 注释可经 `ALTER TAG/EDGE ... COMMENT = '...'` 修改（见第 9 节）。`SHOW CREATE` / `DESC`
> 回显的是当前值（空间为创建期值，标签/边类型含后续修改值）。

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
`SHOW FULLTEXT INDEX` 按声明时的索引名匹配。创建时传入的索引名会随索引元数据保存
（`DESCRIBE` / `SHOW` 可见），同一 `(space, schema, field)` 上的重复创建直接报错，
带 `IF NOT EXISTS` 时返回成功而不重建。

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

### 参数默认值

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `START` | `1` | 序列初值，`curr_val` 在首次 `next_val` 之前返回该值 |
| `INCREMENT` | `1` | 步进，必须非零；为 `0` 时创建与修改均直接报错 |
| `MINVALUE` | 无下界 | 省略时取整数最小值 |
| `MAXVALUE` | 无上界 | 省略时取整数最大值 |
| `CYCLE \| NOCYCLE` | `NOCYCLE` | 越界时是否回绕；`NOCYCLE` 下越界报错 |

`MINVALUE` 大于 `MAXVALUE` 时创建与修改均直接报错。

### 取值函数

```cypher
RETURN next_val('seq_name')   -- 取下一个值并推进序列
RETURN curr_val('seq_name')   -- 取当前值，不推进序列
```

`nextval` / `currval`（无下划线）为同义写法。`next_val` 每次调用按 `INCREMENT`
推进并返回新值；`curr_val` 返回最近一次 `next_val` 之后的值（从未推进时返回
`START`）。对不存在的序列调用任一函数均直接报错。

### 修改与删除

```cypher
ALTER SEQUENCE <seq_name>
    [INCREMENT=<n>] [MINVALUE=<n>] [MAXVALUE=<n>] [CYCLE | NOCYCLE]

DROP SEQUENCE [IF EXISTS] <seq_name>
```

`ALTER SEQUENCE` 仅调整步进、边界与回绕策略（省略的子句保持现状），不重置
当前值。删除不存在的序列时，不带 `IF EXISTS` 报错，带 `IF EXISTS` 返回空结果。

### 示例
```cypher
CREATE SEQUENCE IF NOT EXISTS user_id_seq START=1 INCREMENT=1 NOCYCLE

CREATE SEQUENCE order_seq START=1000 INCREMENT=5 MAXVALUE=999999

RETURN nextval('order_seq')

ALTER SEQUENCE order_seq INCREMENT=10

DROP SEQUENCE IF EXISTS order_seq
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

-- 类型别名（支持别名引用别名，环检测在别名创建与解析时执行）
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
ALTER TAG <tag_name> COMMENT = '<text>'

ALTER EDGE <edge_type> ADD (...)
ALTER EDGE <edge_type> DROP (...)
ALTER EDGE <edge_type> CHANGE (...)
ALTER EDGE <edge_type> RENAME TO <new_name>
ALTER EDGE <edge_type> COMMENT = '<text>'
ALTER EDGE <edge_type> ADD FROM <src_tag> TO <dst_tag>     -- 增加端点约束
ALTER EDGE <edge_type> DROP FROM <src_tag> TO <dst_tag>    -- 移除端点约束
```

### 关键特性
- 支持添加、删除属性
- 支持属性重命名 + 类型修改（CHANGE）
- 支持标签/边类型重命名（RENAME TO）
- 支持修改 Schema 级注释（`COMMENT = '...'`，可与 ADD/DROP/CHANGE 组合在同一语句中；空字符串清空注释）
- 边类型支持增加/移除端点约束（ADD/DROP FROM ... TO ...）
- ADD/DROP/CHANGE/COMMENT 子句可在一个语句中连续出现多个，重复指定 COMMENT 直接报错

> **注意：** `CHANGE` 的语义冻结为"仅改名"：新属性继承目标类型的可空性默认值
> （默认可空），不保留也不修改原属性的默认值、注释与可空性约束。
> 如需调整这些约束，请使用 `DROP` + `ADD` 显式表达。
> TAG 与 EDGE 的 `CHANGE` 均走重命名路径，行为一致。

> **注意：** Schema 注释口径与创建期属性注释一致；`COMMENT = ''`（空字符串）表示清空注释，
> 存为无注释。修改后 `SHOW CREATE` 回显新值（含 `comment='...'` 部分），`DESC` 在属性行之外
> 追加一行 `__schema_comment__` 展示 Schema 注释（无注释时不追加）；重启后经快照与 WAL 回放持久。

### 示例
```cypher
ALTER TAG person ADD (email: STRING, phone: STRING)
ALTER TAG person DROP (temp_field)
ALTER TAG person CHANGE (old_name new_name: STRING)
ALTER TAG person RENAME TO user
ALTER TAG person COMMENT = '用户标签'
ALTER TAG person ADD (phone: STRING) COMMENT = '用户标签'

ALTER EDGE follow ADD (note: STRING)
ALTER EDGE follow RENAME TO follows
ALTER EDGE follow COMMENT = '关注关系'
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

## 15. PURGE EXPIRED - 过期数据物理清理

### 功能
物理删除已配置 TTL 的 Schema 中过期的行。TTL 在读取时只做"逻辑过期"（过滤），
占用的存储不会自动释放；`PURGE EXPIRED` 是将其真正回收的显式入口。删除走正常写入路径，
因此会记录 WAL，可被崩溃恢复。

该语句经通用查询接口执行：gRPC `ExecuteQuery`、HTTP 一元查询、CLI 与嵌入式入口均可使用，
**没有专用 RPC**（不新增 proto 消息与服务端方法），与上游"维护操作复用语句通道"的形态一致。

### 权限
`PURGE EXPIRED` 是维护语句，需要**维护权限**（`Schema` 级，即 GOD / ADMIN / DBA 角色）。
权限按**当前会话空间**判定，与其余语句同一口径：会话空间无角色时回退到全局角色。因此只在目标
空间持有角色的用户，需先 `USE <space_name>` 再执行该空间的清理。

- 只读角色（GUEST）与普通读写角色（USER，无 Schema 权限）执行会被拒绝，返回
  `Permission check failed: ...` 可读错误；
- 拒绝行为在 gRPC 一元、HTTP 一元与流式（SSE / gRPC 流式）入口完全一致；
- 显式语句与后台清理（见下）走同一条 purge 实现，权限门只作用于语句入口；后台任务不经权限门，
  是否运行由配置开关决定。

### 语法结构
```cypher
PURGE EXPIRED [TAG <tag_name> | EDGE <edge_name>] IN <space_name>
```

- 省略 `TAG` / `EDGE` 时，清理该空间内**所有**配置了 TTL 的标签与边类型。
- 指定 `TAG <tag_name>` 或 `EDGE <edge_name>` 时，只清理该目标 Schema。
- `IN <space_name>` 必填，避免误清其他空间的数据。

### 关键特性
- **两种执行形态**：显式语句立即可用；后台自动清理为配置 opt-in（默认关闭），
  开启后按配置间隔自动执行不带目标的清理，见下节。
- **按 Schema 汇报**：结果集为每个被扫描的 TTL Schema 返回一行（含删除 0 行的情况），
  便于观测清理进度。后台清理每轮按同一口径把各 Schema 的删除行数写入日志与指标。
- **有界批量**：按 Schema 顺序删除，删除按批（每批固定条数）提交，避免大表一次性
  收集全部过期行造成内存尖峰。
- **不改写 WAL 格式**：删除复用既有 `DeleteVertex` / `DeleteEdge` 记录。
- **幂等**：删除走正常 MVCC 写路径，已清除的 Schema 再次执行汇报 0 行。

### 结果集
列固定为 `space_name` / `schema_name` / `schema_type`（`TAG` 或 `EDGE`）/ `removed_rows`（bigint）。
空间内没有 TTL Schema 时结果集为空。

### 示例
```cypher
-- 清理空间内所有配置了 TTL 的标签与边类型
PURGE EXPIRED IN my_space

-- 只清理指定的标签
PURGE EXPIRED TAG user_session IN my_space

-- 只清理指定的边类型
PURGE EXPIRED EDGE temp_rel IN my_space
```

### 错误形态
- 空间不存在：显式报错。
- `TAG` / `EDGE` 目标 Schema 不存在：显式报错。
- 目标 Schema 存在但未配置 TTL：不报错，`removed_rows` 为 0（未配置即无过期数据）。
- 同时使用 `TAG` 与 `EDGE`：语法错误。

### 后台自动清理（opt-in）
存储层提供一个后台清理任务，与显式语句共用同一 purge 入口，仅多出"按空间枚举"一步：

- 配置项 `ttl_purge_enabled`（总开关，默认 `false`）与 `ttl_purge_interval_secs`
  （执行间隔，默认 3600 秒）位于 `[storage]` 节，见《07_configuration_reference.md》。
  开关关闭或间隔为 0 均不启动任务。
- 每轮按空间枚举，逐空间调用不带目标的清理，即清理该空间全部配置了 TTL 的标签与
  边类型；单轮内按空间顺序执行，批删除逻辑与内存有界性与显式语句完全一致，不新写删除路径。
- 任务为单实例，复用存储后台线程池与任务句柄（与索引 GC 后台任务同构）；显式语句在请求线程执行，
  与后台任务可并发执行，二者共用同一 purge 实现、无互斥锁，安全性由 MVCC 与幂等保证，重复执行幂等。
- 每轮把各 Schema 的删除行数写入日志（删除行数大于 0 时按 Schema 记录，失败空间按 warn 记录原因），
  并累计到指标 `linkrs_ttl_purge_cycles_total` 与 `linkrs_ttl_purge_rows_total`
  （经 `/metrics` Prometheus 端点暴露）。单轮内某空间失败不影响其余空间，该空间下一轮重试。
- 开关默认关闭：不改变现有部署行为，需要自动回收的部署显式开启。

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
