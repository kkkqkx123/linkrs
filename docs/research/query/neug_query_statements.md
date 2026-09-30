# neug 支持的查询语句清单

> 来源：`ref/neug`（C++ 图数据库，`include/` + `src/`，openCypher 方言 + `nEUG_` 前缀扩展）。
> 权威语法：`ref/neug/src/compiler/antlr4/Cypher.g4`（895 行）；关键字：`src/compiler/antlr4/keywords.txt`（106 个）；
> 语句分发：`src/compiler/parser/transformer.cpp`；类型/子句枚举：`include/neug/compiler/common/enums/{statement_type,clause_type}.h`。
> 注：neug 仓库**无测试文件**，示例来自语法注释、binder/planner 文档注释与头文件示例；与之共享 openCypher 核心规则的 `ref/ladybug` 有 591 个测试文件可用于共享语法佐证，但二者代码库不同，不可混用归属。

## 1. 顶层语句一览

语句根规则 `oC_Statement`（`Cypher.g4:8-28`），语句间 `;` 分隔，支持 `//` 与 `/* */` 注释。

| # | 语句 | 语法示例 | 说明 |
|---|------|---------|------|
| 1 | Cypher 查询 | `MATCH (n:Person) RETURN n.name LIMIT 10` | 核心查询 |
| 2 | `EXPLAIN [LOGICAL] <stmt>` | `EXPLAIN LOGICAL MATCH (n:Person) RETURN n` | 计划展示 |
| 3 | `PROFILE <stmt>` | `PROFILE MATCH (n) RETURN count(n)` | 计划 + 统计 |
| 4 | `CREATE NODE TABLE` | `CREATE NODE TABLE Person(id INT64 PRIMARY KEY, name STRING DEFAULT 'unknown')` | 主键必填；支持 `IF NOT EXISTS`、`AS <query>` |
| 5 | `CREATE REL TABLE [GROUP]` | `CREATE REL TABLE KNOWS(FROM Person TO Person, since TIMESTAMP, MANY_ONE) WITH (storage_direction='FWD')` | 基数 `ONE_ONE/ONE_MANY/MANY_ONE/MANY_MANY`；多 FROM..TO ⇒ 边表组 |
| 6 | `CREATE SEQUENCE` | `CREATE SEQUENCE seq INCREMENT BY 5 START WITH 10 MINVALUE 1 MAXVALUE 1000 NO CYCLE` | |
| 7 | `CREATE TYPE` | `CREATE TYPE price AS DECIMAL(10,2)` | 用户自定义类型 |
| 8 | `CREATE MACRO` | `CREATE MACRO plus(a, b := 1) AS a + b` | 支持默认参数 |
| 9 | `DROP TABLE/SEQUENCE` | `DROP TABLE IF EXISTS Person`、`DROP SEQUENCE seq` | |
| 10 | `ALTER TABLE` | `ADD/DROP [IF NOT EXISTS/IF EXISTS] prop`、`RENAME TO t2`、`RENAME old TO new` | |
| 11 | `COMMENT ON` | `COMMENT ON TABLE Person IS 'people table'` | |
| 12 | `COPY ... FROM` | `COPY Person FROM 'person.csv' (header, delimiter := '\|', skip := 1)` | 源可为文件/列表/GLOB/查询/变量/表函数 |
| 13 | `COPY ... FROM ... BY COLUMN` | `COPY Person FROM ('x.csv') BY COLUMN` | 列式导入 |
| 14 | `COPY (query) TO` | `COPY (MATCH (n:Person) RETURN n.name) TO 'out.csv' (header)` | 查询导出 |
| 15 | `CALL`（独立） | `CALL timeout = 60000`（扩展选项）或 `CALL CSV_SCAN('f.csv', header := true)` | 表函数须独占语句 |
| 16 | 事务控制 | `BEGIN TRANSACTION [READ ONLY]`、`COMMIT`、`ROLLBACK`、`CHECKPOINT` | |
| 17 | 扩展管理 | `LOAD 'path/ext.neug_extension'`、`INSTALL vector`、`UNINSTALL EXTENSION 'fts'` | |
| 18 | `EXPORT/IMPORT DATABASE` | `EXPORT DATABASE 'backup.db' (FORMAT := 'csv')` | |
| 19 | `ATTACH/DETACH/USE DATABASE` | `ATTACH '/data/other.db' AS other (DBTYPE NEUG, readonly := true)` | 附库须有别名 |

