use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_find_path(
        &mut self,
        stmt: &crate::parser::ast::FindPathStmt,
    ) -> DBResult<BoundStatement> {
        let from = stmt
            .from
            .vertices
            .iter()
            .map(|v| self.bind_expr(v))
            .collect::<DBResult<Vec<_>>>()?;
        let to = self.bind_expr(&stmt.to)?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let yield_clause = stmt
            .yield_clause
            .as_ref()
            .map(|yc| self.bind_yield_clause(yc))
            .transpose()?;

        let over = stmt
            .over
            .as_ref()
            .map(|o| (o.edge_types.clone(), o.direction));

        Ok(BoundStatement::FindPath(BoundFindPathStatement {
            from,
            to,
            over,
            where_clause,
            shortest: stmt.shortest,
            max_steps: stmt.max_steps,
            limit: stmt.limit.clone(),
            skip: stmt.skip.clone(),
            yield_clause,
        }))
    }
}
