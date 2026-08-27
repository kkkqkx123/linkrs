use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::expr::ContextualExpression;
use crate::binder::validation::ValidationInfo;
use crate::metadata::MetadataContext;
use crate::parser::ast::pattern::{
    EdgePattern, NodePattern, PathElement, Pattern, RepetitionType, VariablePattern,
};
use crate::parser::ast::stmt::{MatchDeleteClause, MatchStmt};
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::data_modification::delete_nodes::{
    PipeDeleteEdgesNode, PipeDeleteVerticesNode,
};
use crate::planning::plan::core::nodes::data_modification::info::{
    EdgeDeleteInfo, VertexDeleteInfo,
};
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::{
    ArgumentNode, ExpandAllNode, LoopNode, ScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::planning::plan::logical::logical_nodes::control_flow::{
    LogicalArgumentNode, LogicalLoopNode,
};
use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::QueryContext;

pub struct PlanningContext<'a> {
    pub space_id: u64,
    pub space_name: &'a str,
    pub validation_info: &'a ValidationInfo,
    pub qctx: &'a Arc<QueryContext>,
    pub enable_index_optimization: bool,
    pub metadata_context: &'a Option<MetadataContext>,
    pub expr_context: &'a Option<Arc<ExpressionAnalysisContext>>,
    pub where_expression: Option<&'a ContextualExpression>,
}

use super::expression_helpers;
use super::index_scan_planner;
use super::plan_combiner;

/// Logical mirror helpers. The physical plan stays the execution artifact;
/// a parallel pure-logical tree is attached to each SubPlan so the compiler
/// can build the `LogicalPlan` natively (instead of stripping it back out of
/// the physical tree).
fn logical_scan_vertices(
    space_id: u64,
    space_name: &str,
    tag: Option<&str>,
    var_name: &str,
) -> LogicalNodeEnum {
    LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: next_node_id(),
        space_id,
        space_name: space_name.to_string(),
        tag: tag.map(|s| s.to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        output_var: Some(var_name.to_string()),
        col_names: vec![var_name.to_string()],
        column_types: vec![],
    })
}

fn logical_filter(input: LogicalNodeEnum, condition: ContextualExpression) -> LogicalNodeEnum {
    LogicalNodeEnum::Filter(LogicalFilterNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        condition,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    })
}

