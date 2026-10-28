//! UNWIND Statement Planner
//!
//! Query planning for standalone UNWIND statements.
//! UNWIND expands a list expression into multiple rows.

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};

/// UNWIND statement planner
/// Responsible for converting the UNWIND statement into an execution plan.
#[derive(Debug, Clone)]
pub struct UnwindPlanner;

impl UnwindPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for UnwindPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
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

        if let Some(expr_meta) = list_expr.expression() {
            let space_id = ctx.qctx.space_id().unwrap_or(1);
            let space_name = ctx
                .qctx
                .space_name()
                .unwrap_or_else(|| "default".to_string());
            crate::planning::statements::clauses::exists_planner::check_expression_subqueries(
                expr_meta.inner(),
                &ctx.qctx,
                space_id,
                &space_name,
                &[],
            )?;
        }

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
        Ok(SubPlan::from_logical_root(logical_root))
    }
}

impl Default for UnwindPlanner {
    fn default() -> Self {
        Self::new()
    }
}
