# GraphDB CLI 与 psql 功能对比

## 1. 功能映射表

### 1.1 连接管理

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 连接数据库 | `\c [dbname]` | `\connect [space]`或`\c [space]` | 连接到指定数据库/图空间 |
| 显示连接信息 | `\conninfo` | `\conninfo` | 显示当前连接信息 |
| 断开连接 | `\q` | `\disconnect`或`\q` | 断开当前连接 |

### 1.2 对象查看

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 列出数据库 | `\l` | `\show_spaces` 或 `\l` | 列出所有数据库/图空间 |
| 列出表 | `\dt` | `\show_tags` 或 `\dt` | 列出所有表/Tag |
| 列出索引 | `\di` | `\show_indexes` 或 `\di` | 列出所有索引 |
| 查看表结构 | `\d <table>` | `\describe <tag>` 或 `\d <tag>` | 查看表/Tag结构 |
| 列出函数 | `\df` | `\show_functions` | 列出所有函数 |
| 列出用户 | `\du` | `\show_users` | 列出所有用户 |

### 1.3 查询执行

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 执行查询 | 直接输入 SQL | 直接输入 GQL | 执行查询语句 |
| 执行脚本 | `\i <file>` | `\i <file>` | 执行脚本文件 |
| 执行单条命令 | `-c` 参数 | `-c` 参数 | 命令行执行单条语句 |
| 输出重定向 | `\o [file]` | `\o [file]` | 输出重定向到文件 |

### 1.4 输出格式

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 表格格式 | 默认 | 默认 | 对齐的表格格式 |
| 扩展格式 | `\x` | `\x` | 垂直显示格式 |
| CSV 格式 | `\pset format csv` | `\format csv` | CSV 格式输出 |
| JSON 格式 | 需要扩展 | `\format json` | JSON 格式输出 |
| HTML 格式 | `\pset format html` | `\format html` | HTML 格式输出 |
| 设置分页器 | `\pset pager` | `\pager [command]` | 设置分页器 |

### 1.5 变量和参数

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 设置变量 | `\set name value` | `\set name value` | 设置变量 |
| 使用变量 | `:name` | `:name` | 在查询中使用变量 |
| 删除变量 | `\unset name` | `\unset name` | 删除变量 |
| 显示所有变量 | `\set` | `\set` | 显示所有变量 |

### 1.6 编辑和历史

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 编辑器编辑 | `\e` | `\e` | 使用编辑器编辑查询 |
| 编辑函数 | `\ef [func]` | `\ef [func]` | 编辑函数 |
| 命令历史 | 自动保存 | 自动保存 | 自动保存命令历史 |
| 历史搜索 | Ctrl+R | Ctrl+R | 搜索历史命令 |

### 1.7 事务管理

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 开始事务 | `BEGIN` | `\begin` 或 `BEGIN` | 开始事务 |
| 提交事务 | `COMMIT` | `\commit` 或 `COMMIT` | 提交事务 |
| 回滚事务 | `ROLLBACK` | `\rollback` 或 `ROLLBACK` | 回滚事务 |

### 1.8 性能分析

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 显示执行时间 | `\timing` | `\timing` | 显示查询执行时间 |
| 执行计划 | `EXPLAIN` | `EXPLAIN` | 显示查询执行计划 |
| 详细分析 | `EXPLAIN ANALYZE` | `PROFILE` | 显示详细性能分析 |

### 1.9 帮助和信息

| 功能 | psql | GraphDB CLI | 说明 |
|------|------|-------------|------|
| 元命令帮助 | `\?` | `\?` | 显示元命令帮助 |
| SQL 帮助 | `\help [command]` | `\help [command]` | 显示 SQL/GQL 命令帮助 |
| 版本信息 | `SELECT version()` | `\version` | 显示版本信息 |
| 版权信息 | `\copyright` | `\copyright` | 显示版权信息 |

## 2. GraphDB CLI 特有功能

### 2.1 图数据库特有命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\show_tags` | 列出所有 Tag（节点类型） | `\show_tags person*` |
| `\show_edges` | 列出所有 Edge（边类型） | `\show_edges` |
| `\describe_edge` | 查看 Edge 结构 | `\describe_edge follow` |
| `\show_fulltext_indexes` | 列出全文索引 | `\show_fulltext_indexes` |
| `\show_vector_indexes` | 列出向量索引 | `\show_vector_indexes` |

### 2.2 图查询特有功能

- **路径查询可视化**：自动识别路径结果并以图形化方式展示
- **图遍历统计**：显示遍历的节点和边数量
- **模式匹配提示**：提供 MATCH 语句的智能提示

### 2.3 多模型支持

- **全文搜索**：集成全文搜索查询
- **向量搜索**：支持向量相似度查询
- **混合查询**：支持图查询与全文/向量搜索的组合

## 3. 实现优先级

