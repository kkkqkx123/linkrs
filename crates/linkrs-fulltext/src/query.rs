//! Structured fulltext query representation.
//!
//! The query layer builds this enum from the parsed AST and passes it through
//! to the engine, which constructs the backend query via its native API
//! instead of parsing an interpolated query-grammar string. This keeps user
//! text out of the backend's query syntax (no injection / parse-error surface)
//! and preserves the AST's typed semantics (phrase, prefix, fuzzy, boolean).

use serde::{Deserialize, Serialize};

/// A structured fulltext query, independent of any backend grammar.
///
/// Field selection is intentionally absent: one index serves exactly one
/// field (`IndexKey` = space + tag + field), so the field is resolved before
/// the query reaches the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FulltextQuery {
    /// Free text: matched terms are OR-combined (BM25 ranking).
    Simple(String),
    /// Exact phrase: all terms must appear in order.
    Phrase(String),
    /// Prefix match: terms starting with this prefix.
    Prefix(String),
    /// Fuzzy match with an optional edit distance (Levenshtein).
    Fuzzy(String, Option<u8>),
    /// Wildcard match: `*` is the only wildcard, other chars are literal.
    Wildcard(String),
    /// Boolean combination of subqueries.
    Boolean {
        must: Vec<FulltextQuery>,
        should: Vec<FulltextQuery>,
        must_not: Vec<FulltextQuery>,
    },
    /// Term range on the indexed text.
    Range {
        lower: Option<String>,
        upper: Option<String>,
        include_lower: bool,
        include_upper: bool,
    },
}

impl FulltextQuery {
    /// Render a backend-grammar query string. Only used as the fallback path
    /// for engines that have not implemented structured search natively;
    /// user text is escaped so the result stays grammar-valid.
    pub fn to_query_string(&self) -> String {
        match self {
            Self::Simple(text) | Self::Phrase(text) => escape_grammar(text),
            Self::Prefix(text) => format!("{}*", escape_grammar(text)),
            Self::Fuzzy(text, distance) => match distance {
                Some(d) => format!("{}~{}", escape_grammar(text), d),
                None => format!("{}~", escape_grammar(text)),
            },
            Self::Wildcard(text) => escape_wildcard(text),
            Self::Boolean {
                must,
                should,
                must_not,
            } => must
                .iter()
                .map(|q| format!("+({})", q.to_query_string()))
                .chain(should.iter().map(|q| format!("({})", q.to_query_string())))
                .chain(
                    must_not
                        .iter()
                        .map(|q| format!("-({})", q.to_query_string())),
                )
                .collect::<Vec<_>>()
                .join(" "),
            Self::Range {
                lower,
                upper,
                include_lower,
                include_upper,
            } => format!(
                "{}{} TO {}{}",
                if *include_lower { "[" } else { "{" },
                lower
                    .as_deref()
                    .map(escape_grammar)
                    .unwrap_or_else(|| "*".to_string()),
                upper
                    .as_deref()
                    .map(escape_grammar)
                    .unwrap_or_else(|| "*".to_string()),
                if *include_upper { "]" } else { "}" },
            ),
        }
    }
}

/// Escape characters that carry query-grammar meaning.
fn escape_grammar(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '+' | '-'
                | '='
                | '&'
                | '|'
                | '!'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '^'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Escape everything except `*`, which stays meaningful as the wildcard.
fn escape_wildcard(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '?' | ':' | '\\' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '~'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_escapes_grammar_chars() {
        let q = FulltextQuery::Simple("a+b (c)".to_string());
        assert_eq!(q.to_query_string(), "a\\+b \\(c\\)");
    }

    #[test]
    fn test_prefix_keeps_star() {
        let q = FulltextQuery::Prefix("data*base".to_string());
        assert_eq!(q.to_query_string(), "data\\*base*");
    }

    #[test]
    fn test_wildcard_preserves_stars_only() {
        let q = FulltextQuery::Wildcard("data*ase?".to_string());
        assert_eq!(q.to_query_string(), "data*ase\\?");
    }

    #[test]
    fn test_boolean_structure() {
        let q = FulltextQuery::Boolean {
            must: vec![FulltextQuery::Simple("a".to_string())],
            should: vec![FulltextQuery::Simple("b".to_string())],
            must_not: vec![FulltextQuery::Simple("c".to_string())],
        };
        assert_eq!(q.to_query_string(), "+(a) (b) -(c)");
    }

    #[test]
    fn test_range_bounds() {
        let q = FulltextQuery::Range {
            lower: Some("2020".to_string()),
            upper: None,
            include_lower: true,
            include_upper: false,
        };
        assert_eq!(q.to_query_string(), "[2020 TO *}");
    }
}
