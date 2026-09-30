# 查询语句对比分析：当前项目 vs ladybug vs neug

> 对比对象：
> - **当前项目**（linkrs，Rust，Nebula 语句 + Cypher 混合方言）—— 依据 `crates/graphdb-query/**` 与 `docs/release/0x_*.md`
> - **ladybug**（`ref/ladybug`，Kùzu 衍生 openCypher）—— 详见 [ladybug_query_statements.md](./ladybug_query_statements.md)
> - **neug**（`ref/neug`，openCypher）—— 详见 [neug_query_statements.md](./neug_query_statements.md)

## 1. 总体定位差异

| 维度 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| 方言基础 | Nebula Graph 语句（GO/FETCH/LOOKUP/管道 `\|`）+ openCypher（MATCH/RETURN/MERGE）混合 | 纯 openCypher + `iC_` 扩展 | 纯 openCypher + `nEUG_` 扩展 |
| 语法定义 | 手写词法/递归下降（`parser/lexing`、`parser/parsing`） | ANTLR4 `Cypher.g4`（925 行） | ANTLR4 `Cypher.g4`（895 行） |
| 查询组合 | **管道 `\|` 为主**，另有 UNION/INTERSECT/MINUS、多语句封装 | `UNION [ALL]` + `WITH` 多段查询 | `UNION [ALL]` + `WITH` 多段查询，另有 `CALL (args) {union}` 共享作用域扩展 |
| 测试佐证 | `crates/graphdb-query/tests/**`、`tests/e2e/**` | 591 个 `.test` 文件（含 TCK） | **无测试**（仅语法与注释示例） |

三者在"读-投影-更新"的核心骨架上一致（MATCH/WHERE/RETURN/WITH/CREATE/MERGE/SET/DELETE），差异主要在**方言语句族、连接提示形态、索引/搜索语句化程度**。

## 2. 读子句对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| `MATCH` / `OPTIONAL MATCH` | ✅（连续 MATCH 合并、尾随 WHERE） | ✅ | ✅ |
| 属性 map 内联 / 改写 WHERE | ✅ | ✅ | ✅ |
| 逗号多模式（可不连通） | ✅ | ✅ | ✅ |
| **连接顺序提示** | ✅ `USING JOIN BINARY(e1,e2)` / `USING JOIN MULTIWAY(...)` | ✅ `HINT a JOIN b ... MULTI_JOIN ...` | ✅ 同 ladybug `HINT` 形态 |
| `UNWIND` | ✅ | ✅ | ✅ |
| 查询内 `CALL ... [WHERE] [YIELD]` | ✅（`db_version()` 等少量函数 + 标量回退） | ✅（20+ 表函数） | ✅（表函数白名单） |
| `LOAD FROM` 扫描读入 | ✅（File/Glob 源） | ✅（文件/列表/GLOB/查询/参数/附库/表函数） | ✅（文件/列表/GLOB/查询/变量/表函数） |
| `WHERE` 中内联模式谓词 | ❌（`NodePattern.predicates` 解析器从不填充） | ✅ | ✅（改写为 EXISTS） |
| `EXISTS { }` / `COUNT { }` 子查询 | ✅ `EXISTS { MATCH }`；另有 `SUBQUERY {}`、`IN { query }` | ✅ 两者 | ✅ 两者 |
| Nebula `GO` / `LOOKUP` / `FETCH` / `GET SUBGRAPH` | ✅（独有） | ❌ | ❌ |
| `FIND SHORTEST/ALL PATH` | ✅（独有，双向 BFS/Dijkstra） | ❌（用变长模式 `* SHORTEST` 表达） | ❌（同左） |
| `YIELD` 独立语句 / 管道 `WHERE`/`COLLECT` 阶段 | ✅（独有） | ❌ | ❌ |
| `WITH` CTE / `WITH RECURSIVE` | ✅（独有，含递归 CTE） | ❌ | ❌ |

## 3. 查询组合与集合运算

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| 管道 `\|` 链式 | ✅ | ❌ | ❌ |
| `UNION` / `UNION ALL` | ✅ | ✅ | ✅ |
| `INTERSECT` / `MINUS` | ✅ | ❌ | ❌ |
| `EXCEPT` 关键字 | ❌（用 `MINUS`） | ❌ | ❌ |
| 共享作用域联合 `CALL (x) { ... }` | ❌ | ❌ | ✅（neug 独有） |
| `RETURN` 位置约束 | 管道各阶段均可 RETURN | 强制末尾 | 强制末尾 |

