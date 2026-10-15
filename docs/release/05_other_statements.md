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
- 实际执行查询
- 收集执行时间和资源消耗
- 显示实际行数和成本
- 支持 TABLE 和 DOT 两种输出格式（缺省为 TABLE）

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

### 示例
```cypher
ANALYZE
ANALYZE SPACE test_space
```

---

## 5. 会话管理语句

### 5.1 SHOW SESSIONS - 显示会话列表

#### 功能
显示当前所有的会话信息。

#### 语法结构
```cypher
SHOW SESSIONS
```

#### 示例
```cypher
SHOW SESSIONS
```

### 5.2 SHOW QUERIES - 显示查询列表

#### 功能
显示当前正在执行的查询。

#### 语法结构
```cypher
SHOW QUERIES
```

#### 示例
```cypher
SHOW QUERIES
```

### 5.3 KILL QUERY - 终止查询

#### 功能
终止指定的查询。

#### 语法结构
```cypher
KILL QUERY <session_id>, <plan_id>
```

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
  bootstrap、optimizer、monitoring，不区分大小写

#### 输出列
- `section`：配置节，与模块名一致
- `key`：配置项名
- `value`：当前值，字符串保持原样，其余渲染为紧凑 JSON
- `requires_restart`：该项是否需要重启生效

#### 权限
需要管理员权限。

流式查询（`/v1/query/stream`）、gRPC 流式与导出接口不支持
配置语句，会直接返回错误指引使用一元查询接口；
单行结果无需流式，且流式物化无法承载服务端应用语义。

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
  bootstrap、optimizer、monitoring，不区分大小写；省略时配置名必须
  在全部模块中唯一归属，否则未知键与歧义键均明确拒绝
- `config_name`: 配置项名称，已知键才可写，未知键明确拒绝
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
UPDATE CONFIGS database max_connections = 1000
UPDATE CONFIGS max_connections = 1000
UPDATE CONFIGS database max_connections = 50 * 2
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
- 单行结果集，列与其他维护语句一致：`action`（固定 `auth`）/ `name`（空）/ `status`（`executed` 或 `no-storage`）

#### 权限
需要写级别权限（GOD / ADMIN / DBA / USER 角色）；只读角色（GUEST）执行会被拒绝。
权限门同样覆盖流式入口，拒绝行为与一元入口一致。

#### 示例
```cypher
CHECKPOINT
```

#### 与其他语句的关系
- 与 `PURGE EXPIRED` 同属维护语句，均经通用查询接口执行、无专用 RPC
- 过期数据的物理清理见 `PURGE EXPIRED`
  （`docs/release/03_ddl_schema_definition.md` 第 15 节）
