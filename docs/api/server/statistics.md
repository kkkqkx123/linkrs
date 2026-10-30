## 8. Statistics APIs

### 8.1 Get Session Statistics

**Endpoint**: `GET /v1/statistics/sessions/{id}`

**Description**: Get statistics for a specific session.

**Path Parameters**:

- `id` (i64): Session ID

**Response**:

```json
{
  "session_id": 12345,
  "username": "string",
  "statistics": {
    "total_queries": 10,
    "total_changes": 5,
    "last_insert_vertex_id": "vertex_id",
    "last_insert_edge_id": "edge_id",
    "avg_execution_time_ms": 50.5
  }
}
```

**Status Codes**:

- `200 OK`: Statistics retrieved
- `404 Not Found`: Session not found

---

### 8.2 Get Query Statistics

**Endpoint**: `GET /v1/statistics/queries`

**Description**: Get global query statistics.

**Query Parameters**:

- `from` (optional): Start time
- `to` (optional): End time

**Response**:

```json
{
  "total_queries": 1000,
  "global_query_total": 1000,
  "slow_queries": [
    {
      "trace_id": "uuid",
      "session_id": 12345,
      "query": "query text",
      "duration_ms": 5000.0,
      "status": "success"
    }
  ],
  "query_types": {
    "MATCH": 100,
    "CREATE": 50,
    "UPDATE": 30,
    "DELETE": 20,
    "INSERT": 40,
    "GO": 60,
    "FETCH": 70,
    "LOOKUP": 80,
    "SHOW": 90
  },
  "from": null,
  "to": null
}
```

**Status Codes**:

- `200 OK`: Statistics retrieved

---

### 8.3 Get Database Statistics

**Endpoint**: `GET /v1/statistics/database`

**Description**: Get database-level statistics.

**Response**:

```json
{
  "spaces": {
    "count": 5,
    "total_vertices": 10000,
    "total_edges": 5000
  },
  "storage": {
    "total_size_bytes": 0,
    "index_size_bytes": 0,
    "data_size_bytes": 0
  },
  "performance": {
    "total_queries": 1000,
    "global_query_total": 1000,
    "active_queries": 5,
    "query_cache_size": 100,
    "queries_per_second": 10.5,
    "avg_latency_ms": 50.0,
    "cache_hit_rate": 0.0
  }
}
```

**Status Codes**:

- `200 OK`: Statistics retrieved

---

### 8.4 Get System Statistics

**Endpoint**: `GET /v1/statistics/system`

**Description**: Get system resource usage statistics.

**Response**:

```json
{
  "cpu_usage_percent": 25.5,
  "memory_usage": {
    "used_bytes": 1073741824,
    "total_bytes": 8589934592
  },
  "connections": {
    "active": 10,
    "total": 10,
    "max": 100
  }
}
```

**Status Codes**:

- `200 OK`: Statistics retrieved

---

