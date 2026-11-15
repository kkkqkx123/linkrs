#!/usr/bin/env bash
# Assemble the single-host bundle: one server binary plus one static dir.
#
# Layout of the bundle:
#   dist/linkrs-single/
#     linkrs-server       # serves the API and the static dir on one port
#     web/                # frontend static bundle (frontend/build content)
#     config.single.toml  # loopback listener with static hosting enabled
#     run-single.sh       # local start helper
#
# Usage:
#   deploy/scripts/package-single.sh [--debug] [--skip-frontend-install] \
#       [--api-base-url URL] [--port PORT]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST="$ROOT/dist/linkrs-single"
WEB="$DIST/web"

MODE="release"
PORT="9758"
API_BASE_URL=""
FRONT_ARGS=()
for arg in "$@"; do
    case "$arg" in
        --debug) MODE="debug" ;;
        --skip-frontend-install) FRONT_ARGS+=("--skip-install") ;;
        --api-base-url=*) API_BASE_URL="${arg#--api-base-url=}" ;;
        --port=*) PORT="${arg#--port=}" ;;
        -h|--help|help)
            sed -n '2,14p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

if [[ "$MODE" == "debug" ]]; then
    CARGO_ARGS="build"
else
    CARGO_ARGS="build --release"
fi

echo "==> Building backend ($MODE) ..."
(cd "$ROOT" && cargo $CARGO_ARGS --bin linkrs-server)
BIN="$ROOT/target/$MODE/linkrs-server"
if [[ ! -x "$BIN" ]]; then
    echo "Error: missing backend binary $BIN" >&2
    exit 1
fi

echo "==> Building frontend ..."
bash "$ROOT/deploy/scripts/build-frontend.sh" \
    --api-base-url="${API_BASE_URL:-http://127.0.0.1:$PORT}" "${FRONT_ARGS[@]}"
if [[ ! -f "$ROOT/frontend/build/index.html" ]]; then
    echo "Error: missing frontend bundle $ROOT/frontend/build/index.html" >&2
    exit 1
fi

echo "==> Assembling single-host bundle ..."
rm -rf "$DIST"
mkdir -p "$WEB"
install -m 755 "$BIN" "$DIST/linkrs-server"
cp -r "$ROOT/frontend/build/." "$WEB/"
chmod -R a+r "$WEB"

cat > "$DIST/config.single.toml" <<EOF
# Single-host bundle example: loopback API plus static on one port.
[database]
storage_path = "data"

[http]
bind_address = "127.0.0.1"
port = $PORT
static_dir = "web"
EOF

cat > "$DIST/run-single.sh" <<'EOF'
#!/usr/bin/env bash
# Start the single-host bundle from its own directory.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"
exec "$HERE/linkrs-server" serve --config "$HERE/config.single.toml"
EOF
chmod 755 "$DIST/run-single.sh"

echo "==> Bundle ready: $DIST"
ls "$DIST"
echo "    start: $DIST/run-single.sh"
echo "    open:  http://127.0.0.1:$PORT/"
echo "    note:  [http].static_dir takes effect once the backend"
echo "           implements static hosting per docs/plan/single-host-static-hosting-2026-11-15.md"