## 4. 投影子句对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| `DISTINCT` / `ORDER BY` / `SKIP` / `LIMIT` | ✅ | ✅ | ✅ |
| 隐式聚合分组（无 GROUP BY） | ✅（RETURN 内聚合） | ✅ | ✅ |
| **显式 `GROUP BY`**（含 `ROLLUP/CUBE/GROUPING SETS`） | ✅（独有） | ❌ | ❌ |
| `HAVING` | ✅ | ❌（仅 WITH 尾随 WHERE） | ❌ |
| `SAMPLE n` | ✅（已解析，无测试） | ❌ | ❌ |
| `WITH ... WHERE` | ✅ | ✅ | ✅ |
| 递归 CTE `WITH RECURSIVE` | ✅ | ❌ | ❌ |
| `WITH` 项强制别名 | ✅ | ✅ | ✅ |
| `SKIP/LIMIT` 须字面量/参数 | — | ✅ | ✅ |
| 星号展开 `RETURN *` / `n.*` | ✅ | ✅ | ✅ |

## 5. 更新子句对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| Cypher `CREATE (pattern)` | ✅ | ✅ | ✅ |
| Nebula `INSERT VERTEX/EDGE [IF NOT EXISTS]` | ✅（独有） | ❌ | ❌ |
| `MERGE (pattern) [ON MATCH/CREATE SET]` | ✅（裸 `SET` 子句仅校验后丢弃——**文档/实现偏差**） | ✅ | ✅ |
| `MERGE VERTEX/EDGE ON ...`（upsert 形态） | ✅（独有） | ❌ | ❌ |
| `SET prop = expr` 独立语句 | ✅ | ✅（仅随查询内） | ✅（仅随查询内） |
| `REMOVE prop / REMOVE n:Label` | ✅（独有，点/标签删除） | ❌（无 REMOVE） | ❌（无 REMOVE） |
| `+=` 属性合并 | ❌ | ❌ | ❌ |
| `[DETACH] DELETE` | ✅ + Nebula `DELETE VERTEX/EDGE ... WITH EDGE` | ✅ | ✅ |
| `MATCH ... DELETE` | ✅ | ✅ | ✅ |
| `MATCH ... SET/REMOVE` | ❌（仅 MATCH...DELETE/RETURN） | ❌ | ❌ |
| `UPDATE/UPSERT VERTEX/EDGE` | ✅（独有） | ❌ | ❌ |
| `FOREACH` | ❌ | ❌ | ❌ |
| `COPY` 导入导出 | ✅ `COPY VERTEX/EDGE FROM/TO ... [BY COLUMN]` | ✅（文件/查询/列式 + EXPORT DATABASE） | ✅（同 ladybug） |

**要点**：三者均不支持 `FOREACH` 与 `SET +=`；`REMOVE` 是当前项目相对两个参考项目的**净增能力**；ladybug/neug 的 `SET a = {...}` 整 map 覆盖形态当前项目无对应。

## 6. 模式与变长路径对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| 基础模式（多标签/多边类型/无向/路径变量） | ✅ | ✅ | ✅ |
| **命名路径绑定 `p = (a)-[]->(b)`** | ❌ | ✅ | ✅ |
| 变长 `*n` / `*a..b` / `*..b` | ✅ | ✅ | ✅ |
| `TRAIL` / `ACYCLIC` 语义 | ✅ | ✅ | ✅ |
| `SHORTEST` / `ALL SHORTEST` | ✅（含 `ALLSHORTESTPATHS` 别名） | ✅ | ✅ |
| 加权最短 `WEIGHTED(w)` / `WSHORTEST(w)` | ✅ `*WEIGHTED(w)` | ✅ `* WSHORTEST(w)` | ✅ 同 ladybug |
| 递归推导式 `*(v,r \| WHERE \| {proj},{proj})` | ✅ | ✅ | ✅ |
| 边属性内过滤 `[e* {p=v}]` | ✅（解析支持） | ✅（经 WHERE/推导式） | ✅（原生注释示例） |
| `cost(e)` / `nodes(p)` / `rels(p)` 等路径函数 | ✅ `shortest_path, variable_length_path` + `nodes/relationships/is_trail/is_acyclic` | ✅ | ✅ |
| 标签 OR `:A \| :B` | —（Nebula 标签模型） | ✅ | ❌ |
| 独立 `FIND PATH` 语句 | ✅ | ❌ | ❌ |

三者在变长/最短路径语义关键字上高度趋同（WALK/TRAIL/ACYCLIC/SHORTEST/加权），差异仅在关键字拼写（`WEIGHTED(w)` vs `WSHORTEST(w)`）与是否支持独立最短路径语句。

