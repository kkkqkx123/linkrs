use crate::core::types::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct BeginTransactionStmt {
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitTransactionStmt {
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RollbackTransactionStmt {
    pub span: Span,
}
