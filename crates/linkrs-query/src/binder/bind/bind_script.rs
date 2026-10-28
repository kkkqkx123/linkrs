use linkrs_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_assignment_statement(
        &mut self,
        stmt: &crate::parser::ast::AssignmentStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Assignment(BoundAssignmentStatement {
            variable: stmt.variable.clone(),
            statement: Box::new(inner),
        }))
    }

    pub(crate) fn bind_assign_variable(
        &mut self,
        stmt: &crate::parser::ast::AssignVariableStmt,
    ) -> DBResult<BoundStatement> {
        let bound_expr = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::AssignVariable(BoundAssignVariable {
            name: stmt.name.clone(),
            expression: bound_expr,
        }))
    }
}
