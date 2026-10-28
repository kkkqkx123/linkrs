//! The FETCH VERTICES query planner
//! Planning for the FETCH VERTICES query

use crate::binder::BoundStatement;
use crate::planning::plan::core::common::TagProp;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{ArgumentNode, GetVerticesNode, PlanNodeEnum};
use crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::plan_combiner::logical_argument_root;
use crate::planning::statements::projection_util;

/// The FETCH VERTICES query planner
/// Responsible for converting the FETCH VERTICES query into an execution plan.
#[derive(Debug, Clone)]
pub struct FetchVerticesPlanner;

impl FetchVerticesPlanner {
    /// Create a new FETCH VERTICES planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for FetchVerticesPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let fetch = match bound {
            BoundStatement::FetchVertices(f) => f,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchVerticesPlanner requires the FetchVertices statement.".to_string(),
                ));
            }
        };

        let space_id = qctx.space_id().unwrap_or(0);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let expr_ctx = std::sync::Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let ids_str = fetch
            .ids
            .iter()
            .filter_map(|id| {
                let ctx_expr =
                    crate::binder::expr_converter::bound_expr_to_contextual(id, &expr_ctx).ok()?;
                let meta = ctx_expr.expression()?;
                let expr = meta.inner();
                Some(match expr {
                    linkrs_core::Expression::Literal(linkrs_core::Value::Int(i)) => i.to_string(),
                    linkrs_core::Expression::Literal(linkrs_core::Value::BigInt(i)) => {
                        i.to_string()
                    }
                    linkrs_core::Expression::Literal(linkrs_core::Value::String(s)) => {
                        s.to_string()
                    }
                    _ => expr.to_string(),
                })
            })
            .collect::<Vec<_>>()
            .join(",");

        let var_name = "v";
        let entity_var = fetch
            .tag_name
            .clone()
            .unwrap_or_else(|| "vertex".to_string());

        let mut arg_node = ArgumentNode::new(next_node_id(), var_name);
        arg_node.set_col_names(vec!["vid".to_string()]);
        arg_node.set_output_var("vertex_ids".to_string());
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        let mut get_vertices_node = GetVerticesNode::new(space_id, &space_name, &ids_str);
        get_vertices_node.add_dependency(arg_node_enum.clone());
        get_vertices_node.set_output_var("fetched_vertices".to_string());
        get_vertices_node.set_col_names(vec![entity_var.clone()]);
        let fetch_tag = fetch.tag_name.clone().ok_or_else(|| {
            PlannerError::InvalidOperation("FETCH VERTICES requires a tag qualifier".to_string())
        })?;
        get_vertices_node.set_tag(fetch_tag);

        let tag_props = if let Some(ref props) = fetch.properties {
            vec![TagProp::new("default", props.clone())]
        } else {
            vec![]
        };
        get_vertices_node.set_tag_props(tag_props);

        let current_logical = get_vertices_mirror(&get_vertices_node, logical_argument());

        let root = PlanNodeEnum::GetVertices(get_vertices_node);

        let sub_plan = SubPlan::from_dual(Some(root), Some(arg_node_enum), Some(current_logical));
        Ok(sub_plan)
    }
}

impl Default for FetchVerticesPlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Seed logical argument matching the physical `v` argument node.
fn logical_argument() -> LogicalNodeEnum {
    logical_argument_root("v", vec!["vid".to_string()], Some("vertex_ids".to_string()))
}

/// Mirror a physical fetch-vertices lookup over the logical argument seed.
fn get_vertices_mirror(
    get_vertices_node: &GetVerticesNode,
    arg_logical: LogicalNodeEnum,
) -> LogicalNodeEnum {
    LogicalNodeEnum::GetVertices(LogicalGetVerticesNode {
        id: next_node_id(),
        deps: vec![arg_logical],
        space_id: get_vertices_node.space_id(),
        space_name: get_vertices_node.space_name().to_string(),
        tag: get_vertices_node.tag().map(|s| s.to_string()),
        src_ref: get_vertices_node.src_ref().clone(),
        src_vids: get_vertices_node.src_vids().to_string(),
        tag_props: get_vertices_node.tag_props().to_vec(),
        expression: get_vertices_node.filter().cloned(),
        dedup: get_vertices_node.dedup(),
        limit: get_vertices_node.limit(),
        projected_properties: get_vertices_node.projected_properties().to_vec(),
        output_var: get_vertices_node.output_var().map(|s| s.to_string()),
        col_names: get_vertices_node.col_names().to_vec(),
        column_types: projection_util::unknown_column_types(get_vertices_node.col_names().len()),
    })
}
