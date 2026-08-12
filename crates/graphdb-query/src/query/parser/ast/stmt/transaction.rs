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

#[derive(Debug, Clone, PartialEq)]
pub struct RollbackTransactionStmt {
    pub span: Span,
}
