#!/usr/bin/env sh
set -eu
root=$(dirname "$0")/..
cd "$root"

cargo run -q -p linkrs-query --example export_keyword_inventory
git diff --exit-code -- crates/linkrs-grammar/data/keywords.json

node tools/grammar-codegen/gen.mjs
git diff --exit-code -- frontend/src/lib/utils/generated/

echo "grammar inventory in sync"
