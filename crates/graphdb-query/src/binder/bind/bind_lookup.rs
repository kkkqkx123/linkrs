use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_lookup(
        &mut self,
        stmt: &crate::parser::ast::LookupStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            crate::parser::ast::LookupTarget::Tag(t) => {
                self.resolve_tags(std::slice::from_ref(t))?;
                BoundLookupTarget::Tag(t.clone())
            }
            crate::parser::ast::LookupTarget::Edge(e) => {
                self.resolve_edge_types(std::slice::from_ref(e))?;
                BoundLookupTarget::Edge(e.clone())
            }
            crate::parser::ast::LookupTarget::Unspecified(s) => {
                let is_edge = match self.resolve_tags(std::slice::from_ref(s)) {
                    Ok(_) => false,
                    Err(_) => {
                        self.resolve_edge_types(std::slice::from_ref(s))?;
                        true
                    }
                };
                if is_edge {
                    BoundLookupTarget::Edge(s.clone())
                } else {
                    BoundLookupTarget::Tag(s.clone())
                }
            }
        };

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

        Ok(BoundStatement::Lookup(BoundLookupStatement {
            target,
            where_clause,
            yield_clause,
        }))
    }
}
