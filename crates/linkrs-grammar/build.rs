//! Compiles the generated tree-sitter parser and guards the committed
//! artifacts against drift.
//!
//! `src/parser.c` is produced once by the pinned `tree-sitter` CLI and checked
//! in, so the deployment environment never needs the CLI or network access.
//! The CLI version and generated language ABI are recorded in
//! `docs/plan/tree_sitter_syntax_layer.md` and pinned in `package.json` and
//! `abi-version`.
//!
//! This build script verifies that the committed artifacts stay in sync, so a
//! partial regeneration fails fast at compile time instead of surfacing as a
//! silent runtime drift.
//!
//! The committed `parser.c` already contains `arrow_right`/`hash_arrow`/
//! `hash_arrow_right`; `queries/highlights.scm` is aligned with that parser.
//! Regeneration still requires the pinned CLI and network; see
//! `docs/grammer/tree-sitter-generation.md`.

use std::path::Path;

const EXPECTED_GRAMMAR_NAME: &str = "\"name\": \"linkrs\"";
const INVENTORY_VERSION: &str = "\"version\": 1";

fn assert_contains(path: &Path, needle: &str) {
    let content =
        std::fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    assert!(
        content.contains(needle),
        "{} out of sync: missing {needle:?}",
        path.display()
    );
}

/// The language ABI every generated artifact must agree on.
fn expected_abi_version() -> u32 {
    let content = std::fs::read_to_string("abi-version")
        .unwrap_or_else(|_| panic!("read abi-version"));
    content
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("abi-version must be a number, got {content:?}"))
}

fn main() {
    let src = Path::new("src");

    assert_contains(
        &src.join("parser.c"),
        &format!("#define LANGUAGE_VERSION {}", expected_abi_version()),
    );
    assert_contains(&src.join("grammar.json"), EXPECTED_GRAMMAR_NAME);
    assert!(
        src.join("node-types.json").exists(),
        "src/node-types.json must be committed alongside parser.c"
    );
    assert_contains(&Path::new("data").join("keywords.json"), INVENTORY_VERSION);

    if std::env::var_os("CARGO_FEATURE_PARSER").is_some() {
        cc::Build::new()
            .include(src)
            .file(src.join("parser.c"))
            .compile("tree-sitter-linkrs");
    }

    println!("cargo:rerun-if-changed=src/parser.c");
    println!("cargo:rerun-if-changed=src/grammar.json");
    println!("cargo:rerun-if-changed=src/node-types.json");
    println!("cargo:rerun-if-changed=grammar.js");
    println!("cargo:rerun-if-changed=tree-sitter.json");
    println!("cargo:rerun-if-changed=abi-version");
    println!("cargo:rerun-if-changed=data/keywords.json");
    println!("cargo:rerun-if-changed=package.json");
}
