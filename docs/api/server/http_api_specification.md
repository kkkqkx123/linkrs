# GraphDB HTTP API Specification

This document describes all HTTP APIs provided by the GraphDB server in `src/api/server`.

## Overview

The GraphDB HTTP API is organized into the following categories:

- **Public APIs**: Health check and authentication (no authentication required)
- **Core APIs**: Query execution, session management, transactions (authentication required)
- **Schema APIs**: Space, Tag, and Edge Type management
- **Batch APIs**: Bulk data insertion
- **Vector APIs**: Vector index and search operations
- **Statistics APIs**: Query and system statistics
- **Config APIs**: Configuration management
- **Function APIs**: Custom function management
- **Web APIs**: Extended data browsing and management interfaces

Base URL: `http://{host}:{port}/v1`

---

## 1. Public APIs

### 1.1 Health Check

**Endpoint**: `GET /v1/health`

**Description**: Check server health status.

**Request**: None

**Response**:

```json
{
  "status": "healthy",
  "service": "graphdb",
  "version": "0.1.0"
}
```

**Status Codes**:

- `200 OK`: Server is healthy

---

### 1.2 Login

**Endpoint**: `POST /v1/auth/login`

**Description**: Authenticate user and create session.

**Request Body**:

```json
{
  "username": "string",
  "password": "string"
}
```

**Response**:

```json
{
  "session_id": 12345,
  "username": "string",
  "expires_at": null
}
```

**Status Codes**:

- `200 OK`: Login successful
- `401 Unauthorized`: Invalid credentials

---

### 1.3 Logout

**Endpoint**: `POST /v1/auth/logout`

**Description**: Logout and invalidate session.

**Request Body**:

```json
{
  "session_id": 12345
}
```

**Response**: `204 No Content`

**Status Codes**:

- `204 No Content`: Logout successful

---

## 2. Session Management APIs

### 2.1 Create Session

**Endpoint**: `POST /v1/sessions`

**Description**: Create a new session for the authenticated user.

**Request Body**:

```json
{
  "username": "string",
  "client_ip": "string"
}
```

**Response**:

```json
{
  "session_id": 12345,
  "username": "string",
  "created_at": 1234567890
}
```

**Status Codes**:

- `200 OK`: Session created
- `400 Bad Request`: Invalid request

---

### 2.2 Get Session

**Endpoint**: `GET /v1/sessions/{id}`

**Description**: Get session details by ID.

**Path Parameters**:

- `id` (i64): Session ID

**Response**:

```json
{
  "session_id": 12345,
  "username": "string",
  "space_name": "string",
  "graph_addr": "string",
  "timezone": "string"
}
```

**Status Codes**:

- `200 OK`: Session found
- `404 Not Found`: Session not found

---

### 2.3 Delete Session

**Endpoint**: `DELETE /v1/sessions/{id}`

**Description**: Delete a session.

**Path Parameters**:

- `id` (i64): Session ID

**Response**: `204 No Content`

**Status Codes**:

- `204 No Content`: Session deleted
- `404 Not Found`: Session not found

---

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

## 4. Transaction Management APIs

### 4.1 Begin Transaction

**Endpoint**: `POST /v1/transactions`

**Description**: Start a new transaction.

**Request Body**:

```json
{
  "session_id": 12345,
  "read_only": false,
  "timeout_seconds": null,
  "query_timeout_seconds": null,
  "statement_timeout_seconds": null,
  "idle_timeout_seconds": null
}
```

**Response**:

```json
{
  "transaction_id": 12345,
  "status": "Active"
}
```

**Status Codes**:

- `200 OK`: Transaction started
- `500 Internal Server Error`: Failed to begin transaction

---

### 4.2 Commit Transaction

**Endpoint**: `POST /v1/transactions/{id}/commit`

**Description**: Commit a transaction.

**Path Parameters**:

- `id` (u64): Transaction ID

**Request Body**:

```json
{
  "session_id": 12345
}
```

**Response**:

```json
{
  "message": "Transaction committed successfully",
  "transaction_id": 12345
}
```

**Status Codes**:

- `200 OK`: Transaction committed
- `500 Internal Server Error`: Commit failed

---

### 4.3 Rollback Transaction

**Endpoint**: `POST /v1/transactions/{id}/rollback`

**Description**: Rollback a transaction.

**Path Parameters**:

- `id` (u64): Transaction ID

**Request Body**:

```json
{
  "session_id": 12345
}
```

**Response**:

```json
{
  "message": "Transaction rolled back successfully",
  "transaction_id": 12345
}
```

**Status Codes**:

