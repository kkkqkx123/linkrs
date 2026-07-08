//! Query layer error type
//!
//! This includes errors that occur during the processes of query parsing, validation, and execution.
//!
//! ## Design
//!
//! `QueryError` is a struct with boxed source error to keep size small (~24 bytes).
//! This follows the same pattern as `DBError` for consistency.

use std::error::Error;

use super::BoxedError;
use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};
use crate::core::error::manager::ManagerError;
use crate::core::error::storage::StorageError;
use crate::core::error::DBError;

/// Query processing phase enumeration
///
/// Used to identify which phase of query processing an error occurred in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryPhase {
    Parse,
    Validate,
    Plan,
    Optimize,
    Execute,
}

impl std::fmt::Display for QueryPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryPhase::Parse => write!(f, "parse"),
            QueryPhase::Validate => write!(f, "validate"),
            QueryPhase::Plan => write!(f, "plan"),
            QueryPhase::Optimize => write!(f, "optimize"),
            QueryPhase::Execute => write!(f, "execute"),
        }
    }
}

/// Error type during the planned node access
///
/// Errors that occur during the query plan traversal and validation processes
#[derive(Debug, Clone)]
pub enum PlanNodeVisitError {
    VisitError {
        node_id: Option<String>,
        message: String,
    },
    TraversalError {
        path: String,
        message: String,
    },
    ValidationError {
        node_type: String,
        message: String,
    },
}

impl std::fmt::Display for PlanNodeVisitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanNodeVisitError::VisitError { node_id, message } => {
                if let Some(id) = node_id {
                    write!(f, "Visit error at node {}: {}", id, message)
                } else {
                    write!(f, "Visit error: {}", message)
                }
            }
            PlanNodeVisitError::TraversalError { path, message } => {
                write!(f, "Traversal error in {}: {}", path, message)
            }
            PlanNodeVisitError::ValidationError { node_type, message } => {
                write!(f, "Validation failed for {}: {}", node_type, message)
            }
        }
    }
}

impl std::error::Error for PlanNodeVisitError {}

impl PlanNodeVisitError {
    pub fn visit_error(message: impl Into<String>) -> Self {
        PlanNodeVisitError::VisitError {
            node_id: None,
            message: message.into(),
        }
    }

    pub fn visit_error_with_node(node_id: impl Into<String>, message: impl Into<String>) -> Self {
        PlanNodeVisitError::VisitError {
            node_id: Some(node_id.into()),
            message: message.into(),
        }
    }

    pub fn traversal_error(path: impl Into<String>, message: impl Into<String>) -> Self {
        PlanNodeVisitError::TraversalError {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn validation_error(node_type: impl Into<String>, message: impl Into<String>) -> Self {
        PlanNodeVisitError::ValidationError {
            node_type: node_type.into(),
            message: message.into(),
        }
    }
}

/// Query operation result type aliases
pub type QueryResult<T> = Result<T, QueryError>;

/// Structured parse error information
///
/// Preserves detailed error context from the parser for better error reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredParseError {
    /// Error category
    pub kind: ParseErrorKind,
    /// Human-readable error message
    pub message: String,
    /// Line and column position in the source
    pub position: crate::core::types::Position,
    /// Byte offset in the source (if available)
    pub offset: Option<usize>,
    /// The unexpected token that caused the error
    pub unexpected_token: Option<String>,
    /// List of expected tokens at the error location
    pub expected_tokens: Vec<String>,
    /// Helpful hints for fixing the error
    pub hints: Vec<String>,
    /// Context information (converted to string for Clone support)
    pub context: Option<String>,
}

/// Parse error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    LexicalError,
    SyntaxError,
    UnexpectedToken,
    UnterminatedString,
    UnterminatedComment,
    InvalidNumber,
    InvalidEscapeSequence,
    UnicodeEscapeError,
    UnexpectedEndOfInput,
    InvalidCharacter,
    UnknownKeyword,
    RecursionLimitExceeded,
    UnsupportedFeature,
    SemanticError,
}

