#!/usr/bin/env bash
# Build the linkrs-studio frontend static bundle into frontend/build/.
#
# The bundle is plain static files (SvelteKit adapter-static with an
# index.html fallback) and can be served by the backend static hosting
# on the same port as the API. API_BASE_URL is baked into the bundle at
# build time, so pass the address the single-host bundle will listen on.
#
# Usage:
#   deploy/scripts/build-frontend.sh [--skip-install] [--api-base-url URL]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FRONT="$ROOT/frontend"
OUT="$FRONT/build"

SKIP_INSTALL=0
API_BASE_URL="${API_BASE_URL:-http://127.0.0.1:9758}"
for arg in "$@"; do
    case "$arg" in
        --skip-install) SKIP_INSTALL=1 ;;
        --api-base-url=*) API_BASE_URL="${arg#--api-base-url=}" ;;
        -h|--help|help)
            sed -n '2,10p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

if [[ ! -f "$FRONT/package.json" ]]; then
    echo "Error: frontend project not found at $FRONT" >&2
    exit 1
fi
if ! command -v npm >/dev/null 2>&1; then
    echo "Error: npm not found; install Node.js first" >&2
    exit 1
fi

if [[ "$SKIP_INSTALL" -eq 0 ]]; then
    echo "==> Installing frontend dependencies ..."
    (cd "$FRONT" && npm ci)
else
    echo "==> Skipping dependency install (--skip-install)"
fi

echo "==> Building frontend (API_BASE_URL=$API_BASE_URL) ..."
(cd "$FRONT" && API_BASE_URL="$API_BASE_URL" npm run build)

if [[ ! -f "$OUT/index.html" ]]; then
    echo "Error: frontend build produced no $OUT/index.html" >&2
    exit 1
fi
echo "==> Frontend build complete: $OUT"
