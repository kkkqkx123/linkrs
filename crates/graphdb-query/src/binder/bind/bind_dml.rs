use crate::parser::ast::SetOperationType;
use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_pipe(
        &mut self,
        stmt: &crate::parser::ast::PipeStmt,
    ) -> DBResult<BoundStatement> {
        let statements = vec![self.bind_stmt(&stmt.left)?, self.bind_stmt(&stmt.right)?];

        Ok(BoundStatement::Pipe(BoundPipeStatement {
            span: stmt.span,
            statements,
        }))
    }

    pub(crate) fn bind_set_operation(
        &mut self,
        stmt: &crate::parser::ast::SetOperationStmt,
    ) -> DBResult<BoundStatement> {
        let left = Box::new(self.bind_stmt(&stmt.left)?);
        let right = Box::new(self.bind_stmt(&stmt.right)?);
        let operation = match stmt.op_type {
            SetOperationType::Union | SetOperationType::UnionAll => SetOperationKind::Union,
            SetOperationType::Intersect => SetOperationKind::Intersect,
            SetOperationType::Minus => SetOperationKind::Minus,
        };
        Ok(BoundStatement::SetOperation(BoundSetOperationStatement {
            span: stmt.span,
            left,
            right,
            operation,
        }))
    }

    pub(crate) fn bind_group_by(
        &mut self,
        stmt: &crate::parser::ast::GroupByStmt,
    ) -> DBResult<BoundStatement> {
        let keys = stmt
            .group_items
            .iter()
            .map(|k| self.bind_expr(k))
            .collect::<DBResult<Vec<_>>>()?;

        Ok(BoundStatement::GroupBy(BoundGroupByStatement {
            span: stmt.span,
            keys,
            aggregates: Vec::new(),
        }))
    }
}