- `200 OK`: Transaction rolled back
- `500 Internal Server Error`: Rollback failed

---

## 5. Schema Management APIs

### 5.1 List Spaces

**Endpoint**: `GET /v1/schema/spaces`

**Description**: List all graph spaces.

**Response**:

```json
{
  "spaces": [
    {
      "id": 1,
      "name": "space_name",
      "vid_type": "STRING",
      "comment": null
    }
  ]
}
```

**Status Codes**:

- `200 OK`: List retrieved

---

### 5.2 Create Space

**Endpoint**: `POST /v1/schema/spaces`

**Description**: Create a new graph space.

**Request Body**:

```json
{
  "name": "string",
  "vid_type": "STRING",
  "comment": null
}
```

**Response**:

```json
{
  "message": "Space created successfully",
  "space_name": "string"
}
```

**Status Codes**:

- `200 OK`: Space created
- `500 Internal Server Error`: Creation failed

---

### 5.3 Get Space

**Endpoint**: `GET /v1/schema/spaces/{name}`

**Description**: Get space details.

**Path Parameters**:

- `name` (String): Space name

**Response**:

```json
{
  "space": {
    "name": "space_name",
    "id": 1
  }
}
```

**Status Codes**:

- `200 OK`: Space found
- `404 Not Found`: Space not found

---

### 5.4 Drop Space

**Endpoint**: `DELETE /v1/schema/spaces/{name}`

**Description**: Delete a graph space.

**Path Parameters**:

- `name` (String): Space name

**Response**:

```json
{
  "message": "Space deleted successfully",
  "space_name": "string"
}
```

**Status Codes**:

- `200 OK`: Space deleted
- `500 Internal Server Error`: Deletion failed

---

### 5.5 List Tags

**Endpoint**: `GET /v1/schema/spaces/{name}/tags`

**Description**: List all tags in a space.

**Path Parameters**:

- `name` (String): Space name

**Response**:

```json
{
  "tags": [],
  "space_name": "string",
  "note": "This feature is pending implementation"
}
```

**Status Codes**:

- `200 OK`: List retrieved

---

### 5.6 Create Tag

**Endpoint**: `POST /v1/schema/spaces/{name}/tags`

**Description**: Create a new tag in a space.

**Path Parameters**:

- `name` (String): Space name

**Request Body**:

```json
{
  "name": "string",
  "properties": [
    {
      "name": "string",
      "data_type": "string",
      "nullable": false
    }
  ]
}
```

**Response**:

```json
{
  "message": "Tag created successfully",
  "tag_name": "string",
  "space_name": "string"
}
```

**Status Codes**:

- `200 OK`: Tag created
- `500 Internal Server Error`: Creation failed

---

### 5.7 List Edge Types

**Endpoint**: `GET /v1/schema/spaces/{name}/edge-types`

**Description**: List all edge types in a space.

**Path Parameters**:

- `name` (String): Space name

**Response**:

```json
{
  "edge_types": [],
  "space_name": "string",
  "note": "This feature is pending implementation"
}
```

**Status Codes**:

- `200 OK`: List retrieved

---

### 5.8 Create Edge Type

**Endpoint**: `POST /v1/schema/spaces/{name}/edge-types`

**Description**: Create a new edge type in a space.

**Path Parameters**:

- `name` (String): Space name

**Request Body**:

```json
{
  "name": "string",
  "properties": [
    {
      "name": "string",
      "data_type": "string",
      "nullable": false
    }
  ]
}
```

**Response**:

```json
{
  "message": "Edge type created successfully",
  "edge_type_name": "string",
  "space_name": "string"
}
```

**Status Codes**:

- `200 OK`: Edge type created
- `500 Internal Server Error`: Creation failed

---

## 6. Batch Operation APIs

### 6.1 Create Batch Task

**Endpoint**: `POST /v1/batch`

**Description**: Create a new batch task for bulk insertion.

**Request Body**:

```json
{
  "space_id": 1,
  "batch_type": "vertex",
  "batch_size": 1000
}
```

**Batch Types**: `vertex`, `edge`, `mixed`

**Response**:

```json
{
  "batch_id": "uuid-string",
  "status": "created",
  "created_at": "2024-01-01T00:00:00Z"
}
```

**Status Codes**:

- `200 OK`: Task created
- `500 Internal Server Error`: Creation failed

---

### 6.2 Get Batch Status

**Endpoint**: `GET /v1/batch/{id}`

**Description**: Get batch task status.

**Path Parameters**:

- `id` (String): Batch ID

**Response**:

