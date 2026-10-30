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

