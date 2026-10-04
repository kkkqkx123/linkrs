//! Path pattern planning (alternatives, optional and repeated elements).

use std::sync::Arc;

use crate::metadata::MetadataContext;
use crate::parser::ast::pattern::{PathElement, Pattern, RepetitionType};
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::LoopNode;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::plan_combiner;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

use super::edge::{plan_pattern_edge, plan_pattern_edge_with_input};
use super::node::plan_pattern_node;
use super::{plan_pattern, PlanningContext};

pub fn plan_path_pattern(
    pattern: &Pattern,
    ctx: &PlanningContext,
) -> Result<SubPlan, PlannerError> {
    match pattern {
        Pattern::Path(path) => {
            if path.elements.is_empty() {
                return Err(PlannerError::PlanGenerationFailed(
                    "empty path model".to_string(),
                ));
            }

            let mut plan = SubPlan::new(None, None);
            let mut prev_node_alias: Option<String> = None;
            let mut is_first_node = true;
            let mut is_first_edge = true;

            let elements: Vec<_> = path.elements.iter().collect();
            let mut i = 0;

            while i < elements.len() {
                match elements[i] {
                    PathElement::Node(node) => {
                        if is_first_node {
                            let node_plan = plan_pattern_node(
                                node,
                                ctx.space_id,
                                ctx.space_name,
                                ctx.enable_index_optimization,
                                ctx.metadata_context,
                                ctx.expr_context,
                                ctx.where_expression,
                            )?;
                            plan = if let Some(existing_root) = plan.root.take() {
                                plan_combiner::cross_join_plans(
                                    SubPlan {
                                        root: Some(existing_root),
                                        tail: plan.tail,
                                        logical_root: plan.logical_root.take(),
                                    },
                                    node_plan,
                                )?
                            } else {
                                node_plan
                            };
                            let node_alias =
                                node.variable.clone().unwrap_or_else(|| "n".to_string());
                            prev_node_alias = Some(node_alias);
                            is_first_node = false;
                        } else {
                            let node_alias =
                                node.variable.clone().unwrap_or_else(|| "n".to_string());
                            prev_node_alias = Some(node_alias);
                        }
                        i += 1;
                    }
                    PathElement::Edge(edge) => {
                        if prev_node_alias.is_none() {
                            return Err(PlannerError::PlanGenerationFailed(
                                "The edge pattern must follow the node pattern".to_string(),
                            ));
                        }

                        let input_alias = prev_node_alias.as_deref().unwrap();

                        let (dst_var, dst_labels) = if i + 1 < elements.len() {
                            if let PathElement::Node(next_node) = elements[i + 1] {
                                (next_node.variable.as_deref(), next_node.labels.clone())
                            } else {
                                (None, Vec::new())
                            }
                        } else {
                            (None, Vec::new())
                        };

                        let edge_plan = plan_pattern_edge_with_input(
                            edge,
                            ctx.space_id,
                            input_alias,
                            dst_var,
                            &dst_labels,
                            ctx.expr_context,
                        )?;

                        plan = if let Some(existing_root) = plan.root.take() {
                            if is_first_edge {
                                plan_combiner::connect_node_to_edge_expansion(
                                    SubPlan {
                                        root: Some(existing_root),
                                        tail: plan.tail,
                                        logical_root: plan.logical_root.take(),
                                    },
                                    edge_plan,
                                    input_alias,
                                )?
                            } else {
                                plan_combiner::join_edge_expansions(
                                    SubPlan {
                                        root: Some(existing_root),
                                        tail: plan.tail,
                                        logical_root: plan.logical_root.take(),
                                    },
                                    edge_plan,
                                    input_alias,
                                )?
                            }
                        } else {
                            edge_plan
                        };

                        is_first_edge = false;

                        prev_node_alias = dst_var.map(|s| s.to_string());
                        i += 1;
                    }
                    PathElement::Alternative(patterns) => {
                        let alt_plan = plan_alternative_patterns(patterns, ctx)?;
                        plan = if let Some(existing_root) = plan.root.take() {
                            plan_combiner::cross_join_plans(
                                SubPlan {
                                    root: Some(existing_root),
                                    tail: plan.tail,
                                    logical_root: plan.logical_root.take(),
                                },
                                alt_plan,
                            )?
                        } else {
                            alt_plan
                        };
                        i += 1;
                    }
                    PathElement::Optional(elem) => {
                        let opt_plan = plan_optional_element(elem, ctx)?;
                        plan = if let Some(existing_root) = plan.root.take() {
                            plan_combiner::left_join_plans(
                                SubPlan {
                                    root: Some(existing_root),
                                    tail: plan.tail,
                                    logical_root: plan.logical_root.take(),
                                },
                                opt_plan,
                            )?
                        } else {
                            opt_plan
                        };
                        i += 1;
                    }
                    PathElement::Repeated(elem, rep_type) => {
                        let rep_plan = plan_repeated_element(
                            elem,
                            *rep_type,
                            ctx.space_id,
                            ctx.space_name,
                            ctx.expr_context,
                            ctx.enable_index_optimization,
                            ctx.metadata_context,
                        )?;
                        plan = if let Some(existing_root) = plan.root.take() {
                            plan_combiner::cross_join_plans(
                                SubPlan {
                                    root: Some(existing_root),
                                    tail: plan.tail,
                                    logical_root: plan.logical_root.take(),
                                },
                                rep_plan,
                            )?
                        } else {
                            rep_plan
                        };
                        i += 1;
                    }
                    PathElement::Recursive(_) => {
                        return Err(PlannerError::UnsupportedOperation(
                            "Recursive comprehension with variable binding and projection is rejected; rewrite with plain variable-length traversal".to_string(),
                        ));
                    }
                }
            }

            Ok(plan)
        }
        _ => plan_pattern(pattern, ctx),
    }
}
pub fn plan_alternative_patterns(
    patterns: &[Pattern],
    ctx: &PlanningContext,
) -> Result<SubPlan, PlannerError> {
    if patterns.is_empty() {
        return Err(PlannerError::PlanGenerationFailed(
            "The alternative path cannot be empty".to_string(),
        ));
    }

    let mut plan = plan_pattern(&patterns[0], ctx)?;

    for pattern in patterns.iter().skip(1) {
        let pattern_plan = plan_pattern(pattern, ctx)?;
        plan = plan_combiner::union_plans(plan, pattern_plan)?;
    }

    Ok(plan)
}

