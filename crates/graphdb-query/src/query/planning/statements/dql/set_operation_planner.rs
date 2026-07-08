//! Set Operation Planner
//!
//! Query planning for set operation statements such as UNION, UNION ALL, INTERSECT, and MINUS.
//!
//! ## Recursive Planning
//!
//! This planner supports recursive planning of nested set operations.
//! Each side of a set operation can be another set operation or a regular query.

use std::sync::Arc;

use crate::query::parser::ast::{SetOperationStmt, SetOperationType, Stmt};
use crate::query::planning::plan::core::nodes::{IntersectNode, MinusNode, UnionNode};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::QueryContext;

/// Set Operation Planner
/// Responsible for converting set operation statements into execution plans.
/// Supports recursive planning of nested set operations.
#[derive(Debug, Clone)]
pub struct SetOperationPlanner {
    max_depth: usize,
}

impl SetOperationPlanner {
    pub fn new() -> Self {
        Self { max_depth: 100 }
    }

    pub fn with_max_depth(max_depth: usize) -> Self {
        Self { max_depth }
    }

    fn extract_set_operation_stmt(&self, stmt: &Stmt) -> Result<SetOperationStmt, PlannerError> {
        match stmt {
            Stmt::SetOperation(set_op_stmt) => Ok(set_op_stmt.clone()),
            _ => Err(PlannerError::InvalidOperation(
                "SetOperationPlanner requires SetOperation statement".to_string(),
            )),
        }
    }

    fn plan_subquery(
        &mut self,
        stmt: &Stmt,
        qctx: Arc<QueryContext>,
        depth: usize,
    ) -> Result<SubPlan, PlannerError> {
        if depth > self.max_depth {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Maximum set operation nesting depth ({}) exceeded",
                self.max_depth
            )));
        }

        match stmt {
            Stmt::SetOperation(set_op_stmt) => {
                self.transform_recursive(set_op_stmt, qctx, depth + 1)
            }
            _ => {
                let Some(mut planner) = PlannerEnum::from_stmt(&Arc::new(stmt.clone())) else {
                    return Err(PlannerError::InvalidOperation(format!(
                        "Unsupported subquery type in set operation: {:?}",
                        stmt.kind()
                    )));
                };

                let expr_context = Arc::new(ExpressionAnalysisContext::new());
                let validation_info = crate::query::validator::ValidationInfo::new();
                let ast = Arc::new(crate::query::parser::ast::Ast::new(
                    stmt.clone(),
                    expr_context,
                ));
                let validated = ValidatedStatement::new(ast, validation_info);

                planner.transform(&validated, qctx)
            }
        }
    }

    fn transform_recursive(
        &mut self,
        set_op_stmt: &SetOperationStmt,
        qctx: Arc<QueryContext>,
        depth: usize,
    ) -> Result<SubPlan, PlannerError> {
        let left_plan = self.plan_subquery(&set_op_stmt.left, qctx.clone(), depth)?;
        let right_plan = self.plan_subquery(&set_op_stmt.right, qctx, depth)?;

        self.validate_column_compatibility(&left_plan, &right_plan)?;

        let left_root = left_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_root = right_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root node".to_string())
        })?;

        let final_node = match set_op_stmt.op_type {
            SetOperationType::Union => {
                let union_node = UnionNode::new(left_root, right_root, true).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create UnionNode: {}", e))
                })?;
                PlanNodeEnum::Union(union_node)
            }
            SetOperationType::UnionAll => {
                let union_node = UnionNode::new(left_root, right_root, false).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create UnionNode: {}", e))
                })?;
                PlanNodeEnum::Union(union_node)
            }
            SetOperationType::Intersect => {
                let intersect_node = IntersectNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create IntersectNode: {}",
                        e
                    ))
                })?;
                PlanNodeEnum::Intersect(intersect_node)
            }
            SetOperationType::Minus => {
                let minus_node = MinusNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create MinusNode: {}", e))
                })?;
                PlanNodeEnum::Minus(minus_node)
            }
        };

        let tail = left_plan.tail().clone().unwrap_or(final_node.clone());
        Ok(SubPlan::new(Some(final_node), Some(tail)))
    }

    fn validate_column_compatibility(
        &self,
        left_plan: &SubPlan,
        right_plan: &SubPlan,
    ) -> Result<(), PlannerError> {
        let left_cols = left_plan
            .root()
            .as_ref()
            .map(|n| n.col_names())
            .unwrap_or_default();
        let right_cols = right_plan
            .root()
            .as_ref()
            .map(|n| n.col_names())
            .unwrap_or_default();

        if left_cols.len() != right_cols.len() {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Column count mismatch: left has {} columns, right has {} columns",
                left_cols.len(),
                right_cols.len()
            )));
        }

        Ok(())
    }
}

impl Planner for SetOperationPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let set_op_stmt = self.extract_set_operation_stmt(validated.stmt())?;
        self.transform_recursive(&set_op_stmt, qctx, 0)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::SetOperation(_))
    }
}

impl Default for SetOperationPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_operation_planner_creation() {
        let planner = SetOperationPlanner::new();
        assert_eq!(planner.max_depth, 100);
    }

    #[test]
    fn test_set_operation_planner_with_max_depth() {
        let planner = SetOperationPlanner::with_max_depth(50);
        assert_eq!(planner.max_depth, 50);
    }
}
