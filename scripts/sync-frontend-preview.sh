#!/usr/bin/env bash
# Sync frontend sources into frontend-preview (mirror semantics).
#
# Everything under frontend/src plus the support dirs (vite/, public/,
# messages/, project.inlang/) and shared config files is copied over,
# so the preview renders exactly what the studio renders. Only the
# preview-specific files (.env, vite.config.ts, package.json name/scripts,
# README.md) are preserved; run this after any frontend change.
#
# Usage: ./scripts/sync-frontend-preview.sh   (from the repository root)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/frontend"
DST="$ROOT/frontend-preview"

if [ ! -d "$SRC" ]; then
	echo "error: $SRC not found" >&2
	exit 1
fi

mkdir -p "$DST"

copy() {
	# copy <relative-path> — file or directory, overwrite unconditionally
	local rel="$1"
	if [ ! -e "$SRC/$rel" ]; then
		echo "skip (missing in frontend): $rel"
		return
	fi
	mkdir -p "$DST/$(dirname "$rel")"
	rm -rf "$DST/$rel"
	cp -R "$SRC/$rel" "$DST/$rel"
}

# --- Source tree (excluding generated output) -------------------------
rm -rf "$DST/src"
mkdir -p "$DST/src"
(
	cd "$SRC"
	find src -type d \( \
		-path src/lib/paraglide -o \
		-path src/lib/assets \
		\) -prune -o -type f -print
) | while IFS= read -r file; do
	mkdir -p "$DST/$(dirname "$file")"
	cp "$SRC/$file" "$DST/$file"
done

# --- Support directories ----------------------------------------------
copy vite
copy public
copy messages
mkdir -p "$DST/project.inlang"
find "$SRC/project.inlang" -path '*/cache' -prune -o -type f -print |
	while IFS= read -r file; do
		rel="${file#"$SRC"/}"
		mkdir -p "$DST/$(dirname "$rel")"
		cp "$file" "$DST/$rel"
	done

# --- Shared config files ----------------------------------------------
for f in openapi.json tsconfig.json eslint.config.js .prettierrc .prettierignore; do
	copy "$f"
done
mkdir -p "$DST/scripts"
cp "$SRC/scripts/check-i18n.mjs" "$DST/scripts/check-i18n.mjs"

# --- package.json: sync deps only (name/scripts stay preview-specific) --
node -e '
const fs = require("fs");
const src = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const dstPath = process.argv[2];
const dst = JSON.parse(fs.readFileSync(dstPath, "utf8"));
dst.dependencies = src.dependencies;
dst.devDependencies = src.devDependencies;
fs.writeFileSync(dstPath, JSON.stringify(dst, null, "\t") + "\n");
' "$SRC/package.json" "$DST/package.json"

echo "frontend-preview synced from frontend."
