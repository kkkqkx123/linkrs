## 12. Web Management APIs

The Web APIs are mounted under `/api/*` and provide extended functionality for data browsing and management.

### 12.1 Data Browser

#### 12.1.1 List Vertices by Tag

**Endpoint**: `GET /api/spaces/{name}/tags/{tag_name}/vertices`

**Description**: Browse vertices by tag with filtering and pagination.

**Query Parameters**:

- `limit` (i64): Page size (default: 20)
- `offset` (i64): Offset (default: 0)
- `filter` (String): Property filter (e.g., "age>18")
- `sort_by` (String): Sort field
- `sort_order` (String): Sort order (ASC/DESC)

**Response**:

```json
{
  "success": true,
  "data": {
    "items": [{"vertex": {...}}],
    "total": 100,
    "limit": 20,
    "offset": 0
  }
}
```

---

#### 12.1.2 List Edges by Type

**Endpoint**: `GET /api/spaces/{name}/edge-types/{edge_name}/edges`

**Description**: Browse edges by type with filtering and pagination.

**Query Parameters**: Same as vertices

**Response**: Similar to vertices response

---

### 12.2 Graph Data

#### 12.2.1 Get Vertex

**Endpoint**: `GET /api/vertices/{vid}`

**Description**: Get vertex details by ID.

**Query Parameters**:

- `space` (String): Space name (required)

**Response**:

```json
{
  "success": true,
  "data": {"vertex": {...}}
}
```

---

#### 12.2.2 Get Edge

**Endpoint**: `GET /api/edges`

**Description**: Get edge details.

**Query Parameters**:

- `space` (String): Space name (required)
- `src` (String): Source vertex ID (required)
- `dst` (String): Target vertex ID (required)
- `edge_type` (String): Edge type (required)
- `rank` (i64): Edge rank (default: 0)

**Response**:

```json
{
  "success": true,
  "data": {"edge": {...}}
}
```

---

#### 12.2.3 Get Neighbors

**Endpoint**: `GET /api/vertices/{vid}/neighbors`

**Description**: Get neighbors of a vertex.

**Query Parameters**:

- `space` (String): Space name (required)
- `direction` (String): Direction (OUT/IN/BOTH, default: BOTH)
- `edge_type` (String): Edge type filter (optional)

**Response**:

```json
{
  "success": true,
  "data": {
    "vid": "vertex_id",
    "space": "space_name",
    "direction": "BOTH",
    "edge_type": null,
    "neighbors": [{"vertex": {...}}]
  }
}
```

---

### 12.3 Metadata Management

#### 12.3.1 Add History

**Endpoint**: `POST /api/history`

**Description**: Add a query to history.

**Request Body**:

```json
{
  "query": "string",
  "execution_time_ms": 100,
  "rows_returned": 10,
  "success": true
}
```

---

#### 12.3.2 List History

**Endpoint**: `GET /api/history`

**Description**: List query history.

**Query Parameters**:

- `limit` (i64): Page size
- `offset` (i64): Offset

---

#### 12.3.3 Add Favorite

**Endpoint**: `POST /api/favorites`

**Description**: Add a query to favorites.

**Request Body**:

```json
{
  "name": "string",
  "query": "string",
  "description": "string"
}
```

---

#### 12.3.4 List Favorites

**Endpoint**: `GET /api/favorites`

**Description**: List favorite queries.

---

### 12.4 Schema Extension

#### 12.4.1 List Spaces (Extended)

**Endpoint**: `GET /api/spaces`

**Description**: List all spaces with extended details.

---

#### 12.4.2 Get Space Details

**Endpoint**: `GET /api/spaces/{name}/details`

**Description**: Get detailed space information including statistics.

---

#### 12.4.3 List Tags (Extended)

**Endpoint**: `GET /api/spaces/{name}/tags`

**Description**: List all tags in a space.

---

#### 12.4.4 List Edge Types (Extended)

**Endpoint**: `GET /api/spaces/{name}/edge-types`

**Description**: List all edge types in a space.

---

#### 12.4.5 List Indexes

**Endpoint**: `GET /api/spaces/{name}/indexes`

**Description**: List all indexes in a space.

---

