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

