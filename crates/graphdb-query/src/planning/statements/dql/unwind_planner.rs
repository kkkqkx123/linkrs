//! UNWIND Statement Planner
//!
//! Query planning for standalone UNWIND statements.
//! UNWIND expands a list expression into multiple rows.

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::physical_planner::convert_logical_to_physical;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
use crate::planning::plan::core::nodes::{ArgumentNode, ProjectNode};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
use crate::planning::plan::PlanNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::plan_combiner::logical_argument_root;
use crate::QueryContext;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::YieldColumn;

/// UNWIND statement planner
/// Responsible for converting the UNWIND statement into an execution plan.
#[derive(Debug, Clone)]
pub struct UnwindPlanner;

impl UnwindPlanner {
    pub fn new() -> Self {
        Self
    }

    fn extract_unwind_info(
        &self,
        stmt: &Stmt,
    ) -> Result<(ContextualExpression, String, Option<Vec<YieldColumn>>), PlannerError> {
        if let Stmt::Unwind(unwind_stmt) = stmt {
            let return_columns = if let Some(return_clause) = &unwind_stmt.return_clause {
                let mut columns = Vec::new();
                for item in &return_clause.items {
                    match item {
                        crate::parser::ast::stmt::ReturnItem::Expression { expression, alias } => {
                            let col_alias = alias
                                .clone()
                                .unwrap_or_else(|| expression.to_expression_string());
                            columns.push(YieldColumn {
                                expression: expression.clone(),
                                alias: col_alias,
                                is_matched: false,
                            });
                        }
                    }
                }
                Some(columns)
            } else {
                None
            };
            return Ok((
                unwind_stmt.expression.clone(),
                unwind_stmt.variable.clone(),
                return_columns,
            ));
        }
        Err(PlannerError::PlanGenerationFailed(
            "Expected UNWIND statement".to_string(),
        ))
    }
}

impl Planner for UnwindPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let unwind_stmt = match bound {
            BoundStatement::Unwind(stmt) => stmt,
            _ => {
                return Err(PlannerError::UnsupportedOperation(
                    "Expected Unwind statement".to_string(),
                ))
            }
        };

        let expr_ctx = ctx.validated.expr_context().clone();
        let list_expr = crate::binder::expr_converter::bound_expr_to_contextual(
            &unwind_stmt.expression,
            &expr_ctx,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!(
                "Failed to convert UNWIND expression: {}",
                e
            ))
        })?;

        let arg_node = LogicalArgumentNode {
            id: next_node_id(),
            var: "unwind_input".to_string(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        };

        let unwind_node = LogicalUnwindNode {
            id: next_node_id(),
            input: Some(Box::new(LogicalNodeEnum::Argument(arg_node))),
            deps: vec![],
            alias: unwind_stmt.alias.clone(),
            list_expression: list_expr,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        };

        let logical_root = LogicalNodeEnum::Unwind(unwind_node);
        let physical_root = convert_logical_to_physical(logical_root.clone());
        Ok(SubPlan {
            root: Some(physical_root),
            tail: None,
            logical_root: Some(logical_root),
        })
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let (expression, variable, return_columns) = self.extract_unwind_info(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in the
        // UNWIND list expression (or its RETURN projection) are rejected at
        // planning time with a precise error.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        if let Some(expr_meta) = expression.expression() {
            exists_planner::check_expression_subqueries(
                expr_meta.inner(),
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
            )?;
        }
        if let Some(columns) = &return_columns {
            for col in columns {
                if let Some(expr_meta) = col.expression.expression() {
                    exists_planner::check_expression_subqueries(
                        expr_meta.inner(),
                        &qctx,
                        space_id,
                        &space_name,
                        &outer_col_names,
                    )?;
                }
            }
        }

        let arg_node = ArgumentNode::new(next_node_id(), "unwind_input");

        let unwind_node =
            UnwindNode::new(arg_node.clone().into_enum(), &variable, expression.clone())?;

        let mut current_node: PlanNodeEnum = unwind_node.into_enum();
        let mut current_logical = logical_argument_root("unwind_input", vec![], None);
        current_logical = LogicalNodeEnum::Unwind(LogicalUnwindNode {
            id: next_node_id(),
            input: Some(Box::new(current_logical.clone())),
            deps: vec![current_logical.clone()],
            alias: variable.clone(),
            list_expression: expression,
            output_var: None,
            col_names: current_node.col_names().to_vec(),
            column_types: vec![],
        });

        if let Some(columns) = return_columns {
            let project_node =
                ProjectNode::new(current_node.clone(), columns.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create ProjectNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Project(project_node);
            current_logical = LogicalNodeEnum::Project(LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(current_logical.clone())),
                deps: vec![current_logical.clone()],
                columns,
                output_var: None,
                col_names: current_node.col_names().to_vec(),
                column_types: vec![],
            });
        }

        Ok(SubPlan {
            root: Some(current_node),
            tail: None,
            logical_root: Some(current_logical),
        })
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Unwind(_))
    }
}

impl Default for UnwindPlanner {
    fn default() -> Self {
        Self::new()
    }
}
