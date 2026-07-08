# Test 5: Create Tag
POST http://127.0.0.1:9758/v1/auth/login
Content-Type: application/json
{
    "username": "root",
    "password": "root"
}
HTTP 200
[Captures]
session_id: jsonpath "$.session_id"

POST http://127.0.0.1:9758/v1/query
Content-Type: application/json
X-Session-ID: {{session_id}}
{
    "query": "USE hurl_test_space",
    "session_id": {{session_id}},
    "parameters": {}
}
HTTP 200

POST http://127.0.0.1:9758/v1/query
Content-Type: application/json
X-Session-ID: {{session_id}}
{
    "query": "CREATE TAG IF NOT EXISTS person(name STRING NOT NULL, age INT)",
    "session_id": {{session_id}},
    "parameters": {}
}
HTTP 200
[Asserts]
jsonpath "$.success" == true
