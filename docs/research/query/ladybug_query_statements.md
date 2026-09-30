# ladybug 支持的查询语句清单

> 来源：`ref/ladybug`（Kùzu 衍生的嵌入式属性图数据库，openCypher 方言 + `iC_` 前缀扩展）。
> 权威语法：`ref/ladybug/src/antlr4/Cypher.g4`（925 行）；关键字表：`src/antlr4/keywords.txt`；
> AST：`src/include/parser/statement.h`；绑定：`src/binder/bind/**`；测试：`test/test_files/**`。

## 1. 顶层语句一览

语句根规则 `oC_Statement`（`Cypher.g4:8-32`），语句间用 `;` 分隔，支持 `//` 与 `/* */` 注释。

| # | 语句 | 语法示例 | 说明 |
|---|------|---------|------|
| 1 | Cypher 查询 | `MATCH (a:person) RETURN a.ID` | 核心查询 |
| 2 | `EXPLAIN [LOGICAL] <stmt>` | `EXPLAIN LOGICAL MATCH (p1:person)-[e:knows]->(p2) RETURN *` | 计划展示 |
| 3 | `PROFILE <stmt>` | `PROFILE MATCH (a)-[]->(b) RETURN COUNT(*)` | 计划 + 执行统计 |
| 4 | `CREATE NODE TABLE` | `CREATE NODE TABLE person(id INT64 PRIMARY KEY, name STRING)` | 建点表，支持 `AS <query>`、`WITH (opt=val)` |
| 5 | `CREATE REL TABLE [GROUP]` | `CREATE REL TABLE knows(FROM person TO person, since INT64)` | 建边表 / 边表组 |
| 6 | `CREATE SEQUENCE` | `CREATE SEQUENCE s START 10 INCREMENT 1 MAXVALUE 10 CYCLE` | |
| 7 | `CREATE TYPE` | `CREATE TYPE person_info AS STRUCT(age int, name string)` | 用户自定义类型 |
| 8 | `CREATE MACRO` | `CREATE MACRO Add10(x) AS x + 10`（支持默认参数 `y := 40`） | 数据库内宏 |
| 9 | `DROP ...` | `DROP TABLE studyAt`、`DROP SEQUENCE s`、`DROP MACRO m`、`DROP GRAPH g`、`IF EXISTS` | |
| 10 | `ALTER TABLE` | `ADD prop TYPE`、`DROP prop`、`RENAME TO t2`、`RENAME old TO new`、`ADD/DROP FROM A TO B` | |
| 11 | `COMMENT ON` | `COMMENT ON TABLE person IS 'comment'` | |
| 12 | `COPY ... FROM` | `COPY Comment FROM 'x.csv' (DELIM="\|", header=true)` | 批量导入，支持 GLOB、npy/parquet |
| 13 | `COPY ... FROM ... BY COLUMN` | `COPY t FROM ("a.npy","b.npy") BY COLUMN` | 列式导入 |
| 14 | `COPY (query) TO` | `COPY (MATCH (a) RETURN a) TO "out.csv" (header=true)` | 查询导出 |
| 15 | `CALL`（独立） | `CALL threads=8`（配置项）或 `CALL SHOW_TABLES()` | 两种形态 |
| 16 | `CREATE USER / CREATE ROLE` | `CREATE USER alice WITH PASSWORD 'pw'`、`CREATE ROLE analyst` | DCL |
| 17 | 事务控制 | `BEGIN TRANSACTION [READ ONLY]`、`COMMIT`、`ROLLBACK`、`CHECKPOINT` | |
| 18 | 扩展管理 | `INSTALL fts`、`UNINSTALL fts`、`LOAD HTTPFS`、`UPDATE neo4j` | |
| 19 | `EXPORT/IMPORT DATABASE` | `EXPORT DATABASE 'dir' (format="csv", SCHEMA_ONLY=true)` | |
| 20 | `ATTACH/DETACH/USE DATABASE` | `ATTACH 'path' AS alias (DBTYPE lbug)`、`USE mydb` | 多库目录 |
| 21 | `CREATE GRAPH` / `USE GRAPH` | `CREATE GRAPH g1`、`USE GRAPH main` | 多图 |

