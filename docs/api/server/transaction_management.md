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

