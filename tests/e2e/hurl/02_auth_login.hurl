# Test 2: Authentication
POST http://127.0.0.1:9758/v1/auth/login
Content-Type: application/json
{
    "username": "root",
    "password": "root"
}
HTTP 200
[Captures]
session_id: jsonpath "$.session_id"
[Asserts]
jsonpath "$.session_id" exists
