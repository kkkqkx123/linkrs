use linkrs_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_begin_transaction(
        &mut self,
        stmt: &crate::parser::ast::BeginTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::BeginTransaction(BoundBeginTransaction {
            read_only: stmt.read_only,
        }))
    }

    pub(crate) fn bind_commit(
        &mut self,
        _stmt: &crate::parser::ast::CommitTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Commit(BoundCommit))
    }

    pub(crate) fn bind_rollback(
        &mut self,
        stmt: &crate::parser::ast::RollbackTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Rollback(BoundRollback {
            savepoint_name: stmt.savepoint_name.clone(),
        }))
    }

    pub(crate) fn bind_savepoint(
        &mut self,
        stmt: &crate::parser::ast::SavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Savepoint(BoundSavepoint {
            name: stmt.name.clone(),
        }))
    }

    pub(crate) fn bind_release_savepoint(
        &mut self,
        stmt: &crate::parser::ast::ReleaseSavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ReleaseSavepoint(BoundReleaseSavepoint {
            name: stmt.name.clone(),
        }))
    }

    pub(crate) fn bind_use(
        &mut self,
        stmt: &crate::parser::ast::UseStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Use(BoundUse {
            space: stmt.space.clone(),
        }))
    }
}