impl std::fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseErrorKind::LexicalError => write!(f, "Lexical error"),
            ParseErrorKind::SyntaxError => write!(f, "Syntax error"),
            ParseErrorKind::UnexpectedToken => write!(f, "Unexpected token"),
            ParseErrorKind::UnterminatedString => write!(f, "Unterminated string"),
            ParseErrorKind::UnterminatedComment => write!(f, "Unterminated comment"),
            ParseErrorKind::InvalidNumber => write!(f, "Invalid number"),
            ParseErrorKind::InvalidEscapeSequence => write!(f, "Invalid escape sequence"),
            ParseErrorKind::UnicodeEscapeError => write!(f, "Unicode escape error"),
            ParseErrorKind::UnexpectedEndOfInput => write!(f, "Unexpected end of input"),
            ParseErrorKind::InvalidCharacter => write!(f, "Invalid character"),
            ParseErrorKind::UnknownKeyword => write!(f, "Unknown keyword"),
            ParseErrorKind::RecursionLimitExceeded => write!(f, "Recursion limit exceeded"),
            ParseErrorKind::UnsupportedFeature => write!(f, "Unsupported feature"),
            ParseErrorKind::SemanticError => write!(f, "Semantic error"),
        }
    }
}

impl std::fmt::Display for StructuredParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at line {}, column {}: {}",
            self.kind, self.position.line, self.position.column, self.message
        )?;

        if let Some(ref token) = self.unexpected_token {
            writeln!(f, "\n  Unexpected token: {}", token)?;
        }

        if !self.expected_tokens.is_empty() {
            writeln!(
                f,
                "\n  Expected one of: {}",
                self.expected_tokens.join(", ")
            )?;
        }

        if let Some(ref context) = self.context {
            writeln!(f, "\n  Context: {}", context)?;
        }

        if !self.hints.is_empty() {
            writeln!(f, "\n  Hint(s):")?;
            for hint in &self.hints {
                writeln!(f, "    - {}", hint)?;
            }
        }

        Ok(())
    }
}

impl std::error::Error for StructuredParseError {}

/// Query error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryErrorKind {
    Storage,
    Parse,
    Planning,
    Optimization,
    InvalidQuery,
    Execution,
    Expression,
    PlanNodeVisit,
    Session,
    Permission,
    Transaction,
    Type,
    Timeout,
}

impl QueryErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueryErrorKind::Storage => "storage",
            QueryErrorKind::Parse => "parse",
            QueryErrorKind::Planning => "planning",
            QueryErrorKind::Optimization => "optimization",
            QueryErrorKind::InvalidQuery => "invalid_query",
            QueryErrorKind::Execution => "execution",
            QueryErrorKind::Expression => "expression",
            QueryErrorKind::PlanNodeVisit => "plan_node_visit",
            QueryErrorKind::Session => "session",
            QueryErrorKind::Permission => "permission",
            QueryErrorKind::Transaction => "transaction",
            QueryErrorKind::Type => "type",
            QueryErrorKind::Timeout => "timeout",
        }
    }
}

/// Query layer error type
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep struct size minimal (~24 bytes)
/// 2. Full context: Preserves error chain
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Debug)]
pub struct QueryError {
    kind: QueryErrorKind,
    message: String,
    source: Option<BoxedError>,
}

impl QueryError {
    pub fn new(kind: QueryErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: BoxedError) -> Self {
        self.source = Some(source);
        self
    }

    pub fn kind(&self) -> QueryErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn source(&self) -> &Option<BoxedError> {
        &self.source
    }

    fn from_boxed<E: Error + Send + Sync + 'static>(kind: QueryErrorKind, error: E) -> Self {
        Self {
            kind,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for QueryError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl QueryError {
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Storage, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Parse, message)
    }

