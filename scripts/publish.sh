#!/usr/bin/env bash
set -euo pipefail

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
fi

# Crates to publish in dependency order. Submodules (tantivy, simvec,
# llm-suite, gpui-kit) are excluded: they have their own release flows.
# The root package is intentionally omitted — only sub-crates are
# published from this script.
CRATES=(
    "crates/linkrs-metrics"
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
    "crates/vector-client"
)

for crate in "${CRATES[@]}"; do
    name=$(basename "$crate")
    # Extract the package version; fall back to the workspace version
    # for crates using `version.workspace = true`.
    version=$(sed -n 's/^version *= *"\(.*\)".*/\1/p' "$crate/Cargo.toml" | head -1)
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
    set +e
    if $DRY_RUN; then
        (cd "$crate" && cargo publish --dry-run --allow-dirty --registry crates-io)
    else
        (cd "$crate" && cargo publish --allow-dirty --registry crates-io)
    fi
    rc=$?
    set -e
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
