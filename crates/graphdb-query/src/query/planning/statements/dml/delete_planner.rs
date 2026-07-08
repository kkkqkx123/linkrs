//! Deletion Operation Planner
//!
//! Query planning for handling DELETE VERTEX/EDGE/TAG statements.
//! Supports both standalone deletion and pipe-based deletion (e.g., GO ... | DELETE VERTEX $-.id).

use crate::query::metadata::MetadataContext;
use crate::query::parser::ast::{DeleteStmt, DeleteTarget, Stmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{
        DeleteEdgesNode, DeleteIndexNode, DeleteTagsNode, DeleteVerticesNode, EdgeDeleteInfo,
        IndexDeleteInfo, PipeDeleteEdgesNode, PipeDeleteVerticesNode, TagDeleteInfo,
        VertexDeleteInfo,
    },
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

/// Deletion Operation Planner
/// Responsible for converting DELETE statements into execution plans.
#[derive(Debug, Clone)]
pub struct DeletePlanner;

impl DeletePlanner {
    /// Create a new deletion planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the DeleteStmt from the Stmt.
    fn extract_delete_stmt(&self, stmt: &Stmt) -> Result<DeleteStmt, PlannerError> {
        match stmt {
            Stmt::Delete(delete_stmt) => Ok(delete_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain a DELETE".to_string(),
            )),
        }
    }
}

impl Planner for DeletePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        self.transform_with_input(validated, qctx, None)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Delete(_))
    }

    fn transform_with_metadata(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        let validation_info = &validated.validation_info;
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        let referenced_edges = &validation_info.semantic_info.referenced_edges;

        for tag_name in referenced_tags {
            let _space_id = qctx.space_id().unwrap_or(0);
            if metadata_context.get_tag_metadata(tag_name).is_none() {
                log::debug!(
                    "Tag '{}' referenced in DELETE not found in metadata context",
                    tag_name
                );
            }
        }

        for edge_type in referenced_edges {
            if metadata_context.get_edge_type_metadata(edge_type).is_none() {
                log::debug!(
                    "Edge type '{}' referenced in DELETE not found in metadata context",
                    edge_type
                );
            }
        }

        self.transform(validated, qctx)
    }
}

impl DeletePlanner {
    /// Transform with optional input plan (for pipe DELETE)
    fn transform_with_input(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        input_plan: Option<SubPlan>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        let validation_info = &validated.validation_info;
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("DELETE Referenced tags: {:?}", referenced_tags);
        }

        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("DELETE Referenced edge type: {:?}", referenced_edges);
        }

        let delete_stmt = self.extract_delete_stmt(validated.stmt())?;

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let input_node = input_plan.as_ref().and_then(|p| p.root.clone());

        let final_node = match &delete_stmt.target {
            DeleteTarget::Vertices(vertex_ids) => {
                let info = VertexDeleteInfo {
                    space_name,
                    vertex_ids: vertex_ids.clone(),
                    with_edge: delete_stmt.with_edge,
                    condition: delete_stmt.where_clause.clone(),
                };

                if let Some(input) = input_node {
                    let node = PipeDeleteVerticesNode::new(next_node_id(), info, input);
                    PlanNodeEnum::PipeDeleteVertices(node)
                } else {
                    let node = DeleteVerticesNode::new(next_node_id(), info);
                    PlanNodeEnum::DeleteVertices(node)
                }
            }
            DeleteTarget::Edges { edge_type, edges } => {
                let info = EdgeDeleteInfo {
                    space_name,
                    edge_type: edge_type.clone(),
                    edges: edges
                        .iter()
                        .map(|(src, dst, rank)| (src.clone(), dst.clone(), rank.clone()))
                        .collect(),
                    condition: delete_stmt.where_clause.clone(),
                };

                if let Some(input) = input_node {
                    let node = PipeDeleteEdgesNode::new(next_node_id(), info, input);
                    PlanNodeEnum::PipeDeleteEdges(node)
                } else {
                    let node = DeleteEdgesNode::new(next_node_id(), info);
                    PlanNodeEnum::DeleteEdges(node)
                }
            }
            DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                let info = TagDeleteInfo {
                    space_name,
                    tag_names: tag_names.clone(),
                    vertex_ids: vertex_ids.clone(),
                    is_all_tags: *is_all_tags,
                };

                let node = DeleteTagsNode::new(next_node_id(), info);
                PlanNodeEnum::DeleteTags(node)
            }
            DeleteTarget::Index(index_name) => {
                let info = IndexDeleteInfo {
                    space_name,
                    index_name: index_name.clone(),
                };

                let node = DeleteIndexNode::new(next_node_id(), info);
                PlanNodeEnum::DeleteIndex(node)
            }
        };

        let sub_plan = SubPlan::new(Some(final_node), None);

        Ok(sub_plan)
    }
}

impl Default for DeletePlanner {
    fn default() -> Self {
        Self::new()
    }
}
