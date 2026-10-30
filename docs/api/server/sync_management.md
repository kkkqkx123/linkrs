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