## 2. 查询组合结构

```
RegularQuery := SingleQuery (UNION [ALL] SingleQuery)*
SingleQuery  := SinglePartQuery | MultiPartQuery
SinglePart   := 读子句* RETURN | 读子句* 更新子句+ RETURN?
MultiPart    := QueryPart+ SinglePart        （QueryPart := 读子句* 更新子句* WITH）
```

- **`RETURN` 必须位于语句末尾**（解析器强制报错）。
- `UNION` / `UNION ALL`：要求列数与类型一致，二者不可混用。
- 读子句、更新子句、`WITH` 可在多段查询中交错：
  `MATCH (a:User) WHERE a.name='Adam' WITH a MATCH (b:User) ... WITH a, b CREATE (a)-[e:Follows {since:1990}]->(b)`

## 3. 读子句

### 3.1 MATCH / OPTIONAL MATCH

```
[OPTIONAL] MATCH <pattern> [WHERE <expr>] [HINT <join-tree>]
```

- 节点模式：`(a)`、`(a:person)`、多标签 AND `(:org:person)`、多标签 OR `(:org|:person)`、属性 map `{ID: 0}`。
- 边模式：有向 `-[e:knows]->`、无向 `-[e]-`、多类型 `-[e:knows|:meets]`、自环（改写为 `id(a)=id(b)`）。
- **HINT 连接提示（非标准）**：`HINT a JOIN (r join b)`、`MULTI_JOIN` 指定连接树。
- **WHERE 中模式谓词（非标准）**：`WHERE (a)-[:knows]->(b)` 直接作存在性判断。

### 3.2 UNWIND

`UNWIND <expr> AS <var>` — 展开列表，如 `UNWIND [1,'hello',true] AS x RETURN x`；配合 `collect()` 反向聚合。

### 3.3 查询内 CALL（表函数）

```
CALL <func>(args) [WHERE <expr>] [YIELD col [AS alias], ...]
```

- `CALL show_tables() RETURN *`、`CALL TABLE_INFO('person') yield prop_id as id RETURN *`
- 仅允许表函数 / 算法函数。

### 3.4 LOAD FROM（取代 LOAD CSV）

```
LOAD [WITH HEADERS (colDef,...)] FROM <scanSource> [(options)] [WHERE <expr>]
```

- 数据源：文件路径、文件列表、`GLOB('pat')`、`(查询)`、`$param`、`attacheddb.table`、表函数。
- `LOAD FROM 'file.csv'(header=true) RETURN *`；`LOAD FROM (MATCH (a) RETURN a.ID) RETURN *`。

### 3.5 子查询表达式

- `EXISTS { MATCH ... [WHERE ...] }` / `COUNT { MATCH ... }`（替代 `CALL {}` 子查询），支持嵌套相关子查询：
  `RETURN a.name, COUNT { MATCH (a)<-[:Follows]-(b) } AS num_follower`

## 4. 变长 / 最短路径（核心特性）

```
'*' [recursiveType] [range] [recursiveComprehension]
recursiveType := [ALL] WSHORTEST '(' relProperty ')' | SHORTEST | ALL SHORTEST | TRAIL | ACYCLIC
range         := int | [int] '..' [int]
recursiveComprehension := '(' var ',' var ['|' WHERE pred] ['|' {nodeProj}, {relProj}] ')'
```

| 变体 | 示例 |
|------|------|
| 固定跳数 | `MATCH (a)-[e:knows*2]->(b)` |
| 范围 | `MATCH p = (a)-[e:knows*1..2]->(b) RETURN nodes(p)` |
| 无上界 | `-[e*]->`（上界由配置 `var_length_extend_max_depth` 控制） |
| TRAIL（边不重复） | `-[e* TRAIL 1..3]->` |
| ACYCLIC（点不重复） | `-[e* ACYCLIC 2..3]->` |
| 最短路径 | `-[r:knows* SHORTEST 1..30]->` |
| 全部最短 | `-[r* ALL SHORTEST 1..30]-` |
| 加权最短 | `-[e* WSHORTEST(cost1)]->`，配合 `cost(e)` 取权值 |
| 全部加权最短 | `-[e* ALL WSHORTEST(cost)]->` |
| 递归推导式 | `-[e:knows*2..2 (r, n \| WHERE r.comments = [...] \| {r.date}, {})]-` |

