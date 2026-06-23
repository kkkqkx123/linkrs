# 向量检索与全文检索隔离机制调研报告

## 1. PostgreSQL 的隔离机制

### 1.1 全文检索隔离机制

PostgreSQL 的全文检索（Full Text Search）主要通过以下机制实现隔离：

#### Schema 命名空间隔离

- **核心机制**：PostgreSQL 使用 **Schema** 作为命名空间来隔离数据库对象
- **索引隔离**：全文检索索引（GIN/GiST 索引）与表存储在同一个 Schema 中，通过 Schema 名称实现逻辑隔离
- **创建语法**：
  ```sql
  CREATE INDEX pgweb_idx ON pgweb USING GIN (to_tsvector('english', body));
  ```
- **特点**：
  - 索引名称在 Schema 内必须唯一
  - 不同 Schema 可以有相同名称的索引
  - 通过 `schemaname.indexname` 唯一标识索引

#### 扩展机制（Extension）

- **pgvector 扩展**：PostgreSQL 通过扩展机制支持向量检索
- **扩展隔离**：
  - 扩展可以安装在特定 Schema 中：`CREATE EXTENSION vector SCHEMA myschema;`
  - 扩展对象（包括向量类型、操作符、索引方法）在指定 Schema 内隔离
  - 可重定位扩展（relocatable）支持在不同 Schema 间移动

#### Tablespace 物理隔离

- **物理存储**：PostgreSQL 支持通过 TABLESPACE 指定索引的物理存储位置
- **语法**：
  ```sql
  CREATE INDEX idx_name ON table_name USING GIN (column) TABLESPACE tablespace_name;
  ```
- **用途**：可将不同 Schema 或租户的数据分散存储在不同磁盘/存储介质上

#### 多租户隔离方案

| 隔离级别   | 实现方式            | 适用场景       |
| ---------- | ------------------- | -------------- |
| 逻辑隔离   | Schema 分离         | 中小规模多租户 |
| 物理隔离   | Tablespace + Schema | 性能敏感场景   |
| 数据库隔离 | 独立 Database       | 强隔离需求     |

### 1.2 向量检索隔离机制（pgvector）

pgvector 扩展的隔离机制：

- **Schema 级隔离**：向量类型和索引操作符在指定 Schema 中定义
- **表级隔离**：向量列作为表的一部分，继承表的 Schema 隔离
- **索引隔离**：向量索引（ivfflat、hnsw）与普通索引一样遵循 Schema 命名空间规则

---

## 2. Neo4j 的隔离机制

### 2.1 全文检索隔离机制

Neo4j 的全文检索基于 **Apache Lucene** 实现，隔离机制如下：

#### 数据库级隔离

- **多数据库支持**：Neo4j 4.0+ 支持在一个实例中运行多个独立数据库
- **索引隔离**：全文索引在创建时绑定到特定数据库
- **创建语法**：
  ```cypher
  CREATE FULLTEXT INDEX ProductName FOR (n:Product) ON EACH [n.name]
  ```
- **特点**：
  - 全文索引名称在数据库内唯一
  - 不同数据库可以有相同名称的索引
  - 通过 `databaseName.indexName` 实现跨数据库区分

#### Composite Database（Fabric）隔离

- **联邦查询**：Neo4j Fabric 允许跨多个数据库查询
- **索引可见性**：每个数据库的索引仅在该数据库内可见
- **跨库查询**：通过 `USE` 子句指定数据库上下文

### 2.2 向量检索隔离机制

Neo4j 的向量索引（5.15+）隔离机制：

#### 数据库内隔离

- **索引命名空间**：向量索引名称在数据库内唯一
- **标签级隔离**：向量索引可以绑定到特定节点标签
- **创建语法**：
  ```cypher
  CREATE VECTOR INDEX moviePlots FOR (m:Movie) ON (m.plotEmbedding)
  ```

#### 多标签支持（2026.01+）

- **跨标签索引**：支持为多个标签创建统一的向量索引
- **隔离边界**：即使在多标签索引中，数据仍然通过标签区分

#### 隔离特性

| 特性       | 说明                       |
| ---------- | -------------------------- |
| 数据库隔离 | 索引完全隔离在不同数据库中 |
| 标签隔离   | 索引可限定到特定节点标签   |
| 属性隔离   | 每个索引只针对一个向量属性 |

---

## 3. 对比总结

### 3.1 隔离机制对比

| 数据库     | 隔离层级 | 核心机制 | 索引命名规则              |
| ---------- | -------- | -------- | ------------------------- |
| PostgreSQL | Schema   | 命名空间 | `{schema}.{index_name}`   |
| Neo4j      | Database | 多数据库 | `{database}.{index_name}` |

### 3.2 向量检索对比

| 特性     | PostgreSQL (pgvector)         | Neo4j        |
| -------- | ----------------------------- | ------------ |
| 索引类型 | ivfflat, hnsw                 | 专用向量索引 |
| 隔离级别 | Schema                        | Database     |
| 距离函数 | 多种内置                      | 余弦相似度等 |
| 维度限制 | 16000 (hnsw) / 2000 (ivfflat) | 2048         |

### 3.3 全文检索对比

| 特性       | PostgreSQL    | Neo4j         |
| ---------- | ------------- | ------------- |
| 底层引擎   | 内置 GIN/GiST | Apache Lucene |
| 文本分析   | 多语言配置    | 可配置分析器  |
| 多字段索引 | 支持          | 支持          |
| 跨标签/表  | 单表/多表     | 多标签支持    |

---

## 4. 最佳实践总结

### 4.1 PostgreSQL 最佳实践

1. **Schema 设计**：为每个租户/业务域创建独立 Schema
2. **索引命名**：使用 `{prefix}_{table}_{field}` 格式，避免冲突
3. **Tablespace 使用**：对大数据量租户使用独立 Tablespace
4. **权限控制**：通过 Schema 级权限实现访问控制

### 4.2 Neo4j 最佳实践

1. **数据库规划**：为强隔离需求的租户创建独立数据库
2. **索引命名**：使用 `{entity}_{field}_index` 格式
3. **标签设计**：利用标签自然隔离不同实体类型
4. **Fabric 使用**：跨数据库查询时使用 Fabric 统一访问

---

## 5. 参考资料

- [PostgreSQL 17 Documentation - Full Text Search](https://www.postgresql.org/docs/17/textsearch.html)
- [PostgreSQL 17 Documentation - Schemas](https://www.postgresql.org/docs/17/ddl-schemas.html)
- [Neo4j Documentation - Full-Text Indexes](https://neo4j.com/docs/cypher-manual/current/indexes/semantic-indexes/full-text-indexes)
- [Neo4j Documentation - Vector Indexes](https://neo4j.com/docs/cypher-manual/current/indexes/semantic-indexes/vector-indexes)
