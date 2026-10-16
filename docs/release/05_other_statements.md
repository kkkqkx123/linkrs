# Linkrs 其他语句

## 概述

本文档包含不属于DQL、DML、DDL、DCL分类的其他实用语句，即服务与会话管理语句：
切换图空间、查询分析与统计信息收集、会话与查询管理、配置管理、存储维护。

这些语句在查询引擎内部统一归类为管理语句（ADMIN），不读写图数据本身。
查询子句与组合能力（管道、集合操作、GROUP BY、变量赋值、表达式）见
《01_dql_query_syntax.md》第 9、10 节。

---

## 1. USE - 切换图空间

### 功能
将当前会话切换到指定的图空间，之后的无空间限定语句在该空间上执行。

### 语法结构
```cypher
USE <space_name>
```

### 关键特性
- 作用域为当前会话，不影响其他会话
- `USE GRAPH <space_name>` 为同义写法（`GRAPH` 关键字可省略）
- 仅当 `GRAPH` 后跟随另一个标识符时才作为别名前缀；
  单独的 `USE graph` 表示切换到名为 `graph` 的图空间

### 示例
```cypher
USE test_space
USE social_network
USE GRAPH basketball
```

---

## 2. EXPLAIN - 查询计划

### 功能
显示查询的执行计划，用于性能分析和优化。

### 语法结构
```cypher
EXPLAIN [ANALYZE] [FORMAT = {TABLE | DOT}] <statement>
```

### 关键特性
- 显示查询执行计划
- 显示预计成本
- 显示使用的索引
- 不实际执行查询
- 支持 TABLE 和 DOT 两种输出格式（缺省为 TABLE）
- `EXPLAIN ANALYZE` 变体：执行查询并收集各算子的实际执行统计

输出为单行单列结果集，列名为 `plan`（字符串）；
预计成本与索引信息内嵌于该字符串中，而非独立列。

`EXPLAIN ANALYZE` 会实际执行目标语句（含写操作，副作用按会话事务规则提交），
仅返回计划文本，不返回目标查询的数据行。仅查看计划而不执行时使用普通 `EXPLAIN`。

