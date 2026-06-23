# 数据库慢查询日志实现对比分析

## 一、主流数据库慢查询日志实现

### 1.1 PostgreSQL

#### 核心实现方式

PostgreSQL 提供了多层次的慢查询监控方案：

**1. 基础日志记录**

```ini
# postgresql.conf 配置
log_min_duration_statement = 1000  # 记录超过 1 秒的查询（毫秒）
log_duration = off                  # 不记录所有查询的持续时间
log_statement = 'none'              # 不记录所有语句
```

**特点**：
- `log_min_duration_statement`：超过阈值强制记录查询文本
- `log_duration`：记录所有语句的持续时间（但不记录查询文本）
- 两者结合可以跟踪所有查询耗时，但只记录慢查询的完整 SQL

**2. auto_explain 模块**

```ini
# 加载模块
session_preload_libraries = 'auto_explain'

# 配置参数
auto_explain.log_min_duration = '3s'     # 超过 3 秒的查询记录执行计划
auto_explain.log_analyze = on            # 记录实际执行信息
auto_explain.log_buffers = on            # 记录缓冲区使用情况
auto_explain.log_timing = on             # 记录规划和执行时间
```

**输出示例**：
```
LOG:  duration: 3452.123 ms  execute s: SELECT * FROM large_table WHERE ...
LOG:  Plan:
      ->  Seq Scan on large_table  (cost=0.00..355.00 rows=10000 width=4)
            Actual Time=0.012..3450.001 rows=10000 loops=1
            Filter: (condition = 'value')
      Planning Time: 0.123 ms
      Execution Time: 3452.456 ms
```

**3. pg_stat_statements 扩展**

```sql
-- 启用扩展
CREATE EXTENSION pg_stat_statements;

-- 配置
ALTER SYSTEM SET shared_preload_libraries = 'pg_stat_statements';
ALTER SYSTEM SET pg_stat_statements.track = 'all';  -- 跟踪所有语句
ALTER SYSTEM SET pg_stat_statements.max = 10000;    -- 最多跟踪的语句数

-- 查询统计信息
SELECT query, calls, total_exec_time, rows,
       100.0 * shared_blks_hit / 
       nullif(shared_blks_hit + shared_blks_read, 0) AS hit_percent
FROM pg_stat_statements 
ORDER BY total_exec_time DESC 
LIMIT 5;
```

**统计字段**：
- `userid`, `dbid`: 用户和数据库 OID
- `query`: 代表性查询文本
- `calls`: 执行次数
- `total_exec_time`: 总执行时间（毫秒）
- `rows`: 检索或影响的总行数
- `shared_blks_hit/read`: 共享缓冲区命中/读取数
- `mean_exec_time`: 平均执行时间

#### 设计亮点

1. **分层监控**：
   - 基础层：`log_min_duration_statement` 记录慢查询
   - 增强层：`auto_explain` 记录执行计划
   - 统计层：`pg_stat_statements` 聚合统计

2. **低开销**：
   - 基础日志使用现有日志系统
   - `pg_stat_statements` 使用共享内存，避免锁竞争

3. **可配置性**：
   - 支持按会话、用户、数据库级别配置
   - 支持采样和阈值过滤

---

### 1.2 MySQL

#### 核心实现方式

**1. 慢查询日志配置**

```ini
# my.cnf 配置
[mysqld]
slow_query_log = 1
slow_query_log_file = /var/log/mysql/slow.log
long_query_time = 2              # 慢查询阈值（秒，默认 10 秒）
log_queries_not_using_indexes = ON   # 记录未使用索引的查询
log_slow_admin_statements = ON       # 记录管理语句
min_examined_row_limit = 1000        # 最小检查行数
```

**2. 日志限流**

```ini
# 限制每分钟记录的未使用索引查询数
log_throttle_queries_not_using_indexes = 10
```

**3. 日志格式**

```ini
# 可选格式：TEXT 或 JSON
log_output = FILE  # 或 TABLE
```

**输出示例（TEXT 格式）**：
```
# Time: 2024-04-15T10:23:45.123456Z
# User@Host: app_user[app_user] @ localhost []
# Query_time: 2.345678  Lock_time: 0.000123 Rows_sent: 1000  Rows_examined: 50000
# Rows_affected: 0  Bytes_sent: 102400
use mydb;
SET timestamp=1713156225;
SELECT * FROM large_table WHERE indexed_column = 'value';
```

