//! SUBGRAPH Query Planner
//! Planning for handling Nebula SUBGRAPH queries
//!
//! ## Explanation of the improvements
//! Supports zero-step expansion (0 STEPS).
//! Support for the range of M to N steps.
//! Optimize the starting point search strategy

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Steps;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::{
    ArgumentNode as Argument, ExpandAllNode, FilterNode, GetVerticesNode, PlanNodeEnum,
    ProjectNode as Project,
};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
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

        let var_name = "subgraph_args";
        let arg_node = Argument::new(1, var_name);
        let mut current_node: PlanNodeEnum = PlanNodeEnum::Argument(arg_node.clone());

        if m_steps == 0 {
            log::debug!("SUBGRAPH with 0 steps - returning only start vertices");

            let get_vertices_node = GetVerticesNode::new(1, "default", var_name);
            current_node = PlanNodeEnum::GetVertices(get_vertices_node);

            let filters: Vec<Expression> = where_clause
                .into_iter()
                .map(|expr| {
                    expr.into_expression()
                        .map_err(PlannerError::PlanGenerationFailed)
                })
                .collect::<Result<Vec<_>, _>>()?;
            current_node = self.apply_filters(current_node, &filters, validated.expr_context())?;

            let project_node = match Project::new(current_node.clone(), vec![]) {
                Ok(node) => PlanNodeEnum::Project(node),
                Err(_) => current_node,
            };
            current_node = project_node;

            let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Argument(arg_node)));
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
        current_node = self.apply_filters(current_node, &filters, validated.expr_context())?;

        let project_node = match Project::new(current_node.clone(), vec![]) {
            Ok(node) => PlanNodeEnum::Project(node),
            Err(_) => current_node,
        };
        current_node = project_node;

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Argument(arg_node)));

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

        let var_name = "subgraph_args";
        let arg_node = Argument::new(1, var_name);
        let mut current_node: PlanNodeEnum = PlanNodeEnum::Argument(arg_node.clone());

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
            current_node = PlanNodeEnum::GetVertices(get_vertices_node);

            let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
            if let Some(ref where_clause) = subgraph.where_clause {
                let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                    &where_clause.condition,
                    &expr_ctx,
                )
                .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
                current_node = match FilterNode::new(current_node.clone(), condition) {
                    Ok(node) => PlanNodeEnum::Filter(node),
                    Err(_) => current_node,
                };
            }

            let project_node = match Project::new(current_node.clone(), vec![]) {
                Ok(node) => PlanNodeEnum::Project(node),
                Err(_) => current_node,
            };
            current_node = project_node;

            let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Argument(arg_node)));
            return Ok(sub_plan);
        }

        if m_steps > 0 {
            current_node = self.create_expand_node(
                current_node,
                &edge_types,
                &direction_str,
                m_steps as u32,
                n_steps as u32,
            )?;

            if n_steps > m_steps {
                for step in (m_steps + 1)..=n_steps {
                    log::debug!("Adding expansion step {}", step);
                    current_node = self.create_expand_node(
                        current_node,
                        &edge_types,
                        &direction_str,
                        step as u32,
                        n_steps as u32,
                    )?;
                }
            }
        }

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        if let Some(ref where_clause) = subgraph.where_clause {
            let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                &where_clause.condition,
                &expr_ctx,
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
            current_node = match FilterNode::new(current_node.clone(), condition) {
                Ok(node) => PlanNodeEnum::Filter(node),
                Err(_) => current_node,
            };
        }

        let project_node = match Project::new(current_node.clone(), vec![]) {
            Ok(node) => PlanNodeEnum::Project(node),
            Err(_) => current_node,
        };
        current_node = project_node;

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Argument(arg_node)));
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

    /// Apply all filters
    fn apply_filters(
        &self,
        input: PlanNodeEnum,
        filters: &[Expression],
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let mut current = input;

        for condition in filters {
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(condition.clone());
            let id = expr_context.register_expression(expr_meta);
            let ctx_expr = graphdb_core::types::ContextualExpression::new(id, expr_context.clone());
            current = match FilterNode::new(current.clone(), ctx_expr) {
                Ok(node) => PlanNodeEnum::Filter(node),
                Err(_) => current,
            };
        }

        Ok(current)
    }
}

impl Default for SubgraphPlanner {
    fn default() -> Self {
        Self::new()
    }
}