## 2. 查询组合结构

```
RegularQuery := SingleQuery (UNION [ALL] SingleQuery)*
             |  (RETURN)+ SingleQuery          -- 解析错误路径（RETURN 必须末尾）
             |  CallUnionQuery                 -- CALL (args) { ... } 扩展形态
SingleQuery  := SinglePartQuery | MultiPartQuery
SinglePart   := 读子句* RETURN | 读子句* 更新子句+ RETURN?
MultiPart    := QueryPart+ SinglePart          （QueryPart := 读子句* 更新子句* WITH）
```

- 查询必须以 `RETURN` 结尾，或以更新子句结尾（结果为空集）。
- `UNION/UNION ALL`：列数与**数据类型必须完全一致**；两种形态不可混用。
- **`CALL (args) { query UNION query } [query]`（neug 扩展）**：共享作用域的联合子查询，公共表达式由各分支共享：
  `CALL (x) { MATCH (a:T) WHERE a.v = x RETURN a.v UNION MATCH (b:T) WHERE b.v = x RETURN b.v } RETURN *`

## 3. 读子句

子句枚举 `ClauseType`：`MATCH, UNWIND, IN_QUERY_CALL, TABLE_FUNCTION_CALL, GDS_CALL, LOAD_FROM`。

### 3.1 MATCH / OPTIONAL MATCH

```
[OPTIONAL] MATCH <pattern> [WHERE <expr>] [HINT <join-node>]
```

- 内联属性 map 改写为 `WHERE` 等值谓词；自环 `(a)-[e]->(a)` 改写为独立终点 + `id(a)=id(b)`。
- 多标签空格分隔（AND）：`MATCH (n:Person:Student)`；**不支持 `:A | :B` 标签 OR**。
- 多类型边：`-[e:KNOWS|:LIKES]->`。
- **HINT 连接提示（非标准）**：`HINT a JOIN e JOIN b`、`HINT a MULTI_JOIN b`；叶子必须为变量名且覆盖全部命名模式，不连通模式报错。
- 逗号分隔多模式（可不连通）：`MATCH (a:person)-[:knows]->(b), (a)-[:knows]->(c)`。

### 3.2 UNWIND

`UNWIND <expr> AS <var>` — 必须为列表类型，如 `UNWIND [1,2,3,4] AS x RETURN x`、`UNWIND $ids AS id MATCH ...`。

### 3.3 查询内 CALL（表函数）

```
CALL <func>(args) [WHERE <expr>] [YIELD name [AS alias], ...]
```

- `CALL CSV_SCAN('data.csv', header := true) YIELD id, name WHERE id > 10 RETURN *`
- 仅接受目录中的表函数 / 算法函数。

### 3.4 LOAD FROM（读入扫描子句）

```
LOAD [WITH HEADERS (colDef,...)] FROM <scanSource> [(options)] [WHERE <expr>]
scanSource := 文件 | [文件列表] | GLOB('pat') | (查询) | $变量 | var.schema | 表函数
```

- `LOAD FROM 'people.csv' (header, delimiter := ',') RETURN *`
- `LOAD WITH HEADERS (id INT64, name STRING) FROM ['a.csv','b.csv'] (header) WHERE id > 0 RETURN *`
- `LOAD FROM (MATCH (n:Person) RETURN n) RETURN *`

## 4. 投影：RETURN / WITH

```
(RETURN|WITH) [DISTINCT] <items> [ORDER BY ...] [SKIP n] [LIMIT n] [WHERE pred]   -- WHERE 仅 WITH
```

