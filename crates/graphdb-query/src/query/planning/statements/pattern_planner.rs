use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::binder::validation::ValidationInfo;
use crate::query::metadata::MetadataContext;
use crate::query::parser::ast::pattern::{
    EdgePattern, NodePattern, PathElement, Pattern, RepetitionType, VariablePattern,
};
use crate::query::parser::ast::stmt::{MatchDeleteClause, MatchStmt};
use crate::query::planning::plan::core::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::data_modification::delete_nodes::{
    PipeDeleteEdgesNode, PipeDeleteVerticesNode,
};
use crate::query::planning::plan::core::nodes::data_modification::info::{
    EdgeDeleteInfo, VertexDeleteInfo,
};
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::core::nodes::{
    ArgumentNode, ExpandAllNode, LoopNode, ScanVerticesNode,
};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::QueryContext;

pub struct PlanningContext<'a> {
    pub space_id: u64,
    pub space_name: &'a str,
    pub validation_info: &'a ValidationInfo,
    pub qctx: &'a Arc<QueryContext>,
    pub enable_index_optimization: bool,
    pub metadata_context: &'a Option<MetadataContext>,
    pub expr_context: &'a Option<Arc<ExpressionAnalysisContext>>,
}

use super::expression_helpers;
use super::index_scan_planner;
use super::plan_combiner;

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
                            )?;
                            plan = if let Some(existing_root) = plan.root.take() {
                                plan_combiner::cross_join_plans(
                                    SubPlan::new(Some(existing_root), plan.tail),
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

                        let dst_var = if i + 1 < elements.len() {
                            if let PathElement::Node(next_node) = elements[i + 1] {
                                next_node.variable.as_deref()
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let edge_plan = plan_pattern_edge_with_input(
                            edge,
                            ctx.space_id,
                            input_alias,
                            dst_var,
                            ctx.expr_context,
                        )?;

                        plan = if let Some(existing_root) = plan.root.take() {
                            if is_first_edge {
                                plan_combiner::connect_node_to_edge_expansion(
                                    SubPlan::new(Some(existing_root), plan.tail),
                                    edge_plan,
                                    input_alias,
                                )?
                            } else {
                                plan_combiner::join_edge_expansions(
                                    SubPlan::new(Some(existing_root), plan.tail),
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
                                SubPlan::new(Some(existing_root), plan.tail),
                                alt_plan,
                            )?
                        } else {
                            alt_plan
                        };
                    }
                    PathElement::Optional(elem) => {
                        let opt_plan = plan_optional_element(elem, ctx)?;
                        plan = if let Some(existing_root) = plan.root.take() {
                            plan_combiner::left_join_plans(
                                SubPlan::new(Some(existing_root), plan.tail),
                                opt_plan,
                            )?
                        } else {
                            opt_plan
                        };
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
                                SubPlan::new(Some(existing_root), plan.tail),
                                rep_plan,
                            )?
                        } else {
                            rep_plan
                        };
                    }
                }
            }

            Ok(plan)
        }
        _ => plan_pattern(pattern, ctx),
    }
}

pub fn plan_pattern_node(
    node: &NodePattern,
    space_id: u64,
    space_name: &str,
    enable_index_optimization: bool,
    metadata_context: &Option<MetadataContext>,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
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
        )? {
            return Ok(index_plan);
        }
    }

    let mut scan_node = ScanVerticesNode::new(space_id, space_name);
    scan_node.set_col_names(vec![var_name.clone()]);
    scan_node.set_output_var(var_name.clone());
    let mut plan = SubPlan::from_root(scan_node.into_enum());

    if !node.labels.is_empty() {
        let expr_ctx = expr_context.as_ref().expect("expr_context should be set");
        let label_filter = expression_helpers::build_label_filter_expression(
            &node.variable,
            &node.labels,
            expr_ctx,
        );
        let root_node = plan.root.as_ref().expect("The root of plan should exist");
        let filter_node = FilterNode::new(root_node.clone(), label_filter)
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
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
            filter_expr,
        )
        .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
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
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }
    }

    Ok(plan)
}

pub fn plan_pattern_edge(
    edge: &EdgePattern,
    space_id: u64,
    _space_name: &str,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
    let direction = match edge.direction {
        crate::query::parser::ast::types::EdgeDirection::Out => "out",
        crate::query::parser::ast::types::EdgeDirection::In => "in",
        crate::query::parser::ast::types::EdgeDirection::Both => "both",
    };

    let edge_types = match &edge.edge_types {
        types if !types.is_empty() => types.clone(),
        _ => vec![],
    };

    let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

    if edge.edge_types.is_empty() {
        expand_node.set_any_edge_type(true);
    }

    expand_node.set_step_limit(1);

    let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
    expand_node.set_col_names(vec![edge_var.clone()]);

    let mut plan = SubPlan::from_root(expand_node.into_enum());

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
            filter_expr,
        )
        .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
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
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }
    }

    Ok(plan)
}

