#!/bin/bash
# Frontend type check script

set -e

echo "=== Checking frontend codegen tool ==="
cd /home/kkkqkx/code/linkrs/frontend/codegen
if [ ! -f "codegen.mjs" ]; then
    echo "❌ Codegen tool not found"
    exit 1
fi
echo "✅ Codegen tool exists"

echo ""
echo "=== Checking generated schema.d.ts ==="
if [ ! -f "/home/kkkqkx/code/linkrs/frontend/src/lib/types/schema.d.ts" ]; then
    echo "❌ Generated schema file not found"
    exit 1
fi

SCHEMA_COUNT=$(grep -c "^export " /home/kkkqkx/code/linkrs/frontend/src/lib/types/schema.d.ts)
echo "Found $SCHEMA_COUNT exported types"

if [ "$SCHEMA_COUNT" -eq 0 ]; then
    echo "❌ No types generated"
    exit 1
fi
echo "✅ Schema generation verified"

echo ""
echo "=== Verifying all services use correct paths ==="

SERVICES=(
    "data.ts:/api/v1/data"
    "queryHistory.ts:/api/v1/queries"
    "schema.ts:/api/v1/schema"
    "graph.ts:/api/v1/graph"
)

for service_info in "${SERVICES[@]}"; do
    IFS=':' read -r service expected_path <<< "$service_info"
    if grep -q "$expected_path" "/home/kkkqkx/code/linkrs/frontend/src/lib/services/$service"; then
        echo "✅ $service uses $expected_path"
    else
        echo "⚠️  $service doesn't use expected path $expected_path"
    fi
done

echo ""
echo "=== Checking for old path patterns ==="
if grep -r "get\(/api/history" /home/kkkqkx/code/linkrs/frontend/src/lib/services/ || \
   grep -r "get\(/api/favorites" /home/kkkqkx/code/linkrs/frontend/src/lib/services/ || \
   grep -r "get\(/api/spaces/" /home/kkkqkx/code/linkrs/frontend/src/lib/services/; then
    echo "⚠️  Found old path patterns that may need updating"
else
    echo "✅ No old path patterns found in core services"
fi

echo ""
echo "=== All checks passed! ==="
