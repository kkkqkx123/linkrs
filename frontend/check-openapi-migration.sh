#!/bin/bash
# Frontend OpenAPI contract guard.
# Ensures the checked-in generated types match `frontend/openapi.json`
# via the isolated codegen tool (mirrors code-context-engine flow).
#
# Usage:
#   bash frontend/check-openapi-migration.sh        # verify only
#   bash frontend/check-openapi-migration.sh --regen # regenerate then verify
#
# CI usage (fail on drift):
#   npm run gen:api && git diff --exit-code frontend/src/lib/api/schema.d.ts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$ROOT_DIR/frontend"
PREVIEW_DIR="$ROOT_DIR/frontend-preview"
CODEGEN_DIR="$ROOT_DIR/tools/openapi-codegen"
GENERATED="$FRONTEND_DIR/src/lib/api/schema.d.ts"
LEGACY="$FRONTEND_DIR/src/lib/types/schema.gen.d.ts"

echo "=== Checking codegen tool ==="
if [ ! -f "$CODEGEN_DIR/package.json" ]; then
    echo "Codegen tool not found at $CODEGEN_DIR"
    exit 1
fi
echo "Codegen tool exists: tools/openapi-codegen"

echo ""
echo "=== Checking generated schema.d.ts ==="
if [ ! -f "$GENERATED" ]; then
    echo "Generated schema file not found: $GENERATED"
    echo "Run: npm run gen:api (from frontend/) or npm run gen (from tools/openapi-codegen/)"
    exit 1
fi

SCHEMA_COUNT=$(grep -c "^export " "$GENERATED" || true)
echo "Found $SCHEMA_COUNT exported types in src/lib/api/schema.d.ts"
if [ "$SCHEMA_COUNT" -eq 0 ]; then
    echo "No types generated"
    exit 1
fi
echo "Schema generation verified"

echo ""
echo "=== Checking legacy artifact is gone ==="
if [ -f "$LEGACY" ]; then
    echo "Legacy file still present: $LEGACY (delete it, contract lives in src/lib/api/schema.d.ts)"
    exit 1
fi
if [ -f "$PREVIEW_DIR/src/lib/types/schema.gen.d.ts" ]; then
    echo "Legacy preview file still present: frontend-preview/src/lib/types/schema.gen.d.ts"
    exit 1
fi
echo "No legacy schema.gen.d.ts"

echo ""
echo "=== Checking for stale schema.gen imports ==="
if grep -rn "schema\.gen" "$FRONTEND_DIR/src/" 2>/dev/null; then
    echo "Stale '\$types/schema.gen' imports found in frontend/src (use '\$lib/api/schema' or './schema')"
    exit 1
fi
if [ -d "$PREVIEW_DIR/src" ] && grep -rn "schema\.gen" "$PREVIEW_DIR/src/" 2>/dev/null; then
    echo "Stale 'schema.gen' imports found in frontend-preview/src"
    exit 1
fi
echo "No stale schema.gen imports"

echo ""
echo "=== Checking generated types are fresh ==="
if [ "${1:-}" = "--regen" ]; then
    echo "Regenerating via tools/openapi-codegen ..."
    (cd "$CODEGEN_DIR" && npm run gen)
fi
TMP_GEN="$(mktemp)"
trap 'rm -f "$TMP_GEN"' EXIT
(cd "$CODEGEN_DIR" && npx openapi-typescript ../../frontend/openapi.json -o "$TMP_GEN")
if ! diff -q "$TMP_GEN" "$GENERATED" > /dev/null; then
    echo "Generated types are STALE: $GENERATED differs from openapi.json"
    echo "Run: npm run gen:api (from frontend/) then commit the result"
    exit 1
fi
echo "Generated types are fresh"

echo ""
echo "=== Verifying services use versioned API paths ==="
SERVICES=(
    "data.ts:/api/v1/data"
    "schema.ts:/api/v1/schema"
    "graph.ts:/api/v1/graph"
    "query.ts:/v1/query"
)
for service_info in "${SERVICES[@]}"; do
    IFS=':' read -r service expected_path <<< "$service_info"
    if grep -q "$expected_path" "$FRONTEND_DIR/src/lib/services/$service"; then
        echo "$service uses $expected_path"
    else
        echo "WARNING: $service doesn't use expected path $expected_path"
    fi
done

echo ""
echo "=== All checks passed! ==="
