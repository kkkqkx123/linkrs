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

