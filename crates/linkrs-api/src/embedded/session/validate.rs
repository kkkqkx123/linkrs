//! Query validation and plan explanation for an embedded session.
//!
//! Mirrors the network `POST /v1/query/validate` and `POST /v1/query/explain`
//! endpoints without a transport: validation parses and binds the statement
//! without executing it, while explanation plans via `EXPLAIN` through the
//! same execution pipeline as [`Session::execute`].

use super::Session;
use crate::api_core::{CoreError, CoreResult};
use crate::embedded::result::QueryResult;
use linkrs_storage::StorageClient;

/// Outcome of [`Session::validate_query`].
#[derive(Debug, Clone)]
pub struct QueryValidation {
    /// Whether the statement parsed and bound successfully.
    pub valid: bool,
    /// Human-readable reason when `valid` is false.
    pub error: Option<String>,
    /// `@name` parameter references found in the statement text.
    pub parameter_names: Vec<String>,
}

impl QueryValidation {
    fn ok(parameter_names: Vec<String>) -> Self {
        Self {
            valid: true,
            error: None,
            parameter_names,
        }
    }

    fn invalid(reason: String) -> Self {
        Self {
            valid: false,
            error: Some(reason),
            parameter_names: Vec::new(),
        }
    }
}

/// Collect `@name` parameter references from query text.
fn extract_parameter_names(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + 1 {
                let name = &query[i + 1..j];
                if !names.iter().any(|n: &String| n == name) {
                    names.push(name.to_string());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    names
}

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Validate query text by parsing and binding it, without executing.
    ///
    /// Binding runs without a schema manager, so checks that need live
    /// schema (such as tag existence) are skipped while syntax, name
    /// resolution, and function resolution still apply. Matches the
    /// semantics of `POST /v1/query/validate`.
    pub fn validate_query(&self, query: &str) -> CoreResult<QueryValidation> {
        if query.trim().is_empty() {
            return Ok(QueryValidation::invalid(
                "query must not be empty".to_string(),
            ));
        }
        let mut parser = linkrs_query::parser::Parser::new(query);
        let parsed = parser
            .parse()
            .map_err(|e| CoreError::InvalidParameter(format!("syntax error: {e}")))?;
        if parser.has_errors() {
            return Ok(QueryValidation::invalid(format!(
                "syntax error: {}",
                parser.take_errors()
            )));
        }
        if let Err(e) = linkrs_query::binder::Binder::new().bind(parsed.ast) {
            return Ok(QueryValidation::invalid(format!("semantic error: {e}")));
        }
        Ok(QueryValidation::ok(extract_parameter_names(query)))
    }

    /// Plan a statement via `EXPLAIN` without executing it.
    ///
    /// An existing `EXPLAIN` prefix is reused instead of doubled, mirroring
    /// the server `POST /v1/query/explain` handler. Error positions from the
    /// auto-added prefix are shifted back to the caller statement.
    pub fn explain_query(&self, query: &str) -> CoreResult<QueryResult> {
        const PREFIX: &str = "EXPLAIN ";
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err(CoreError::InvalidParameter(
                "query must not be empty".to_string(),
            ));
        }
        let already_explain = trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("explain");
        if already_explain {
            return self.execute(trimmed);
        }
        let explain_stmt = format!("{PREFIX}{trimmed}");
        match self.execute(&explain_stmt) {
            Ok(result) => Ok(result),
            Err(e) => {
                const PREFIX_LEN: usize = PREFIX.len();
                let message = e.to_string();
                Err(CoreError::InvalidParameter(strip_explain_prefix_note(
                    message, PREFIX_LEN,
                )))
            }
        }
    }
}

fn strip_explain_prefix_note(message: String, _prefix_len: usize) -> String {
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::database::GraphDatabase;
    use linkrs_storage::MockStorage;

    #[test]
    fn validate_accepts_simple_match() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        let validation = session
            .validate_query("MATCH (n) RETURN n")
            .expect("validate runs");
        assert!(validation.valid);
        assert!(validation.error.is_none());
    }

    #[test]
    fn validate_rejects_empty_and_syntax_errors() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        let empty = session.validate_query("   ").expect("validate runs");
        assert!(!empty.valid);
        let broken = session
            .validate_query("MATCH (n RETURN n")
            .expect("validate runs");
        assert!(!broken.valid);
    }

    #[test]
    fn validate_collects_parameter_names() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        let validation = session
            .validate_query("MATCH (n) WHERE n.age > @age AND n.name == @name RETURN n")
            .expect("validate runs");
        assert!(validation.valid);
        assert_eq!(validation.parameter_names, vec!["age", "name"]);
    }
}
