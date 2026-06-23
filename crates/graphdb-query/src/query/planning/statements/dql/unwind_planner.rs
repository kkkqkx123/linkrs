//! UNWIND Statement Planner
//!
//! Query planning for standalone UNWIND statements.
//! UNWIND expands a list expression into multiple rows.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::YieldColumn;
use crate::query::parser::ast::stmt::Stmt;
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
use crate::query::planning::plan::core::nodes::{ArgumentNode, ProjectNode};
use crate::query::planning::plan::PlanNodeEnum;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

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
                        crate::query::parser::ast::stmt::ReturnItem::Expression {
                            expression,
                            alias,
                        } => {
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
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        let (expression, variable, return_columns) = self.extract_unwind_info(validated.stmt())?;

        let arg_node = ArgumentNode::new(next_node_id(), "unwind_input");

        let unwind_node = UnwindNode::new(arg_node.clone().into_enum(), &variable, expression)?;

        let mut current_node: PlanNodeEnum = unwind_node.into_enum();

        if let Some(columns) = return_columns {
            let project_node = ProjectNode::new(current_node.clone(), columns).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Project(project_node);
        }

        Ok(SubPlan::new(Some(current_node), None))
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
