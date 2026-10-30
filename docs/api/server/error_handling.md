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

