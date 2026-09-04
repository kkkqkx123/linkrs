//! SUBGRAPH Query Planner
//! Planning for handling Nebula SUBGRAPH queries
//!
//! ## Explanation of the improvements
//! Supports zero-step expansion (0 STEPS).
//! Support for the range of M to N steps.
//! Optimize the starting point search strategy

use std::sync::Arc;

use crate::binder::{BoundExpression, BoundStatement};
use crate::parser::ast::stmt::Steps;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    ArgumentNode as Argument, ExpandAllNode, FilterNode, GetVerticesNode, PlanNodeEnum,
    ProjectNode as Project,
};
use crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalFilterNode, LogicalProjectNode,
};
use crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::plan_combiner::logical_argument_root;
use crate::QueryContext;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::EdgeDirection;
use graphdb_core::Expression;

/// SUBGRAPH Query Planner
/// Responsible for converting SUBGRAPH queries into execution plans.
#[derive(Debug, Clone)]
pub struct SubgraphPlanner;

impl SubgraphPlanner {
    /// Create a new SUBGRAPH planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for SubgraphPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        let subgraph_stmt = match validated.stmt() {
            Stmt::Subgraph(subgraph_stmt) => subgraph_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "SubgraphPlanner requires the Subgraph statement.".to_string(),
                ));
            }
        };

        log::debug!("Processing SUBGRAPH query planning");

        let steps = &subgraph_stmt.steps;
        let over = subgraph_stmt.over.as_ref();
        let where_clause = subgraph_stmt.where_clause.clone();

        let (m_steps, n_steps) = match steps {
            Steps::Fixed(n) => (*n, *n),
            Steps::Range { min, max } => (*min, *max),
            Steps::Variable(_) => {
                return Err(PlannerError::InvalidOperation(
                    "SUBGRAPH does not support variable steps".to_string(),
                ));
            }
        };

        log::debug!("SUBGRAPH steps: {} to {}", m_steps, n_steps);

        let pipe_var = subgraph_stmt
            .from
            .vertices
            .first()
            .and_then(|v| v.expression())
            .and_then(|meta| expr_is_pipe_ref(meta.inner()));
        let var_name = pipe_var.as_deref().unwrap_or("subgraph_args");
        let arg_node = Argument::new(1, var_name);
        let mut current_node: PlanNodeEnum = PlanNodeEnum::Argument(arg_node.clone());
        let mut current_logical = logical_argument_root(var_name, vec![], None);

        if m_steps == 0 {
            log::debug!("SUBGRAPH with 0 steps - returning only start vertices");

            let get_vertices_node = GetVerticesNode::new(1, "default", var_name);
            current_logical = LogicalNodeEnum::GetVertices(LogicalGetVerticesNode {
                id: next_node_id(),
                deps: vec![current_logical],
                space_id: get_vertices_node.space_id(),
                space_name: get_vertices_node.space_name().to_string(),
                src_ref: get_vertices_node.src_ref().clone(),
                src_vids: get_vertices_node.src_vids().to_string(),
                tag_props: get_vertices_node.tag_props().to_vec(),
                expression: None,
                dedup: false,
                limit: None,
                projected_properties: vec![],
                output_var: None,
                col_names: get_vertices_node.col_names().to_vec(),
                column_types: vec![],
            });
            current_node = PlanNodeEnum::GetVertices(get_vertices_node);

            let filters: Vec<Expression> = where_clause
                .into_iter()
                .map(|expr| {
                    expr.into_expression()
                        .map_err(PlannerError::PlanGenerationFailed)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (filtered, conditions) =
                self.apply_filters(current_node, &filters, validated.expr_context())?;
            current_node = filtered;
            for condition in conditions {
                current_logical = LogicalNodeEnum::Filter(LogicalFilterNode {
                    id: next_node_id(),
                    input: Some(Box::new(current_logical.clone())),
                    deps: vec![current_logical.clone()],
                    condition,
                    output_var: None,
                    col_names: current_node.col_names().to_vec(),
                    column_types: vec![],
                });
            }

            let project_node = match Project::new(current_node.clone(), vec![]) {
                Ok(node) => {
                    let project_enum = PlanNodeEnum::Project(node);
                    current_logical = LogicalNodeEnum::Project(LogicalProjectNode {
                        id: next_node_id(),
                        input: Some(Box::new(current_logical.clone())),
                        deps: vec![current_logical.clone()],
                        columns: vec![],
                        output_var: None,
                        col_names: project_enum.col_names().to_vec(),
                        column_types: vec![],
                    });
                    project_enum
                }
                Err(_) => current_node,
            };
            current_node = project_node;

            let sub_plan = SubPlan {
                root: Some(current_node),
                tail: Some(PlanNodeEnum::Argument(arg_node)),
                logical_root: Some(current_logical),
            };
            return Ok(sub_plan);
        }

        let edge_types = over.map(|o| o.edge_types.clone()).unwrap_or_default();
        let direction_str = over
            .map(|o| match o.direction {
                EdgeDirection::Out => "out",
                EdgeDirection::In => "in",
                EdgeDirection::Both => "both",
            })
            .unwrap_or("out");

        if m_steps > 0 {
            current_node = self.create_expand_node(
                current_node,
                &edge_types,
                direction_str,
                m_steps as u32,
                n_steps as u32,
            )?;
            current_logical = expand_mirror(
                current_logical,
                &edge_types,
                direction_str,
                n_steps as u32,
                current_node.col_names().to_vec(),
            );

            if n_steps > m_steps {
                for step in (m_steps + 1)..=n_steps {
                    log::debug!("Adding expansion step {}", step);
                    current_node = self.create_expand_node(
                        current_node,
                        &edge_types,
                        direction_str,
                        step as u32,
                        n_steps as u32,
                    )?;
                    current_logical = expand_mirror(
                        current_logical,
                        &edge_types,
                        direction_str,
                        n_steps as u32,
                        current_node.col_names().to_vec(),
                    );
                }
            }
        }

        let filters: Vec<Expression> = where_clause
            .into_iter()
            .map(|expr| {
                expr.into_expression()
                    .map_err(PlannerError::PlanGenerationFailed)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (filtered, conditions) =
            self.apply_filters(current_node, &filters, validated.expr_context())?;
        current_node = filtered;
        for condition in conditions {
            current_logical = LogicalNodeEnum::Filter(LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(current_logical.clone())),
                deps: vec![current_logical.clone()],
                condition,
                output_var: None,
                col_names: current_node.col_names().to_vec(),
                column_types: vec![],
            });
        }

        let project_node = match Project::new(current_node.clone(), vec![]) {
            Ok(node) => {
                let project_enum = PlanNodeEnum::Project(node);
                current_logical = LogicalNodeEnum::Project(LogicalProjectNode {
                    id: next_node_id(),
                    input: Some(Box::new(current_logical.clone())),
                    deps: vec![current_logical.clone()],
                    columns: vec![],
                    output_var: None,
                    col_names: project_enum.col_names().to_vec(),
                    column_types: vec![],
                });
                project_enum
            }
            Err(_) => current_node,
        };
        current_node = project_node;

        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Argument(arg_node)),
            logical_root: Some(current_logical),
        };

        Ok(sub_plan)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let subgraph = match bound {
            BoundStatement::Subgraph(s) => s,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "SubgraphPlanner requires the Subgraph statement.".to_string(),
                ));
            }
        };

        log::debug!("Processing SUBGRAPH query planning (plan_bound)");

        let (m_steps, n_steps) = match &subgraph.steps {
            Steps::Fixed(n) => (*n, *n),
            Steps::Range { min, max } => (*min, *max),
            Steps::Variable(_) => {
                return Err(PlannerError::InvalidOperation(
                    "SUBGRAPH does not support variable steps".to_string(),
                ));
            }
        };

        log::debug!("SUBGRAPH steps: {} to {}", m_steps, n_steps);

        let pipe_var = detect_pipe_input_from_bound(&subgraph.from);
        let var_name = pipe_var.as_deref().unwrap_or("subgraph_args");
        let arg_node = Argument::new(1, var_name);
        let mut current_node: PlanNodeEnum = PlanNodeEnum::Argument(arg_node.clone());
        let mut current_logical = logical_argument_root(var_name, vec![], None);

        let (edge_types, direction_str) = match &subgraph.over {
            Some((types, dir)) => {
                let direction_str = match dir {
                    EdgeDirection::Out => "out",
                    EdgeDirection::In => "in",
                    EdgeDirection::Both => "both",
                };
                (types.clone(), direction_str)
            }
            None => (vec![], "out"),
        };

        if m_steps == 0 {
            log::debug!("SUBGRAPH with 0 steps - returning only start vertices");

            let get_vertices_node = GetVerticesNode::new(1, "default", var_name);
            current_logical = LogicalNodeEnum::GetVertices(LogicalGetVerticesNode {
                id: next_node_id(),
                deps: vec![current_logical],
                space_id: get_vertices_node.space_id(),
                space_name: get_vertices_node.space_name().to_string(),
                src_ref: get_vertices_node.src_ref().clone(),
                src_vids: get_vertices_node.src_vids().to_string(),
                tag_props: get_vertices_node.tag_props().to_vec(),
                expression: None,
                dedup: false,
                limit: None,
                projected_properties: vec![],
                output_var: None,
                col_names: get_vertices_node.col_names().to_vec(),
                column_types: vec![],
            });
            current_node = PlanNodeEnum::GetVertices(get_vertices_node);

            let expr_ctx = ctx.validated.expr_context().clone();
            if let Some(ref where_clause) = subgraph.where_clause {
                let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                    &where_clause.condition,
                    &expr_ctx,
                )
                .map_err(PlannerError::PlanGenerationFailed)?;
                current_node = match FilterNode::new(current_node.clone(), condition.clone()) {
                    Ok(node) => PlanNodeEnum::Filter(node),
                    Err(_) => current_node,
                };
                if matches!(current_node, PlanNodeEnum::Filter(_)) {
                    current_logical = LogicalNodeEnum::Filter(LogicalFilterNode {
                        id: next_node_id(),
                        input: Some(Box::new(current_logical.clone())),
                        deps: vec![current_logical.clone()],
                        condition,
                        output_var: None,
                        col_names: current_node.col_names().to_vec(),
                        column_types: vec![],
                    });
                }
            }

            let project_node = match Project::new(current_node.clone(), vec![]) {
                Ok(node) => {
                    let project_enum = PlanNodeEnum::Project(node);
                    current_logical = LogicalNodeEnum::Project(LogicalProjectNode {
                        id: next_node_id(),
                        input: Some(Box::new(current_logical.clone())),
                        deps: vec![current_logical.clone()],
                        columns: vec![],
                        output_var: None,
                        col_names: project_enum.col_names().to_vec(),
                        column_types: vec![],
                    });
                    project_enum
                }
                Err(_) => current_node,
            };
            current_node = project_node;

            let sub_plan = SubPlan {
                root: Some(current_node),
                tail: Some(PlanNodeEnum::Argument(arg_node)),
                logical_root: Some(current_logical),
            };
            return Ok(sub_plan);
        }

        if m_steps > 0 {
            current_node = self.create_expand_node(
                current_node,
                &edge_types,
                direction_str,
                m_steps as u32,
                n_steps as u32,
            )?;
            current_logical = expand_mirror(
                current_logical,
                &edge_types,
                direction_str,
                n_steps as u32,
                current_node.col_names().to_vec(),
            );

            if n_steps > m_steps {
                for step in (m_steps + 1)..=n_steps {
                    log::debug!("Adding expansion step {}", step);
                    current_node = self.create_expand_node(
                        current_node,
                        &edge_types,
                        direction_str,
                        step as u32,
                        n_steps as u32,
                    )?;
                    current_logical = expand_mirror(
                        current_logical,
                        &edge_types,
                        direction_str,
                        n_steps as u32,
                        current_node.col_names().to_vec(),
                    );
                }
            }
        }

        let expr_ctx = ctx.validated.expr_context().clone();
        if let Some(ref where_clause) = subgraph.where_clause {
            let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                &where_clause.condition,
                &expr_ctx,
            )
            .map_err(PlannerError::PlanGenerationFailed)?;
            current_node = match FilterNode::new(current_node.clone(), condition.clone()) {
                Ok(node) => PlanNodeEnum::Filter(node),
                Err(_) => current_node,
            };
            if matches!(current_node, PlanNodeEnum::Filter(_)) {
                current_logical = LogicalNodeEnum::Filter(LogicalFilterNode {
                    id: next_node_id(),
                    input: Some(Box::new(current_logical.clone())),
                    deps: vec![current_logical.clone()],
                    condition,
                    output_var: None,
                    col_names: current_node.col_names().to_vec(),
                    column_types: vec![],
                });
            }
        }

        let project_node = match Project::new(current_node.clone(), vec![]) {
            Ok(node) => {
                let project_enum = PlanNodeEnum::Project(node);
                current_logical = LogicalNodeEnum::Project(LogicalProjectNode {
                    id: next_node_id(),
                    input: Some(Box::new(current_logical.clone())),
                    deps: vec![current_logical.clone()],
                    columns: vec![],
                    output_var: None,
                    col_names: project_enum.col_names().to_vec(),
                    column_types: vec![],
                });
                project_enum
            }
            Err(_) => current_node,
        };
        current_node = project_node;

        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Argument(arg_node)),
            logical_root: Some(current_logical),
        };
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Subgraph(_))
    }
}

