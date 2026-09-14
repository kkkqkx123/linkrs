# 保留关键字列表

本文档列出了 GraphDB 查询语言中所有保留关键字。保留关键字不能用作**属性名**、**标签名**、**边类型名**或**标识符**。

> 关键字表来源：词法器 `lookup_keyword`（`crates/graphdb-query/src/parser/lexing/lexer.rs`），与代码一一对应。

---

## 1. DDL / 模式定义

| 关键字                  | TokenKind               | 说明         |
| ----------------------- | ----------------------- | ------------ |
| `CREATE`                | `Create`                | 创建语句     |
| `ALTER`                 | `Alter`                 | 修改语句     |
| `DROP`                  | `Drop`                  | 删除语句     |
| `SHOW`                  | `Show`                  | 显示语句     |
| `DESC` / `DESCRIBE`     | `Desc`                  | 描述结构     |
| `TAG` / `TAGS`          | `Tag` / `Tags`          | 标签类型     |
| `EDGE` / `EDGES`        | `Edge` / `Edges`        | 边类型       |
| `VERTEX` / `VERTICES`   | `Vertex` / `Vertices`   | 顶点类型     |
| `INDEX` / `INDEXES`     | `Index` / `Indexes`     | 索引         |
| `SPACE` / `SPACES`      | `Space` / `Spaces`      | 图空间       |
| `SEQUENCE` / `SEQUENCES`| `Sequence` / `Sequences`| 序列         |
| `IF`                    | `If`                    | 条件存在判断 |
| `EXISTS`                | `Exists`                | 存在性检查   |
| `COMMENT`               | `Comment`               | 注释属性     |
| `CHARSET`               | `Charset`               | 字符集       |
| `COLLATE` / `COLLATION` | `Collate` / `Collation` | 排序规则     |
| `DEFAULT`               | `Default`               | 默认值       |
| `SERIAL`                | `Serial`                | 自增列类型   |
| `STRUCT`                | `Struct`                | 结构体类型   |
| `ARRAY`                 | `Array`                 | 数组类型     |
| `MACROS`                | `Macros`                | 宏列表       |
| `FUNCTIONS`             | `Functions`             | 函数列表     |
| `GRAPHS`                | `Graphs`                | 图空间列表   |
| `SUBQUERY`              | `Subquery`              | 子查询       |

> 注意：`TTL_DURATION` / `TTL_COL` **不是**保留关键字（词法器中无对应 TokenKind），可作为普通标识符使用。

## 2. DML / 数据操作

| 关键字      | TokenKind   | 说明       |
| ----------- | ----------- | ---------- |
| `INSERT`    | `Insert`    | 插入语句   |
| `UPDATE`    | `Update`    | 更新语句   |
| `UPSERT`    | `Upsert`    | 插入或更新 |
| `DELETE`    | `Delete`    | 删除语句   |
| `DETACH`    | `Detach`    | 级联删除   |
| `MERGE`     | `Merge`     | 合并语句   |
| `VALUES`    | `Values`    | 值列表     |
| `SET`       | `Set`       | 设置属性   |
| `REMOVE`    | `Remove`    | 移除属性   |
| `OVERWRITE` | `Overwrite` | 覆盖写入   |
| `COPY`      | `Copy`      | 批量导入导出 |
| `HEADER`    | `Header`    | CSV 表头选项 |
| `DELIMITER` | `Delimiter` | CSV 分隔符选项 |
| `CSV`       | `Csv`       | CSV 格式声明 |

## 3. DQL / 查询

