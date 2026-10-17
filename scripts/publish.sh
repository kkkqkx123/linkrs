#!/usr/bin/env bash
set -euo pipefail

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
fi

# Crates to publish in dependency order. The final entry (`.`) is the
# umbrella `linkrs` crate. Submodules (tantivy, simvec, llm-suite,
# gpui-kit) are excluded: they live in their own repositories and ship
# through their own release flows (simvec has a dedicated script).
CRATES=(
    "crates/linkrs-metrics"
    "crates/vector-client"
    "crates/linkrs-core"
    "crates/linkrs-config"
    "crates/linkrs-wire"
    "crates/linkrs-fulltext"
    "crates/linkrs-sync"
    "crates/linkrs-transaction"
    "crates/linkrs-storage"
    "crates/linkrs-migration"
    "crates/linkrs-query"
    "crates/linkrs-api"
    "crates/linkrs-server"
    "crates/linkrs-cli"
    "crates/linkrs-migration-cli"
    "."
)

for crate in "${CRATES[@]}"; do
    manifest="$crate/Cargo.toml"
    # The package name comes from the manifest, not the directory: the two
    # can differ (e.g. crates/linkrs-cli holds package `linkrs_cli`).
    name=$(awk '
        /^\[package\]/ { in_pkg = 1; next }
        /^\[/ { in_pkg = 0 }
        in_pkg && /^name[[:space:]]*=/ {
            sub(/^[^"]*"/, ""); sub(/".*/, ""); print; exit
        }
    ' "$manifest")
    # Extract the package version; fall back to the workspace version
    # for crates using `version.workspace = true`.
    version=$(sed -n 's/^version *= *"\(.*\)".*/\1/p' "$manifest" | head -1)
    if [[ -z "$version" ]]; then
        version=$(sed -n 's/^version *= *"\(.*\)".*/\1/p' Cargo.toml | head -1)
    fi
    echo "=== Publishing $name ==="
    # Skip if this version is already on crates.io (User-Agent is
    # required by the crates.io API).
    if curl -sf -A "linkrs-publish-script" -o /dev/null "https://crates.io/api/v1/crates/$name/$version"; then
        echo "Skipping $name v$version: already published."
        echo ""
        continue
    fi
    # linkrs-fulltext has a dev-dependency on linkrs-sync, creating a
    # publish-time cycle (linkrs-sync depends on linkrs-fulltext). Strip
    # the dev-dependency and skip verification (tests require linkrs-sync),
    # then restore the manifest.
    restore_manifest=""
    if [[ "$crate" == "crates/linkrs-fulltext" ]]; then
        if grep -q '^linkrs-sync = ' "$manifest"; then
            sed -i 's/^linkrs-sync = /# linkrs-sync = /' "$manifest"
            restore_manifest="linkrs-sync"
        fi
    fi

    set +e
    if $DRY_RUN; then
        if [[ -n "$restore_manifest" ]]; then
            (cd "$crate" && cargo publish --dry-run --no-verify --allow-dirty --registry crates-io)
        else
            (cd "$crate" && cargo publish --dry-run --allow-dirty --registry crates-io)
        fi
    else
        if [[ -n "$restore_manifest" ]]; then
            (cd "$crate" && cargo publish --no-verify --allow-dirty --registry crates-io)
        else
            (cd "$crate" && cargo publish --allow-dirty --registry crates-io)
        fi
    fi
    rc=$?
    set -e
    if [[ -n "$restore_manifest" ]]; then
        sed -i 's/^# linkrs-sync = /linkrs-sync = /' "$manifest"
    fi
    if [[ $rc -ne 0 ]]; then
        # In dry-run mode, unpublished internal dependencies cannot be
        # resolved from the real index yet; warn instead of aborting.
        if $DRY_RUN; then
            echo "WARNING: dry-run failed for $name (likely unpublished internal deps); continuing."
        else
            exit $rc
        fi
    fi
done

echo "All crates published successfully."