## 7. 表达式与函数对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| 比较/逻辑/算术 | ✅（`==` 与 `=` 均可、`**` 幂） | ✅（`^` 幂，`!=` 拒绝） | ✅（`^` 幂，`!=` 拒绝） |
| `=~` 正则操作符 | ✅ | ✅ | ❌（transform 拒绝） |
| 列表切片 `list[a:b]` | ✅ | ✅ | ❌ |
| 位运算 `\| & >> <<` | ✅ | ✅（已注册） | ⚠️ 语法有、函数未注册 |
| 类型转换 `expr::type` | ✅（`::` 形态） | ✅（`CAST(x AS T)`） | ✅（`CAST`） |
| JSON 路径 `-> ->> #> #>>` | ✅（独有） | 经 JSON 扩展函数 | 经 JSON 扩展函数 |
| `CASE` / 量词 `ALL/ANY/NONE/SINGLE` / Lambda | ✅ | ✅ | ✅（量词函数本快照未注册） |
| 列表推导 `[x IN list \| expr]` | ✅ | ❌（TCK 预期报错） | ❌（语法缺失） |
| 变量作用域语法 `$^.tag` / `$$.tag` / `$-` | ✅（Nebula 独有） | ❌ | ❌ |
| 聚合函数广度 | ✅ 20+（percentile、std、bool_and、vec_sum 等） | 7 个核心 | 7 个核心 |
| 窗口函数 `OVER(PARTITION BY ...)` | ✅（解析/执行存在，缺测试） | ❌ | ❌ |
| 地理函数（ST_*） | ✅ 30+ | ❌ | ❌ |
| 日期时间函数 | ✅ 广 | ✅ 广 | ⚠️ 少（DATE_PART 等） |
| 列表/Map/Struct 函数 | ✅ 广 | ✅ 广 | ⚠️ 少（多未注册） |

## 8. 搜索（全文/向量）与 DDL/DCL 对比

| 能力 | 当前项目 | ladybug | neug |
|------|---------|---------|------|
| 全文索引 | ✅ **一等语句**：`CREATE/DROP/ALTER FULLTEXT INDEX`、`SEARCH INDEX ... MATCH ... [YIELD/WHERE/ORDER BY]`（BM25/tantivy） | ⚠️ 经扩展 `CALL CREATE_FTS_INDEX/QUERY_FTS_INDEX` | ⚠️ 同左（扩展函数） |
| 向量索引 | ✅ **一等语句**：`CREATE/DROP VECTOR INDEX`、`SEARCH VECTOR ... WITH vector=[...] [THRESHOLD] [YIELD]`、`LOOKUP VECTOR`、`MATCH VECTOR` | ⚠️ 经扩展 `CALL CREATE/QUERY_VECTOR_INDEX` | ⚠️ 同左 |
| 向量相似标量函数 | ✅ `cosine_similarity` 等 13+ | ✅ `ARRAY_COSINE_SIMILARITY` 等 | ⚠️ 仅扩展 |
| GDS 图算法 | ⚠️ 函数级（`pagerank, bfs, connected_components, shortest_path`） | ⚠️ 扩展（ALGO：PAGE_RANK/K_CORE/SCC） | ⚠️ 扩展（GDS 同名） |
| `CREATE INDEX`（普通属性索引） | ✅ `CREATE TAG/EDGE INDEX` | ❌（仅 FTS/VECTOR 扩展索引） | ❌（同左） |
| DDL 体系 | TAG/EDGE/SPACE、TTL、SERIAL、`AS (query)` 建表、`SHOW CREATE`、`MIGRATE PLAN/EXECUTE` | NODE/REL TABLE、`CREATE TYPE/SEQUENCE/MACRO/COMMENT`、`ALTER` 全套 | 同 ladybug + 基数/`storage_direction` 表选项 |
| DCL | ✅ USER/ROLE/GRANT/REVOKE/CHANGE PASSWORD（5 级角色） | ✅ `CREATE USER/ROLE`（经扩展 transformer） | ❌（语法无） |
| 事务 | `BEGIN [READ ONLY/WRITE] / COMMIT / ROLLBACK [TO SAVEPOINT] / SAVEPOINT / RELEASE` | `BEGIN TRANSACTION [READ ONLY] / COMMIT / ROLLBACK / CHECKPOINT` | 同 ladybug |
| 多库/多图 | ✅ `USE`、`ATTACH/DETACH DATABASE`、`CREATE GRAPH` | ✅ + `EXPORT/IMPORT DATABASE`、`CREATE/USE GRAPH` | ✅ + `EXPORT/IMPORT DATABASE` |
| 扩展管理 | ✅ `LOAD/INSTALL/UNINSTALL/UPDATE EXTENSION` | ✅ `INSTALL [FORCE] ... FROM` | ✅ `LOAD/INSTALL/UNINSTALL [EXTENSION]` |
| `EXPLAIN/PROFILE` | ✅（含 `FORMAT=TABLE\|DOT`、`ANALYZE`） | ✅（含 `EXPLAIN LOGICAL`） | ✅（含 `EXPLAIN LOGICAL`） |