impl SubgraphPlanner {
    /// Create an extended node.
    fn create_expand_node(
        &self,
        input: PlanNodeEnum,
        edge_types: &[String],
        direction: &str,
        _current_step: u32,
        max_step: u32,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let mut expand_node = ExpandAllNode::new(1, edge_types.to_vec(), direction);
        expand_node.set_step_limit(max_step);

        // Structurally close the plan: the input becomes the expand node's
        // input inside the SubPlan itself.
        let connected = SubPlan::connect_upstream(
            SubPlan::from_single_node(PlanNodeEnum::ExpandAll(expand_node)),
            SubPlan::from_single_node(input),
        )?;
        connected.root.ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "SUBGRAPH expand sub-plan has no root node".to_string(),
            )
        })
    }

    /// Apply all filters, returning the chained physical node together with
    /// the registered filter conditions for the logical mirror.
    fn apply_filters(
        &self,
        input: PlanNodeEnum,
        filters: &[Expression],
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<(PlanNodeEnum, Vec<graphdb_core::types::ContextualExpression>), PlannerError> {
        let mut current = input;
        let mut conditions = Vec::new();

        for condition in filters {
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(condition.clone());
            let id = expr_context.register_expression(expr_meta);
            let ctx_expr = graphdb_core::types::ContextualExpression::new(id, expr_context.clone());
            current = match FilterNode::new(current.clone(), ctx_expr.clone()) {
                Ok(node) => {
                    conditions.push(ctx_expr);
                    PlanNodeEnum::Filter(node)
                }
                Err(_) => current,
            };
        }

        Ok((current, conditions))
    }
}

