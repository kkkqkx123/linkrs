use linkrs_core::DBResult;

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

        // WEIGHT only makes sense for the shortest-path variant; FIND ALL
        // PATH enumerates every path and never compares costs.
        let weight_expression = stmt.weight_expression.clone();
        if weight_expression.is_some() && !stmt.shortest {
            return Err(crate::binder::error::invalid_query(
                "WEIGHT is only supported for FIND SHORTEST PATH",
            ));
        }

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
            weight_expression,
            with_loop: stmt.with_loop,
            with_trail: stmt.with_trail,
            with_walk: stmt.with_walk,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::error::DBError;

    fn bind_find_path_query(query: &str) -> DBResult<BoundStatement> {
        let mut parser = crate::parser::Parser::new(query);
        let ast = parser
            .parse()
            .map_err(|e| DBError::from(linkrs_core::error::QueryError::pipeline_parse_error(e)))?
            .ast;
        Binder::new().with_space(None, 0).bind(ast)
    }

    #[test]
    fn weight_on_all_path_is_rejected_at_bind_time() {
        let err = bind_find_path_query("FIND ALL PATH FROM 1 TO 4 OVER KNOWS WEIGHT weight")
            .expect_err("WEIGHT on FIND ALL PATH must not bind");
        assert!(
            err.to_string()
                .contains("WEIGHT is only supported for FIND SHORTEST PATH"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn weight_on_shortest_path_reaches_the_bound_statement() {
        let bound = bind_find_path_query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT weight")
            .expect("WEIGHT on FIND SHORTEST PATH binds");
        let stmt = match bound {
            BoundStatement::FindPath(s) => s,
            other => panic!("expected FindPath, got {:?}", other.kind()),
        };
        assert_eq!(stmt.weight_expression.as_deref(), Some("weight"));
    }

    #[test]
    fn shortest_path_without_weight_keeps_none() {
        let bound = bind_find_path_query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
            .expect("plain FIND SHORTEST PATH binds");
        let stmt = match bound {
            BoundStatement::FindPath(s) => s,
            other => panic!("expected FindPath, got {:?}", other.kind()),
        };
        assert!(stmt.weight_expression.is_none());
    }
}