| 关键字              | TokenKind    | 说明         |
| ------------------- | ------------ | ------------ |
| `MATCH`             | `Match`      | 模式匹配     |
| `RETURN`            | `Return`     | 返回子句     |
| `WHERE`             | `Where`      | 条件过滤     |
| `FROM`              | `From`       | 来源子句     |
| `TO`                | `To`         | 目标子句     |
| `AS`                | `As`         | 别名         |
| `WITH`              | `With`       | 管道传递     |
| `YIELD`             | `Yield`      | 输出子句     |
| `GO`                | `Go`         | 遍历语句     |
| `OVER`              | `Over`       | 边类型遍历   |
| `STEP` / `STEPS`    | `Step`       | 步           |
| `UPTO`              | `Upto`       | 最大步数     |
| `LIMIT`             | `Limit`      | 限制条数     |
| `SKIP`              | `Skip`       | 跳过条数     |
| `ORDER`             | `Order`      | 排序         |
| `BY`                | `By`         | 排序依据     |
| `ASC`               | `Asc`        | 升序         |
| `DESC` / `DESCRIBE` | `Desc`       | 降序         |
| `ASCENDING`         | `Ascending`  | 升序（全拼） |
| `DESCENDING`        | `Descending` | 降序（全拼） |
| `DISTINCT`          | `Distinct`   | 去重         |
| `ALL`               | `All`        | 全部         |
| `GROUP`             | `Group`      | 分组         |
| `HAVING`            | `Having`     | 分组过滤     |
| `FILTER`            | `Filter`     | 过滤         |
| `ROLLUP`            | `Rollup`     | 分组小计     |
| `CUBE`              | `Cube`       | 分组立方     |
| `GROUPING`          | `Grouping`   | 分组集合     |
| `SETS`              | `Sets`       | 集合         |
| `UNWIND`            | `Unwind`     | 展开列表     |
| `OPTIONAL`          | `Optional`   | 可选匹配     |
| `UNION`             | `Union`      | 并集         |
| `INTERSECT`         | `Intersect`  | 交集         |
| `MINUS`             | `SetMinus`   | 差集         |
| `FETCH`             | `Fetch`      | 获取属性     |
| `PROP`              | `Prop`       | 属性操作     |

## 4. 路径 / 图算法

| 关键字             | TokenKind          | 说明         |
| ------------------ | ------------------ | ------------ |
| `FIND`             | `Find`             | 路径查找     |
| `PATH`             | `Path`             | 路径类型     |
| `SHORTEST`         | `Shortest`         | 最短路径     |
| `ALLSHORTESTPATHS` | `AllShortestPaths` | 全部最短路径 |
| `TRAIL`            | `Trail`            | 不重复经过边 |
| `ACYCLIC`          | `Acyclic`          | 不重复经过点 |
| `WEIGHTED`         | `Weighted`         | 带权路径     |
| `RECURSIVE`        | `Recursive`        | 递归模式     |
| `LOOP`             | `Loop`             | 自环允许     |
| `CYCLE`            | `Cycle`            | 回路允许     |
| `SUBGRAPH`         | `Subgraph`         | 子图         |
| `BIDIRECT`         | `Bidirect`         | 双向         |

> 注意：`FIND_PATH` 不是保留关键字（词法器中无对应 TokenKind）。

## 5. 方向 / 拓扑

| 关键字        | TokenKind     | 说明     |
| ------------- | ------------- | -------- |
| `BOTH`        | `Both`        | 双向     |
| `OUT`         | `Out`         | 出边     |
| `IN`          | `In`          | 入边     |
| `REVERSELY`   | `Reversely`   | 反向     |
| `SOURCE`      | `Source`      | 源点     |
| `DESTINATION` | `Destination` | 目标点   |
| `INPUT`       | `Input`       | 输入引用 |

> 注意：`OUTBOUND` / `INBOUND` 不是保留关键字（词法器中无对应 TokenKind）。

## 6. 数据类型

| 关键字          | TokenKind     | 说明       |
| --------------- | ------------- | ---------- |
| `BOOL`          | `Bool`        | 布尔       |
| `INT`           | `Int`         | 整数       |
| `INT8`          | `Int8`        | 8位整数    |
| `INT16`         | `Int16`       | 16位整数   |
| `INT32`         | `Int32`       | 32位整数   |
| `INT64`         | `Int64`       | 64位整数   |
| `FLOAT`         | `Float`       | 浮点数     |
| `DOUBLE`        | `Double`      | 双精度     |
| `STRING`        | `String`      | 字符串     |
| `FIXED_STRING`  | `FixedString` | 定长字符串 |
| `DATE`          | `Date`        | 日期       |
| `TIME`          | `Time`        | 时间       |
| `DATETIME`      | `Datetime`    | 日期时间   |
| `TIMESTAMP`     | `Timestamp`   | 时间戳     |
| `DURATION`      | `Duration`    | 持续时间   |
| `GEOGRAPHY`     | `Geography`   | 地理空间   |
| `POINT`         | `Point`       | 点         |
| `LINESTRING`    | `Linestring`  | 线         |
| `POLYGON`       | `Polygon`     | 面         |
| `LIST`          | `List`        | 列表       |
| `MAP`           | `Map`         | 映射       |
| `UUID`          | `UUID`        | UUID 类型  |
| `RANK`          | `Rank`        | 边排序值   |

## 7. 条件 / 逻辑 / 聚合

