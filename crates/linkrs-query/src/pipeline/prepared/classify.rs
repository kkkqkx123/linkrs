use crate::dml_events::DmlOp;
use crate::parser::ast::stmt::CreateTarget;
use crate::parser::ast::Stmt;

/// Map a bound-equivalent parsed statement to its DML operation.
///
/// Single source of truth for statement-level DML hooks: external hooks
/// must consume this (via the prepare-stage [`StatementClass`] / `Stmt`).
/// Composite writes (`Pipe` / `SetOperation`) merge branch operations with
/// `Delete > Insert > Update` priority to preserve the single-event shape;
/// data-form `CREATE` (node / edge / path patterns) counts as `Insert`
/// while schema `CREATE` targets stay silent; DDL/DCL/reads/diagnostics
/// return `None`.
pub fn dml_op_from_stmt(stmt: &Stmt) -> Option<DmlOp> {
    match stmt {
        Stmt::Insert(_) | Stmt::Copy(_) | Stmt::Merge(_) => Some(DmlOp::Insert),
        Stmt::Create(c) => match &c.target {
            CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. } => {
                Some(DmlOp::Insert)
            }
            _ => None,
        },
        Stmt::Update(_) | Stmt::Set(_) | Stmt::Remove(_) => Some(DmlOp::Update),
        Stmt::Delete(_) => Some(DmlOp::Delete),
        Stmt::Match(m) => {
            if m.delete_clause.is_some() {
                Some(DmlOp::Delete)
            } else {
                None
            }
        }
        Stmt::Pipe(pipe) => merge_dml_ops(
            dml_op_from_stmt(&pipe.left),
            dml_op_from_stmt(&pipe.right),
        ),
        Stmt::SetOperation(set_op) => merge_dml_ops(
            dml_op_from_stmt(&set_op.left),
            dml_op_from_stmt(&set_op.right),
        ),
        _ => None,
    }
}

fn merge_dml_ops(left: Option<DmlOp>, right: Option<DmlOp>) -> Option<DmlOp> {
    match (left, right) {
        (None, other) => other,
        (other, None) => other,
        (Some(a), Some(b)) => Some(prioritize_dml_op(a, b)),
    }
}

fn prioritize_dml_op(a: DmlOp, b: DmlOp) -> DmlOp {
    if a == DmlOp::Delete || b == DmlOp::Delete {
        DmlOp::Delete
    } else if a == DmlOp::Insert || b == DmlOp::Insert {
        DmlOp::Insert
    } else {
        DmlOp::Update
    }
}

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
            | Stmt::PurgeExpired(_)
            | Stmt::ImportDatabase(_)
            | Stmt::ExportDatabase(_)
            | Stmt::AttachDatabase(_)
            | Stmt::DetachDatabase(_)
    )
}

