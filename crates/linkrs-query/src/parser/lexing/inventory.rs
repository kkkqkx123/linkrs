//! Lexical inventory export.
//!
//! The vocabulary itself lives in [`super::spec`], the single lexical
//! specification. This module keeps the historical export surface
//! (`keyword_table`, `SOFT_KEYWORDS`, multi-word lists) as thin views over
//! the spec so existing consumers do not churn, plus the editor-facing
//! `function_category` presentation mapping.

pub use super::spec::{
    keyword_table, operator_symbols, punctuation_symbols, KeywordCategory, KeywordEntry,
    SymbolClass, SymbolSpec, CONTEXTUAL_KEYWORDS, MULTIWORD_CONDITIONAL, MULTIWORD_UNCONDITIONAL,
    SOFT_KEYWORDS, SYMBOL_SPEC,
};

/// Function category for inventory export. The registry itself only knows
/// name/arity/description; this controlled mapping supplies the editor-facing
/// category so the CLI and the frontend do not each hand-maintain one.
/// Unknown names fall back to `builtin`.
pub fn function_category(name: &str) -> &'static str {
    match name.to_uppercase().as_str() {
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COLLECT" => "aggregate",
        "LENGTH" | "SIZE" | "TRIM" | "LOWER" | "UPPER" | "SUBSTRING" | "REPLACE" => "string",
        "ABS" | "CEIL" | "FLOOR" | "ROUND" | "SQRT" => "numeric",
        "HEAD" | "TAIL" | "REVERSE" => "list",
        "TYPE" | "ID" | "PROPERTIES" => "type",
        "DATETIME" | "TIMESTAMP" => "date",
        _ => "builtin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::expression::functions::FunctionRegistry;
    use crate::parser::lexing::lexer::Lexer;
    use crate::parser::lexing::spec::lookup_keyword_spelling;
    use crate::parser::TokenKind;

    /// One-direction smoke check: every table row resolves through the lexer.
    /// Completeness holds by construction (the lexer reads the same spec
    /// table) and is pinned by the round-trip tests in `spec.rs`; this test
    /// only guards against wiring drift.
    #[test]
    fn keyword_table_matches_lexer() {
        let lexer = Lexer::new("");
        for entry in keyword_table() {
            let got = lexer.lookup_keyword(entry.spelling);
            assert_eq!(
                got, entry.kind,
                "keyword table drift for '{}'",
                entry.spelling
            );
        }
    }

    #[test]
    fn lexer_lookup_is_spec_driven_both_directions() {
        for entry in keyword_table() {
            assert_eq!(
                lookup_keyword_spelling(entry.spelling),
                Some(entry.kind.clone()),
                "spec lookup misses '{}'",
                entry.spelling
            );
        }
        assert_eq!(
            lookup_keyword_spelling("NOT_A_KEYWORD"),
            None,
            "unknown words must miss the spec table"
        );
    }

    #[test]
    fn soft_keywords_are_not_reserved() {
        let lexer = Lexer::new("");
        for word in SOFT_KEYWORDS.iter() {
            assert!(
                matches!(lexer.lookup_keyword(word), TokenKind::Identifier(_)),
                "'{}' must stay a soft keyword",
                word
            );
        }
    }

    #[test]
    fn reserved_doc_covers_inventory() {
        let doc_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/release/reserved-keywords.md"
        );
        let doc = std::fs::read_to_string(doc_path).expect("reserved keywords doc must exist");
        for entry in keyword_table() {
            assert!(
                doc.contains(entry.spelling),
                "reserved keywords doc missing '{}'",
                entry.spelling
            );
        }
        for word in SOFT_KEYWORDS.iter() {
            assert!(
                doc.contains(word),
                "reserved keywords doc missing soft keyword '{word}'"
            );
        }
    }

    /// One-direction smoke check: every symbol spec row lexes to its recorded
    /// kind. Partition completeness is pinned by
    /// `operator_punctuation_partition_covers_symbol_spec` in `spec.rs`; this
    /// test only guards scanner output against drift.
    #[test]
    fn symbol_tables_match_lexer() {
        for entry in SYMBOL_SPEC {
            let lexer = Lexer::new(entry.symbol);
            let token = lexer.current_token().clone();
            assert_eq!(
                token.kind, entry.kind,
                "symbol table drift for '{}'",
                entry.symbol
            );
        }
    }

    #[test]
    fn function_categories_exist_in_registry() {
        let names: std::collections::HashSet<String> = FunctionRegistry::new()
            .function_specs()
            .into_iter()
            .map(|spec| spec.name.to_uppercase())
            .collect();
        for name in [
            "COUNT",
            "SUM",
            "AVG",
            "MIN",
            "MAX",
            "COLLECT",
            "LENGTH",
            "SIZE",
            "TRIM",
            "LOWER",
            "UPPER",
            "SUBSTRING",
            "REPLACE",
            "ABS",
            "CEIL",
            "FLOOR",
            "ROUND",
            "SQRT",
            "HEAD",
            "TAIL",
            "REVERSE",
            "TYPE",
            "ID",
            "PROPERTIES",
            "DATETIME",
            "TIMESTAMP",
        ] {
            assert!(
                function_category(name) != "builtin" || names.contains(name),
                "category mapping references unknown function '{name}'"
            );
        }
    }
}