### 示例
```cypher
EXPLAIN MATCH (p:Person {name: 'Alice'}) RETURN p
EXPLAIN FORMAT = TABLE MATCH (p:Person) RETURN p
EXPLAIN FORMAT = DOT GO 2 STEP FROM "101" OVER follow
EXPLAIN ANALYZE MATCH (p:Person) RETURN p
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
- 实际执行查询（含写操作，副作用按会话事务规则提交），仅返回画像数据，不返回目标查询的数据行
- 收集执行时间和资源消耗
- 显示实际行数和成本
- 支持 TABLE 和 DOT 两种输出格式（缺省为 TABLE）：
  TABLE 返回逐算子画像行（`id` / `name` / `dependencies` / `profiling_data` / `operator_info`，
  末尾附带一行 `Parallel` 汇总行）；DOT 返回携带画像标注的单列 `plan` 图字符串

### 示例
```cypher
PROFILE MATCH (p:Person)-[:FRIEND]->(f) RETURN count(f)
PROFILE FORMAT = DOT GO 3 STEPS FROM "player100" OVER follow
```

---

## 4. ANALYZE - 统计信息收集

### 功能
收集图空间的统计信息，用于优化器选择执行计划。

### 语法结构
```cypher
ANALYZE [SPACE <space_name>]
```

### 关键特性
- 省略 `SPACE <space_name>` 时统计当前图空间
- 无当前空间且未指定空间时明确报错
- 同步采集并返回成功标记（无数据行）

#### 权限
需要 Schema 级权限（DBA 及以上角色）。

### 示例
```cypher
ANALYZE
ANALYZE SPACE test_space
```

---

## 5. 会话管理语句

### 5.1 SHOW SESSIONS - 显示会话列表

#### 功能
显示当前所有的会话信息，数据来自服务端的实时会话注册表。

#### 语法结构
```cypher
SHOW SESSIONS
```

#### 输出列
- `session_id`：会话标识
- `user_name`：会话所属用户
- `space_name`：会话当前图空间（未切换时为空）
- `created_at`：会话创建时间（unix epoch 秒）
- `last_active`：最近活跃时间（unix epoch 秒）
- `active_queries`：该会话当前活跃查询数
- `graph_addr`：客户端地址（未知时为空）

#### 权限
需要管理员权限。

流式查询、导出与游标接口上的 `SHOW SESSIONS` 会自动转到一元执行路径，结果一致。

#### 示例
```cypher
SHOW SESSIONS
```

### 5.2 SHOW QUERIES - 显示查询列表

#### 功能
显示当前正在执行的查询，数据来自服务端的实时查询跟踪。
`query_id` 列的值可直接作为 `KILL QUERY` 的终止目标。

#### 语法结构
```cypher
SHOW QUERIES
```

#### 输出列
- `query_id`：服务端分配的执行标识（全局单调，全通道统一）
- `session_id`：所属会话标识
- `user_name`：执行用户
- `space_name`：执行所在图空间（未指定时为空）
- `status`：固定为 `running`（仅列出运行中的查询）
- `duration_ms`：已执行时长（毫秒，实时计算）
- `query`：查询文本

#### 权限
读权限即可执行。

流式查询、导出与游标接口上的 `SHOW QUERIES` 会自动转到一元执行路径，结果一致。

#### 示例
```cypher
SHOW QUERIES
```

### 5.3 KILL QUERY - 终止查询

#### 功能
终止指定的查询。目标会话必须存在，且该查询必须是该会话名下正在运行的、
`SHOW QUERIES` 可见的跟踪查询。终止后目标条目标记为 killed 并取消其执行令牌；
对已结束或不属于目标会话的标识明确报错。

#### 语法结构
```cypher
KILL QUERY <session_id>, <plan_id>
```

`plan_id` 即 `SHOW QUERIES` 输出中的 `query_id`（服务端统一执行标识）。

#### 输出列
- `session_id`：目标会话标识
- `query_id`：被终止的查询标识
- `status`：固定为 `killed`

#### 权限
需要管理员权限。

流式查询、导出与游标接口上的 `KILL QUERY` 会自动转到一元执行路径，行为一致。

#### 示例
```cypher
KILL QUERY 123, 456
```

---

## 6. 配置管理语句

### 6.1 SHOW CONFIGS - 显示配置

#### 功能
读取真实配置存储并显示系统配置信息。无模块时列出全部模块，
有模块时仅列出该模块。未知模块明确拒绝。

#### 语法结构
```cypher
SHOW CONFIGS [<module>]
```

#### 参数说明
- `module`: 可选，模块名，仅支持 database、transaction、log、auth、
  bootstrap、http、security、optimizer、monitoring，不区分大小写。
  其中 http、security 为服务侧配置节；`migration` 节仅能通过配置文件与管理接口维护，
  不在查询配置词汇内

#### 输出列
- `section`：配置节，与模块名一致
- `key`：配置项名
- `value`：当前值，字符串保持原样，其余渲染为紧凑 JSON
- `requires_restart`：该项是否需要重启生效

#### 权限
需要管理员权限。

流式查询（`/v1/query/stream`）、gRPC 流式与导出、游标接口不支持
配置语句，会直接返回错误指引使用一元查询接口；
单行结果无需流式，且流式物化无法承载服务端应用语义。
批量一元接口（`/v1/query/batch`）支持配置语句。
嵌入式会话不支持配置语句，会返回明确错误而非空结果；
`auth` 的口令秘密键不可经查询读写。

#### 示例
```cypher
SHOW CONFIGS
SHOW CONFIGS database
SHOW CONFIGS optimizer
```

### 6.2 UPDATE CONFIGS - 更新配置

#### 功能
更新系统配置值。查询引擎仅做语法解析与常量求值并构造意图，
不直接触碰全局存储；服务层复用管理接口的校验、持久化、重启标记
与回滚语义后应用意图。不新增查询内逐项调用形态。

#### 语法结构
```cypher
UPDATE CONFIGS [<module>] <config_name> = <value>
```

#### 参数说明
- `module`: 可选，模块名，仅支持 database、transaction、log、auth、
  bootstrap、http、security、optimizer、monitoring，不区分大小写；省略时配置名必须
  在全部模块中唯一归属，否则未知键与歧义键均明确拒绝
- `config_name`: 配置项名称，已知键才可写，未知键明确拒绝
  （保留字键名如 `host` 同样可用）
- `value`: 配置值，支持常量表达式（含参数与会话变量参与的确定性
  标量计算）；需要行输入的变量、属性引用、子查询明确拒绝

#### 返回列
- `updated`：实际生效的 `section.key`
- `requires_restart`：是否需要重启生效
- `persisted`：是否已持久化到保留的配置文件

#### 权限
需要管理员权限，走直接写路径，不经只读快照与计划缓存。

#### 示例
```cypher
UPDATE CONFIGS database max_sessions = 1000
UPDATE CONFIGS max_sessions = 1000
UPDATE CONFIGS database max_sessions = 50 * 2
UPDATE CONFIGS database host = '127.0.0.1'
```

#### 与参考写法的对应关系
- 参考项目的查询内逐项配置调用对应本项目的 `UPDATE CONFIGS`，
  不再单独提供查询内配置形态。
- 参考项目的表函数调用对应本项目的 `CALL <func>(...) [YIELD ...]`，
  与配置设置职责分离。

---

## 7. 存储维护语句

### 7.1 CHECKPOINT - 触发存储快照

#### 功能
立即触发一次存储快照（checkpoint），把当前已提交状态落成可恢复的组合 manifest，
并按保留策略截断可回收的 WAL。存储自身还有按 WAL 大小/时间间隔的自动 checkpoint，
本语句用于运维场景下的手动触发（如大批量导入后、发布前）。

#### 语法结构
```cypher
CHECKPOINT
```

#### 关键特性
- 无参数，作用于当前实例的全部空间
- 维护动作，不读写图数据本身
- 与自动 checkpoint 共用同一实现与同一触发原因记录（本语句记为显式触发）
- 持久化未启用时为空操作（不报错）

#### 返回列
- 单行结果集，列与其他维护语句一致：`action`（固定 `auth`）/ `name`（空字符串）/ `status`
  （`executed` 或 `no-storage`；有存储但未启用持久化时同样报告 `no-storage`）

#### 权限
需要写级别权限（GOD / ADMIN / DBA / USER 角色）；只读角色（GUEST）执行会被拒绝。
权限按当前会话空间判定（会话空间无角色时回退到全局角色）。权限门同样覆盖流式入口，
拒绝行为与一元入口一致。

#### 示例
```cypher
CHECKPOINT
```

#### 与其他语句的关系
- 与 `PURGE EXPIRED` 同属维护语句，均经通用查询接口执行、无专用 RPC
- 过期数据的物理清理见 `PURGE EXPIRED`
  （`docs/release/03_ddl_schema_definition.md` 第 15 节）

---

## 8. 管理语句权限矩阵

| 语句 | 所需权限 | 说明 |
|---|---|---|
| USE / 事务命令（BEGIN/COMMIT/ROLLBACK/SAVEPOINT/RELEASE）/ LET | 无 | 会话命令直接放行 |
| SHOW QUERIES | 读 | 任何可查询角色 |
| ANALYZE | Schema | DBA 及以上角色 |
| CHECKPOINT | 写 | 按当前会话空间判定，会话空间无角色时回退到全局角色 |
| SHOW SESSIONS / SHOW CONFIGS / UPDATE CONFIGS / KILL QUERY | 管理员 | 查询通道要求当前会话空间的 Admin（或全局管理员）；HTTP 管理接口要求全局管理员 |

`space_id = 0`（尚未 `USE` 任何空间）的调用按全局空间回退规则判定；
跨空间管理员在 HTTP 管理接口与查询通道的结论可能不一致，以各通道的实际报错为准。