    pub fn planning(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Planning, message)
    }

    pub fn optimization(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Optimization, message)
    }

    pub fn invalid_query(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::InvalidQuery, message)
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Execution, message)
    }

    pub fn expression(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Expression, message)
    }

    pub fn transaction(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Transaction, message)
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Type, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(QueryErrorKind::Timeout, message)
    }

    pub fn structured_parse_error(err: StructuredParseError) -> Self {
        Self::from_boxed(QueryErrorKind::Parse, err)
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::structured_parse_error(StructuredParseError {
            kind: ParseErrorKind::SyntaxError,
            message: message.into(),
            position: crate::core::types::Position::new(0, 0),
            offset: None,
            unexpected_token: None,
            expected_tokens: Vec::new(),
            hints: Vec::new(),
            context: None,
        })
    }

    pub fn parse_error_with_offset(message: impl Into<String>, offset: usize) -> Self {
        Self::structured_parse_error(StructuredParseError {
            kind: ParseErrorKind::SyntaxError,
            message: message.into(),
            position: crate::core::types::Position::new(0, 0),
            offset: Some(offset),
            unexpected_token: None,
            expected_tokens: Vec::new(),
            hints: Vec::new(),
            context: None,
        })
    }

    pub fn parse_error_with_location(
        message: impl Into<String>,
        offset: usize,
        location: impl Into<String>,
    ) -> Self {
        Self::structured_parse_error(StructuredParseError {
            kind: ParseErrorKind::SyntaxError,
            message: message.into(),
            position: crate::core::types::Position::new(0, 0),
            offset: Some(offset),
            unexpected_token: None,
            expected_tokens: Vec::new(),
            hints: vec![location.into()],
            context: None,
        })
    }

    pub fn offset(&self) -> Option<usize> {
        if self.kind == QueryErrorKind::Parse {
            if let Some(ref source) = self.source {
                if let Some(pe) = source.downcast_ref::<StructuredParseError>() {
                    return pe.offset;
                }
            }
        }
        None
    }

    pub fn location(&self) -> Option<&str> {
        if self.kind == QueryErrorKind::Parse {
            if let Some(ref source) = self.source {
                if let Some(pe) = source.downcast_ref::<StructuredParseError>() {
                    if !pe.hints.is_empty() {
                        return Some(&pe.hints[0]);
                    }
                }
            }
        }
        None
    }

    pub fn parse_error_position(&self) -> Option<crate::core::types::Position> {
        if self.kind == QueryErrorKind::Parse {
            if let Some(ref source) = self.source {
                if let Some(pe) = source.downcast_ref::<StructuredParseError>() {
                    return Some(pe.position);
                }
            }
        }
        None
    }

    pub fn parse_error_kind(&self) -> Option<ParseErrorKind> {
        if self.kind == QueryErrorKind::Parse {
            if let Some(ref source) = self.source {
                if let Some(pe) = source.downcast_ref::<StructuredParseError>() {
                    return Some(pe.kind);
                }
            }
        }
        None
    }

    pub fn pipeline_parse_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::from_boxed(QueryErrorKind::Parse, e)
    }

    pub fn pipeline_validation_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::from_boxed(QueryErrorKind::InvalidQuery, e)
    }

    pub fn pipeline_planning_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::from_boxed(QueryErrorKind::Planning, e)
    }

    pub fn pipeline_optimization_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::from_boxed(QueryErrorKind::Optimization, e)
    }

    pub fn pipeline_execution_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::from_boxed(QueryErrorKind::Execution, e)
    }

    pub fn pipeline_error(phase: QueryPhase, message: String) -> Self {
        match phase {
            QueryPhase::Parse => Self::parse(message),
            QueryPhase::Validate => Self::invalid_query(message),
            QueryPhase::Plan => Self::planning(message),
            QueryPhase::Optimize => Self::optimization(message),
            QueryPhase::Execute => Self::execution(message),
        }
    }
}

impl From<StorageError> for QueryError {
    fn from(e: StorageError) -> Self {
        Self::from_boxed(QueryErrorKind::Storage, e)
    }
}

