## Authentication

Most APIs (except `/v1/health`, `/v1/auth/login`, `/v1/auth/logout`) require authentication. The authentication middleware validates the session token from the request headers.

### Authentication Flow

1. Call `POST /v1/auth/login` with username and password
2. Receive `session_id` in response
3. Include session information in subsequent requests (implementation-specific, typically via headers or request body)

---

