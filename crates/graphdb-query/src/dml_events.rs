//! Statement-level DML classification and notification.
//!
//! Graph databases have no per-row identity to report, so the classic
//! per-row update hook degrades to a statement-level notification: which
//! kind of write a statement performed, in which space, affecting how many
//! rows. [`classify_dml`] is the single shared classifier used by the
//! embedded session path and the C-API update hook, so both report the
//! same operation for the same text.
//!
//! The mapping is a best-effort heuristic over the leading keyword (after
//! skipping whitespace and `--` / `//` / `#` / `/* */` comments); the
//! affected-rows count always comes from the result metadata and must be
//! treated as a row count, never as a stable row id.

use std::sync::Arc;

/// Kind of write a DML statement performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmlOp {
    Insert,
    Update,
    Delete,
}

/// Statement-level DML notification.
///
/// Emitted after a data-modifying statement succeeds. Valid only for the
/// dispatch call; observers must not retain it beyond the callback.
#[derive(Debug, Clone)]
pub struct DmlStatementEvent {
    pub op: DmlOp,
    pub space_name: String,
    pub rows: u64,
}

/// Runtime observer for statement-level DML notifications.
pub type DmlStatementCallback = Arc<dyn Fn(&DmlStatementEvent) + Send + Sync>;

/// Classify a query text into a DML operation, or `None` for read-only
/// (or unrecognized) statements.
///
/// `MATCH`/`WITH`/`UNWIND`/`OPTIONAL` openers carry the write in a trailing
/// clause, so the whole text is scanned for a write keyword in that case.
pub fn classify_dml(query_text: &str) -> Option<DmlOp> {
    let keyword = leading_keyword(query_text);
    if keyword.eq_ignore_ascii_case("MATCH")
        || keyword.eq_ignore_ascii_case("WITH")
        || keyword.eq_ignore_ascii_case("UNWIND")
        || keyword.eq_ignore_ascii_case("OPTIONAL")
    {
        let upper = query_text.to_uppercase();
        if upper.contains("DELETE") || upper.contains("DETACH DELETE") {
            return Some(DmlOp::Delete);
        }
        if upper.contains("SET")
            || upper.contains("REMOVE")
            || upper.contains("MERGE")
            || upper.contains("CREATE")
        {
            if upper.contains("MERGE") || upper.contains("CREATE") {
                return Some(DmlOp::Insert);
            }
            return Some(DmlOp::Update);
        }
        return None;
    }
    if keyword.eq_ignore_ascii_case("INSERT")
        || keyword.eq_ignore_ascii_case("CREATE")
        || keyword.eq_ignore_ascii_case("MERGE")
    {
        return Some(DmlOp::Insert);
    }
    if keyword.eq_ignore_ascii_case("UPDATE")
        || keyword.eq_ignore_ascii_case("SET")
        || keyword.eq_ignore_ascii_case("REMOVE")
        || keyword.eq_ignore_ascii_case("ALTER")
    {
        return Some(DmlOp::Update);
    }
    if keyword.eq_ignore_ascii_case("DELETE")
        || keyword.eq_ignore_ascii_case("DETACH")
        || keyword.eq_ignore_ascii_case("DROP")
    {
        return Some(DmlOp::Delete);
    }
    None
}

/// Extract the first keyword token, skipping whitespace and SQL/Cypher comments.
fn leading_keyword(query: &str) -> &str {
    let bytes = query.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        if query[pos..].starts_with("--")
            || query[pos..].starts_with("//")
            || query[pos..].starts_with('#')
        {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        if query[pos..].starts_with("/*") {
            if let Some(end) = query[pos..].find("*/") {
                pos += end + 2;
                continue;
            }
            return "";
        }
        break;
    }
    let rest = &query[pos..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
        .unwrap_or(rest.len());
    let mut token = &rest[..end];
    // Handle `OPTIONAL MATCH` / `DETACH DELETE` two-word openers.
    if token.eq_ignore_ascii_case("OPTIONAL") || token.eq_ignore_ascii_case("DETACH") {
        let after: &str = rest[end..].trim_start();
        let after_end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
            .unwrap_or(after.len());
        if !after[..after_end].is_empty() {
            token = &after[..after_end];
        }
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_keywords_classify() {
        assert_eq!(classify_dml("INSERT INTO t"), Some(DmlOp::Insert));
        assert_eq!(classify_dml("  CREATE TAG u"), Some(DmlOp::Insert));
        assert_eq!(classify_dml("-- comment\nMERGE (n)"), Some(DmlOp::Insert));
        assert_eq!(classify_dml("UPDATE t SET a"), Some(DmlOp::Update));
        assert_eq!(classify_dml("/* c */ SET n.a = 1"), Some(DmlOp::Update));
        assert_eq!(classify_dml("DELETE FROM t"), Some(DmlOp::Delete));
        assert_eq!(classify_dml("DETACH DELETE n"), Some(DmlOp::Delete));
        assert_eq!(classify_dml("DROP TAG t"), Some(DmlOp::Delete));
    }

    #[test]
    fn match_family_scans_trailing_clause() {
        assert_eq!(classify_dml("MATCH (n) DELETE n"), Some(DmlOp::Delete));
        assert_eq!(
            classify_dml("MATCH (n) SET n.a = 1 RETURN n"),
            Some(DmlOp::Update)
        );
        assert_eq!(
            classify_dml("MATCH (n) RETURN n"),
            None,
            "pure reads stay silent"
        );
    }
}
