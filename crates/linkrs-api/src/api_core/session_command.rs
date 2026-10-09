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

/// Command classes intercepted by the API layer before the query pipeline
/// sees the statement text. Undefined combinations never reach a host:
/// classification guarantees the AST variant matches the returned kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCommand {
    Begin,
    Commit,
    Rollback,
    Savepoint,
    ReleaseSavepoint,
    AssignVariable,
    Extension,
}

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
            (None, Some(offset)) => {
                CoreError::detailed_query_error(error.message, ExtendedErrorCode::SyntaxError, Some(offset))
            }
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
        || upper.starts_with("START TRANSACTION")
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

/// Parse the statement and classify it when it is one of the
/// transaction / session commands.
///
/// Returns `Ok(None)` for regular statements (and for command-like text
/// that parses into another statement kind, which the pipeline reports).
/// Returns `Err` with the first specific parse error for malformed
/// command-like statements.
pub fn classify_session_command(
    text: &str,
) -> Result<Option<(ParserResult, SessionCommand)>, CommandParseError> {
    if !is_command_like(text) {
        return Ok(None);
    }
    let mut parser = Parser::new(text);
    match parser.parse() {
        Ok(result) if !parser.has_errors() => match result.ast.stmt() {
            Stmt::BeginTransaction(_) => Ok(Some((result, SessionCommand::Begin))),
            Stmt::CommitTransaction(_) => Ok(Some((result, SessionCommand::Commit))),
            Stmt::RollbackTransaction(_) => Ok(Some((result, SessionCommand::Rollback))),
            Stmt::Savepoint(_) => Ok(Some((result, SessionCommand::Savepoint))),
            Stmt::ReleaseSavepoint(_) => Ok(Some((result, SessionCommand::ReleaseSavepoint))),
            Stmt::AssignVariable(_) => Ok(Some((result, SessionCommand::AssignVariable))),
            Stmt::Extension(_) => Ok(Some((result, SessionCommand::Extension))),
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

    fn kind_of(text: &str) -> Option<SessionCommand> {
        classify_session_command(text).ok().flatten().map(|(_, k)| k)
    }

    #[test]
    fn classifies_every_transaction_and_session_command() {
        assert_eq!(kind_of("BEGIN"), Some(SessionCommand::Begin));
        assert_eq!(kind_of("BEGIN TRANSACTION"), Some(SessionCommand::Begin));
        assert_eq!(
            kind_of("BEGIN READ ONLY"),
            Some(SessionCommand::Begin)
        );
        assert_eq!(kind_of("COMMIT"), Some(SessionCommand::Commit));
        assert_eq!(kind_of("ROLLBACK"), Some(SessionCommand::Rollback));
        assert_eq!(
            kind_of("ROLLBACK TO sp1"),
            Some(SessionCommand::Rollback)
        );
        assert_eq!(kind_of("SAVEPOINT sp1"), Some(SessionCommand::Savepoint));
        assert_eq!(
            kind_of("RELEASE SAVEPOINT sp1"),
            Some(SessionCommand::ReleaseSavepoint)
        );
        assert_eq!(kind_of("LET $x = 1"), Some(SessionCommand::AssignVariable));
        assert_eq!(
            kind_of("LOAD EXTENSION '/tmp/a.so'"),
            Some(SessionCommand::Extension)
        );
        assert_eq!(
            kind_of("INSTALL EXTENSION my_udf FROM '/tmp/a.so'"),
            Some(SessionCommand::Extension)
        );
        assert_eq!(
            kind_of("UNINSTALL EXTENSION my_udf"),
            Some(SessionCommand::Extension)
        );
        assert_eq!(
            kind_of("UPDATE EXTENSION my_udf"),
            Some(SessionCommand::Extension)
        );
    }

    #[test]
    fn regular_statements_and_non_commands_pass_through() {
        assert_eq!(kind_of("MATCH (n) RETURN n"), None);
        assert_eq!(kind_of("BEGINNING OF TEXT"), None);
        assert_eq!(kind_of(""), None);
        // Command keyword that parses into another statement kind.
        assert_eq!(kind_of("COMMIT; MATCH (n) RETURN n"), None);
    }

    #[test]
    fn malformed_command_reports_first_error_with_position() {
        let error = classify_session_command("BEGIN").expect("bare BEGIN parses");
        assert_eq!(error.map(|(_, kind)| kind), Some(SessionCommand::Begin));

        let error =
            classify_session_command("SAVEPOINT").expect_err("savepoint needs a name");
        assert!(!error.message.is_empty());
        assert!(error.offset.is_some() || error.position.is_some());
    }
}