| 关键字         | TokenKind    | 说明       |
| -------------- | ------------ | ---------- |
| `IS`           | `Is`         | IS 判断    |
| `NOT`          | `Not`        | 逻辑非     |
| `AND`          | `And`        | 逻辑与     |
| `OR`           | `Or`         | 逻辑或     |
| `XOR`          | `Xor`        | 逻辑异或   |
| `NULL`         | `Null`       | 空值       |
| `NOT IN`       | `NotIn`      | 不在集合中 |
| `IS NULL`      | `IsNull`     | 为空判断   |
| `IS NOT NULL`  | `IsNotNull`  | 非空判断   |
| `IS EMPTY`     | `IsEmpty`    | 为空判断   |
| `IS NOT EMPTY` | `IsNotEmpty` | 非空判断   |
| `CONTAINS`     | `Contains`   | 包含       |
| `STARTS WITH`  | `StartsWith` | 以…开头    |
| `ENDS WITH`    | `EndsWith`   | 以…结尾    |
| `BETWEEN`      | `Between`    | 区间范围   |
| `COUNT`        | `Count`      | 计数       |
| `SUM`          | `Sum`        | 求和       |
| `AVG`          | `Avg`        | 平均值     |
| `MIN`          | `Min`        | 最小值     |
| `MAX`          | `Max`        | 最大值     |

## 8. CASE 表达式

| 关键字 | TokenKind | 说明      |
| ------ | --------- | --------- |
| `CASE` | `Case`    | CASE 开始 |
| `WHEN` | `When`    | WHEN 条件 |
| `THEN` | `Then`    | THEN 结果 |
| `ELSE` | `Else`    | ELSE 默认 |
| `END`  | `End`     | 结束      |

## 9. 全文搜索 / 向量

| 关键字   | TokenKind       | 说明     |
| -------- | --------------- | -------- |
| `SEARCH` | `Search`        | 搜索语句 |
| `TEXT`   | `Text`          | 文本搜索 |
| `VECTOR` | `KeywordVector` | 向量搜索 |
| `LOOKUP` | `Lookup`        | 查找索引 |

## 10. 图空间 / 分片 / 存储

| 关键字           | TokenKind        | 说明         |
| ---------------- | ---------------- | ------------ |
| `HOST` / `HOSTS` | `Host` / `Hosts` | 主机         |
| `PART` / `PARTS` | `Part` / `Parts` | 分片         |
| `DATA`           | `Data`           | 数据         |
| `LEADER`         | `Leader`         | 领导者       |
| `VID_TYPE`       | `VIdType`        | 顶点 ID 类型 |
| `PARTITION_NUM`  | `PartitionNum`   | 分区数       |
| `REPLICA_FACTOR` | `ReplicaFactor`  | 副本因子     |

## 11. 用户 / 权限

| 关键字           | TokenKind        | 说明         |
| ---------------- | ---------------- | ------------ |
| `USER` / `USERS` | `User` / `Users` | 用户         |
| `PASSWORD`       | `Password`       | 密码         |
| `ROLE` / `ROLES` | `Role` / `Roles` | 角色         |
| `GOD`            | `God`            | 超级管理员   |
| `DBA`            | `Dba`            | 数据库管理员 |
| `ADMIN`          | `Admin`          | 管理角色     |
| `GUEST`          | `Guest`          | 访客角色     |
| `LOCKED`         | `Locked`         | 锁定状态     |
| `CREATEUSER`     | `CreateUser`     | 创建用户     |
| `ALTERUSER`      | `AlterUser`      | 修改用户     |
| `DROPUSER`       | `DropUser`       | 删除用户     |
| `CHANGEPASSWORD` | `ChangePassword` | 修改密码     |
| `GRANT`          | `Grant`          | 授权         |
| `REVOKE`         | `Revoke`         | 撤销权限     |
| `ON`             | `On`             | 作用对象     |
| `OF`             | `Of`             | 所属         |

## 12. 事务 / 会话

| 关键字                | TokenKind      | 说明           |
| --------------------- | -------------- | -------------- |
| `BEGIN`               | `Begin`        | 开始事务       |
| `COMMIT`              | `Commit`       | 提交事务       |
| `ROLLBACK`            | `Rollback`     | 回滚事务       |
| `SAVEPOINT`           | `Savepoint`    | 保存点         |
| `RELEASE`             | `Release`      | 释放保存点     |
| `TRANSACTION`         | `Transaction`  | 事务关键字     |
| `READ` / `ONLY` / `WRITE` | `Read` / `Only` / `Write` | 事务读写模式 |
| `LET`                 | `Let`          | 会话变量赋值   |

## 13. 管理命令 / 运维

