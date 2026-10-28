use linkrs_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_explain(
        &mut self,
        stmt: &crate::parser::ast::ExplainStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Explain(BoundExplain {
            statement: Box::new(inner),
            format: stmt.format.clone(),
            analyze: stmt.analyze,
        }))
    }

    pub(crate) fn bind_profile(
        &mut self,
        stmt: &crate::parser::ast::ProfileStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Profile(BoundProfile {
            statement: Box::new(inner),
            format: stmt.format.clone(),
        }))
    }
}