pub fn plan_optional_element(
    element: &PathElement,
    ctx: &PlanningContext,
) -> Result<SubPlan, PlannerError> {
    let opt_plan = match element {
        PathElement::Node(node) => plan_pattern_node(
            node,
            ctx.space_id,
            ctx.space_name,
            ctx.enable_index_optimization,
            ctx.metadata_context,
            ctx.expr_context,
            ctx.where_expression,
        )?,
        PathElement::Edge(edge) => {
            plan_pattern_edge(edge, ctx.space_id, ctx.space_name, ctx.expr_context)?
        }
        _ => {
            return Err(PlannerError::PlanGenerationFailed(
                "Optional paths do not support nested complex patterns".to_string(),
            ));
        }
    };

    Ok(opt_plan)
}

pub fn plan_repeated_element(
    element: &PathElement,
    rep_type: RepetitionType,
    space_id: u64,
    space_name: &str,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
    enable_index_optimization: bool,
    metadata_context: &Option<MetadataContext>,
) -> Result<SubPlan, PlannerError> {
    let base_plan = match element {
        PathElement::Node(node) => plan_pattern_node(
            node,
            space_id,
            space_name,
            enable_index_optimization,
            metadata_context,
            expr_context,
            None,
        )?,
        PathElement::Edge(edge) => plan_pattern_edge(edge, space_id, space_name, expr_context)?,
        _ => {
            return Err(PlannerError::PlanGenerationFailed(
                "Repeated paths do not support nested complex patterns".to_string(),
            ));
        }
    };

    let condition_str = match rep_type {
        RepetitionType::ZeroOrMore => "loop_count >= 0".to_string(),
        RepetitionType::OneOrMore => "loop_count >= 1".to_string(),
        RepetitionType::ZeroOrOne => "loop_count <= 1".to_string(),
        RepetitionType::Exactly(n) => format!("loop_count == {}", n),
        RepetitionType::Range(min, max) => {
            format!("loop_count >= {} && loop_count <= {}", min, max)
        }
    };

    let expr_ctx = expr_context.as_ref().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("Expression context is unavailable".to_string())
    })?;
    let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
        graphdb_core::Expression::Variable(condition_str),
    );
    let id = expr_ctx.register_expression(expr_meta);
    let ctx_expr = graphdb_core::types::ContextualExpression::new(id, expr_ctx.clone());

    let mut loop_node = LoopNode::new(-1, ctx_expr.clone());

    if let Some(base_root) = &base_plan.root {
        loop_node.set_body(base_root.clone());
    }

    // Logical mirror: the loop body carries the base plan's logical root.
    let logical_root = base_plan
        .logical_root()
        .cloned()
        .map(|body| LogicalNodeEnum::Loop(LogicalLoopNode::new_with_body(ctx_expr, body)));

    Ok(SubPlan {
        root: Some(loop_node.into_enum()),
        tail: base_plan.tail,
        logical_root,
    })
}