### 3.1 第一阶段（核心功能）

**优先级：高**

1. **连接管理**
   - 基本连接功能
   - 会话管理
   - 认证支持

2. **查询执行**
   - GQL 查询执行
   - 结果格式化
   - 错误处理

3. **基础元命令**
   - `\show_spaces`, `\show_tags`, `\show_edges`
   - `\describe`
   - `\format`
   - `\help`, `\?`, `\q`

4. **输出格式**
   - 表格格式
   - JSON 格式
   - CSV 格式

### 3.2 第二阶段（用户体验）

**优先级：中**

1. **自动补全**
   - 关键字补全
   - 对象名补全
   - 函数名补全

2. **历史和编辑**
   - 命令历史
   - 编辑器集成
   - 多行编辑

3. **变量管理**
   - 变量设置和使用
   - 环境变量支持

4. **脚本执行**
   - 文件执行
   - 批处理模式

### 3.3 第三阶段（高级功能）

**优先级：低**

1. **性能分析**
   - 执行计划展示
   - 性能统计
   - 查询优化建议

2. **数据导入导出**
   - CSV 导入导出
   - JSON 导入导出
   - 数据备份恢复

3. **事务管理**
   - 事务控制
   - 保存点
   - 隔离级别

4. **扩展功能**
   - 插件系统
   - 自定义命令
   - 脚本 API

## 4. 技术差异

### 4.1 查询语言

| 特性 | psql (SQL) | GraphDB CLI (GQL) |
|------|-----------|-------------------|
| 数据模型 | 关系表 | 图（节点和边） |
| 查询模式 | 声明式 | 声明式 + 模式匹配 |
| 连接操作 | JOIN | 图遍历 |
| 路径查询 | 递归 CTE | 原生支持 |
| 索引类型 | B-tree, Hash, GiST 等 | Tag 索引、全文索引、向量索引 |

### 4.2 结果类型

| 类型 | psql | GraphDB CLI |
|------|------|-------------|
| 基本类型 | 行、列 | 行、列 |
| 复杂类型 | 数组、JSON | 顶点、边、路径 |
| 特殊类型 | 几何类型 | 地理空间类型 |

### 4.3 元数据管理

| 特性 | psql | GraphDB CLI |
|------|------|-------------|
| 系统表 | pg_catalog | 内部元数据 |
| 信息模式 | information_schema | Schema API |
| 统计信息 | pg_stat | StatsManager |

## 5. 用户迁移指南

### 5.1 从 psql 迁移到 GraphDB CLI

#### 5.1.1 概念映射

| PostgreSQL 概念 | GraphDB 概念 | 说明 |
|----------------|-------------|------|
| Database | Space | 数据库 → 图空间 |
| Table | Tag | 表 → Tag（节点类型） |
| Foreign Key | Edge | 外键关系 → 边 |
| Row | Vertex | 行 → 节点 |
| Column | Property | 列 → 属性 |

#### 5.1.2 常用操作对比

**创建表/Tag**：
```sql
-- PostgreSQL
CREATE TABLE person (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100),
    age INTEGER
);
```

```cypher
// GraphDB
CREATE TAG person (
    name STRING,
    age INT
);
```

**插入数据**：
```sql
-- PostgreSQL
INSERT INTO person (name, age) VALUES ('Alice', 30);
```

```cypher
// GraphDB
INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);
```

**查询数据**：
```sql
-- PostgreSQL
SELECT * FROM person WHERE age > 25;
```

```cypher
// GraphDB
MATCH (p:person) WHERE p.age > 25 RETURN p;
```

**关联查询**：
```sql
-- PostgreSQL
SELECT p1.name, p2.name
FROM person p1
JOIN friend f ON p1.id = f.person_id
JOIN person p2 ON f.friend_id = p2.id;
```

```cypher
// GraphDB
MATCH (p1:person)-[:friend]->(p2:person)
RETURN p1.name, p2.name;
```

### 5.2 最佳实践

1. **使用图遍历代替 JOIN**：图数据库擅长处理关联查询
2. **利用索引**：为常用查询属性创建索引
3. **合理设计 Tag 和 Edge**：根据业务场景设计图模型
4. **使用变量**：在复杂查询中使用变量提高可读性
5. **查看执行计划**：使用 EXPLAIN 优化查询性能

## 6. 总结

GraphDB CLI 在设计上参考了 psql 的优秀实践，同时针对图数据库的特点进行了优化和扩展。主要差异体现在：

1. **查询语言**：从 SQL 到 GQL，更符合图数据的查询模式
2. **对象模型**：从表和行到 Tag、Edge 和 Vertex
3. **特有功能**：路径查询、图遍历、多模型支持
4. **输出格式**：支持图数据的可视化展示

通过合理的功能映射和用户引导，可以帮助用户快速从关系数据库迁移到图数据库，并充分利用图数据库的优势。