/// Mirror a physical expand step over the standalone logical tree.
fn expand_mirror(
    input: LogicalNodeEnum,
    edge_types: &[String],
    direction: &str,
    max_step: u32,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::ExpandAll(LogicalExpandAllNode {
        id: next_node_id(),
        deps: vec![input],
        space_id: 1,
        edge_types: edge_types.to_vec(),
        direction: direction.to_string(),
        any_edge_type: false,
        step_limit: Some(max_step),
        step_limits: None,
        join_input: false,
        sample: false,
        edge_props: vec![],
        vertex_props: vec![],
        filter: None,
        src_vids: vec![],
        include_empty_paths: true,
        input_var: None,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Detect whether the FROM clause references pipe input (`$-`).
///
/// Returns `Some(var_name)` if the first FROM expression is a pipe reference
/// (e.g. `$-` or `$-.id`), indicating the subgraph should receive vertex IDs
/// from an upstream pipe stage rather than using literal IDs.
fn detect_pipe_input_from_bound(from: &[BoundExpression]) -> Option<String> {
    from.first().and_then(|expr| match expr {
        BoundExpression::Variable(name, _) if name == "$-" => Some(name.clone()),
        BoundExpression::Property { object, .. } => match object.as_ref() {
            BoundExpression::Variable(name, _) if name == "$-" => Some(name.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// Check a single AST expression for a pipe reference (`$-`).
fn expr_is_pipe_ref(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Variable(name) if name == "$-" => Some(name.clone()),
        Expression::Property { object, .. } => match object.as_ref() {
            Expression::Variable(name) if name == "$-" => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

impl Default for SubgraphPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::DataType;

    #[test]
    fn test_detect_pipe_input_from_bound_variable() {
        let from = vec![BoundExpression::Variable(
            "$-".to_string(),
            DataType::BigInt,
        )];
        assert_eq!(detect_pipe_input_from_bound(&from), Some("$-".to_string()));
    }

    #[test]
    fn test_detect_pipe_input_from_bound_property() {
        let from = vec![BoundExpression::Property {
            object: Box::new(BoundExpression::Variable(
                "$-".to_string(),
                DataType::BigInt,
            )),
            property: "id".to_string(),
            value_type: DataType::BigInt,
        }];
        assert_eq!(detect_pipe_input_from_bound(&from), Some("$-".to_string()));
    }

    #[test]
    fn test_detect_pipe_input_from_bound_literal() {
        let from = vec![BoundExpression::Literal(
            graphdb_core::Value::Int(1),
            DataType::BigInt,
        )];
        assert_eq!(detect_pipe_input_from_bound(&from), None);
    }

    #[test]
    fn test_detect_pipe_input_from_bound_named_variable() {
        let from = vec![BoundExpression::Variable(
            "myvar".to_string(),
            DataType::BigInt,
        )];
        assert_eq!(detect_pipe_input_from_bound(&from), None);
    }

    #[test]
    fn test_detect_pipe_input_from_bound_empty() {
        let from: Vec<BoundExpression> = vec![];
        assert_eq!(detect_pipe_input_from_bound(&from), None);
    }

    #[test]
    fn test_expr_is_pipe_ref_variable() {
        assert_eq!(
            expr_is_pipe_ref(&Expression::Variable("$-".to_string())),
            Some("$-".to_string())
        );
    }

    #[test]
    fn test_expr_is_pipe_ref_property() {
        assert_eq!(
            expr_is_pipe_ref(&Expression::Property {
                object: Box::new(Expression::Variable("$-".to_string())),
                property: "id".to_string(),
            }),
            Some("$-".to_string())
        );
    }

    #[test]
    fn test_expr_is_pipe_ref_non_pipe() {
        assert_eq!(
            expr_is_pipe_ref(&Expression::Variable("myvar".to_string())),
            None
        );
    }

    #[test]
    fn test_expr_is_pipe_ref_literal() {
        assert_eq!(
            expr_is_pipe_ref(&Expression::Literal(graphdb_core::Value::Int(1))),
            None
        );
    }
}
