# 2. Schema 扩展 API

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

