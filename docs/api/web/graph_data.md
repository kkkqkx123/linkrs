# 4. 图形数据 API

### 4. 图形数据 API

路径前缀: `/v1/graph`

身份与空间语义同第 3 节；此外，边的 `rank` 仅在 `/v1/graph/edges` 生效，`/v1/graph/vertices/{vid}/neighbors` 不做 rank 过滤。

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

