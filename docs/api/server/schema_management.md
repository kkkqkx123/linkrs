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