impl From<DBError> for QueryError {
    fn from(e: DBError) -> Self {
        use crate::core::error::ErrorKind;
        match e.kind() {
            ErrorKind::Query => {
                if let Some(ref source) = e.source() {
                    if let Some(qe) = source.downcast_ref::<QueryError>() {
                        return qe.clone();
                    }
                }
                Self::execution(e.message().to_string())
            }
            ErrorKind::Storage => Self::storage(e.message()),
            ErrorKind::Expression => Self::expression(e.message()),
            ErrorKind::Plan => Self::execution(e.message()),
            ErrorKind::Manager => Self::execution(e.message()),
            ErrorKind::Validation => Self::invalid_query(e.message()),
            ErrorKind::Io => Self::execution(e.message()),
            ErrorKind::TypeDeduction => Self::type_error(e.message()),
            ErrorKind::Serialization => Self::execution(e.message()),
            ErrorKind::Index => Self::execution(e.message()),
            ErrorKind::Transaction => Self::transaction(e.message()),
            ErrorKind::Internal => Self::execution(e.message()),
            ErrorKind::Session => Self::execution(e.message()),
            ErrorKind::Auth => Self::execution(e.message()),
            ErrorKind::Permission => Self::execution(e.message()),
            ErrorKind::MemoryLimitExceeded => Self::execution(e.message()),
            ErrorKind::Fulltext => Self::execution(e.message()),
            ErrorKind::Coordinator => Self::execution(e.message()),
            ErrorKind::Vector => Self::execution(e.message()),
            ErrorKind::VectorCoordinator => Self::execution(e.message()),
            ErrorKind::Search => Self::execution(e.message()),
            ErrorKind::GraphService => Self::execution(e.message()),
        }
    }
}

impl From<std::io::Error> for QueryError {
    fn from(e: std::io::Error) -> Self {
        Self::execution(e.to_string())
    }
}

impl From<PlanNodeVisitError> for QueryError {
    fn from(e: PlanNodeVisitError) -> Self {
        Self::from_boxed(QueryErrorKind::PlanNodeVisit, e)
    }
}

impl From<ManagerError> for QueryError {
    fn from(e: ManagerError) -> Self {
        Self::execution(e.to_string())
    }
}

impl ToPublicError for QueryError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self.kind {
            QueryErrorKind::Parse => ErrorCode::ParseError,
            QueryErrorKind::InvalidQuery => ErrorCode::ValidationError,
            QueryErrorKind::Planning => ErrorCode::ExecutionError,
            QueryErrorKind::Optimization => ErrorCode::ExecutionError,
            QueryErrorKind::Execution => ErrorCode::ExecutionError,
            QueryErrorKind::Expression => ErrorCode::ExecutionError,
            QueryErrorKind::Storage => ErrorCode::InternalError,
            QueryErrorKind::PlanNodeVisit => ErrorCode::ExecutionError,
            QueryErrorKind::Session => ErrorCode::Unauthorized,
            QueryErrorKind::Permission => ErrorCode::PermissionDenied,
            QueryErrorKind::Transaction => ErrorCode::ExecutionError,
            QueryErrorKind::Type => ErrorCode::TypeError,
            QueryErrorKind::Timeout => ErrorCode::Timeout,
        }
    }

    fn to_public_message(&self) -> String {
        match self.kind {
            QueryErrorKind::Session => self.message.clone(),
            QueryErrorKind::Permission => self.message.clone(),
            QueryErrorKind::Storage => "Storage operation failed".to_string(),
            _ => self.message.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queryerror_size() {
        assert!(
            std::mem::size_of::<QueryError>() <= 64,
            "QueryError should be small, got {} bytes",
            std::mem::size_of::<QueryError>()
        );
    }

    #[test]
    fn test_queryerror_creation() {
        let err = QueryError::parse("test parse error");
        assert_eq!(err.kind(), QueryErrorKind::Parse);
        assert!(err.message().contains("test parse error"));
    }

    #[test]
    fn test_queryerror_with_source() {
        let storage_err =
            StorageError::node_not_found(crate::core::types::VertexId::from_int64(42));
        let query_err = QueryError::from(storage_err);
        assert_eq!(query_err.kind(), QueryErrorKind::Storage);
    }

    #[test]
    fn test_queryerror_clone() {
        let err = QueryError::execution("test error");
        let cloned = err.clone();
        assert_eq!(err.kind(), cloned.kind());
        assert_eq!(err.message(), cloned.message());
    }

    #[test]
    fn test_pipeline_error() {
        let err = QueryError::pipeline_error(QueryPhase::Parse, "parse failed".to_string());
        assert_eq!(err.kind(), QueryErrorKind::Parse);

        let err = QueryError::pipeline_error(QueryPhase::Execute, "exec failed".to_string());
        assert_eq!(err.kind(), QueryErrorKind::Execution);
    }
}