**输出示例（JSON 格式）**：
```json
{
  "timestamp": "2024-04-15T10:23:45.123456Z",
  "user_host": "app_user[app_user] @ localhost []",
  "query_time": 2.345678,
  "lock_time": 0.000123,
  "rows_sent": 1000,
  "rows_examined": 50000,
  "rows_affected": 0,
  "bytes_sent": 102400,
  "sql_text": "SELECT * FROM large_table WHERE indexed_column = 'value';"
}
```

#### 设计亮点

1. **多维度过滤**：
   - 基于执行时间 (`long_query_time`)
   - 基于索引使用 (`log_queries_not_using_indexes`)
   - 基于检查行数 (`min_examined_row_limit`)
   - 基于语句类型 (`log_slow_admin_statements`)

2. **限流保护**：
   - `log_throttle_queries_not_using_indexes` 防止日志爆炸

3. **灵活输出**：
   - 支持 FILE（文本文件）、TABLE（数据库表）、JSON 格式

---

### 1.3 MongoDB

#### 核心实现方式

**1. Profiler 配置**

```javascript
// 启用 Profiler（级别 1：仅慢查询）
db.setProfilingLevel(1, {
  slowms: 100,        // 慢查询阈值（毫秒）
  sampleRate: 1.0     // 采样率（0.0-1.0）
});

// 或使用 profile 命令
db.runCommand({
  profile: 1,
  slowms: 200,
  sampleRate: 0.5,
  filter: { 
    "op": "query", 
    "ns": "mydb.mycollection" 
  }
});
```

**Profiling 级别**：
- `0`: 关闭
- `1`: 仅记录慢查询
- `2`: 记录所有查询

**2. 查询系统表**

```javascript
// 查询最近的慢查询
db.system.profile.find().sort({$natural:-1}).limit(5)

// 分析慢查询
db.system.profile.aggregate([
  { $match: { millis: { $gt: 100 } } },
  { $group: { 
      _id: "$ns", 
      count: { $sum: 1 },
      avgTime: { $avg: "$millis" }
  }}
])
```

**3. 输出示例**

```json
{
  "op": "query",
  "ns": "mydb.users",
  "query": { "age": { "$gt": 18 } },
  "planSummary": "IXSCAN { age: 1 }",
  "millis": 152,
  "timeReadingDiskMillis": 45,
  "keysExamined": 5000,
  "docsExamined": 1000,
  "nreturned": 234,
  "ts": ISODate("2024-04-15T10:23:45.123Z"),
  "user": "app_user",
  "locks": {
    "Global": { "acquireCount": { "r": 2 } }
  }
}
```

#### 设计亮点

1. **丰富的性能指标**：
   - `planSummary`: 执行计划摘要
   - `timeReadingDiskMillis`: 磁盘读取时间
   - `keysExamined`: 检查的索引键数
   - `docsExamined`: 检查的文档数
   - `nreturned`: 返回的文档数

2. **灵活的过滤**：
   - 支持按操作类型、命名空间、时间等过滤
   - 支持采样减少开销

3. **内置分析**：
   - 使用聚合管道分析 `system.profile` 集合
   - 支持自定义分析查询

---

## 二、实现对比总结

### 2.1 核心特性对比

| 特性 | PostgreSQL | MySQL | MongoDB | GraphDB (当前) |
|------|-----------|-------|---------|----------------|
| **独立日志文件** | ✅ | ✅ | ✅ (system.profile) | ❌ |
| **异步写入** | ✅ (系统日志) | ✅ (系统日志) | ✅ (内部) | ❌ |
| **日志轮转** | ✅ (外部工具) | ✅ (外部工具) | ✅ (内置) | ❌ |
| **可配置阈值** | ✅ | ✅ | ✅ | ✅ |
| **多级别配置** | ✅ | ✅ | ✅ | ❌ |
| **执行计划记录** | ✅ (auto_explain) | ❌ | ✅ (planSummary) | ❌ |
| **聚合统计** | ✅ (pg_stat_statements) | ❌ | ✅ (聚合查询) | ⚠️ (基础) |
| **采样支持** | ❌ | ❌ | ✅ | ❌ |
| **JSON 格式** | ❌ | ✅ | ✅ | ❌ |
| **I/O 统计** | ✅ (buffers) | ❌ | ✅ (disk time) | ❌ |

### 2.2 日志格式对比

**PostgreSQL**（简洁，依赖工具分析）：
```
LOG:  duration: 3452.123 ms  statement: SELECT ...
```

