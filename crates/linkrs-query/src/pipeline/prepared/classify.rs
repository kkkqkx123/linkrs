use std::sync::Arc;

use crate::parser::ast::Stmt;

/// Classification of a prepared statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    Analyze,
    ReadOnly,
    Dml,
    Ddl,
    Transaction,
    Diagnostic,
}

/// Check whether a statement performs any write operations to storage.
///
/// This detects both standalone DML (INSERT/DELETE/UPDATE), MATCH statements
/// with embedded DELETE clauses, and DML nested inside pipe or set-operation
/// statements.
pub fn requires_write_storage(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Match(m) => m.delete_clause.is_some(),
        Stmt::Pipe(pipe) => {
            requires_write_storage(&pipe.left) || requires_write_storage(&pipe.right)
        }
        Stmt::SetOperation(set_op) => {
            requires_write_storage(&set_op.left) || requires_write_storage(&set_op.right)
        }
        _ => requires_auto_commit(stmt),
    }
}

pub fn classify_statement(stmt: &Stmt) -> StatementClass {
    if is_diagnostic(stmt) {
        StatementClass::Diagnostic
    } else if is_analyze(stmt) {
        StatementClass::Analyze
    } else if is_transaction(stmt) {
        StatementClass::Transaction
    } else if is_ddl(stmt) {
        StatementClass::Ddl
    } else if requires_write_storage(stmt) {
        StatementClass::Dml
    } else {
        StatementClass::ReadOnly
    }
}

pub fn is_analyze(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Analyze(_))
}

pub fn requires_auto_commit(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Pipe(pipe) => requires_auto_commit(&pipe.left) || requires_auto_commit(&pipe.right),
        Stmt::SetOperation(set_op) => {
            requires_auto_commit(&set_op.left) || requires_auto_commit(&set_op.right)
        }
        _ => is_direct_write_statement(stmt),
    }
}

pub(crate) fn is_direct_write_statement(stmt: &Stmt) -> bool {
    is_direct_dml_statement(stmt) || is_direct_dcl(stmt)
}

pub fn is_direct_dml_statement(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Insert(_)
            | Stmt::Copy(_)
            | Stmt::Delete(_)
            | Stmt::Update(_)
            | Stmt::Merge(_)
            | Stmt::Set(_)
            | Stmt::Remove(_)
    )
}

pub(crate) fn is_direct_dcl(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::CreateUser(_)
            | Stmt::AlterUser(_)
            | Stmt::DropUser(_)
            | Stmt::ChangePassword(_)
            | Stmt::Grant(_)
            | Stmt::Revoke(_)
            | Stmt::UpdateConfigs(_)
    )
}

pub fn is_transaction(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::BeginTransaction(..)
            | Stmt::CommitTransaction(..)
            | Stmt::RollbackTransaction(..)
            | Stmt::Savepoint(..)
            | Stmt::ReleaseSavepoint(..)
    )
}

pub fn is_ddl(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Create(_)
            | Stmt::Drop(_)
            | Stmt::Alter(_)
            | Stmt::ClearSpace(_)
            | Stmt::CreateFulltextIndex(_)
            | Stmt::DropFulltextIndex(_)
            | Stmt::AlterFulltextIndex(_)
            | Stmt::CreateVectorIndex(_)
            | Stmt::DropVectorIndex(_)
            | Stmt::CreateMacro(_)
            | Stmt::DropMacro(_)
            | Stmt::CreateType(_)
            | Stmt::DropType(_)
            | Stmt::CommentOn(_)
    )
}

pub fn is_diagnostic(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Explain(_) | Stmt::Profile(_))
}

pub fn is_read_only_cacheable(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Pipe(pipe) => {
            is_read_only_cacheable(&pipe.left) && is_read_only_cacheable(&pipe.right)
        }
        Stmt::SetOperation(set_op) => {
            is_read_only_cacheable(&set_op.left) && is_read_only_cacheable(&set_op.right)
        }
        _ => !matches!(
            stmt,
            Stmt::Insert(_)
                | Stmt::Copy(_)
                | Stmt::Update(_)
                | Stmt::Delete(_)
                | Stmt::Set(_)
                | Stmt::Remove(_)
                | Stmt::Merge(_)
                | Stmt::Create(_)
                | Stmt::Drop(_)
                | Stmt::Alter(_)
                | Stmt::ClearSpace(_)
                | Stmt::CreateFulltextIndex(_)
                | Stmt::DropFulltextIndex(_)
                | Stmt::AlterFulltextIndex(_)
                | Stmt::CreateVectorIndex(_)
                | Stmt::DropVectorIndex(_)
                | Stmt::CreateMacro(_)
                | Stmt::DropMacro(_)
                | Stmt::CreateType(_)
                | Stmt::DropType(_)
                | Stmt::CreateUser(_)
                | Stmt::AlterUser(_)
                | Stmt::DropUser(_)
                | Stmt::ChangePassword(_)
                | Stmt::Grant(_)
                | Stmt::Revoke(_)
                | Stmt::UpdateConfigs(_)
                | Stmt::BeginTransaction(_)
                | Stmt::CommitTransaction(_)
                | Stmt::RollbackTransaction(_)
                | Stmt::Explain(_)
                | Stmt::Profile(_)
                | Stmt::Analyze(_)
                | Stmt::CommentOn(_)
                | Stmt::Checkpoint(_)
                | Stmt::ExportDatabase(_)
                | Stmt::ImportDatabase(_)
                | Stmt::AttachDatabase(_)
                | Stmt::DetachDatabase(_)
        ),
    }
}

pub fn build_validated_fallback(
    ast: &Arc<crate::parser::ast::stmt::Ast>,
) -> crate::binder::validation::ValidatedStatement {
    crate::binder::validation::ValidatedStatement::new(
        ast.clone(),
        crate::binder::validation::ValidationInfo::new(),
    )
}
