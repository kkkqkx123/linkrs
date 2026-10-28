//! Pattern planner: turns AST node/edge/path patterns into sub-plans.
//!
//! ## Module Structure
//!
//! - `logical_mirror` - pure-logical mirror nodes attached to each sub-plan
//! - `node` - node pattern planning
//! - `edge` - edge pattern planning
//! - `path` - path pattern planning (alternatives, optional, repeated)

use std::collections::HashSet;
use std::sync::Arc;

use crate::metadata::MetadataContext;
use crate::parser::ast::pattern::{PathElement, Pattern, VariablePattern};
use crate::parser::ast::stmt::{MatchDeleteClause, MatchStmt};
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::data_modification::delete_nodes::{
    PipeDeleteEdgesNode, PipeDeleteVerticesNode,
};
use crate::planning::plan::core::nodes::data_modification::info::{
    EdgeDeleteInfo, VertexDeleteInfo,
};
use crate::planning::plan::core::nodes::ArgumentNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::QueryContext;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::ContextualExpression;

mod edge;
mod logical_mirror;
mod node;
mod path;

pub use edge::{plan_pattern_edge, plan_pattern_edge_with_input};
pub use node::{plan_node_pattern, plan_pattern_node};
pub use path::{
    plan_alternative_patterns, plan_optional_element, plan_path_pattern, plan_repeated_element,
};

use logical_mirror::logical_argument;

pub struct PlanningContext<'a> {
    pub space_id: u64,
    pub space_name: &'a str,
    /// Variables known to be bound by the enclosing scope.
    ///
    /// Derived from the bound query graph by statement planners; subquery
    /// planners that re-plan from AST patterns pass an empty set, which
    /// preserves the previous behavior of rejecting bare variable patterns
    /// there. Planners must not synthesize a validation wrapper anymore: the
    /// bound statement is the only planning input.
    pub known_variables: &'a HashSet<String>,
    pub qctx: &'a Arc<QueryContext>,
    pub enable_index_optimization: bool,
    pub metadata_context: &'a Option<MetadataContext>,
    pub expr_context: &'a Option<Arc<ExpressionAnalysisContext>>,
    pub where_expression: Option<&'a ContextualExpression>,
    pub optional: bool,
}

pub fn plan_match_delete(
    input_plan: SubPlan,
    delete_clause: &MatchDeleteClause,
    space_name: &str,
    match_stmt: &MatchStmt,
) -> Result<SubPlan, PlannerError> {
    let input_node = input_plan.root().as_ref().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;

    let delete_node = match &delete_clause.target {
        crate::parser::ast::stmt::MatchDeleteTarget::Vertices(vertex_exprs) => {
            let info = VertexDeleteInfo {
                space_name: space_name.to_string(),
                tag: None,
                vertex_ids: vertex_exprs.clone(),
                with_edge: delete_clause.with_edge,
                cascade: delete_clause.with_edge,
            };
            PipeDeleteVerticesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::parser::ast::stmt::MatchDeleteTarget::Edges(edge_exprs) => {
            let edges: Vec<_> = edge_exprs
                .iter()
                .map(|e| (e.clone(), e.clone(), None))
                .collect();
            let edge_type = extract_edge_type_from_patterns(&match_stmt.patterns);

            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type,
            };
            PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::parser::ast::stmt::MatchDeleteTarget::EdgeRefs(edge_refs) => {
            let edges = edge_refs.clone();
            let edge_type = extract_edge_type_from_patterns(&match_stmt.patterns);

            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type,
            };
            PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
    };

    Ok(SubPlan::new(Some(delete_node), input_plan.tail))
}

/// Bound-statement counterpart of [`plan_match_delete`].
///
/// Plans the MATCH DELETE clause from the binder output instead of the AST,
/// so the bound validation and the executed plan cannot drift apart.
pub fn plan_match_delete_bound(
    input_plan: SubPlan,
    delete_clause: &crate::binder::bound::BoundMatchDeleteClause,
    space_name: &str,
    edge_type: Option<String>,
) -> Result<SubPlan, PlannerError> {
    let input_node = input_plan.root().as_ref().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;

    let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
    let convert = |bound_expr: &crate::binder::bound::BoundExpression| {
        crate::binder::expr_converter::bound_expr_to_contextual(bound_expr, &expr_ctx)
            .map_err(PlannerError::PlanGenerationFailed)
    };

    let delete_node = match &delete_clause.target {
        crate::binder::bound::BoundMatchDeleteTarget::Vertices(exprs) => {
            let vertex_ids = exprs
                .iter()
                .map(convert)
                .collect::<Result<Vec<ContextualExpression>, _>>()?;
            let info = VertexDeleteInfo {
                space_name: space_name.to_string(),
                tag: None,
                vertex_ids,
                with_edge: delete_clause.with_edge,
                cascade: delete_clause.with_edge,
            };
            PipeDeleteVerticesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::binder::bound::BoundMatchDeleteTarget::Edges(exprs) => {
            let edges: Vec<ContextualExpression> =
                exprs.iter().map(convert).collect::<Result<Vec<_>, _>>()?;
            let edges = edges.iter().map(|e| (e.clone(), e.clone(), None)).collect();
            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type,
            };
            PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::binder::bound::BoundMatchDeleteTarget::EdgeRefs(refs) => {
            let mut edges = Vec::with_capacity(refs.len());
            for (src, dst, rank) in refs {
                edges.push((
                    convert(src)?,
                    convert(dst)?,
                    rank.as_ref().map(convert).transpose()?,
                ));
            }
            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type,
            };
            PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
    };

    Ok(SubPlan::new(Some(delete_node), input_plan.tail))
}

pub fn plan_pattern(pattern: &Pattern, ctx: &PlanningContext) -> Result<SubPlan, PlannerError> {
    match pattern {
        Pattern::Node(node) => plan_pattern_node(
            node,
            ctx.space_id,
            ctx.space_name,
            ctx.enable_index_optimization,
            ctx.metadata_context,
            ctx.expr_context,
            ctx.where_expression,
        ),
        Pattern::Edge(edge) => plan_pattern_edge(
            edge,
            ctx.space_id,
            ctx.space_name,
            ctx.expr_context,
            ctx.optional,
        ),
        Pattern::Path(_) => plan_path_pattern(pattern, ctx),
        Pattern::Variable(var) => plan_variable_pattern(var, ctx.known_variables),
    }
}

pub fn plan_variable_pattern(
    var: &VariablePattern,
    known_variables: &HashSet<String>,
) -> Result<SubPlan, PlannerError> {
    if !known_variables.contains(&var.name) {
        return Err(PlannerError::PlanGenerationFailed(format!(
            "Variable '{}' undefined",
            var.name
        )));
    }

    let argument_node = ArgumentNode::new(0, &var.name);
    let arg_root = argument_node.into_enum();
    let logical_root = logical_argument(&var.name);
    let sub_plan = SubPlan::from_dual(Some(arg_root.clone()), Some(arg_root), Some(logical_root));
    Ok(sub_plan)
}

pub(crate) fn extract_edge_type_from_patterns(patterns: &[Pattern]) -> Option<String> {
    for pattern in patterns {
        if let Pattern::Path(path_pattern) = pattern {
            for element in &path_pattern.elements {
                if let PathElement::Edge(edge_pattern) = element {
                    if let Some(edge_type) = edge_pattern.edge_types.first() {
                        return Some(edge_type.clone());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
