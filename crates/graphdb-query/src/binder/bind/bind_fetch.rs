use crate::parser::ast::FetchTarget;
use graphdb_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_fetch(
        &mut self,
        stmt: &crate::parser::ast::FetchStmt,
    ) -> DBResult<BoundStatement> {
        match &stmt.target {
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties,
            } => {
                if let Some(tag_name) = tag_name {
                    self.resolve_tags(std::slice::from_ref(tag_name))?;
                }
                let bound_ids = ids
                    .iter()
                    .map(|id| self.bind_expr(id))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundStatement::FetchVertices(BoundFetchVerticesStatement {
                    tag_name: tag_name.clone(),
                    ids: bound_ids,
                    properties: properties.clone(),
                }))
            }
            FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                properties,
            } => {
                let bound_src = self.bind_expr(src)?;
                let bound_dst = self.bind_expr(dst)?;
                let bound_rank = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                Ok(BoundStatement::FetchEdges(BoundFetchEdgesStatement {
                    src: bound_src,
                    dst: bound_dst,
                    edge_type: edge_type.clone(),
                    rank: bound_rank,
                    properties: properties.clone(),
                }))
            }
        }
    }
}
