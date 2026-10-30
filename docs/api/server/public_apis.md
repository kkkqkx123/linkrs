## 1. Public APIs

### 1.1 Health Check

**Endpoint**: `GET /v1/health`

**Description**: Check server health status.

**Request**: None

**Response**:

```json
{
  "status": "healthy",
  "service": "linkrs",
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

