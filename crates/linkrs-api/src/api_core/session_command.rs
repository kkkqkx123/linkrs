//! Shared classification of transaction and session commands.
//!
//! One implementation serves both the embedded session and the network
//! service layer: the keyword gate, the single parse, and the AST match
//! live here so the two transports cannot drift apart. Execution of the
//! classified command stays with each host, which owns its own session
//! state (bindings, variable store, permissions).

use crate::api_core::error::{CoreError, ExtendedErrorCode};
use linkrs_query::parser::ast::Stmt;
use linkrs_query::parser::{Parser, ParserResult};

/// First parse error of a command-like statement, carrying the location
/// so each transport can report it in its own error shape.
#[derive(Debug, Clone)]
pub struct CommandParseError {
    pub message: String,
    pub offset: Option<usize>,
    pub position: Option<linkrs_core::types::Position>,
}

impl std::fmt::Display for CommandParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error: {}", self.message)
    }
}

/// Malformed command-like statements become syntax errors that keep the
/// original location, so transports can point at the offending token.
impl From<CommandParseError> for CoreError {
    fn from(error: CommandParseError) -> Self {
        match (error.position, error.offset) {
            (Some(position), offset) => CoreError::detailed_query_error_with_position(
                error.message,
                ExtendedErrorCode::SyntaxError,
                offset,
                Some(position),
            ),
            (None, Some(offset)) => CoreError::detailed_query_error(
                error.message,
                ExtendedErrorCode::SyntaxError,
                Some(offset),
            ),
            (None, None) => CoreError::QueryExecutionFailed(error.message),
        }
    }
}

/// Keyword gate: whether the text opens with a transaction or session
/// command keyword. Regular statements skip the API-layer parse entirely
/// and are parsed once inside the query engine.
pub fn is_command_like(text: &str) -> bool {
    let upper = text.trim().to_uppercase();
    upper == "BEGIN"
        || upper.starts_with("BEGIN ")
        || upper.starts_with("COMMIT")
        || upper.starts_with("ROLLBACK")
        || upper.starts_with("SAVEPOINT")
        || upper.starts_with("RELEASE SAVEPOINT")
        || upper == "LET"
        || upper.starts_with("LET ")
        || upper.starts_with("LOAD EXTENSION")
        || upper.starts_with("INSTALL EXTENSION")
        || upper.starts_with("UPDATE EXTENSION")
        || upper.starts_with("UNINSTALL EXTENSION")
}

/// Parse the statement when it is one of the transaction / session
/// commands intercepted by the API layer.
///
/// Returns `Ok(None)` for regular statements (and for command-like text
/// that parses into another statement kind, which the pipeline reports).
/// Returns `Err` with the first specific parse error for malformed
/// command-like statements. Hosts dispatch on the returned AST variant;
/// there is no separate command kind to fall out of sync with it.
pub fn parse_session_command(text: &str) -> Result<Option<ParserResult>, CommandParseError> {
    if !is_command_like(text) {
        return Ok(None);
    }
    let mut parser = Parser::new(text);
    match parser.parse() {
        Ok(result) if !parser.has_errors() => match result.ast.stmt() {
            Stmt::BeginTransaction(_)
            | Stmt::CommitTransaction(_)
            | Stmt::RollbackTransaction(_)
            | Stmt::Savepoint(_)
            | Stmt::ReleaseSavepoint(_)
            | Stmt::AssignVariable(_)
            | Stmt::Extension(_) => Ok(Some(result)),
            _ => Ok(None),
        },
        Ok(_) => Ok(None),
        Err(_) => match parser.errors().iter().next() {
            Some(first) => Err(CommandParseError {
                message: first.message.to_string(),
                offset: first.offset,
                position: first.position.is_valid().then_some(first.position),
            }),
            None => Ok(None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Option<Stmt> {
        parse_session_command(text)
            .ok()
            .flatten()
            .map(|result| result.ast.stmt().clone())
    }

    #[test]
    fn parses_every_transaction_and_session_command() {
        assert!(matches!(parsed("BEGIN"), Some(Stmt::BeginTransaction(_))));
        assert!(matches!(
            parsed("BEGIN TRANSACTION"),
            Some(Stmt::BeginTransaction(_))
        ));
        assert!(matches!(
            parsed("BEGIN READ ONLY"),
            Some(Stmt::BeginTransaction(_))
        ));
        assert!(matches!(parsed("COMMIT"), Some(Stmt::CommitTransaction(_))));
        assert!(matches!(
            parsed("ROLLBACK"),
            Some(Stmt::RollbackTransaction(_))
        ));
        assert!(matches!(
            parsed("ROLLBACK TO sp1"),
            Some(Stmt::RollbackTransaction(_))
        ));
        assert!(matches!(parsed("SAVEPOINT sp1"), Some(Stmt::Savepoint(_))));
        assert!(matches!(
            parsed("RELEASE SAVEPOINT sp1"),
            Some(Stmt::ReleaseSavepoint(_))
        ));
        assert!(matches!(
            parsed("LET $x = 1"),
            Some(Stmt::AssignVariable(_))
        ));
        for query in [
            "LOAD EXTENSION '/tmp/a.so'",
            "INSTALL EXTENSION my_udf FROM '/tmp/a.so'",
            "UNINSTALL EXTENSION my_udf",
            "UPDATE EXTENSION my_udf",
        ] {
            assert!(
                matches!(parsed(query), Some(Stmt::Extension(_))),
                "{query} must parse as an EXTENSION statement"
            );
        }
    }

    #[test]
    fn regular_statements_and_non_commands_pass_through() {
        assert!(parsed("MATCH (n) RETURN n").is_none());
        assert!(parsed("BEGINNING OF TEXT").is_none());
        assert!(parsed("").is_none());
        // Command keyword that parses into another statement kind.
        assert!(parsed("COMMIT; MATCH (n) RETURN n").is_none());
    }

    #[test]
    fn malformed_command_reports_first_error_with_position() {
        let error = parse_session_command("BEGIN").expect("bare BEGIN parses");
        assert!(error.is_some());

        let error = parse_session_command("SAVEPOINT").expect_err("savepoint needs a name");
        assert!(!error.message.is_empty());
        assert!(error.offset.is_some() || error.position.is_some());
    }
}
