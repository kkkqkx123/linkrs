use gpui_kit::{App, Task};
use linkrs_client::{ClientError, QueryResult};

use crate::connection::SessionStore;
use crate::runtime;
use crate::state::AppState;

/// Execute one statement with explicit parameters and session variables.
/// Both maps bind by name; empty JSON objects mean no bindings.
pub fn execute_with_options(
    cx: &App,
    sql: &str,
    parameters: std::collections::HashMap<String, serde_json::Value>,
    session_variables: std::collections::HashMap<String, serde_json::Value>,
) -> Task<Result<QueryResult, ClientError>> {
    let manager = SessionStore::manager(cx);
    let sql = sql.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard
            .client()
            .execute_query_with_options(&sql, id, parameters, session_variables)
            .await
    })
}

/// Execute one statement (or script) through the session. `raw` skips the
/// client-side variable substitution performed by `execute_query`.
pub fn execute(cx: &App, sql: &str, raw: bool) -> Task<Result<QueryResult, ClientError>> {
    let manager = SessionStore::manager(cx);
    let sql = sql.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        if raw {
            guard.execute_query_raw(&sql).await
        } else {
            guard.execute_query(&sql).await
        }
    })
}

/// One trimmed statement plus its raw byte span in the source script.
///
/// `start` is the byte offset of the raw segment (right after the previous
/// top-level `;` or zero); `end` is the byte offset of the terminating `;`
/// or the script length for the final segment. Comment and whitespace bytes
/// stay inside the span so a cursor sitting in a gap still resolves to the
/// surrounding statement; callers fall back to the full script when the
/// offset sits past the final terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSpan {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Split a script into statements with byte spans. Comment text is stripped
/// from the emitted statements; empty statements are removed.
pub fn split_statement_spans(script: &str) -> Vec<StatementSpan> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut seg_start = 0usize;
    let mut chars = script.char_indices().peekable();

    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some((offset, ch)) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            if ch == '*' && chars.peek().map(|(_, next)| next) == Some(&'/') {
                chars.next().expect("peeked char exists");
                in_block_comment = false;
            }
            continue;
        }
        match ch {
            '\\' if in_single || in_double => {
                // Skip the escaped character so quote delimiters survive it.
                current.push(ch);
                if let Some((_, escaped)) = chars.next() {
                    current.push(escaped);
                }
                continue;
            }
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '-' if !in_single
                && !in_double
                && !in_backtick
                && chars.peek().map(|(_, next)| next) == Some(&'-') =>
            {
                chars.next().expect("peeked char exists");
                in_line_comment = true;
                continue;
            }
            '/' if !in_single
                && !in_double
                && !in_backtick
                && chars.peek().map(|(_, next)| next) == Some(&'*') =>
            {
                chars.next().expect("peeked char exists");
                in_block_comment = true;
                continue;
            }
            ';' if !in_single && !in_double && !in_backtick => {
                let statement = current.trim();
                if !statement.is_empty() {
                    spans.push(StatementSpan {
                        text: statement.to_string(),
                        start: seg_start,
                        end: offset,
                    });
                }
                current.clear();
                // `;` is one ASCII byte, so the next segment starts here.
                seg_start = offset + 1;
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }

    // Escapes inside quoted strings are left untouched: we only track delimiters.
    if !in_single && !in_double && !in_backtick && !in_block_comment {
        let statement = current.trim();
        if !statement.is_empty() {
            spans.push(StatementSpan {
                text: statement.to_string(),
                start: seg_start,
                end: script.len(),
            });
        }
    }

    spans
}

/// Split a script into statements on semicolons that sit outside quoted
/// strings. Comment text is stripped from the emitted statements; empty
/// statements are removed.
pub fn split_statements(script: &str) -> Vec<String> {
    split_statement_spans(script)
        .into_iter()
        .map(|span| span.text)
        .collect()
}

