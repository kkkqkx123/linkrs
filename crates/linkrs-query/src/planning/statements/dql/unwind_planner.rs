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
use crate::planning::statements::projection_util;
use crate::QueryContext;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::YieldColumn;

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
                    columns.push(projection_util::return_item_to_yield_column(item));
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
        let unwind_stmt = match bound {
            BoundStatement::Unwind(stmt) => stmt,
            _ => {
                return Err(PlannerError::UnsupportedOperation(
                    "Expected Unwind statement".to_string(),
                ))
            }
        };

        // Unified entry for expression-level EXISTS / IN (mirrors
        // `transform`): the bound path must also reject subqueries in the
        // UNWIND list expression at planning time with the precise error.
        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
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


}

impl Default for UnwindPlanner {
    fn default() -> Self {
        Self::new()
    }
}
