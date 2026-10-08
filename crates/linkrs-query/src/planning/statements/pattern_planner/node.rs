//! Node pattern planning.

use std::sync::Arc;

use crate::metadata::MetadataContext;
use crate::parser::ast::pattern::NodePattern;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::ScanVerticesNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::{expression_helpers, index_scan_planner};
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::ContextualExpression;

use super::logical_mirror::{logical_filter, logical_scan_vertices};

pub fn plan_pattern_node(
    node: &NodePattern,
    space_id: u64,
    space_name: &str,
    enable_index_optimization: bool,
    metadata_context: &Option<MetadataContext>,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
    where_expression: Option<&ContextualExpression>,
) -> Result<SubPlan, PlannerError> {
    let var_name = node.variable.clone().unwrap_or_else(|| "n".to_string());

    if enable_index_optimization {
        if let Some(index_plan) = index_scan_planner::try_create_index_scan_plan(
            node,
            space_id,
            space_name,
            &var_name,
            enable_index_optimization,
            metadata_context.as_ref(),
            where_expression,
        )? {
            // Index scans are a physical choice; the logical mirror is a
            // tagged vertex scan (matching the physical→logical converter).
            let logical_root = logical_scan_vertices(
                space_id,
                space_name,
                node.labels.first().map(|s| s.as_str()),
                &var_name,
            );
            return Ok(SubPlan {
                root: index_plan.root,
                tail: index_plan.tail,
                logical_root: Some(logical_root),
            });
        }
    }

    // Single label execution scans the first label only; the full label
    // filter below guarantees distinct multi label patterns match nothing.
    let mut scan_node = ScanVerticesNode::new(space_id, space_name);
    scan_node.set_col_names(vec![var_name.clone()]);
    scan_node.set_output_var(var_name.clone());
    if let Some(label) = node.labels.first() {
        scan_node.set_tag(label);
    }
    let scan_root = scan_node.into_enum();
    let mut logical_root = logical_scan_vertices(
        space_id,
        space_name,
        node.labels.first().map(|s| s.as_str()),
        &var_name,
    );
    let mut plan = SubPlan {
        root: Some(scan_root.clone()),
        tail: Some(scan_root),
        logical_root: Some(logical_root.clone()),
    };

    if !node.labels.is_empty() {
        let expr_ctx = expr_context.as_ref().expect("expr_context should be set");
        let label_filter = expression_helpers::build_label_filter_expression(
            &node.variable,
            &node.labels,
            expr_ctx,
        );
        let root_node = plan.root.as_ref().expect("The root of plan should exist");
        let filter_node = FilterNode::new(root_node.clone(), label_filter.clone())
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        logical_root = logical_filter(logical_root, label_filter);
        plan = SubPlan {
            root: Some(filter_node.into_enum()),
            tail: plan.tail,
            logical_root: Some(logical_root.clone()),
        };
    }

    if let Some(ref props) = node.properties {
        let filter_expr = if let Some(ref expr_ctx) = expr_context {
            expression_helpers::convert_properties_to_filter(&var_name, props, expr_ctx)
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

    if !node.predicates.is_empty() {
        for pred in &node.predicates {
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
pub fn plan_node_pattern(space_id: u64, space_name: &str) -> Result<SubPlan, PlannerError> {
    let scan_node = ScanVerticesNode::new(space_id, space_name);
    let scan_root = scan_node.into_enum();
    let logical_root = logical_scan_vertices(space_id, space_name, None, "n");
    Ok(SubPlan {
        root: Some(scan_root.clone()),
        tail: Some(scan_root),
        logical_root: Some(logical_root),
    })
}