**MySQL**（结构化，信息丰富）：
```
# Time: 2024-04-15T10:23:45.123456Z
# User@Host: app_user[app_user] @ localhost []
# Query_time: 2.345678  Lock_time: 0.000123 
# Rows_sent: 1000  Rows_examined: 50000
SELECT * FROM table WHERE ...
```

**MongoDB**（JSON，最适合程序分析）：
```json
{
  "op": "query",
  "millis": 152,
  "keysExamined": 5000,
  "docsExamined": 1000,
  "nreturned": 234
}
```

### 2.3 最佳实践总结

#### 1. 分层监控架构

```
┌─────────────────────────────────────┐
│   实时指标 (pg_stat_statements)     │  ← 聚合统计，低开销
├─────────────────────────────────────┤
│   慢查询日志 (slow_query.log)       │  ← 详细记录，异步写入
├─────────────────────────────────────┤
│   执行计划 (auto_explain)           │  ← 深度分析，按需启用
└─────────────────────────────────────┘
```

#### 2. 关键设计原则

**低开销**：
- 异步写入，不阻塞查询
- 采样机制，减少记录量
- 使用共享内存（PostgreSQL）

**可分析性**：
- 结构化格式（JSON 优先）
- 包含足够的上下文信息
- 支持聚合和统计

**可维护性**：
- 自动日志轮转
- 配置热更新
- 独立的日志文件

#### 3. 推荐指标

**基础指标**（必须）：
- 查询文本
- 执行时间
- 时间戳
- 状态（成功/失败）

**增强指标**（推荐）：
- 执行阶段分解（parse, plan, execute）
- 行数统计（scanned, returned）
- I/O 统计（disk reads, buffer hits）
- 执行器统计

**高级指标**（可选）：
- 执行计划
- 锁等待时间
- 内存使用
- 缓存命中率

---

## 三、对 GraphDB 的启示

### 3.1 当前差距

1. **❌ 没有独立日志文件**：慢查询与系统日志混合
2. **❌ 没有异步写入**：使用 `log::warn!` 同步写入
3. **❌ 没有日志轮转**：可能无限增长
4. **❌ 缺少执行计划记录**：无法分析查询优化
5. **❌ 缺少聚合统计**：没有类似 `pg_stat_statements` 的组件
6. **❌ I/O 统计不完善**：缺少磁盘访问指标

### 3.2 改进方向

**短期（P0）**：
1. 实现独立的慢查询日志文件
2. 实现异步写入机制
3. 实现日志轮转

**中期（P1）**：
4. 增加执行计划记录（类似 auto_explain）
5. 完善 I/O 统计
6. 实现 JSON 格式输出

**长期（P2）**：
7. 实现聚合统计组件（类似 pg_stat_statements）
8. 支持采样机制
9. 提供分析工具

---

## 四、参考实现

### 4.1 PostgreSQL 配置示例

```ini
# postgresql.conf
shared_preload_libraries = 'pg_stat_statements,auto_explain'

# 慢查询日志
log_min_duration_statement = 1000

# pg_stat_statements
pg_stat_statements.max = 10000
pg_stat_statements.track = all

# auto_explain
auto_explain.log_min_duration = 3000
auto_explain.log_analyze = on
auto_explain.log_buffers = on
```

### 4.2 MySQL 配置示例

```ini
# my.cnf
[mysqld]
slow_query_log = 1
slow_query_log_file = /var/log/mysql/slow.log
long_query_time = 2
log_queries_not_using_indexes = ON
log_throttle_queries_not_using_indexes = 10
log_output = FILE
```

### 4.3 MongoDB 配置示例

```javascript
// 启用 profiler
db.setProfilingLevel(1, {
  slowms: 100,
  sampleRate: 1.0
});

// 分析慢查询
db.system.profile.aggregate([
  { $match: { millis: { $gt: 100 } } },
  { $group: { 
      _id: "$query",
      count: { $sum: 1 },
      avgTime: { $avg: "$millis" },
      maxTime: { $max: "$millis" }
  }},
  { $sort: { count: -1 } },
  { $limit: 10 }
]);
```

---

## 五、总结

主流数据库的慢查询日志实现具有以下共同特点：

1. **独立日志**：与系统日志分离，便于分析
2. **异步写入**：最小化对查询性能的影响
3. **结构化格式**：JSON 或结构化文本，便于程序分析
4. **丰富指标**：不仅记录时间，还包括 I/O、行数等
5. **可配置性**：支持多级配置和动态调整
6. **配套工具**：提供分析工具和聚合统计

GraphDB 的当前实现仅完成了基础功能，需要在独立性、异步性、可维护性和可分析性方面进行改进。