- `RETURN *, 1 AS one`（星号 + 附加项）；`n.*` / `s.*` 属性展开。
- 聚合 `COUNT/SUM/AVG/MIN/MAX/COLLECT`（内含 `DISTINCT`），隐式分组（无 `GROUP BY` 关键字）。
- 约束：`WITH` 项必须 `AS` 别名；作用域无变量时禁止 `*`；结果列名唯一；`SKIP/LIMIT` 须字面量或参数；ORDER BY 不可作用于 NODE/REL/RECURSIVE_REL/INTERNAL_ID/LIST/STRUCT/MAP/UNION；嵌套聚合仅允许引用在作用域别名。
- `ORDER BY` 作用域：有聚合/DISTINCT 时仅可见投影项，否则可见投影前作用域。

## 5. 更新子句

枚举：`INSERT(=CREATE), MERGE, SET, DELETE_`。

- **CREATE**：`CREATE (a:Person {name:'Alice'})`；主键必须提供或有默认值；不支持多标签点、无向边、递归边 `[r*]` 的创建。
- **MERGE**：`MERGE <pattern> [ON MATCH SET ...] [ON CREATE SET ...]`；匹配键改写为谓词，内部 `__existence/__distinct` 标记。
- **SET**：仅 `SET propExpr = expr, ...` 一种形式；LHS 必须是点/边模式属性；**无 `+=`、无标签增删、无 `REMOVE`**。
- **[DETACH] DELETE**：`MATCH (n:Person {id:1}) DETACH DELETE n`；边表不支持 DETACH DELETE；不支持无向边删除。

## 6. 模式、路径与变长 / 最短路径

### 6.1 基础模式

```
Pattern      := PatternPart (',' PatternPart)*
PatternPart  := Variable '=' ? AnonymousPatternPart
NodePattern  := '(' Variable? NodeLabels? Properties? ')'      -- NodeLabels 空格分隔（AND），无 '|'
RelPattern   := '<-' '-' Detail? '-' | '-' Detail? '-' '>' | '-' Detail? '-'
Detail       := '[' Variable? RelTypes? RecursiveDetail? Properties? ']'
RelTypes     := ':' Name (('|'? ':'? Name))*                  -- 边类型支持 '|'
```

- 路径变量：`MATCH p = (a:Person)-[:KNOWS]->(b) RETURN p`。
- 属性 map 值可为任意表达式（含参数值），但**整体 `$param` map 形态明确不支持**（语法注释注明长期决策）。
- 无向边存为 `BOTH` 方向。

### 6.2 变长 / 最短路径

```
'*' [RecursiveType] [RangeLiteral] [RecursiveComprehension]
RecursiveType := [ALL] WSHORTEST '(' PropertyKey ')' | SHORTEST | ALL SHORTEST | TRAIL | ACYCLIC
RecursiveComprehension := '(' Var ',' Var ('|' WHERE)? ('|' {nodeProj}, {relProj})? ')'
```

| 变体 | 示例 |
|------|------|
| 遍历 walk（默认） | `MATCH (a)-[:KNOWS*]->(b)` |
| 范围 / 固定 | `*2..5`、`*3`、`*..7` |
| TRAIL / ACYCLIC | `* TRAIL`、`* ACYCLIC` |
| 最短 / 全部最短 | `* SHORTEST`、`* ALL SHORTEST`（下界须为 1） |
| 加权最短 | `-[e:KNOWS* WSHORTEST(distance)]->(b)` + `cost(e)` |
| 全部加权最短 | `* ALL WSHORTEST(distance)` |
| 边属性过滤 | `[e* {date=1999-01-01}]` |
| 递归推导式 | `[e* (n, r \| WHERE n.age > 25 \| {n.age}, {r.since})]` |

- 路径语义枚举：`WALK / TRAIL / ACYCLIC`；查询关系类型：`VARIABLE_LENGTH_WALK/TRAIL/ACYCLIC、SHORTEST、ALL_SHORTEST、WEIGHTED_SHORTEST、ALL_WEIGHTED_SHORTEST`。
- 权值属性须为数值类型；`WHERE` 谓词不可同时依赖中间点与边；字面假谓词使模式为空。
- 路径函数：`NODES, RELS, RELATIONSHIPS, PROPERTIES, IS_TRAIL, IS_ACYCLIC, LENGTH, COST`。