pub fn plan_pattern_edge_with_input(
    edge: &EdgePattern,
    space_id: u64,
    input_var: &str,
    dst_var: Option<&str>,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
    let direction = match edge.direction {
        crate::query::parser::ast::types::EdgeDirection::Out => "out",
        crate::query::parser::ast::types::EdgeDirection::In => "in",
        crate::query::parser::ast::types::EdgeDirection::Both => "both",
    };

    let edge_types = match &edge.edge_types {
        types if !types.is_empty() => types.clone(),
        _ => vec![],
    };

    let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

    if edge.edge_types.is_empty() {
        expand_node.set_any_edge_type(true);
    }

    expand_node.set_step_limit(1);

    expand_node.set_input_var(input_var.to_string());

    let src_col_name = input_var.to_string();
    let edge_col_name = edge.variable.clone().unwrap_or_else(|| "edge".to_string());
    let dst_col_name = dst_var.unwrap_or("dst").to_string();
    expand_node.set_col_names(vec![src_col_name, edge_col_name, dst_col_name]);

    expand_node.set_include_empty_paths(false);

    let mut plan = SubPlan::from_root(expand_node.into_enum());

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
            filter_expr,
        )
        .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
        plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
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
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }
    }

    Ok(plan)
}

pub fn plan_node_pattern(space_id: u64, space_name: &str) -> Result<SubPlan, PlannerError> {
    let scan_node = ScanVerticesNode::new(space_id, space_name);
    Ok(SubPlan::from_root(scan_node.into_enum()))
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
        crate::query::parser::ast::stmt::MatchDeleteTarget::Vertices(vertex_exprs) => {
            let info = VertexDeleteInfo {
                space_name: space_name.to_string(),
                vertex_ids: vertex_exprs.clone(),
                with_edge: delete_clause.with_edge,
                condition: None,
            };
            PipeDeleteVerticesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::query::parser::ast::stmt::MatchDeleteTarget::Edges(edge_exprs) => {
            let edges: Vec<_> = edge_exprs
                .iter()
                .map(|e| (e.clone(), e.clone(), None))
                .collect();

            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type: None,
                condition: None,
            };
            PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
        }
        crate::query::parser::ast::stmt::MatchDeleteTarget::EdgeRefs(edge_refs) => {
            let edges = edge_refs.clone();
            let edge_type = extract_edge_type_from_patterns(&match_stmt.patterns);

            let info = EdgeDeleteInfo {
                space_name: space_name.to_string(),
                edges,
                edge_type,
                condition: None,
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
        ),
        Pattern::Edge(edge) => {
            plan_pattern_edge(edge, ctx.space_id, ctx.space_name, ctx.expr_context)
        }
        Pattern::Path(_) => plan_path_pattern(pattern, ctx),
        Pattern::Variable(var) => plan_variable_pattern(var, ctx.space_id, ctx.validation_info),
    }
}

pub fn plan_variable_pattern(
    var: &VariablePattern,
    _space_id: u64,
    validation_info: &ValidationInfo,
) -> Result<SubPlan, PlannerError> {
    if !validation_info.alias_map.contains_key(&var.name) {
        return Err(PlannerError::PlanGenerationFailed(format!(
            "Variable '{}' undefined",
            var.name
        )));
    }

    let argument_node = ArgumentNode::new(0, &var.name);
    let sub_plan = SubPlan::from_root(argument_node.into_enum());
    Ok(sub_plan)
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
    let expr_meta = crate::core::types::expr::ExpressionMeta::new(
        crate::core::Expression::Variable(condition_str),
    );
    let id = expr_ctx.register_expression(expr_meta);
    let ctx_expr = crate::core::types::ContextualExpression::new(id, expr_ctx.clone());

    let mut loop_node = LoopNode::new(-1, ctx_expr);

    if let Some(base_root) = base_plan.root {
        loop_node.set_body(base_root);
    }

    Ok(SubPlan {
        root: Some(loop_node.into_enum()),
        tail: base_plan.tail,
    })
}

fn extract_edge_type_from_patterns(patterns: &[Pattern]) -> Option<String> {
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
