#!/bin/bash
# sync-frontend-preview.sh
# Sync source files from frontend/ into frontend-preview/.
# Preview-only files (mock layer, double-mode client, config) are preserved.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$ROOT_DIR/frontend"
PREVIEW_DIR="$ROOT_DIR/frontend-preview"

if [ ! -d "$FRONTEND_DIR" ]; then
    echo "Error: frontend directory not found at $FRONTEND_DIR"
    exit 1
fi
if [ ! -d "$PREVIEW_DIR" ]; then
    echo "Error: frontend-preview directory not found at $PREVIEW_DIR"
    exit 1
fi

echo "=== Syncing frontend-preview from frontend ==="

# Mirror-delete files under $1 (preview tree) that have no counterpart under
# $2 (frontend tree). Relative paths are matched against $3 (space-separated
# basenames to preserve). Empty preview dirs left behind are pruned.
prune_deleted() {
    local preview_tree="$1" frontend_tree="$2" preserve="$3"
    (cd "$preview_tree" && find . -type f) | while read -r rel; do
        local base
        base="$(basename "$rel")"
        case " $preserve " in
            *" $base "*) continue ;;
        esac
        if [ ! -e "$frontend_tree/$rel" ]; then
            rm "$preview_tree/$rel"
            echo "  removed (deleted in frontend): ${rel#./}"
        fi
    done
    find "$preview_tree" -type d -empty -delete 2>/dev/null || true
}

# Static assets
echo "Syncing public/ ..."
mkdir -p "$PREVIEW_DIR/public"
cp -r "$FRONTEND_DIR/public/." "$PREVIEW_DIR/public/" 2>/dev/null || true
[ -d "$FRONTEND_DIR/public" ] && prune_deleted "$PREVIEW_DIR/public" "$FRONTEND_DIR/public" ""

# Entry files and global styles
echo "Syncing src root files ..."
for f in index.html app.css app.d.ts app.html; do
    [ -f "$FRONTEND_DIR/src/$f" ] && cp "$FRONTEND_DIR/src/$f" "$PREVIEW_DIR/src/"
done

# Types, utils, config, stores, services: whole directories
echo "Syncing lib directories (types, utils, config, stores, services, i18n, assets) ..."
for dir in types utils config stores services i18n assets; do
    if [ -d "$FRONTEND_DIR/src/lib/$dir" ]; then
        mkdir -p "$PREVIEW_DIR/src/lib/$dir"
        cp -r "$FRONTEND_DIR/src/lib/$dir/." "$PREVIEW_DIR/src/lib/$dir/"
        prune_deleted "$PREVIEW_DIR/src/lib/$dir" "$FRONTEND_DIR/src/lib/$dir" ""
    fi
done

# Project-local tooling (i18n key checker, ...)
echo "Syncing scripts/ ..."
if [ -d "$FRONTEND_DIR/scripts" ]; then
    mkdir -p "$PREVIEW_DIR/scripts"
    cp -r "$FRONTEND_DIR/scripts/." "$PREVIEW_DIR/scripts/"
    prune_deleted "$PREVIEW_DIR/scripts" "$FRONTEND_DIR/scripts" ""
fi

# Components and pages: whole directories
echo "Syncing components and pages ..."
for dir in components pages; do
    if [ -d "$FRONTEND_DIR/src/lib/$dir" ]; then
        mkdir -p "$PREVIEW_DIR/src/lib/$dir"
        cp -r "$FRONTEND_DIR/src/lib/$dir/." "$PREVIEW_DIR/src/lib/$dir/"
        prune_deleted "$PREVIEW_DIR/src/lib/$dir" "$FRONTEND_DIR/src/lib/$dir" ""
    fi
done

# API modules except client.ts (preview keeps its mock-enabled version)
echo "Syncing api modules (preserving preview client.ts) ..."
mkdir -p "$PREVIEW_DIR/src/lib/api"
for file in "$FRONTEND_DIR/src/lib/api/"*; do
    base="$(basename "$file")"
    if [ "$base" != "client.ts" ]; then
        cp "$file" "$PREVIEW_DIR/src/lib/api/"
    fi
done
prune_deleted "$PREVIEW_DIR/src/lib/api" "$FRONTEND_DIR/src/lib/api" "client.ts"

# Root source files (main.ts, App.svelte, ...) except app.css/app.d.ts handled above
echo "Syncing remaining src files ..."
find "$FRONTEND_DIR/src" -maxdepth 1 -type f ! -name 'app.css' ! -name 'app.d.ts' ! -name 'app.html' \
    -exec cp {} "$PREVIEW_DIR/src/" \;

# OpenAPI contract used to regenerate types
echo "Syncing openapi.json ..."
cp "$FRONTEND_DIR/openapi.json" "$PREVIEW_DIR/openapi.json" 2>/dev/null || true

# Remove legacy codegen artifact superseded by src/lib/api/schema.d.ts
echo "Cleaning legacy generated types ..."
rm -f "$PREVIEW_DIR/src/lib/types/schema.gen.d.ts"

echo "=== Preserved preview-only files ==="
for f in \
    "$PREVIEW_DIR/package.json" \
    "$PREVIEW_DIR/vite.config.ts" \
    "$PREVIEW_DIR/svelte.config.js" \
    "$PREVIEW_DIR/tsconfig.json" \
    "$PREVIEW_DIR/tsconfig.app.json" \
    "$PREVIEW_DIR/tsconfig.node.json" \
    "$PREVIEW_DIR/.env" \
    "$PREVIEW_DIR/src/lib/api/client.ts"
do
    echo "  kept: $f"
done
echo "  kept: $PREVIEW_DIR/src/lib/mock/**"

# package.json is preview-owned, so the npm script wiring check:i18n depends on
# must be asserted here rather than copied from frontend/.
if ! grep -q '"check:i18n"' "$PREVIEW_DIR/package.json"; then
    echo "Error: $PREVIEW_DIR/package.json is missing the check:i18n script."
    exit 1
fi

echo "=== Done. Run: cd frontend-preview && npm install && npm run dev ==="
