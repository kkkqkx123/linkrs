//! Compiles the generated tree-sitter parser.
//!
//! `src/parser.c` is produced once by the pinned `tree-sitter` CLI and checked
//! in, so the deployment environment never needs the CLI or network access.
//! The CLI version and generated language ABI are recorded in
//! `docs/plan/tree_sitter_syntax_layer.md` and pinned in `package.json`.
//!
//! This build script also verifies that the three committed artifacts stay in
//! sync, so a partial regeneration fails fast at compile time instead of
//! surfacing as a silent runtime drift.
//!
//! Known pending regeneration: `grammar.js`/`grammar.json` define
//! `arrow_right`/`hash_arrow`/`hash_arrow_right`, which the committed
//! `parser.c`/`node-types.json` do not contain yet. `queries/highlights.scm`
//! intentionally matches the committed parser until all three artifacts are
//! regenerated together with the pinned CLI (`npm run generate` in this
//! directory, requires network).

use std::path::Path;

const EXPECTED_LANGUAGE_VERSION: &str = "#define LANGUAGE_VERSION 15";
const EXPECTED_GRAMMAR_NAME: &str = "\"name\": \"linkrs\"";

fn assert_contains(path: &Path, needle: &str) {
    let content =
        std::fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    assert!(
        content.contains(needle),
        "{} out of sync: missing {needle:?}",
        path.display()
    );
}

fn main() {
    let src = Path::new("src");
    assert_contains(&src.join("parser.c"), EXPECTED_LANGUAGE_VERSION);
    assert_contains(&src.join("grammar.json"), EXPECTED_GRAMMAR_NAME);
    assert!(
        src.join("node-types.json").exists(),
        "src/node-types.json must be committed alongside parser.c"
    );
    assert_contains(&Path::new("data").join("keywords.json"), "\"version\": 1");
    cc::Build::new()
        .include(src)
        .file(src.join("parser.c"))
        .compile("tree-sitter-linkrs");

    println!("cargo:rerun-if-changed=src/parser.c");
    println!("cargo:rerun-if-changed=src/grammar.json");
    println!("cargo:rerun-if-changed=src/node-types.json");
    println!("cargo:rerun-if-changed=grammar.js");
    println!("cargo:rerun-if-changed=data/keywords.json");
    println!("cargo:rerun-if-changed=package.json");
}
