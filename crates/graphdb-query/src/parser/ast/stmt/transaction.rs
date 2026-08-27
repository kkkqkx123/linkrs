use crate::core::types::expr::ContextualExpression;
use crate::core::types::Span;

/// `BEGIN [TRANSACTION] [READ ONLY | READ WRITE]` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct BeginTransactionStmt {
    pub span: Span,
    /// Transaction access mode: `Some(true)` for READ ONLY, `Some(false)`
    /// for READ WRITE, `None` when unspecified (defaults to READ WRITE).
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitTransactionStmt {
    pub span: Span,
}

/// `ROLLBACK [TRANSACTION] [TO <savepoint-name>]` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct RollbackTransactionStmt {
    pub span: Span,
    /// Savepoint name for `ROLLBACK TO <savepoint>`; `None` for a full
    /// transaction rollback. The name keeps its original case (the
    /// transaction layer matches savepoint names verbatim).
    pub savepoint_name: Option<String>,
}

/// `SAVEPOINT <savepoint-name>` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct SavepointStmt {
    pub span: Span,
    pub name: String,
}

/// `RELEASE SAVEPOINT <savepoint-name>` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseSavepointStmt {
    pub span: Span,
    pub name: String,
}

/// `LET [$]name = expr` session-variable assignment statement.
#[derive(Debug, Clone, PartialEq)]
pub struct AssignVariableStmt {
    pub span: Span,
    /// Session variable name (without the leading `$`), validated against
    /// `[A-Za-z_][A-Za-z0-9_]*`.
    pub name: String,
    /// Right-hand side expression, evaluated through the standard expression
    /// pipeline (may reference `$name` session variables and `@name`
    /// parameters).
    pub expression: ContextualExpression,
}