```json
{
  "batch_id": "uuid-string",
  "status": "created",
  "progress": {
    "total": 0,
    "processed": 0,
    "succeeded": 0,
    "failed": 0,
    "buffered": 0
  },
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

**Status Codes**:

- `200 OK`: Status retrieved
- `404 Not Found`: Batch not found

---

### 6.3 Add Batch Items

**Endpoint**: `POST /v1/batch/{id}/items`

**Description**: Add items to a batch task.

**Path Parameters**:

- `id` (String): Batch ID

**Request Body**:

```json
{
  "items": [
    {
      "type": "vertex",
      "data": {
        "vid": "vertex_id",
        "tags": ["tag1"],
        "properties": { "key": "value" }
      }
    },
    {
      "type": "edge",
      "data": {
        "edge_type": "type_name",
        "src_vid": "source_id",
        "dst_vid": "target_id",
        "properties": { "key": "value" }
      }
    }
  ]
}
```

**Response**:

```json
{
  "accepted": 10,
  "buffered": 10,
  "total_buffered": 10
}
```

**Status Codes**:

- `200 OK`: Items added
- `400 Bad Request`: Invalid items
- `404 Not Found`: Batch not found

---

### 6.4 Execute Batch

**Endpoint**: `POST /v1/batch/{id}/execute`

**Description**: Execute the batch task.

**Path Parameters**:

- `id` (String): Batch ID

**Response**:

```json
{
  "batch_id": "uuid-string",
  "status": "completed",
  "result": {
    "vertices_inserted": 10,
    "edges_inserted": 5,
    "errors": []
  },
  "completed_at": "2024-01-01T00:00:00Z"
}
```

**Status Codes**:

- `200 OK`: Batch executed
- `404 Not Found`: Batch not found
- `500 Internal Server Error`: Execution failed

---

### 6.5 Cancel Batch

**Endpoint**: `POST /v1/batch/{id}/cancel`

**Description**: Cancel a batch task.

**Path Parameters**:

- `id` (String): Batch ID

**Response**:

```json
{
  "message": "Batch task cancelled",
  "batch_id": "uuid-string"
}
```

**Status Codes**:

- `200 OK`: Batch cancelled
- `400 Bad Request`: Cannot cancel

---

### 6.6 Delete Batch

**Endpoint**: `DELETE /v1/batch/{id}`

**Description**: Delete a batch task.

**Path Parameters**:

- `id` (String): Batch ID

**Response**:

```json
{
  "message": "Batch task deleted",
  "batch_id": "uuid-string"
}
```

**Status Codes**:

- `200 OK`: Batch deleted
- `404 Not Found`: Batch not found

---

## 7. Vector Index APIs

### 7.1 List Vector Indexes

**Endpoint**: `GET /v1/vector/indexes`

**Description**: List all vector indexes.

**Response**:

```json
{
  "indexes": ["index1", "index2"],
  "count": 2
}
```

**Status Codes**:

- `200 OK`: List retrieved
- `500 Internal Server Error`: Vector API not available

---

### 7.2 Create Vector Index

**Endpoint**: `POST /v1/vector/indexes`

**Description**: Create a new vector index.

**Request Body**:

```json
{
  "space_id": 1,
  "tag_name": "string",
  "field_name": "string",
  "vector_size": 128,
  "distance": "cosine"
}
```

**Distance Metrics**: `cosine`, `euclidean`, `dot`

**Response**:

```json
{
  "success": true,
  "message": "Vector index created successfully",
  "collection_name": "space_1_tag_field"
}
```

**Status Codes**:

- `200 OK`: Index created
- `500 Internal Server Error`: Creation failed

---

### 7.3 Get Vector Index Info

**Endpoint**: `GET /v1/vector/indexes/{space_id}/{tag_name}/{field_name}`

**Description**: Get vector index details.

**Path Parameters**:

- `space_id` (u64): Space ID
- `tag_name` (String): Tag name
- `field_name` (String): Field name

**Response**:

```json
{
  "collection_name": "space_1_tag_field",
  "status": "green",
  "vectors_count": 1000,
  "points_count": 0,
  "indexed_vectors_count": 0,
  "vector_size": 128,
  "distance": "Cosine"
}
```

**Status Codes**:

- `200 OK`: Info retrieved
- `404 Not Found`: Index not found
- `500 Internal Server Error`: Vector API not available

---

### 7.4 Drop Vector Index

**Endpoint**: `DELETE /v1/vector/indexes/{space_id}/{tag_name}/{field_name}`

**Description**: Delete a vector index.

**Path Parameters**:

- `space_id` (u64): Space ID
- `tag_name` (String): Tag name
- `field_name` (String): Field name

**Response**:

```json
{
  "success": true,
  "message": "Vector index dropped successfully"
}
```

**Status Codes**:

- `200 OK`: Index dropped
- `500 Internal Server Error`: Deletion failed

---

### 7.5 Search Vectors

**Endpoint**: `POST /v1/vector/search`

**Description**: Search for similar vectors.

**Request Body**:

```json
{
  "space_id": 1,
  "tag_name": "string",
  "field_name": "string",
  "query_vector": [0.1, 0.2, 0.3],
  "limit": 10,
  "threshold": 0.8,
  "filter": null
}
```

**Response**:

```json
{
  "results": [
    {
      "id": "point_id",
      "score": 0.95,
      "vector": [0.1, 0.2, 0.3],
      "payload": null
    }
  ],
  "count": 1
}
```

**Status Codes**:

- `200 OK`: Search completed
- `500 Internal Server Error`: Search failed

---

### 7.6 Get Vector Point

**Endpoint**: `GET /v1/vector/{space_id}/{tag_name}/{field_name}/{point_id}`

**Description**: Get a specific vector point by ID.

**Path Parameters**:

- `space_id` (u64): Space ID
- `tag_name` (String): Tag name
- `field_name` (String): Field name
- `point_id` (String): Point ID

**Response**:

```json
{
  "success": true,
  "point": {
    "id": "point_id",
    "vector": [0.1, 0.2, 0.3],
    "payload": null
  }
}
```

**Status Codes**:

- `200 OK`: Point retrieved
- `500 Internal Server Error`: Retrieval failed

---

### 7.7 Get Vector Count

**Endpoint**: `GET /v1/vector/{space_id}/{tag_name}/{field_name}/count`

**Description**: Get the count of vectors in an index.

**Path Parameters**:

- `space_id` (u64): Space ID
- `tag_name` (String): Tag name
- `field_name` (String): Field name

**Response**:

```json
{
  "success": true,
  "count": 1000
}
```

**Status Codes**:

- `200 OK`: Count retrieved
- `500 Internal Server Error`: Retrieval failed

---

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

## 9. Configuration APIs

### 9.1 Get Configuration

**Endpoint**: `GET /v1/config`

**Description**: Get current server configuration.

**Response**:

```json
{
  "database": {
    "host": "127.0.0.1",
    "port": 3699,
    "storage_path": "./data",
    "max_connections": 100
  },
  "transaction": {
    "default_timeout": 30,
    "max_concurrent_transactions": 100
  },
  "log": {
    "level": "info",
    "dir": "./logs",
    "file": "graphdb.log",
    "max_file_size": 104857600,
    "max_files": 5
  },
  "auth": {
    "enable_authorize": true,
    "failed_login_attempts": 3,
    "session_idle_timeout_secs": 3600,
    "force_change_default_password": true,
    "default_username": "root"
  },
  "bootstrap": {
    "auto_create_default_space": true,
    "default_space_name": "default",
    "single_user_mode": false
  },
  "optimizer": {
    "max_iteration_rounds": 100,
    "max_exploration_rounds": 10,
    "enable_cost_model": true,
    "enable_multi_plan": true,
    "enable_property_pruning": true,
    "enable_adaptive_iteration": true,
    "stable_threshold": 5,
    "min_iteration_rounds": 10
  },
  "monitoring": {
    "enabled": true,
    "memory_cache_size": 1000,
    "slow_query_threshold_ms": 1000
  }
}
```

**Status Codes**:

- `200 OK`: Configuration retrieved

---

### 9.2 Update Configuration

**Endpoint**: `PUT /v1/config`

**Description**: Update server configuration (hot update where supported).

**Request Body**:

```json
{
  "database": {
    "max_connections": 200
  }
}
```

**Response**:

```json
{
  "updated": ["database.max_connections"],
  "requires_restart": [],
  "message": "Configuration update received, some changes may require restart to take effect"
}
```

**Status Codes**:

- `200 OK`: Configuration updated

---

### 9.3 Get Configuration Key

**Endpoint**: `GET /v1/config/{section}/{key}`

**Description**: Get a specific configuration value.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3699
}
```

