//! Corpus parity gate: the handwritten parser and the tree-sitter subset
//! grammar must agree on every corpus statement.
//!
//! The corpus lives in `tests/corpus/` as one statement per `.gql` file so
//! the semantic runner can reuse it:
//! - `structured/` holds statements the grammar covers structurally. Their
//!   trees must contain no `tail` fallback node.
//! - `tail/` holds statements the handwritten parser accepts but the subset
//!   grammar only covers through its generic `tail` rule. Their trees must
//!   contain a `tail` node, pinning the exact subset boundary.
//!
//! Counts are exact: every file must parse with the handwritten parser (no
//! silent skips), so shrinking the corpus or regressing the grammar fails
//! loudly instead of weakening the gate.

use std::path::{Path, PathBuf};

use linkrs_query::parser::Parser;

/// Exact corpus sizes. Update together with the `tests/corpus/` files when
/// the grammar subset boundary moves deliberately.
const STRUCTURED_COUNT: usize = 47;
const TAIL_COUNT: usize = 25;

fn corpus_files(dir: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(dir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read corpus dir {}: {e}", root.display()))
        .map(|entry| entry.expect("corpus entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gql"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "corpus dir {} must contain .gql files",
        root.display()
    );
    files
}

fn load_statement(path: &Path) -> String {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let stmt = text.trim().to_string();
    assert!(
        !stmt.is_empty(),
        "corpus file {} must hold one statement",
        path.display()
    );
    assert!(
        !stmt.contains('\n'),
        "corpus file {} must hold exactly one statement",
        path.display()
    );
    stmt
}

fn assert_token_cover(stmt: &str) {
    let toks = linkrs_grammar::tokens(stmt);
    assert!(
        !toks.is_empty(),
        "parity failure for {stmt:?}: empty token list"
    );
    let mut last = 0;
    for t in &toks {
        assert!(
            t.start < t.end,
            "parity failure for {stmt:?}: empty span {t:?}"
        );
        assert!(
            t.end <= stmt.len(),
            "parity failure for {stmt:?}: span {t:?} out of bounds"
        );
        assert!(
            t.start >= last,
            "parity failure for {stmt:?}: spans out of order {t:?}"
        );
        last = t.start;
        assert!(
            [
                "keyword",
                "variable",
                "property",
                "function",
                "number",
                "string",
                "operator",
                "comment",
                "punctuation"
            ]
            .contains(&t.class),
            "parity failure for {stmt:?}: unknown class {t:?}"
        );
    }
}

#[test]
fn structured_corpus_has_no_tail() {
    let files = corpus_files("structured");
    assert_eq!(
        files.len(),
        STRUCTURED_COUNT,
        "structured corpus size drift: update STRUCTURED_COUNT with the corpus files"
    );
    for path in &files {
        let stmt = load_statement(path);
        let mut parser = Parser::new(&stmt);
        let parsed = parser.parse();
        assert!(
            parsed.is_ok(),
            "structured corpus {} must parse: {:?}",
            path.display(),
            parsed.err()
        );
        let outcome = linkrs_grammar::parse(&stmt);
        assert!(
            !outcome.has_error,
            "parity failure for {stmt:?}: tree contains ERROR/MISSING"
        );
        assert!(
            !linkrs_grammar::has_tail(&stmt),
            "parity failure for {stmt:?}: structured statement grew a tail node"
        );
        assert_token_cover(&stmt);
    }
}

#[test]
fn tail_corpus_requires_tail() {
    let files = corpus_files("tail");
    assert_eq!(
        files.len(),
        TAIL_COUNT,
        "tail corpus size drift: update TAIL_COUNT with the corpus files"
    );
    for path in &files {
        let stmt = load_statement(path);
        let mut parser = Parser::new(&stmt);
        let parsed = parser.parse();
        assert!(
            parsed.is_ok(),
            "tail corpus {} must parse: {:?}",
            path.display(),
            parsed.err()
        );
        let outcome = linkrs_grammar::parse(&stmt);
        assert!(
            !outcome.has_error,
            "parity failure for {stmt:?}: tree contains ERROR/MISSING"
        );
        assert!(
            linkrs_grammar::has_tail(&stmt),
            "parity failure for {stmt:?}: tail-covered statement lost its tail node"
        );
        assert_token_cover(&stmt);
    }
}