## 9. 能力矩阵小结

### 当前项目独有（参考项目没有）

1. Nebula 语句族：`GO`、`LOOKUP`、`FETCH PROP ON`、`GET SUBGRAPH`、`FIND PATH`、`YIELD` 独立语句、管道 `|` 阶段（`WHERE`/`COLLECT` 阶段化）、`$^/$$/$-` 变量语法。
2. 显式 `GROUP BY`（含 ROLLUP/CUBE/GROUPING SETS）+ `HAVING`、递归 CTE（`WITH RECURSIVE`）。
3. `INTERSECT`/`MINUS` 集合运算；`REMOVE` 子句；`UPDATE`/`UPSERT VERTEX|EDGE`、`INSERT ... IF NOT EXISTS`。
4. 全文/向量索引的**一等 DDL+查询语句**（参考项目降级为扩展 `CALL` 函数）。
5. 地理 ST_* 函数、窗口函数、JSON 路径操作符、`::` 转换、SAVEPOINT。
6. DCL 角色体系、`SHOW CREATE`、`MIGRATE PLAN/EXECUTE` 迁移语句。

### 参考项目有、当前项目缺（按价值排序）

| 优先级 | 能力 | 来源 | 说明 |
|--------|------|------|------|
| 高 | **命名路径绑定 `p = (a)-[]->(b)`** | 两者 | 变长/最短查询取整条路径的惯用写法；当前 `PathPattern` 已有 AST 但解析器无 `=` 绑定 |
| 高 | `WHERE` 内联模式谓词 `WHERE (a)-[:k]->(b)` | 两者 | 等价 EXISTS 的便捷写法；`NodePattern.predicates` 字段已预留但从未填充 |
| 高 | `LOAD WITH HEADERS (列定义)` + 查询作为扫描源 | 两者 | 当前 `LOAD FROM` 仅 File/Glob；补 `(查询)` 源与列定义可打通导入管道 |
| 中 | `CALL option = value` 会话/扩展配置设置 | 两者 | 与当前 `UPDATE CONFIGS` 部分重叠，可评估是否合并 |
| 中 | `SET a = {map}` 整属性 map 覆盖 | 两者 | 当前 SET 仅逐属性赋值 |
| 中 | `CREATE MACRO` 默认参数 | 两者 | 当前宏已支持默认参数形态（`sep = '-'`），此项实际已对齐 |
| 低 | `EXPLAIN LOGICAL`、`PROJECT_GRAPH` 投影图 | ladybug | 按需 |
| 低 | `CALL (x) { union }` 共享作用域联合 | neug | 当前递归 CTE/管道已覆盖多数场景 |

### 参考项目缺失、当前项目无需回退

- `FOREACH`、`SET +=`、`GROUP BY` 关键字、`CALL {}` 开放子查询、列表推导 —— 三者皆无或当前项目更强。
- 链式比较与 `!=` 拒绝：当前项目支持 `!=`/`==`，与 Nebula 习惯一致，保留即可。
- ladybug 的 `CREATE INDEX` 不存在（其文档声称有属文档陈旧）；当前项目普通属性索引 DDL 更完整。

### 当前项目的实现/文档偏差（对比中暴露）

1. `DELETE TAG ...` 文档有、解析器无；`SHOW INDEXES`/`SHOW STATS` 枚举有、解析分支无。
2. `LOOKUP FULLTEXT`/`MATCH FULLTEXT`/`SHOW|DESCRIBE FULLTEXT INDEX` AST+planner 存在但顶层分发不可达。
3. `MERGE (pattern)` 后裸 `SET` 子句仅校验后丢弃。
4. `SAMPLE <n>` 与窗口函数缺测试覆盖。

## 10. 结论

- **语法骨架**：三者同源 openCypher 核心（MATCH/WHERE/RETURN/WITH/UNION/CREATE/MERGE/SET/DELETE + 变长路径语义关键字），当前项目叠加 Nebula 方言形成混合体系，语句覆盖面整体**大于**两个参考项目。
- **主要差距**集中在 Cypher 惯用便捷形态：命名路径绑定、WHERE 内联模式谓词、`LOAD FROM` 扫描源扩展；这三项均属解析层补强，AST/执行侧多已具备承载结构。
- **搜索语句化**是当前项目的架构优势：ladybug/neug 将 FTS/VECTOR 放在扩展 `CALL` 内，当前项目提升为一等 DDL/DQL 语句，与 DDL 体系一致性更好。
- **验证成熟度**：ladybug 有 591 个测试文件（含 TCK）可作回归参照；neug 无测试且存在"语法可解析但函数未注册"的断裂（位运算、量词），引用其特性时应以注册表为准而非语法。