**Status Codes**:

- `200 OK`: Value retrieved

---

### 9.4 Update Configuration Key

**Endpoint**: `PUT /v1/config/{section}/{key}`

**Description**: Update a specific configuration value.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Request Body**:

```json
{
  "value": 3700
}
```

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3700,
  "requires_restart": true,
  "message": "Configuration item updated, but restart required to take effect"
}
```

**Status Codes**:

- `200 OK`: Value updated

---

### 9.5 Reset Configuration Key

**Endpoint**: `DELETE /v1/config/{section}/{key}`

**Description**: Reset a configuration value to default.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3699,
  "message": "Configuration reset to default value"
}
```

**Status Codes**:

- `200 OK`: Value reset

---

## 10. Function Management APIs

### 10.1 List Functions

**Endpoint**: `GET /v1/functions`

**Description**: List all registered functions.

**Response**:

```json
{
  "functions": [{ "name": "function1" }, { "name": "function2" }],
  "total": 2
}
```

**Status Codes**:

- `200 OK`: List retrieved

---

### 10.2 Register Function

**Endpoint**: `POST /v1/functions`

**Description**: Register a custom function.

**Request Body**:

```json
{
  "name": "string",
  "type": "string",
  "parameters": ["param1", "param2"],
  "return_type": "string",
  "description": "string",
  "implementation": null
}
```

