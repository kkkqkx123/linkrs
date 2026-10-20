#!/usr/bin/env sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The lexical inventory comes from the Rust lexer, the parser comes from the
# tree-sitter CLI and the wasm grammar is compiled from that parser. Every
# artifact is committed, so a regeneration that changes nothing must leave
# the working tree untouched.

cargo run -q -p linkrs-query --example export_keyword_inventory
git diff --exit-code -- crates/linkrs-grammar/data/keywords.json

node tools/grammar-codegen/gen.mjs
git diff --exit-code -- frontend/src/lib/utils/generated/

# The parser is regenerated in a scratch directory, because the vendored
# runtime headers track the tree-sitter runtime version and not the CLI.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cp crates/linkrs-grammar/grammar.js crates/linkrs-grammar/tree-sitter.json "$scratch/"
cp -R crates/linkrs-grammar/data "$scratch/"
(cd "$scratch" && tree-sitter generate --abi "$(cat "$root/crates/linkrs-grammar/abi-version")")
for artifact in parser.c grammar.json node-types.json; do
	diff -u "crates/linkrs-grammar/src/$artifact" "$scratch/src/$artifact"
done

# The wasm grammar is a build output of the frontend and is therefore not
# tracked. It is still compiled here, so a broken toolchain is reported by the
# gate instead of by the editor at runtime.
(cd crates/linkrs-grammar && tree-sitter build --wasm -o "$scratch/tree-sitter-linkrs.wasm")
grep -q "tree_sitter_linkrs" "$scratch/tree-sitter-linkrs.wasm"

echo "grammar artifacts in sync"