/// Whether the statement changes process-global catalog state rather than
/// one space. Attach/Detach mutate the process-global attached registry, so
/// per-space invalidation would leave other spaces holding stale plans.
pub fn is_global_catalog_change(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::AttachDatabase(_) | Stmt::DetachDatabase(_))
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
                | Stmt::Checkpoint(_)
                | Stmt::ExportDatabase(_)
                | Stmt::ImportDatabase(_)
                | Stmt::AttachDatabase(_)
                | Stmt::DetachDatabase(_)
                | Stmt::PurgeExpired(_)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::stmt::{CheckpointStmt, PurgeExpiredStmt, PurgeTarget};
    use crate::parser::Parser;
    use linkrs_core::types::Span;

    fn parse(query: &str) -> Stmt {
        let mut parser = Parser::new(query);
        let result = parser
            .parse()
            .unwrap_or_else(|e| panic!("parse failed for {query:?}: {e}"));
        assert!(!parser.has_errors(), "parse errors for {query:?}");
        result.ast.stmt().clone()
    }

    #[test]
    fn dml_op_matches_execution_plan_classification() {
        assert_eq!(
            dml_op_from_stmt(&parse("INSERT VERTEX person(name) VALUES \"p1\": (\"a\")")),
            Some(DmlOp::Insert)
        );
        assert_eq!(
            dml_op_from_stmt(&parse("COPY person FROM 'f.csv'")),
            Some(DmlOp::Insert)
        );
        assert_eq!(
            dml_op_from_stmt(&parse("UPDATE VERTEX \"v1\" SET name = \"x\"")),
            Some(DmlOp::Update)
        );
        assert_eq!(
            dml_op_from_stmt(&parse("MATCH (n:person) RETURN n")),
            None,
            "pure reads stay silent"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("CREATE TAG IF NOT EXISTS Person(name: STRING)")),
            None,
            "schema DDL is not DML"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("DROP TAG Person")),
            None,
            "schema DDL is not DML"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("EXPLAIN MATCH (n) RETURN n")),
            None,
            "diagnostics stay silent"
        );
    }

    #[test]
    fn dml_op_covers_bare_clauses_and_comment_prefixes() {
        assert_eq!(
            dml_op_from_stmt(&parse("  -- comment\nCREATE (n)")),
            Some(DmlOp::Insert),
            "data-form CREATE is a write"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("/* block */ MERGE (n)")),
            Some(DmlOp::Insert)
        );
        assert_eq!(
            dml_op_from_stmt(&parse("// slash\n-- dash\nSET n.x = 1")),
            Some(DmlOp::Update)
        );
        assert_eq!(
            dml_op_from_stmt(&parse("  match (n) return n")),
            None,
            "pure reads stay silent"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("OPTIONAL MATCH (n) RETURN n")),
            None,
            "pure reads stay silent"
        );
        assert_eq!(
            dml_op_from_stmt(&parse("DELETE VERTEX person FROM \"v1\"")),
            Some(DmlOp::Delete)
        );
    }
    #[test]
    fn dml_op_pipe_merges_branches_with_delete_priority() {
        let left = parse("INSERT VERTEX person(name) VALUES \"p1\": (\"a\")");
        let right = parse("DELETE VERTEX person FROM \"v1\"");
        if let (Stmt::Insert(_), Stmt::Delete(_)) = (&left, &right) {
            let pipe = Stmt::Pipe(crate::parser::ast::stmt::PipeStmt {
                span: Span::default(),
                left: Box::new(left),
                right: Box::new(right),
            });
            assert_eq!(dml_op_from_stmt(&pipe), Some(DmlOp::Delete));
        } else {
            panic!("expected Insert/Delete branches");
        }
    }

    #[test]
    fn purge_expired_is_not_read_only_cacheable() {
        let stmt = Stmt::PurgeExpired(PurgeExpiredStmt {
            span: Span::default(),
            space_name: "s".to_string(),
            target: Some(PurgeTarget::Tag("t".to_string())),
        });
        assert!(!is_read_only_cacheable(&stmt));
    }

    #[test]
    fn checkpoint_is_not_read_only_cacheable() {
        let stmt = Stmt::Checkpoint(CheckpointStmt {
            span: Span::default(),
        });
        assert!(!is_read_only_cacheable(&stmt));
    }

    #[test]
    fn import_and_attach_are_ddl() {
        for query in [
            "IMPORT DATABASE '/tmp/x'",
            "ATTACH '/tmp/a' AS analytics",
            "DETACH analytics",
        ] {
            let stmt = parse(query);
            assert!(is_ddl(&stmt), "DDL: {query}");
            assert_eq!(classify_statement(&stmt), StatementClass::Ddl);
            assert!(!is_read_only_cacheable(&stmt));
        }
    }

    #[test]
    fn only_attach_detach_are_global_catalog_changes() {
        assert!(is_global_catalog_change(&parse(
            "ATTACH '/tmp/a' AS analytics"
        )));
        assert!(is_global_catalog_change(&parse("DETACH analytics")));
        assert!(!is_global_catalog_change(&parse(
            "IMPORT DATABASE '/tmp/x'"
        )));
        assert!(!is_global_catalog_change(&parse(
            "CREATE TAG IF NOT EXISTS Person(name: STRING)"
        )));
    }
}
