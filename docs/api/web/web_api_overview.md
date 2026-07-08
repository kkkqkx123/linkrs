# Web API 功能概览

本文档整理了 `src\api\server\web` 目录提供的所有 API 及功能。

## 目录

- [API 架构](#api-架构)
- [认证机制](#认证机制)
- [API 分类](#api-分类)
  - [1. 查询历史和收藏夹 API](#1-查询历史和收藏夹-api)
  - [2. Schema 扩展 API](#2-schema-扩展-api)
  - [3. 数据浏览 API](#3-数据浏览-api)
  - [4. 图形数据 API](#4-图形数据-api)
- [数据模型](#数据模型)
- [错误处理](#错误处理)
- [存储实现](#存储实现)

---

## API 架构

Web 模块基于 Axum 框架构建，提供 RESTful API 接口，主要包含以下组件：

- **Handlers**: 处理 HTTP 请求的业务逻辑
- **Models**: 定义请求和响应的数据结构
- **Services**: 提供业务逻辑层
- **Storage**: 元数据持久化层（基于 SQLite）
- **Middleware**: 认证和授权中间件

### 路由结构

```
/v1/queries     - 查询历史和收藏夹
/v1/schema      - Schema 扩展管理
/v1/data        - 数据浏览
/v1/graph       - 图形数据查询
```

---

## 认证机制

所有 Web API 都需要通过认证中间件验证。

### 认证方式

- **Header**: `X-Session-ID`
- **类型**: `i64` (会话 ID)
- **验证**: 会话 ID 必须在 Session Manager 中存在且有效

### 认证流程

1. 客户端在请求头中携带 `X-Session-ID`
2. 中间件验证会话 ID 是否有效
3. 如果有效，将 `session_id` 注入到请求扩展中
4. Handler 可以通过 `Extension<session_id>` 获取会话 ID

---

## API 分类

### 1. 查询历史和收藏夹 API

路径前缀: `/v1/queries`

#### 1.1 查询历史管理

| 方法 | 路径 | 描述 |
|------|------|------|
| POST | `/v1/queries/history` | 添加查询历史记录 |
| GET | `/v1/queries/history` | 列出查询历史（支持分页） |
| DELETE | `/v1/queries/history/{id}` | 删除指定历史记录 |
| DELETE | `/v1/queries/history/clear` | 清空所有历史记录 |

**请求示例 (POST):**
```json
{
  "query": "MATCH (v:person) RETURN v LIMIT 10",
  "execution_time_ms": 150,
  "rows_returned": 10,
  "success": true,
  "error_message": null
}
```

**查询参数 (GET):**
- `limit`: 每页数量（默认 20）
- `offset`: 偏移量（默认 0）

#### 1.2 查询收藏夹管理

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/queries/favorites` | 列出所有收藏 |
| POST | `/v1/queries/favorites` | 添加收藏 |
| GET | `/v1/queries/favorites/{id}` | 获取指定收藏 |
| PUT | `/v1/queries/favorites/{id}` | 更新收藏 |
| DELETE | `/v1/queries/favorites/{id}` | 删除收藏 |
| DELETE | `/v1/queries/favorites/clear` | 清空所有收藏 |

**请求示例 (POST):**
```json
{
  "name": "常用查询",
  "query": "MATCH (v:person) RETURN v LIMIT 10",
  "description": "查询所有人员"
}
```

**请求示例 (PUT):**
```json
{
  "name": "更新后的名称",
  "query": "MATCH (v:person) WHERE v.age > 18 RETURN v",
  "description": "查询成年人"
}
```

---

### 2. Schema 扩展 API

路径前缀: `/v1/schema`

#### 2.1 Space 管理

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/schema/spaces` | 列出所有 spaces |
| GET | `/v1/schema/spaces/{name}/details` | 获取 space 详情 |
| GET | `/v1/schema/spaces/{name}/statistics` | 获取 space 统计信息 |

**响应示例 (GET /spaces):**
```json
{
  "success": true,
  "data": {
    "spaces": [
      {
        "id": 1,
        "name": "my_space",
        "vid_type": "INT64"
      }
    ]
  }
}
```

#### 2.2 Tag 管理

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/schema/spaces/{name}/tags` | 列出所有 tags |
| POST | `/v1/schema/spaces/{name}/tags` | 创建 tag |
| GET | `/v1/schema/spaces/{name}/tags/{tag_name}` | 获取 tag 详情 |
| PUT | `/v1/schema/spaces/{name}/tags/{tag_name}` | 更新 tag（待实现） |
| DELETE | `/v1/schema/spaces/{name}/tags/{tag_name}` | 删除 tag |

**请求示例 (POST):**
```json
{
  "name": "person",
  "properties": [
    {
      "name": "name",
      "data_type": "STRING",
      "nullable": false,
      "comment": "姓名"
    },
    {
      "name": "age",
      "data_type": "INT32",
      "nullable": true,
      "comment": "年龄"
    }
  ]
}
```

#### 2.3 Edge Type 管理

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/schema/spaces/{name}/edge-types` | 列出所有 edge types |
| POST | `/v1/schema/spaces/{name}/edge-types` | 创建 edge type |
| GET | `/v1/schema/spaces/{name}/edge-types/{edge_name}` | 获取 edge type 详情 |
| PUT | `/v1/schema/spaces/{name}/edge-types/{edge_name}` | 更新 edge type（待实现） |
| DELETE | `/v1/schema/spaces/{name}/edge-types/{edge_name}` | 删除 edge type |

**请求示例 (POST):**
```json
{
  "name": "friend",
  "properties": [
    {
      "name": "since",
      "data_type": "DATE",
      "nullable": true,
      "comment": "成为朋友的日期"
    }
  ]
}
```

#### 2.4 Index 管理

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/schema/spaces/{name}/indexes` | 列出所有索引 |
| POST | `/v1/schema/spaces/{name}/indexes` | 创建索引 |
| GET | `/v1/schema/spaces/{name}/indexes/{index_name}` | 获取索引详情 |
| DELETE | `/v1/schema/spaces/{name}/indexes/{index_name}` | 删除索引 |
| POST | `/v1/schema/spaces/{name}/indexes/{index_name}/rebuild` | 重建索引 |

**请求示例 (POST):**
```json
{
  "name": "person_name_index",
  "index_type": "INDEX",
  "entity_type": "TAG",
  "entity_name": "person",
  "fields": ["name"],
  "comment": "人员姓名索引"
}
```

**支持的数据类型:**
- `BOOL`, `INT8`, `INT16`, `INT32`, `INT64`
- `UINT8`, `UINT16`, `UINT32`, `UINT64`
- `FLOAT`, `DOUBLE`, `STRING`
- `DATE`, `TIME`, `DATETIME`, `TIMESTAMP`

---

### 3. 数据浏览 API

路径前缀: `/v1/data`

#### 3.1 Vertex 浏览

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/data/spaces/{name}/tags/{tag_name}/vertices` | 按标签浏览顶点 |

**查询参数:**
- `limit`: 每页数量（默认 20）
- `offset`: 偏移量（默认 0）
- `filter`: 属性过滤条件（如 `"age>18"`）
- `sort_by`: 排序字段
- `sort_order`: 排序顺序（ASC/DESC）

**示例请求:**
```
GET /v1/data/spaces/my_space/tags/person/vertices?limit=10&filter=age>18&sort_by=name&sort_order=ASC
```

#### 3.2 Edge 浏览

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/data/spaces/{name}/edge-types/{edge_name}/edges` | 按边类型浏览边 |

**查询参数:**
- `limit`: 每页数量（默认 20）
- `offset`: 偏移量（默认 0）
- `filter`: 属性过滤条件
- `sort_by`: 排序字段
- `sort_order`: 排序顺序（ASC/DESC）

---

### 4. 图形数据 API

路径前缀: `/v1/graph`

#### 4.1 Vertex 查询

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/graph/vertices/{vid}` | 获取顶点详情 |

**查询参数:**
- `space`: 空间名称

**示例请求:**
```
GET /v1/graph/vertices/12345?space=my_space
```

#### 4.2 Edge 查询

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/graph/edges` | 获取边详情 |

**查询参数:**
- `space`: 空间名称
- `src`: 源顶点 ID
- `dst`: 目标顶点 ID
- `edge_type`: 边类型
- `rank`: 边的 rank（默认 0）

**示例请求:**
```
GET /v1/graph/edges?space=my_space&src=12345&dst=67890&edge_type=friend&rank=0
```

#### 4.3 Neighbor 查询

| 方法 | 路径 | 描述 |
|------|------|------|
| GET | `/v1/graph/vertices/{vid}/neighbors` | 获取顶点的邻居 |

**查询参数:**
- `space`: 空间名称
- `direction`: 方向（OUT/IN/BOTH，默认 BOTH）
- `edge_type`: 边类型过滤（可选）

**示例请求:**
```
GET /v1/graph/vertices/12345/neighbors?space=my_space&direction=OUT&edge_type=friend
```

---

## 数据模型

### 标准响应格式

```rust
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ApiError>,
}
```

### 分页响应

```rust
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}
```

### 查询历史模型

```rust
pub struct HistoryItem {
    pub id: String,
    pub session_id: String,
    pub query: String,
    pub executed_at: DateTime<Utc>,
    pub execution_time_ms: i64,
    pub rows_returned: i64,
    pub success: bool,
    pub error_message: Option<String>,
}
```

### 收藏夹模型

```rust
pub struct FavoriteItem {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub query: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}
```

### Space 详情模型

```rust
pub struct SpaceDetail {
    pub id: u64,
    pub name: String,
    pub vid_type: String,
    pub partition_num: i32,
    pub replica_factor: i32,
    pub comment: Option<String>,
    pub created_at: i64,
    pub statistics: SpaceStatistics,
}
```

### Tag/Edge Type 详情模型

```rust
pub struct TagDetail {
    pub id: i64,
    pub name: String,
    pub properties: Vec<PropertyDef>,
    pub indexes: Vec<IndexInfo>,
    pub created_at: i64,
}

pub struct PropertyDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
}
```

---

## 错误处理

### 错误类型

| 错误代码 | HTTP 状态码 | 描述 |
|----------|------------|------|
| `STORAGE_ERROR` | 500 | 存储层错误 |
| `BAD_REQUEST` | 400 | 请求参数错误 |
| `NOT_FOUND` | 404 | 资源不存在 |
| `UNAUTHORIZED` | 401 | 未授权访问 |
| `INTERNAL_ERROR` | 500 | 内部服务器错误 |
| `QUERY_ERROR` | 400 | 查询执行错误 |

### 错误响应格式

```json
{
  "success": false,
  "error": {
    "code": "NOT_FOUND",
    "message": "Space 'my_space' not found"
  }
}
```

---

## 存储实现

### SQLite 存储

元数据（查询历史和收藏夹）使用 SQLite 持久化存储。

#### 数据表结构

**query_history 表:**
```sql
CREATE TABLE query_history (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    query TEXT NOT NULL,
    executed_at TIMESTAMP NOT NULL,
    execution_time_ms INTEGER NOT NULL,
    rows_returned INTEGER NOT NULL,
    success BOOLEAN NOT NULL,
    error_message TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
)
```

**query_favorites 表:**
```sql
CREATE TABLE query_favorites (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    name TEXT NOT NULL,
    query TEXT NOT NULL,
    description TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
)
```

#### 索引

- `idx_history_session`: (session_id, executed_at DESC)
- `idx_favorites_session`: (session_id, created_at DESC)

---

## 技术栈

- **Web 框架**: Axum
- **异步运行时**: Tokio
- **序列化**: serde
- **数据库**: SQLite (sqlx)
- **日期时间**: chrono
- **UUID**: uuid

---

## 待实现功能

以下功能已预留接口，但尚未完全实现：

1. **Tag 更新**: `PUT /v1/schema/spaces/{name}/tags/{tag_name}` - 需要核心 API 提供 `alter_tag` 功能
2. **Edge Type 更新**: `PUT /v1/schema/spaces/{name}/edge-types/{edge_name}` - 需要核心 API 提供 `alter_edge_type` 功能
3. **Index 管理**: 部分索引管理功能需要核心 API 支持
4. **统计信息**: Space 统计中的顶点和边数量估算需要核心 API 支持

---

## 相关文件

### Handlers
- [handlers/mod.rs](file:///d:/项目/database/graphDB/src/api/server/web/handlers/mod.rs)
- [handlers/data_browser.rs](file:///d:/项目/database/graphDB/src/api/server/web/handlers/data_browser.rs)
- [handlers/graph_data.rs](file:///d:/项目/database/graphDB/src/api/server/web/handlers/graph_data.rs)
- [handlers/metadata.rs](file:///d:/项目/database/graphDB/src/api/server/web/handlers/metadata.rs)
- [handlers/schema_ext.rs](file:///d:/项目/database/graphDB/src/api/server/web/handlers/schema_ext.rs)

### Models
- [models/mod.rs](file:///d:/项目/database/graphDB/src/api/server/web/models/mod.rs)
- [models/metadata.rs](file:///d:/项目/database/graphDB/src/api/server/web/models/metadata.rs)
- [models/schema.rs](file:///d:/项目/database/graphDB/src/api/server/web/models/schema.rs)

### Services
- [services/mod.rs](file:///d:/项目/database/graphDB/src/api/server/web/services/mod.rs)
- [services/metadata_service.rs](file:///d:/项目/database/graphDB/src/api/server/web/services/metadata_service.rs)
- [services/schema_service.rs](file:///d:/项目/database/graphDB/src/api/server/web/services/schema_service.rs)

### Storage
- [storage/mod.rs](file:///d:/项目/database/graphDB/src/api/server/web/storage/mod.rs)
- [storage/sqlite.rs](file:///d:/项目/database/graphDB/src/api/server/web/storage/sqlite.rs)

### 其他
- [mod.rs](file:///d:/项目/database/graphDB/src/api/server/web/mod.rs)
- [error.rs](file:///d:/项目/database/graphDB/src/api/server/web/error.rs)
- [middleware.rs](file:///d:/项目/database/graphDB/src/api/server/web/middleware.rs)
