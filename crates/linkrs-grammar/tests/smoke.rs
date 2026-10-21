use linkrs_grammar::{language, parse, tokens};
use tree_sitter::Query;

#[test]
fn highlights_query_loads() {
    let lang = language();
    Query::new(&lang, include_str!("../queries/highlights.scm"))
        .expect("highlights query must load");
}

#[test]
fn smoke_parse_match_return() {
    let source = "MATCH (a) RETURN a";
    let outcome = parse(source);
    assert!(
        !outcome.has_error,
        "smoke statement must parse without ERROR/MISSING"
    );
    let toks = tokens(source);
    assert!(!toks.is_empty(), "token list must be non-empty");
    assert!(
        toks.iter().any(|t| t.class == "keyword"),
        "keyword tokens must be classified"
    );
}

#[test]
fn tokens_snapshot_match_return() {
    let source = "MATCH (a) RETURN a";
    let toks = tokens(source);
    let rendered: Vec<String> = toks
        .iter()
        .map(|t| format!("{}..{}:{}", t.start, t.end, t.class))
        .collect();
    assert_eq!(
        rendered,
        vec![
            "0..5:keyword",
            "6..7:punctuation",
            "7..8:variable",
            "8..9:punctuation",
            "10..16:keyword",
            "17..18:variable",
        ],
        "token snapshot drift for {source:?}"
    );
}

#[test]
fn tokens_cover_all_operator_kinds() {
    let source = "MATCH (a) WHERE a.age > 18 RETURN a";
    let outcome = parse(source);
    assert!(
        !outcome.has_error,
        "operator probe must parse without ERROR/MISSING"
    );
    let toks = tokens(source);
    assert!(
        toks.iter().any(|t| t.class == "operator"),
        "operator tokens must be classified"
    );
    for t in &toks {
        assert!(t.start < t.end, "empty token span {t:?}");
        assert!(t.end <= source.len(), "token out of bounds {t:?}");
    }
    let mut last = 0;
    for t in &toks {
        assert!(t.start >= last, "tokens out of order {t:?}");
        last = t.start;
    }
}

#[test]
fn fixtures_parse_cleanly() {
    use std::fs;
    use std::path::Path;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = manifest.join("fixtures");
    for entry in fs::read_dir(&fixtures_dir).expect("read fixtures") {
        let path = entry.expect("entry").path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        if name.contains("error") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read fixture");
        let outcome = parse(&source);
        assert!(
            !outcome.has_error,
            "fixture {name} must parse without ERROR/MISSING"
        );
    }
}
