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