/// Owning text of the statement covering a UTF-8 byte `offset` (for example
/// an editor cursor), or `None` when the offset sits past the final
/// terminator or outside the script.
pub fn statement_at_offset(script: &str, offset: usize) -> Option<String> {
    if offset > script.len() {
        return None;
    }
    split_statement_spans(script)
        .into_iter()
        .find(|span| span.start <= offset && offset <= span.end)
        .map(|span| span.text)
}

/// Keywords uppercased by the formatter without starting a new line.
const FORMAT_KEYWORDS: &[&str] = &[
    "AND", "OR", "NOT", "AS", "ON", "ASC", "DESC", "DISTINCT", "VERTEX", "EDGE", "TAG", "TYPE",
    "SPACE", "SPACES", "INDEX", "UNIQUE", "IF", "EXISTS", "TRUE", "FALSE", "NULL", "COUNT", "SUM",
    "AVG", "MIN", "MAX", "COLLECT", "IN", "CONTAINS", "STARTS", "ENDS", "OPTIONAL", "DETACH", "BY",
    "THEN", "END", "ADD", "INTO", "VALUES", "KEY",
];

/// Keywords that start a new line in formatted output.
const FORMAT_CLAUSES: &[&str] = &[
    "MATCH", "WHERE", "RETURN", "WITH", "ORDER", "GROUP", "LIMIT", "SKIP", "CREATE", "DELETE",
    "SET", "REMOVE", "UNION", "UNWIND", "INSERT", "UPDATE", "USE", "LET", "CALL", "CASE", "WHEN",
    "ELSE", "BEGIN", "COMMIT", "ROLLBACK", "SHOW", "ALTER", "DROP", "EXPLAIN", "PROFILE",
];