## 7. 表达式与操作符

优先级（高→低，`Cypher.g4:443-519`）：`.` 属性 / `[i]` 索引 → `STARTS WITH/ENDS WITH/CONTAINS/IN/IS [NOT] NULL` → 一元 `-`、`!` → `^` → `* / %` → `+ -` → `>> <<` → `&` → `|` → 比较（不可链式）→ `NOT` → `AND` → `XOR` → `OR`。

| 操作符 | 状态 |
|--------|------|
| `= <> < <= > >=`、`OR XOR AND NOT` | 支持 |
| `+ - * / % ^`、一元 `-` | 支持 |
| `x IN y` | 支持（改写 `LIST_CONTAINS`） |
| `list[i]` | 支持（`LIST_EXTRACT`） |
| `IS NULL / IS NOT NULL` | 支持 |
| `!=` | 拒绝（提示用 `<>`） |
| 链式 `a=b=c` | 拒绝 |
| `list[a:b]` 切片 | 拒绝（运行时解析错误） |
| `=~` 正则 | 拒绝（transform 阶段报错） |
| `\| & >> << !` 位运算/阶乘 | 语法可解析为函数，但函数未在 `function_collection.cpp` 注册 → 绑定失败 |

其他表达式形态：
- 字面量：数字、字符串、`TRUE/FALSE/NULL`、列表 `[1,2,3]`、struct map `{k:1}`、参数 `$name/$0`。
- `CASE`（简单 / 通用两种形态）。
- 内联模式谓词 `WHERE (a)-[:KNOWS]->(b)` 改写为 EXISTS 子查询。
- `EXISTS { MATCH ... }` / `COUNT { ... }` 子查询（内部仅允许 MATCH+WHERE+HINT；`COUNT{}` ⇒ `COUNT(*)`，`EXISTS{}` ⇒ `COUNT(*) > 0`）。
- 量词 `ALL/ANY/NONE/SINGLE (x IN list WHERE ...)`（改写为函数 + lambda；本快照函数未注册）。
- Lambda `x -> expr`、`(x,y) -> expr`；命名可选参数 `f(x := 1)`。

## 8. 内置函数（`function_collection.cpp:80-155` 权威注册表）

- **聚合**：`COUNT_STAR, COUNT, SUM, AVG, MIN, MAX, COLLECT`（支持 `DISTINCT`）。
- **算术/比较**：`+ - * / % ^ ABS NEGATE`、`EQUALS NOT_EQUALS GREATER_THAN ...`。
- **列表**：`LIST_CREATION, LIST_EXTRACT, LIST_CONTAINS/LIST_HAS`（实现但未注册：`LIST_CONCAT, LIST_UNIQUE, ALL/ANY/NONE/SINGLE`）。
- **字符串**：`LOWER/TOLOWER/LCASE, UPPER/TOUPPER/UCASE, CONTAINS, ENDS_WITH/SUFFIX, STARTS_WITH, REVERSE`。
- **转换/时间**：`CAST, TO_DATE/DATE, TIMESTAMP, TO_INTERVAL/INTERVAL/DURATION, DATE_PART/DATEPART`。
- **结构体**：`STRUCT_EXTRACT`。
- **路径/模式改写**：`ID, START_NODE, END_NODE, LABEL, LABELS, COST, NODES, RELS, RELATIONSHIPS, PROPERTIES, IS_TRAIL, IS_ACYCLIC, LENGTH`。
- **表/导出函数（CALL 调用）**：`show_loaded_extensions, CSV_SCAN, COPY_CSV`。
- 实现未注册：`POW, GEN_RANDOM_UUID, CURRVAL, NEXTVAL`（`nextval` 仅用于 DEFAULT 编译期求值）等。

### 扩展入口（`src/compiler/extension/extension_entries.cpp`，二进制经 `.neug_extension` 加载）