路径函数：`nodes(p)`、`rels(p)`、`relationships(p)`、`length(p)`、`properties(...)`、`cost(e)`、`is_trail`、`is_acyclic`。

## 5. 更新子句

仅 **CREATE / MERGE / SET / DELETE** 四种：

- `CREATE <pattern> [, ...]`：`CREATE (:Person {name:'Alice', age:25})`；不支持多标签点创建、无向边创建。
- `MERGE <pattern> [ON MATCH SET ...] [ON CREATE SET ...]`。
- `SET`：仅两种形式 —— `SET prop = expr, ...` 与整 map 覆盖 `SET a = { age: 20 }`；**无 `+=`，无 `REMOVE`**。
- `[DETACH] DELETE expr, ...`：`MATCH (a:person) DETACH DELETE a`；边表不支持 DETACH DELETE。

## 6. 投影：RETURN / WITH

```
(RETURN|WITH) [DISTINCT] <items> [ORDER BY ...] [SKIP n] [LIMIT n] [WHERE pred]   -- WHERE 仅 WITH
```

- `RETURN *`、`RETURN a.*`、`RETURN a.state.*` 展开；聚合 `COUNT/SUM/AVG/MIN/MAX/COLLECT` 支持内层 `DISTINCT`。
- `ORDER BY ... ASC|DESC`；`SKIP`/`LIMIT` 必须为字面量或参数。
- 约束：`WITH` 每项必须 `AS` 起别名；`WITH` 中 `ORDER BY` 必须跟 `SKIP/LIMIT`；禁止嵌套聚合；ORDER BY 不可作用于 NODE/REL/MAP/STRUCT/列表/INTERNAL_ID。

## 7. 表达式与操作符

优先级（高→低）：`.` 属性访问 → 一元 `-`/`!` → `=~`/`STARTS WITH`/`ENDS WITH`/`CONTAINS`/`IN`/索引切片/`IS [NOT] NULL` → `^` → `* / %` → `+ -` → `>> <<` → `&` → `|` → 比较（禁止链式）→ `NOT` → `AND` → `XOR` → `OR`。

- 字面量：数字、字符串（含转义）、`TRUE/FALSE/NULL`、列表、struct/map `{a:1}`。
- `CASE [expr] WHEN ... THEN ... [ELSE ...] END`。
- 量词：`ALL/ANY/NONE/SINGLE (x IN list WHERE ...)`。
- Lambda：`x -> expr`、`(x,y) -> expr`，配合 `LIST_FILTER/LIST_TRANSFORM/LIST_REDUCE`。
- 参数：`$name` / `$1`（预处理语句），可用于 `LIMIT $l`。
- 拒绝：`!=`（提示用 `<>`）、链式比较 `a=b=c`。

## 8. 内置函数

- **聚合（7）**：`COUNT(*)`、`COUNT`、`SUM`、`AVG`、`MIN`、`MAX`、`COLLECT`。
- **标量（按类）**：
  - 算术：`ABS POW CEIL FLOOR ROUND SQRT LN LOG10 ... RANDOM BITWISE_* BITSHIFT_*`
  - 字符串：`CONCAT SUBSTR TRIM UPPER LOWER LPAD RPAD LEVENSHTEIN REGEXP_* STRING_SPLIT ...`
  - 列表：`RANGE SIZE LIST_EXTRACT LIST_CONCAT LIST_SORT LIST_DISTINCT LIST_FILTER LIST_TRANSFORM LIST_REDUCE ...`
  - **向量相似（核心内置）**：`ARRAY_COSINE_SIMILARITY`、`ARRAY_DISTANCE`、`ARRAY_INNER_PRODUCT`、`ARRAY_DOT_PRODUCT` 等
  - Map/Struct/Union：`MAP_EXTRACT MAP_KEYS STRUCT_PACK UNION_VALUE ...`
  - 路径：`NODES RELS PROPERTIES IS_TRAIL IS_ACYCLIC LENGTH`
  - 节点/边：`ID OFFSET ROWID START_NODE END_NODE LABEL LABELS COST`
  - 类型转换：`CAST TO_INT* TO_STRING TO_DATE TO_TIMESTAMP TO_INTERVAL ...`
  - 日期/时间/间隔：`DATE_PART DATE_TRUNC CURRENT_DATE CURRENT_TIMESTAMP LAST_DAY ...`
  - 哈希/UUID：`MD5 SHA256 GEN_RANDOM_UUID`；序列：`NEXTVAL CURRVAL`
  - 工具：`COALESCE IFNULL NULLIF TYPEOF ERROR`