/// Format a script for display: collapse stray whitespace, uppercase
/// keywords, break major clauses onto their own lines and separate
/// statements with a blank line. String literals and comments are copied
/// verbatim; only layout changes, never semantics.
pub fn format_script(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut word = String::new();
    let mut pending_space = false;
    let mut line_start = true;
    let mut skip_ws = false;
    let mut last_word: Option<String> = None;
    let mut chars = script.char_indices().peekable();

    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;

    let emit_pending_space = |out: &mut String, pending_space: &mut bool, line_start: bool| {
        if *pending_space && !line_start {
            out.push(' ');
        }
        *pending_space = false;
    };
    let trim_trailing_space = |out: &mut String| {
        if out.ends_with(' ') {
            out.pop();
        }
    };

    // Uppercase one word and place it, starting a new line for clauses.
    // Identifiers outside the keyword vocabulary keep their spelling.
    let flush_word = |word: &mut String,
                      out: &mut String,
                      pending_space: &mut bool,
                      line_start: &mut bool,
                      last_word: &mut Option<String>| {
        if word.is_empty() {
            return;
        }
        let raw = std::mem::take(word);
        let upper = raw.to_ascii_uppercase();
        let is_clause = FORMAT_CLAUSES.contains(&upper.as_str());
        let glued = matches!(
            (last_word.as_deref(), upper.as_str()),
            (Some("OPTIONAL"), "MATCH") | (Some("DETACH"), "DELETE")
        );
        if is_clause && !*line_start && !glued {
            out.push('\n');
            *line_start = true;
            *pending_space = false;
        }
        emit_pending_space(out, pending_space, *line_start);
        if is_clause || FORMAT_KEYWORDS.contains(&upper.as_str()) {
            out.push_str(&upper);
        } else {
            out.push_str(&raw);
        }
        *line_start = false;
        *last_word = Some(upper);
    };

    while let Some((_, ch)) = chars.next() {
        if in_single || in_double || in_backtick {
            if ch == '\\' && !in_backtick {
                out.push(ch);
                if let Some((_, escaped)) = chars.next() {
                    out.push(escaped);
                }
                continue;
            }
            out.push(ch);
            if ch == '\'' && !in_double && !in_backtick {
                in_single = false;
                // A closing quote ends the literal only when it is not an
                // escaped quote, handled above.
            } else if ch == '"' && !in_single && !in_backtick {
                in_double = false;
            } else if ch == '`' && !in_single && !in_double {
                in_backtick = false;
            }
            line_start = false;
            continue;
        }
        if ch == '\'' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            in_single = true;
            line_start = false;
            continue;
        }
        if ch == '"' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            in_double = true;
            line_start = false;
            continue;
        }
        if ch == '`' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            in_backtick = true;
            line_start = false;
            continue;
        }
        if ch == '-' && chars.peek().map(|(_, next)| next) == Some(&'-') {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            if let Some((_, dash)) = chars.next() {
                out.push(dash);
            }
            for (_, comment_ch) in chars.by_ref() {
                out.push(comment_ch);
                if comment_ch == '\n' {
                    break;
                }
            }
            line_start = true;
            pending_space = false;
            skip_ws = false;
            continue;
        }
        if ch == '/' && chars.peek().map(|(_, next)| next) == Some(&'*') {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            if let Some((_, star)) = chars.next() {
                out.push(star);
            }
            while let Some((_, comment_ch)) = chars.next() {
                out.push(comment_ch);
                if comment_ch == '*' && chars.peek().map(|(_, next)| next) == Some(&'/') {
                    if let Some((_, slash)) = chars.next() {
                        out.push(slash);
                    }
                    break;
                }
            }
            line_start = false;
            continue;
        }
        if ch.is_whitespace() {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            if !skip_ws {
                pending_space = true;
            }
            continue;
        }
        if ch == ';' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            trim_trailing_space(&mut out);
            out.push(';');
            out.push('\n');
            out.push('\n');
            line_start = true;
            pending_space = false;
            skip_ws = true;
            last_word = None;
            continue;
        }
        if ch == ',' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            trim_trailing_space(&mut out);
            out.push(',');
            pending_space = true;
            skip_ws = false;
            line_start = false;
            continue;
        }
        if ch == '(' || ch == '[' || ch == '{' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            trim_trailing_space(&mut out);
            emit_pending_space(&mut out, &mut pending_space, line_start);
            out.push(ch);
            pending_space = false;
            skip_ws = false;
            line_start = false;
            continue;
        }
        if ch == ')' || ch == ']' || ch == '}' || ch == '.' || ch == ':' {
            flush_word(
                &mut word,
                &mut out,
                &mut pending_space,
                &mut line_start,
                &mut last_word,
            );
            trim_trailing_space(&mut out);
            out.push(ch);
            pending_space = false;
            skip_ws = false;
            line_start = false;
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            word.push(ch);
            skip_ws = false;
            continue;
        }
        flush_word(
            &mut word,
            &mut out,
            &mut pending_space,
            &mut line_start,
            &mut last_word,
        );
        emit_pending_space(&mut out, &mut pending_space, line_start);
        out.push(ch);
        pending_space = true;
        skip_ws = false;
        line_start = false;
    }
    flush_word(
        &mut word,
        &mut out,
        &mut pending_space,
        &mut line_start,
        &mut last_word,
    );
    while out.ends_with(' ') || out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Execute a query in streaming mode.
pub fn execute_stream(cx: &App, sql: &str) -> Task<Result<QueryResult, ClientError>> {
    let manager = SessionStore::manager(cx);
    let sql = sql.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().execute_query_stream(&sql, id).await
    })
}

/// Validate a query without executing it.
pub fn validate(cx: &App, sql: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let sql = sql.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().validate_query(&sql, id).await
    })
}

/// List currently running queries across all sessions.
pub fn list_running(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_queries().await
    })
}

/// Cancel a running query by `(session_id, query_id)`.
pub fn kill_running(
    cx: &App,
    session_id: i64,
    query_id: i64,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().kill_query(session_id, query_id).await
    })
}

/// Explain a query.
pub fn explain(cx: &App, sql: &str) -> Task<Result<QueryResult, ClientError>> {
    let manager = SessionStore::manager(cx);
    let sql = sql.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().explain_query(&sql, id).await
    })
}