**Response**:

```json
{
  "function_id": "hash-string",
  "name": "string",
  "function_type": "string",
  "parameters": ["param1", "param2"],
  "return_type": "string",
  "status": "registered",
  "message": "Function registered successfully"
}
```

**Status Codes**:

- `200 OK`: Function registered
- `400 Bad Request`: Function already exists

---

### 10.3 Get Function Info

**Endpoint**: `GET /v1/functions/{name}`

**Description**: Get function details.

**Path Parameters**:

- `name` (String): Function name

**Response**:

```json
{
  "name": "function_name",
  "type": "builtin",
  "is_builtin": true,
  "is_custom": false,
  "parameters": [],
  "return_type": "any",
  "registered_at": "2024-01-01T00:00:00Z"
}
```

**Status Codes**:

- `200 OK`: Info retrieved
- `404 Not Found`: Function not found

---

### 10.4 Unregister Function

**Endpoint**: `DELETE /v1/functions/{name}`

**Description**: Unregister a custom function.

**Path Parameters**:

- `name` (String): Function name

**Response**:

```json
{
  "message": "Function unregistered",
  "name": "function_name"
}
```

**Status Codes**:

- `200 OK`: Function unregistered
- `400 Bad Request`: Cannot unregister built-in function
- `404 Not Found`: Function not found

---

## 11. Sync Management APIs

### 11.1 Get Sync Status

**Endpoint**: `GET /v1/sync/status`

**Description**: Get data synchronization status.

**Response**:

```json
{
  "is_running": false,
  "dlq_size": 0,
  "unrecovered_dlq_size": 0
}
```

**Status Codes**:

- `200 OK`: Status retrieved

---

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

## Authentication

Most APIs (except `/v1/health`, `/v1/auth/login`, `/v1/auth/logout`) require authentication. The authentication middleware validates the session token from the request headers.

### Authentication Flow

1. Call `POST /v1/auth/login` with username and password
2. Receive `session_id` in response
3. Include session information in subsequent requests (implementation-specific, typically via headers or request body)

---

## Error Handling

All APIs return consistent error responses:

```json
{
  "success": false,
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable error message",
    "details": "Additional details (optional)"
  }
}
```

### Common HTTP Status Codes

- `200 OK`: Request successful
- `201 Created`: Resource created
- `204 No Content`: Request successful, no content to return
- `400 Bad Request`: Invalid request parameters
- `401 Unauthorized`: Authentication required or failed
- `404 Not Found`: Resource not found
- `500 Internal Server Error`: Server-side error

---

## Data Types

### Property Data Types

- `BOOL` / `BOOLEAN`: Boolean values
- `SMALLINT` / `INT16`: 16-bit integer
- `INT` / `INT32` / `INTEGER`: 32-bit integer
- `BIGINT` / `INT64`: 64-bit integer
- `FLOAT` / `REAL`: 32-bit floating point
- `DOUBLE` / `DOUBLE PRECISION`: 64-bit floating point
- `STRING`: Variable-length string
- `DATE`: Date value
- `TIME`: Time value
- `DATETIME`: Date and time value
- `TIMESTAMP`: Timestamp value

### Batch Status Values

- `created`: Task created
- `running`: Task is running
- `completed`: Task completed successfully
- `failed`: Task failed
- `cancelled`: Task was cancelled

### Batch Types

- `vertex`: Vertex batch insertion
- `edge`: Edge batch insertion
- `mixed`: Mixed batch insertion

---

## Rate Limiting and Timeouts

- **Request Body Limit**: 10 MB
- **Request Timeout**: 30 seconds (configurable)
- **CORS**: Enabled for all origins in development (should be restricted in production)

---

## gRPC API

The server also provides gRPC APIs when the `grpc` feature is enabled. See [feature flag documentation](../../build/feature_flags_design.md) for the current build-time configuration.
