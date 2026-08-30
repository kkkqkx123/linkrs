use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_subgraph(
        &mut self,
        stmt: &crate::parser::ast::SubgraphStmt,
    ) -> DBResult<BoundStatement> {
        let from = stmt
            .from
            .vertices
            .iter()
            .map(|v| self.bind_expr(v))
            .collect::<DBResult<Vec<_>>>()?;

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

        Ok(BoundStatement::Subgraph(BoundSubgraphStatement {
            span: stmt.span,
            steps: stmt.steps.clone(),
            from,
            over,
            where_clause,
            yield_clause,
        }))
    }
}
