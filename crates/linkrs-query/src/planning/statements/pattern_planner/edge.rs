//! Edge pattern planning.

use std::sync::Arc;

use crate::parser::ast::pattern::{EdgePattern, PathSemantic};
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::ExpandAllNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::expression_helpers;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;

use super::logical_mirror::{logical_filter, neighbor_tag};

/// Map an edge range to fixed or variable expand steps, and resolve the
/// effective repeat rule for the hop. An unbounded range means "any number
/// of hops": with a path semantic the walk is bounded by that semantic's
/// repeat rule, while a bare `*` carries none and would enumerate an
/// unterminating walk on a cyclic graph, so it enumerates as a trail —
/// the weakest repeat rule that keeps the enumeration finite. Everywhere
/// else a missing or oversized range keeps the single-hop shape so
/// execution falls back to the row path instead of changing results.
///
/// Returns the effective path semantic so the logical mirror carries the
/// same policy as the physical node.
fn apply_edge_range(expand_node: &mut ExpandAllNode, edge: &EdgePattern) -> Option<PathSemantic> {
    expand_node.set_path_semantic(edge.path_semantic.clone());
    let Some(range) = edge.range.as_ref() else {
        expand_node.set_step_limit(1);
        return edge.path_semantic.clone();
    };
    match (range.min, range.max) {
        (Some(min), Some(max)) if min == max => {
            expand_node.set_step_limit(min.max(1) as u32);
        }
        (Some(min), Some(max)) if max > min => {
            let lo = min.max(1);
            let hi = max;
            if hi - lo > 64 {
                expand_node.set_step_limit(1);
                return edge.path_semantic.clone();
            }
            expand_node.set_step_limits((lo..=hi).map(|v| v as u32).collect());
        }
        (None, Some(max)) if max >= 1 => {
            if max > 64 {
                expand_node.set_step_limit(1);
                return edge.path_semantic.clone();
            }
            expand_node.set_step_limits((1..=max).map(|v| v as u32).collect());
        }
        (Some(min), None) => {
            if min <= 1 {
                expand_node.set_step_limit(1);
            } else {
                expand_node.set_step_limit(min as u32);
            }
        }
        _ => {
            // The grammar gives a bare path semantic no range slot, so an
            // unbounded range is the only shape it can arrive in: capping
            // the hops would silently discard the requested semantic.
            expand_node.set_unbounded_steps();
            if edge.path_semantic.is_none() {
                expand_node.set_path_semantic(Some(PathSemantic::Trail));
            }
        }
    }
    expand_node.path_semantic()
}

