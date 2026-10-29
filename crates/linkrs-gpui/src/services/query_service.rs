use gpui_kit::{App, Task};
use linkrs_client::{ClientError, QueryResult};

use crate::connection::SessionStore;
use crate::state::AppState;
use crate::runtime;

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

/// Split a script into statements on semicolons that sit outside quoted
/// strings. Comment text is stripped from the emitted statements; empty
/// statements are removed.
pub fn split_statements(script: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = script.chars().peekable();

    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(ch) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next().expect("peeked char exists");
                in_block_comment = false;
            }
            continue;
        }
        match ch {
            '\\' if in_single || in_double => {
                // Skip the escaped character so quote delimiters survive it.
                current.push(ch);
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
                continue;
            }
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '-' if !in_single && !in_double && !in_backtick && chars.peek() == Some(&'-') => {
                chars.next().expect("peeked char exists");
                in_line_comment = true;
                continue;
            }
            '/' if !in_single && !in_double && !in_backtick && chars.peek() == Some(&'*') => {
                chars.next().expect("peeked char exists");
                in_block_comment = true;
                continue;
            }
            ';' if !in_single && !in_double && !in_backtick => {
                let statement = current.trim();
                if !statement.is_empty() {
                    statements.push(statement.to_string());
                }
                current.clear();
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
            statements.push(statement.to_string());
        }
    }

    statements
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
}