| 扩展 | 入口名 |
|------|--------|
| FTS（全文） | `STEM, QUERY_FTS_INDEX, CREATE_FTS_INDEX, DROP_FTS_INDEX` |
| VECTOR（向量索引） | `QUERY_VECTOR_INDEX, CREATE_VECTOR_INDEX, DROP_VECTOR_INDEX` |
| GDS | `PAGE_RANK, K_CORE_DECOMPOSITION, STRONGLY/WEAKLY_CONNECTED_COMPONENTS[_KOSARAJU]` |
| JSON（+ JSON 类型） | `TO_JSON, JSON_EXTRACT, JSON_ARRAY_LENGTH, JSON_KEYS, JSON_VALID, ...` |
| DUCKDB / DELTA / ICEBERG | `CLEAR_ATTACHED_DB_CACHE, DELTA_SCAN, ICEBERG_SCAN/METADATA/SNAPSHOTS` |

无 `CREATE INDEX` / `CREATE CONSTRAINT` 语法；索引仅经扩展函数管理。

## 9. 数据类型（DDL / CAST / CREATE TYPE）

- 整型：`INT8/16/32(INT)/64/128`、`UINT8/16/32/64`、`SERIAL`
- 浮点：`DOUBLE/FLOAT8`、`FLOAT/FLOAT4/REAL`
- 数值：`DECIMAL(p,s)/NUMERIC(p,s)`；布尔：`BOOLEAN/BOOL`
- 文本：`STRING`、`VARCHAR(n)`；二进制：`BLOB/BYTEA`、`UUID`
- 时间：`DATE`、`TIMESTAMP[_NS/_MS/_SEC/_TZ]`、`INTERVAL/DURATION`
- 嵌套：`T[]`（列表）、`T[k]`（定长数组）、`STRUCT(...)`、`MAP(...)`、`UNION(...)`
- 其他：`INTERNAL_ID`、`CREATE TYPE` 定义的别名

## 10. 非查询语句示例（要点）

```cypher
-- DDL
CREATE NODE TABLE Person (id INT64, name STRING DEFAULT 'unknown', PRIMARY KEY (id));
CREATE REL TABLE GROUP KNOWS (FROM Person TO Person, FROM Person TO Student, MANY_MANY);
CREATE TYPE price AS DECIMAL(10,2);
COMMENT ON TABLE Person IS 'people table';

-- 批量导入导出
COPY Person FROM GLOB('data/*.csv') (header);
COPY Person FROM CSV_SCAN('f.csv', header := true);
COPY (MATCH (n:Person) RETURN n.name) TO 'out.csv' (header);
EXPORT DATABASE 'backup.db' (FORMAT := 'csv');

-- 宏 / 独立 CALL / 选项
CREATE MACRO plus(a, b := 1) AS a + b;
CALL timeout = 60000;
CALL show_loaded_extensions();

-- 事务
BEGIN TRANSACTION READ ONLY;  COMMIT;  ROLLBACK;  CHECKPOINT;

-- 扩展与多库
INSTALL vector;  LOAD 'path/ext.neug_extension';
ATTACH '/data/other.db' AS other (DBTYPE NEUG, readonly := true);  USE other;
```

## 11. 明确不支持（语法缺失验证）

| 特性 | 证据 |
|------|------|
| `REMOVE` 子句 | 语法无规则，`ClauseType` 无 REMOVE |
| `FOREACH` | 缺失 |
| `CALL { ... }` 开放过程子查询 / `YIELD *` | 仅 `CALL func(...) [WHERE][YIELD]` 与 `CALL (…) { union }` |
| `GROUP BY`、`USING PERIODIC COMMIT` | 缺失（分组隐式） |
| `SET n += map`、`SET n:Label` | `SetItem` 仅 `prop = expr` |
| `CREATE INDEX / CREATE CONSTRAINT / UNIQUE` | 缺失（索引仅经扩展） |
| 链式比较、`!=` | 显式报错 |
| 正则 `=~`、列表切片 `list[a:b]` | transform 显式拒绝 |
| 模式参数 map `MATCH (n $props)` | 语法注释明确长期不支持 |
| 非字面量 `SKIP/LIMIT` | binder 限制 |
| 节点标签 OR `:A \| :B` | `NodeLabels` 仅空格分隔（ladybug 支持，neug 不支持） |
