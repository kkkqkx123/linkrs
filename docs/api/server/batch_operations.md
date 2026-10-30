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

