use graphdb_core::types::EdgeDirection;
use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_go(
        &mut self,
        stmt: &crate::parser::ast::GoStmt,
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

        if let Some(ref o) = stmt.over {
            self.resolve_edge_types(&o.edge_types)?;
        }

        let over = stmt.over.as_ref().map(|o| o.edge_types.clone());

        Ok(BoundStatement::Go(BoundGoStatement {
            steps: stmt.steps.clone(),
            from,
            over,
            direction: stmt
                .over
                .as_ref()
                .map(|o| o.direction)
                .unwrap_or(EdgeDirection::Out),
            where_clause,
            yield_clause,
        }))
    }
}
