# 1. 查询历史和收藏夹 API

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

