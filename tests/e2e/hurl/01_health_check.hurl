# Test 1: Health Check
GET http://127.0.0.1:9758/v1/health
HTTP 200
[Asserts]
jsonpath "$.service" == "graphdb"
jsonpath "$.status" == "healthy"
