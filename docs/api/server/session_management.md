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