pub fn plan_pattern_edge(
    edge: &EdgePattern,
    space_id: u64,
    _space_name: &str,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
    if edge.recursive_comprehension.is_some() {
        return Err(PlannerError::UnsupportedOperation(
            "Recursive comprehension with variable binding and projection is rejected; rewrite with plain variable-length traversal".to_string(),
        ));
    }
    let direction = match edge.direction {
        crate::parser::ast::types::EdgeDirection::Out => "out",
        crate::parser::ast::types::EdgeDirection::In => "in",
        crate::parser::ast::types::EdgeDirection::Both => "both",
    };

    let edge_types = match &edge.edge_types {
        types if !types.is_empty() => types.clone(),
        _ => vec![],
    };

    let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

    if edge.edge_types.is_empty() {
        expand_node.set_any_edge_type(true);
    }

    let path_semantic = apply_edge_range(&mut expand_node, edge);
    let step_limit = expand_node.step_limit();
    let step_limits = expand_node.step_limits().cloned();

    let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
    expand_node.set_col_names(vec![edge_var.clone()]);

    let expand_root = expand_node.into_enum();
    let mut logical_root = super::logical_mirror::logical_expand_all_with_range(
        space_id,
        edge.edge_types.clone(),
        direction,
        edge.edge_types.is_empty(),
        None,
        vec![edge_var.clone()],
        path_semantic,
        None,
        step_limit,
        step_limits,
    );
    let mut plan = SubPlan {
        root: Some(expand_root.clone()),
        tail: Some(expand_root),
        logical_root: Some(logical_root.clone()),
    };

    if let Some(ref props) = edge.properties {
        let filter_expr = if let Some(ref expr_ctx) = expr_context {
            expression_helpers::convert_properties_to_filter(&edge_var, props, expr_ctx)
        } else {
            None
        };

        let filter_expr = match filter_expr {
            Some(expr) => expr,
            None => props.clone(),
        };

        let filter_node = FilterNode::new(
            plan.root
                .as_ref()
                .expect("The root of plan should exist")
                .clone(),
            filter_expr.clone(),
        )
        .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        logical_root = logical_filter(logical_root, filter_expr);
        plan = SubPlan {
            root: Some(filter_node.into_enum()),
            tail: plan.tail,
            logical_root: Some(logical_root.clone()),
        };
    }

    if !edge.predicates.is_empty() {
        for pred in &edge.predicates {
            let filter_node = FilterNode::new(
                plan.root
                    .as_ref()
                    .expect("The root of plan should exist")
                    .clone(),
                pred.clone(),
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            logical_root = logical_filter(logical_root, pred.clone());
            plan = SubPlan {
                root: Some(filter_node.into_enum()),
                tail: plan.tail,
                logical_root: Some(logical_root.clone()),
            };
        }
    }

    Ok(plan)
}

pub fn plan_pattern_edge_with_input(
    edge: &EdgePattern,
    space_id: u64,
    input_var: &str,
    dst_var: Option<&str>,
    dst_labels: &[String],
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
    if edge.recursive_comprehension.is_some() {
        return Err(PlannerError::UnsupportedOperation(
            "Recursive comprehension with variable binding and projection is rejected; rewrite with plain variable-length traversal".to_string(),
        ));
    }
    let direction = match edge.direction {
        crate::parser::ast::types::EdgeDirection::Out => "out",
        crate::parser::ast::types::EdgeDirection::In => "in",
        crate::parser::ast::types::EdgeDirection::Both => "both",
    };

    let edge_types = match &edge.edge_types {
        types if !types.is_empty() => types.clone(),
        _ => vec![],
    };

    let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

    if edge.edge_types.is_empty() {
        expand_node.set_any_edge_type(true);
    }

    let path_semantic = apply_edge_range(&mut expand_node, edge);
    let dst_tag = neighbor_tag(dst_labels)?;
    expand_node.set_dst_tag(dst_tag.clone());

    expand_node.set_input_var(input_var.to_string());

    let src_col_name = input_var.to_string();
    let edge_col_name = edge.variable.clone().unwrap_or_else(|| "edge".to_string());
    let dst_col_name = dst_var.unwrap_or("dst").to_string();
    expand_node.set_col_names(vec![
        src_col_name.clone(),
        edge_col_name.clone(),
        dst_col_name.clone(),
    ]);

    expand_node.set_include_empty_paths(false);

    let step_limit = expand_node.step_limit();
    let step_limits = expand_node.step_limits().cloned();

    let expand_root = expand_node.into_enum();
    let mut logical_root = super::logical_mirror::logical_expand_all_with_range(
        space_id,
        edge.edge_types.clone(),
        direction,
        edge.edge_types.is_empty(),
        Some(input_var.to_string()),
        vec![src_col_name, edge_col_name, dst_col_name],
        path_semantic,
        Some(dst_tag),
        step_limit,
        step_limits,
    );
    let mut plan = SubPlan {
        root: Some(expand_root.clone()),
        tail: Some(expand_root),
        logical_root: Some(logical_root.clone()),
    };

    if let Some(ref props) = edge.properties {
        let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
        let filter_expr = if let Some(ref expr_ctx) = expr_context {
            expression_helpers::convert_properties_to_filter(&edge_var, props, expr_ctx)
        } else {
            None
        };

        let filter_expr = match filter_expr {
            Some(expr) => expr,
            None => props.clone(),
        };

        let filter_node = FilterNode::new(
            plan.root
                .as_ref()
                .expect("The root of plan should exist")
                .clone(),
            filter_expr.clone(),
        )
        .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        logical_root = logical_filter(logical_root, filter_expr);
        plan = SubPlan {
            root: Some(filter_node.into_enum()),
            tail: plan.tail,
            logical_root: Some(logical_root.clone()),
        };
    }

    if !edge.predicates.is_empty() {
        for pred in &edge.predicates {
            let filter_node = FilterNode::new(
                plan.root
                    .as_ref()
                    .expect("The root of plan should exist")
                    .clone(),
                pred.clone(),
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            logical_root = logical_filter(logical_root, pred.clone());
            plan = SubPlan {
                root: Some(filter_node.into_enum()),
                tail: plan.tail,
                logical_root: Some(logical_root.clone()),
            };
        }
    }

    Ok(plan)
}