| 关键字                 | TokenKind              | 说明                        |
| ---------------------- | ---------------------- | --------------------------- |
| `USE`                  | `Use`                  | 切换图空间                  |
| `GET`                  | `Get`                  | 获取配置                    |
| `ADD`                  | `Add`                  | 添加                        |
| `CHANGE`               | `Change`               | 修改                        |
| `RENAME`               | `Rename`               | 重命名                      |
| `REBUILD`              | `Rebuild`              | 重建                        |
| `FLUSH`                | `Flush`                | 刷新                        |
| `COMPACT`              | `Compact`              | 压缩                        |
| `SUBMIT`               | `Submit`               | 提交                        |
| `STOP`                 | `Stop`                 | 停止                        |
| `REVERT`               | `Revert`               | 回滚                        |
| `CLEAR`                | `Clear`                | 清除                        |
| `STATS`                | `Stats`                | 统计信息                    |
| `STATUS`               | `Status`               | 状态                        |
| `JOBS` / `JOB`         | `Jobs` / `Job`         | 作业                        |
| `RECOVER`              | `Recover`              | 恢复                        |
| `EXPLAIN`              | `Explain`              | 执行计划                    |
| `PROFILE`              | `Profile`              | 性能分析                    |
| `ANALYZE`              | `Analyze`              | 统计收集 / 计划分析         |
| `FORMAT`               | `Format`               | 输出格式                    |
| `DOWNLOAD`             | `Download`             | 下载                        |
| `HDFS`                 | `HDFS`                 | HDFS 路径                   |
| `CONFIGS`              | `Configs`              | 配置信息                    |
| `FORCE`                | `Force`                | 强制操作                    |
| `LOCAL`                | `Local`                | 本地                        |
| `SESSIONS` / `SESSION` | `Sessions` / `Session` | 会话                        |
| `SAMPLE`               | `Sample`               | 采样                        |
| `QUERIES` / `QUERY`    | `Queries` / `Query`    | 查询                        |
| `KILL`                 | `Kill`                 | 终止                        |
| `TOP`                  | `Top`                  | TopN                        |
| `CLIENT` / `CLIENTS`   | `Client` / `Clients`   | 客户端                      |
| `SIGN`                 | `Sign`                 | 签名                        |
| `SERVICE`              | `Service`              | 服务                        |
| `NO`                   | `No`                   | 否定标识                    |
| `WEIGHT`               | `Weight`               | **边权重 — 不可用作属性名** |
| `ATOMIC_EDGE`          | `AtomicEdge`           | 原子边                      |
| `SETLIST`              | `SetList`              | 设置列表                    |
| `DIVIDE`               | `Divide`               | 切分操作                    |

> 注意：`BALANCE` 在词法器中被映射为 `Tk::Identifier("balance")`，**不是**保留关键字，可作为普通标识符使用。

## 14. 特殊引用

以下符号模式也是保留的，不可用作标识符：

| 符号      | TokenKind   | 说明       |
| --------- | ----------- | ---------- |
| `$^`      | `SrcRef`    | 源点引用   |
| `$$`      | `DstRef`    | 目标点引用 |
| `$-`      | `InputRef`  | 输入引用   |
| `$.id`    | `IdProp`    | ID 属性    |
| `$.type`  | `TypeProp`  | 类型属性   |
| `$-.src`  | `SrcIdProp` | 源点 ID    |
| `$-.dst`  | `DstIdProp` | 目标点 ID  |
| `$-.rank` | `RankProp`  | 边排序属性 |

此外，下划线前缀的属性引用同样保留：

| 符号      | TokenKind   | 说明       |
| --------- | ----------- | ---------- |
| `_src`    | `SrcIdProp` | 源点 ID    |
| `_dst`    | `DstIdProp` | 目标点 ID  |
| `_rank`   | `RankProp`  | 边排序属性 |

---

## 注意事项

1. **大小写不敏感**：关键字匹配不区分大小写，`CREATE`、`create`、`Create` 均被视为关键字。
2. **属性名冲突**：使用 `WEIGHT` 作为属性名会导致解析错误，因为该关键字被词法分析器映射为 `TokenKind::Weight`。应使用 `strength`、`score` 等替代名称。
3. **数据类型关键字**：所有数据类型名（`INT`、`STRING`、`GEOGRAPHY` 等）均为保留关键字，不可用作标识符。
4. **`TRUE` / `FALSE`**：被解析为 `BooleanLiteral` 字面量，同样不可用作标识符。
5. **非保留字**：`BALANCE`、`TTL_DURATION`、`TTL_COL`、`FIND_PATH`、`OUTBOUND`、`INBOUND` 看似关键字但词法器未保留，可作为标识符使用。