/// Fetch the server-side portrait for a finished query by trace id.
pub fn profile(
    cx: &App,
    trace_id: &str,
) -> Task<Result<linkrs_wire::meta::QueryProfileDetailResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let trace_id = trace_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_query_profile_detail(&trace_id).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_top_level_semicolons() {
        let script = "USE demo; MATCH (a)-[e]->(b) RETURN a; INSERT VERTEX Person";
        let statements = split_statements(script);
        assert_eq!(statements.len(), 3);
        assert_eq!(statements[0], "USE demo");
    }

    #[test]
    fn keeps_semicolons_inside_quotes_and_comments() {
        let script =
            "INSERT VERTEX Person(name) VALUES 'a;b' -- trailing; comment\n; /* x;y */ SELECT 1";
        let statements = split_statements(script);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("'a;b'"));
        assert_eq!(statements[1], "SELECT 1");
    }

    #[test]
    fn drops_empty_statements() {
        assert!(split_statements(";;;  \n  ;").is_empty());
        assert_eq!(split_statements("USE demo"), vec!["USE demo".to_string()]);
    }

    #[test]
    fn spans_cover_byte_ranges() {
        let script = "SELECT 1; SELECT 2";
        let spans = split_statement_spans(script);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "SELECT 1");
        assert_eq!(
            &script[spans[0].start..spans[0].end],
            "SELECT 1;".trim_end_matches(';')
        );
        assert_eq!(spans[1].text, "SELECT 2");
        assert_eq!(spans[1].end, script.len());
    }

    #[test]
    fn spans_track_multibyte_text() {
        let script = "INSERT VERTEX P(name) VALUES 'caf\u{00e9}'; SELECT 2";
        let spans = split_statement_spans(script);
        assert_eq!(spans.len(), 2);
        assert!(spans[0].text.contains("caf"));
        assert_eq!(spans[1].text, "SELECT 2");
        assert!(script.is_char_boundary(spans[0].end));
        assert!(script.is_char_boundary(spans[1].start));
        assert_eq!(&script[spans[1].start..spans[1].end], " SELECT 2");
    }

    #[test]
    fn offset_resolves_cursor_statement() {
        let script = "SELECT 1; SELECT 2";
        assert_eq!(statement_at_offset(script, 0).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at_offset(script, 8).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at_offset(script, 9).as_deref(), Some("SELECT 2"));
        assert_eq!(
            statement_at_offset(script, script.len()).as_deref(),
            Some("SELECT 2")
        );
        assert_eq!(statement_at_offset("SELECT 1;  ", 11), None);
        assert_eq!(statement_at_offset(script, script.len() + 1), None);
        assert_eq!(statement_at_offset("   ", 1), None);
    }

    #[test]
    fn format_uppercases_clauses_and_keeps_identifiers() {
        let formatted = format_script("match (person) where person.age > 30 return person");
        assert!(formatted.contains("MATCH (person)"), "{formatted}");
        assert!(formatted.contains("WHERE person.age > 30"), "{formatted}");
        assert!(formatted.contains("RETURN person"), "{formatted}");
    }

    #[test]
    fn format_keeps_strings_and_comments_verbatim() {
        let script = "match (a) where a.name = 'match; return' -- keep; me\nreturn a /* ;stay; */";
        let formatted = format_script(script);
        assert!(formatted.contains("'match; return'"), "{formatted}");
        assert!(formatted.contains("-- keep; me"), "{formatted}");
        assert!(formatted.contains("/* ;stay; */"), "{formatted}");
        assert!(formatted.contains("MATCH (a)"), "{formatted}");
    }

    #[test]
    fn format_separates_statements_and_is_idempotent() {
        let script = "select 1;select 2;";
        let once = format_script(script);
        assert!(once.contains(";\n\n"), "{once:?}");
        assert_eq!(format_script(&once), once);
        assert_eq!(format_script("   \n  "), String::new());
    }
}
