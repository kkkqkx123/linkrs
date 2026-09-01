use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_create(
        &mut self,
        stmt: &crate::parser::ast::CreateStmt,
    ) -> DBResult<BoundStatement> {
        // Validate property definitions for tag/edge creation: no duplicate names.
        if let crate::parser::ast::CreateTarget::Tag { properties, .. }
        | crate::parser::ast::CreateTarget::EdgeType { properties, .. } = &stmt.target
        {
            let mut seen = std::collections::HashSet::new();
            for prop in properties {
                if !seen.insert(&prop.name) {
                    return Err(graphdb_core::error::DBError::from(
                        graphdb_core::error::QueryError::invalid_query(format!(
                            "Duplicate property name '{}'",
                            prop.name
                        )),
                    ));
                }
            }
        }
        Ok(BoundStatement::Create(BoundCreate {
            span: stmt.span,
            target: stmt.target.clone(),
            if_not_exists: stmt.if_not_exists,
        }))
    }

    pub(crate) fn bind_drop(
        &mut self,
        stmt: &crate::parser::ast::DropStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Drop(BoundDrop {
            span: stmt.span,
            target: stmt.target.clone(),
            if_exists: stmt.if_exists,
        }))
    }

    pub(crate) fn bind_alter(
        &mut self,
        stmt: &crate::parser::ast::AlterStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Alter(BoundAlter {
            span: stmt.span,
            target: stmt.target.clone(),
        }))
    }

    pub(crate) fn bind_desc(
        &mut self,
        stmt: &crate::parser::ast::DescStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::Desc(stmt.clone()),
        )))
    }

    pub(crate) fn bind_show_create(
        &mut self,
        stmt: &crate::parser::ast::ShowCreateStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ShowCreate(stmt.clone()),
        )))
    }

    pub(crate) fn bind_clear_space(
        &mut self,
        stmt: &crate::parser::ast::ClearSpaceStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ClearSpace(stmt.clone()),
        )))
    }

    pub(crate) fn bind_begin_transaction(
        &mut self,
        stmt: &crate::parser::ast::BeginTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::BeginTransaction(BoundBeginTransaction {
            span: stmt.span,
            read_only: stmt.read_only,
        }))
    }

    pub(crate) fn bind_commit(
        &mut self,
        stmt: &crate::parser::ast::CommitTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Commit(BoundCommit { span: stmt.span }))
    }

    pub(crate) fn bind_rollback(
        &mut self,
        stmt: &crate::parser::ast::RollbackTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Rollback(BoundRollback {
            span: stmt.span,
            savepoint_name: stmt.savepoint_name.clone(),
        }))
    }

    pub(crate) fn bind_savepoint(
        &mut self,
        stmt: &crate::parser::ast::SavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::Savepoint(stmt.clone()),
        )))
    }

    pub(crate) fn bind_release_savepoint(
        &mut self,
        stmt: &crate::parser::ast::ReleaseSavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ReleaseSavepoint(stmt.clone()),
        )))
    }

    pub(crate) fn bind_assign_variable(
        &mut self,
        stmt: &crate::parser::ast::AssignVariableStmt,
    ) -> DBResult<BoundStatement> {
        let bound_expr = self.bind_expr(&stmt.expression)?;
        // Validate variable name is defined in current scope? For assignment, we just ensure expression is valid.
        // No extra scope mutation needed at bind time.
        let _ = bound_expr;
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::AssignVariable(stmt.clone()),
        )))
    }
}
