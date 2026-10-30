## 3. Query Execution APIs

### 3.1 Execute Query

**Endpoint**: `POST /v1/query`

**Description**: Execute a graph query.

**Request Body**:

```json
{
  "query": "string",
  "session_id": 12345,
  "parameters": {}
}
```

**Response**:

```json
{
  "success": true,
  "data": {
    "columns": ["col1", "col2"],
    "rows": [{ "col1": "value1", "col2": "value2" }],
    "row_count": 1
  },
  "error": null,
  "metadata": {
    "execution_time_ms": 100,
    "rows_scanned": 1000,
    "rows_returned": 1,
    "space_id": null
  }
}
```

**Status Codes**:

- `200 OK`: Query executed
- `400 Bad Request`: Invalid query
- `500 Internal Server Error`: Execution error

---

### 3.2 Validate Query

**Endpoint**: `POST /v1/query/validate`

**Description**: Validate query syntax without execution.

**Request Body**:

```json
{
  "query": "string",
  "session_id": 12345,
  "parameters": {}
}
```

**Response**:

```json
{
  "valid": true,
  "message": "Syntax is correct"
}
```

**Status Codes**:

- `200 OK`: Validation complete

---

### 3.3 Execute Streaming Query

**Endpoint**: `POST /v1/query/stream`

**Description**: Execute a query and stream results using Server-Sent Events (SSE).

**Request Body**:

```json
{
  "query": "string",
  "session_id": 12345,
  "batch_size": 100
}
```

**Response**: SSE stream with events:

- `data`: Row data
- `metadata`: Query metadata
- `done`: Stream completion
- `error`: Error information

**Status Codes**:

- `200 OK`: Stream started

---