fn logical_expand_all(
    space_id: u64,
    edge_types: Vec<String>,
    direction: &str,
    any_edge_type: bool,
    input_var: Option<String>,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::ExpandAll(LogicalExpandAllNode {
        id: next_node_id(),
        deps: vec![],
        space_id,
        edge_types,
        direction: direction.to_string(),
        any_edge_type,
        step_limit: Some(1),
        step_limits: None,
        join_input: false,
        sample: false,
        edge_props: vec![],
        vertex_props: vec![],
        filter: None,
        src_vids: vec![],
        include_empty_paths: false,
        input_var,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

fn logical_argument(var_name: &str) -> LogicalNodeEnum {
    LogicalNodeEnum::Argument(LogicalArgumentNode {
        id: next_node_id(),
        var: var_name.to_string(),
        output_var: Some(var_name.to_string()),
        col_names: vec![var_name.to_string()],
        column_types: vec![],
    })
}

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

pub fn plan_pattern_edge(
    edge: &EdgePattern,
    space_id: u64,
    _space_name: &str,
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
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

    expand_node.set_step_limit(1);

    let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
    expand_node.set_col_names(vec![edge_var.clone()]);

    let expand_root = expand_node.into_enum();
    let mut logical_root = logical_expand_all(
        space_id,
        edge.edge_types.clone(),
        direction,
        edge.edge_types.is_empty(),
        None,
        vec![edge_var.clone()],
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
    expr_context: &Option<Arc<ExpressionAnalysisContext>>,
) -> Result<SubPlan, PlannerError> {
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

    expand_node.set_step_limit(1);

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

    let expand_root = expand_node.into_enum();
    let mut logical_root = logical_expand_all(
        space_id,
        edge.edge_types.clone(),
        direction,
        edge.edge_types.is_empty(),
        Some(input_var.to_string()),
        vec![src_col_name, edge_col_name, dst_col_name],
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
                vertex_ids: vertex_exprs.clone(),
                with_edge: delete_clause.with_edge,
                condition: None,
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
                condition: None,
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
            ctx.where_expression,
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
    let arg_root = argument_node.into_enum();
    let logical_root = logical_argument(&var.name);
    let sub_plan = SubPlan {
        root: Some(arg_root.clone()),
        tail: Some(arg_root),
        logical_root: Some(logical_root),
    };
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
    let expr_meta = crate::core::types::expr::ExpressionMeta::new(
        crate::core::Expression::Variable(condition_str),
    );
    let id = expr_ctx.register_expression(expr_meta);
    let ctx_expr = crate::core::types::ContextualExpression::new(id, expr_ctx.clone());

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::graph_schema::EdgeDirection;
    use crate::core::types::Span;
    use crate::binder::validation::ValidationInfo;
    use crate::metadata::MetadataContext;
    use crate::parser::ast::pattern::PathPattern;
    use crate::QueryRequestContext;
    use std::collections::HashMap;

    #[allow(clippy::arc_with_non_send_sync)]
    fn create_test_components() -> (
        Arc<QueryContext>,
        ValidationInfo,
        Option<MetadataContext>,
        Option<Arc<ExpressionAnalysisContext>>,
    ) {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: String::new(),
            parameters: HashMap::new(),
            ..Default::default()
        });
        let qctx = Arc::new(QueryContext::new(rctx));
        let validation_info = ValidationInfo::default();
        let metadata_context: Option<MetadataContext> = None;
        let expr_context: Option<Arc<ExpressionAnalysisContext>> =
            Some(Arc::new(ExpressionAnalysisContext::new()));
        (qctx, validation_info, metadata_context, expr_context)
    }

    fn create_test_ctx<'a>(
        qctx: &'a Arc<QueryContext>,
        validation_info: &'a ValidationInfo,
        metadata_context: &'a Option<MetadataContext>,
        expr_context: &'a Option<Arc<ExpressionAnalysisContext>>,
    ) -> PlanningContext<'a> {
        PlanningContext {
            space_id: 1,
            space_name: "test",
            validation_info,
            qctx,
            enable_index_optimization: false,
            metadata_context,
            expr_context,
            where_expression: None,
        }
    }

    fn node_pattern(var: &str) -> PathElement {
        PathElement::Node(NodePattern::new(
            Some(var.to_string()),
            vec![],
            None,
            vec![],
            Span::default(),
        ))
    }

    fn edge_pattern(var: &str, direction: EdgeDirection) -> PathElement {
        PathElement::Edge(EdgePattern::new(
            Some(var.to_string()),
            vec!["KNOWS".to_string()],
            None,
            vec![],
            direction,
            None,
            Span::default(),
        ))
    }

    fn node_path(var: &str) -> Pattern {
        Pattern::Path(PathPattern::new(vec![node_pattern(var)], Span::default()))
    }

    #[test]
    fn test_plan_path_pattern_keeps_logical_root() {
        let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
        let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
        let pattern = Pattern::Path(PathPattern::new(
            vec![
                node_pattern("a"),
                edge_pattern("e", EdgeDirection::Out),
                node_pattern("b"),
            ],
            Span::default(),
        ));

        let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
        let logical_root = plan
            .logical_root()
            .expect("logical root should be attached");

        match logical_root {
            LogicalNodeEnum::ExpandAll(expand) => {
                assert_eq!(expand.input_var.as_deref(), Some("a"));
                assert_eq!(expand.deps.len(), 1);
                assert!(matches!(&expand.deps[0], LogicalNodeEnum::ScanVertices(_)));
            }
            other => panic!("unexpected logical root: {:?}", other),
        }
    }

    #[test]
    fn test_plan_path_pattern_multi_edge_logical_chain() {
        let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
        let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
        let pattern = Pattern::Path(PathPattern::new(
            vec![
                node_pattern("a"),
                edge_pattern("e1", EdgeDirection::Out),
                node_pattern("b"),
                edge_pattern("e2", EdgeDirection::Out),
                node_pattern("c"),
            ],
            Span::default(),
        ));

        let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
        let logical_root = plan
            .logical_root()
            .expect("logical root should be attached");

        match logical_root {
            LogicalNodeEnum::ExpandAll(second_expand) => {
                assert_eq!(second_expand.input_var.as_deref(), Some("b"));
                assert_eq!(second_expand.deps.len(), 1);
                match &second_expand.deps[0] {
                    LogicalNodeEnum::ExpandAll(first_expand) => {
                        assert_eq!(first_expand.input_var.as_deref(), Some("a"));
                        assert_eq!(first_expand.deps.len(), 1);
                        assert!(matches!(
                            &first_expand.deps[0],
                            LogicalNodeEnum::ScanVertices(_)
                        ));
                    }
                    other => panic!("unexpected middle logical node: {:?}", other),
                }
            }
            other => panic!("unexpected logical root: {:?}", other),
        }
    }

    #[test]
    fn test_plan_path_pattern_optional_keeps_logical_root() {
        let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
        let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
        let pattern = Pattern::Path(PathPattern::new(
            vec![
                node_pattern("a"),
                PathElement::Optional(Box::new(edge_pattern("e", EdgeDirection::Out))),
                node_pattern("b"),
            ],
            Span::default(),
        ));

        let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
        assert!(
            plan.logical_root().is_some(),
            "logical root should survive optional element combination"
        );
    }

    #[test]
    fn test_plan_path_pattern_alternative_keeps_logical_root() {
        let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
        let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
        let pattern = Pattern::Path(PathPattern::new(
            vec![
                node_pattern("a"),
                PathElement::Alternative(vec![node_path("x"), node_path("y")]),
            ],
            Span::default(),
        ));

        let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
        let logical_root = plan
            .logical_root()
            .expect("logical root should be attached");

        match logical_root {
            LogicalNodeEnum::CrossJoin(join) => {
                assert_eq!(join.deps.len(), 2);
            }
            other => panic!("unexpected logical root: {:?}", other),
        }
    }

    #[test]
    fn test_plan_path_pattern_repeated_keeps_logical_root() {
        let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
        let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
        let pattern = Pattern::Path(PathPattern::new(
            vec![
                node_pattern("a"),
                PathElement::Repeated(
                    Box::new(edge_pattern("e", EdgeDirection::Out)),
                    RepetitionType::OneOrMore,
                ),
            ],
            Span::default(),
        ));

        let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
        let logical_root = plan
            .logical_root()
            .expect("logical root should be attached");

        match logical_root {
            LogicalNodeEnum::CrossJoin(join) => {
                assert_eq!(join.deps.len(), 2);
                match &join.deps[1] {
                    LogicalNodeEnum::Loop(loop_node) => {
                        assert!(matches!(
                            loop_node.body(),
                            Some(LogicalNodeEnum::ExpandAll(_))
                        ));
                    }
                    other => panic!("unexpected loop node in join: {:?}", other),
                }
            }
            other => panic!("unexpected logical root: {:?}", other),
        }
    }
}