- **表函数（CALL 调用）**：`SHOW_TABLES TABLE_INFO STORAGE_INFO SHOW_SEQUENCES SHOW_FUNCTIONS ...`、`READ_CSV_* READ_PARQUET READ_NPY`。
- **独立表函数**：`PROJECT_GRAPH PROJECT_GRAPH_CYPHER DROP_PROJECTED_GRAPH`（投影图伪库）。
- **宏**：`CREATE MACRO` 定义后可作标量函数调用。

## 9. 非标准扩展与扩展包

1. `HINT ... JOIN ... / MULTI_JOIN` 连接顺序提示。
2. `TRAIL / ACYCLIC / SHORTEST / ALL SHORTEST / WSHORTEST(prop)` 递归语义关键字。
3. 递归模式推导式（节点/边投影）。
4. `EXISTS { } / COUNT { }` 子查询表达式 + WHERE 内联模式谓词。
5. `LOAD FROM [WITH HEADERS]` + 查询作为扫描源。
6. `CALL option = value` 会话配置。
7. `COPY` 三种形态（FROM/列式/查询 TO）。
8. 多库/多图：`CREATE GRAPH / USE GRAPH / ATTACH / USE DATABASE`。
9. `PROJECT_GRAPH` 投影图；`CREATE MACRO`（默认参数）；`CREATE TYPE / SEQUENCE / COMMENT ON`。
10. 属性通配 `n.*`、`n.map.*`；`EXPLAIN LOGICAL`；`rowid()/offset(id(n))/cost(e)` 访问器。

### 扩展包（INSTALL/LOAD）

- **FTS（BM25 全文）**：`CALL CREATE_FTS_INDEX('Person','idx',['name'], stemmer:='english')`、`CALL QUERY_FTS_INDEX(...)`（BM25 排序）、`CALL DROP_FTS_INDEX(...)`。
- **VECTOR（HNSW ANN）**：`CALL CREATE_VECTOR_INDEX('t','idx','vec', metric:='cosine')`、`CALL QUERY_VECTOR_INDEX('t','idx',[...], k)`；度量 Cosine/L2/DotProduct。
- **ALGO**：`PAGE_RANK`、`K_CORE_DECOMPOSITION`、`STRONGLY/WEAKLY_CONNECTED_COMPONENTS`。
- **JSON / LLM / NEO4J / DELTA / ICEBERG / DUCKDB / POSTGRES / SQLITE** 等官方扩展。
- 核心向量相似函数（`ARRAY_COSINE_SIMILARITY` 等）无需索引即可用。

## 10. 明确不支持

| 特性 | 说明 |
|------|------|
| `FOREACH` | 语法不存在 |
| `REMOVE` 子句 | 无（仅 SET/DELETE） |
| `SET x += {...}` | 无 |
| `CALL { ... }` 开放子查询 | 仅 EXISTS/COUNT 形态 |
| 模式推导 `[p = (a)-->(b) \| expr]` | 无 |
| 列表推导 `[x IN list \| f(x)]` | TCK 用例均预期报错 |
| `LOAD CSV`（Cypher 形态） | 由 `LOAD FROM` 取代 |
| `!=`、链式比较、RETURN 非末尾 | 解析器显式报错 |
| 标签表达式 `:A & :B`、`:A \| :B` 之外的组合 | 仅 AND/OR 两种 |
| `(n $props)` 模式参数 map | 属性 map 仅限字面量 |
| `CREATE INDEX` / `ANALYZE` | 文档声称但语法不存在；索引仅经扩展函数创建 |
