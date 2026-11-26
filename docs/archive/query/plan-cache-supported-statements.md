# 查询计划缓存支持语句清单

本文列出当前 SQL 管道中会被查询计划缓存覆盖的语句。这里只讨论物理计划缓存，不包含 CTE 结果缓存、模板 AST 解析缓存或直接 API 层的其它缓存。

判定入口位于 `crates/linkrs-query/src/pipeline/prepared/classify.rs`：只读语句由 `is_read_only_cacheable()` 判定，写入语句由 `dml_op_from_stmt()` 产生 DML 事件，最终统一由 `is_plan_cacheable()` 决定是否允许存储计划。

## 当前支持的语句

| 类别 | 语句 | 复用方式 | 说明 |
|---|---|---|---|
| 图查询 | `MATCH`（不含 DELETE）、`GO`、`FETCH`、`LOOKUP`、`FIND PATH`、`GET SUBGRAPH` | 主计划缓存；分区计划可使用只读预绑定 memo | MATCH 带 DELETE 时不按只读缓存处理 |
| 组合查询 | `RETURN`、`WITH`、`YIELD`、`FILTER`、`COLLECT`、`UNWIND`、`GROUP BY` | 主计划缓存 | 作为独立语句或通过 PIPE / SET OPERATION 组合时均可覆盖 |
| 全文检索 | `SEARCH`、`LOOKUP FULLTEXT`、`MATCH FULLTEXT` | 主计划缓存 | 走 SQL 管道的检索语句 |
| 向量检索 | `SEARCH VECTOR`、`LOOKUP VECTOR`、`MATCH VECTOR` | 主计划缓存 | 走 SQL 管道的检索语句 |
| 元数据只读 | `SHOW`、`DESC/DESCRIBE`、`SHOW CREATE`、`SHOW USERS`、`SHOW ROLES`、`SHOW SESSIONS`、`SHOW QUERIES` | 主计划缓存 | `SHOW CONFIGS` 是执行层直接处理，见下文 |
| 索引元数据只读 | `SHOW FULLTEXT INDEX`、`DESCRIBE FULLTEXT INDEX`、`SHOW VECTOR INDEX`、`DESCRIBE VECTOR INDEX` | 主计划缓存 | DDL 改变 schema/index generation 后自动失效 |
| 直接写入 | `INSERT`、`DELETE`、`UPDATE`、`UPSERT`、`MERGE`、`SET`、`REMOVE` | shape 模板缓存优先；不可归一化时按完整文本缓存 | 字面量可改写为 `@__dml_N` 参数槽的语句复用率更高 |
| 数据型 CREATE | `CREATE (n:Tag {...})`、`CREATE (a)-[:Edge]->(b)` 等 Node/Edge/Path 形式 | shape 模板缓存优先 | 归类为 DML，不会触发 DDL 失效 |
| 批量写入 | `COPY` | 完整文本缓存 | 文件路径等字面量进入 key，不作 shape 归一化 |
| MATCH 写入 | `MATCH ... DELETE` | 完整文本缓存 | 归类为 DML，不进入只读 memo |
| 组合写入 | 含写入分支的 `PIPE`、`SET OPERATION` | 完整文本缓存 | 分支中只要产生 DML 事件即可缓存 |

## shape 归一化覆盖范围

以下写入语句在表达式可被安全重渲染时，会把字面量替换为参数槽，从而让相同 shape、不同字面值的语句复用同一计划：

- `INSERT`
- `DELETE`
- `UPDATE` / `UPSERT`
- `MERGE`
- `SET`
- `REMOVE`
- 数据型 `CREATE`

以下情况会退回完整文本缓存：

- `COPY`
- `MATCH ... DELETE`
- 含写入分支的组合语句
- 表达式无法忠实重渲染的写入语句，例如部分 IN / NOT IN、下标、JSON 路径、复杂赋值目标、带 YIELD 的 UPDATE 等

完全由显式参数构成的 DML（例如 `INSERT ... VALUES @id: (@name)`）也属于 shape 缓存范围，不再要求至少出现一个字面量。

## 特殊说明

### EXPLAIN 与 PROFILE

`EXPLAIN` / `PROFILE` 自身不作为可缓存语句存储。它们会按目标语句的源码 span 构造内层文本 key：

- 目标是只读语句时，可复用已有只读计划，也可为同一种直接执行语句 warming 缓存。
- 目标是 DML 或条件 MERGE 时，只允许命中已有计划，不写入缓存。
- `EXPLAIN ANALYZE` 与 `PROFILE` 的运行时统计仍在执行后即时叠加，不写入计划。

### SHOW CONFIGS 与会话命令

`SHOW CONFIGS` 在管道中直接返回配置结果，不进入计划缓存。`BEGIN`、`COMMIT`、`ROLLBACK`、`SAVEPOINT`、`RELEASE SAVEPOINT`、`LET`、extension 管理等会话命令在 API 层拦截，也不进入计划缓存。

### 条件 MERGE

依赖运行时 probe 决定分支的 MERGE 会绕过计划缓存，避免缓存命中后返回错误分支。普通 MERGE 仍可按 shape 或完整文本复用。

## 当前不支持缓存的语句

| 类别 | 语句 | 原因 |
|---|---|---|
| Schema DDL | `CREATE TAG/EDGE/SPACE/INDEX/SEQUENCE/MACRO/TYPE`、`ALTER`、`DROP` | 改变 catalog；执行后触发失效 |
| 维护 DDL | `CLEAR SPACE`、`CHECKPOINT`、`PURGE EXPIRED`、`IMPORT DATABASE`、`EXPORT DATABASE`、`ATTACH DATABASE`、`DETACH DATABASE` | 改变目录、存储布局或全局 attached 状态 |
| DCL | `CREATE USER`、`ALTER USER`、`DROP USER`、`CHANGE PASSWORD`、`GRANT`、`REVOKE` | 低频且安全状态敏感 |
| 事务控制 | `BEGIN`、`COMMIT`、`ROLLBACK`、`SAVEPOINT`、`RELEASE SAVEPOINT` | 会话状态操作，不属于物理计划复用 |
| 配置管理 | `UPDATE CONFIGS`、`SHOW CONFIGS` | 直接构造 intent/result，无物理计划 |
| 统计维护 | `ANALYZE` | 收集统计，不编译可复用计划 |
| 诊断语句 | `EXPLAIN`、`PROFILE` 外层语句 | 只复用目标计划，自身不缓存 |
| SQL 迁移 | `MIGRATE` | SQL 路径当前拒绝，迁移通过独立 API 执行 |

直接调用 `BulkInsertOperation`、`SchemaApi`、`FulltextApi`、`VectorApi` 的路径不经过 SQL 物理计划编译，因此也不属于本清单中的查询计划缓存。

## 缓存身份与失效

缓存 key 当前包含：

- 空白归一化后的查询文本
- space 名称
- schema / index generation
- 参数类型签名
- optimizer cost epoch 与 partitioning 配置哈希
- 是否处于可见 staged writes 的上下文
- 分区计划的分区指纹与布局版本

只读计划还包含统计 epoch 与反馈指纹；写入计划保持 epoch 独立，避免 `ANALYZE` 打冷热 DML 模板。DDL 提交后按 space 失效；attach/detach 等全局 catalog 变更清空整个计划缓存。
