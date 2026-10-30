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

