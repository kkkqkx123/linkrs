# 3. 数据浏览 API

### 3. 数据浏览 API

路径前缀: `/v1/data`

**身份与空间语义**

- 语句按**调用方会话身份**执行，权限门按该用户的角色判定；`enable_authorize = false`
  或 `single_user_mode = true` 时，按配置的默认身份（`auth.default_username`）执行。
- 目标空间不存在返回 `404 NOT_FOUND`；空间存在但标签/边类型不存在返回 `400 QUERY_ERROR`。

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

