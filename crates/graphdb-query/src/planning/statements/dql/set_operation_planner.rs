//! Set Operation Planner
//!
//! Query planning for set operation statements such as UNION, UNION ALL, INTERSECT, and MINUS.
//!
//! ## Recursive Planning
//!
//! This planner supports recursive planning of nested set operations.
//! Each side of a set operation can be another set operation or a regular query.

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::{IntersectNode, MinusNode, UnionNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::QueryContext;

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
        _validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        Err(PlannerError::InvalidOperation(
            "SetOperationPlanner::transform is not supported; use plan_bound with BoundStatement."
                .to_string(),
        ))
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let _ = &bound;
        let set_op = match bound {
            BoundStatement::SetOperation(s) => s,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "SetOperationPlanner requires SetOperation statement".to_string(),
                ));
            }
        };

        let left_ctx = ctx.with_bound(&set_op.left);
        let right_ctx = ctx.with_bound(&set_op.right);
        let left_plan = self.plan_bound_subquery(&left_ctx, 1)?;
        let right_plan = self.plan_bound_subquery(&right_ctx, 1)?;

        self.validate_column_compatibility(&left_plan, &right_plan)?;

        let left_root = left_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_root = right_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root node".to_string())
        })?;

        let final_node = match set_op.operation {
            crate::binder::bound::SetOperationKind::Union => {
                let union_node = UnionNode::new(left_root, right_root, true).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create UnionNode: {}", e))
                })?;
                PlanNodeEnum::Union(union_node)
            }
            crate::binder::bound::SetOperationKind::Intersect => {
                let intersect_node = IntersectNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create IntersectNode: {}",
                        e
                    ))
                })?;
                PlanNodeEnum::Intersect(intersect_node)
            }
            crate::binder::bound::SetOperationKind::Minus => {
                let minus_node = MinusNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create MinusNode: {}", e))
                })?;
                PlanNodeEnum::Minus(minus_node)
            }
        };

        let tail = left_plan.tail().clone().unwrap_or(final_node.clone());
        Ok(SubPlan::new(Some(final_node), Some(tail)))
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::SetOperation(_))
    }
}

impl SetOperationPlanner {
    fn plan_bound_subquery(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
        depth: usize,
    ) -> Result<SubPlan, PlannerError> {
        if depth > self.max_depth {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Maximum set operation nesting depth ({}) exceeded",
                self.max_depth
            )));
        }

        match ctx.bound {
            BoundStatement::SetOperation(set_op_stmt) => {
                self.plan_bound_set_op(set_op_stmt, ctx, depth + 1)
            }
            _ => {
                let Some(mut planner) = PlannerEnum::from_bound_statement(ctx.bound) else {
                    return Err(PlannerError::InvalidOperation(format!(
                        "Unsupported subquery type in set operation: {}",
                        ctx.bound.kind()
                    )));
                };
                planner.plan_bound(ctx)
            }
        }
    }

    fn plan_bound_set_op(
        &mut self,
        set_op: &crate::binder::bound::BoundSetOperationStatement,
        ctx: &crate::planning::context::PlanContext<'_>,
        depth: usize,
    ) -> Result<SubPlan, PlannerError> {
        let left_ctx = ctx.with_bound(&set_op.left);
        let right_ctx = ctx.with_bound(&set_op.right);
        let left_plan = self.plan_bound_subquery(&left_ctx, depth)?;
        let right_plan = self.plan_bound_subquery(&right_ctx, depth)?;

        self.validate_column_compatibility(&left_plan, &right_plan)?;

        let left_root = left_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_root = right_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root node".to_string())
        })?;

        let final_node = match set_op.operation {
            crate::binder::bound::SetOperationKind::Union => {
                let union_node = UnionNode::new(left_root, right_root, true).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create UnionNode: {}", e))
                })?;
                PlanNodeEnum::Union(union_node)
            }
            crate::binder::bound::SetOperationKind::Intersect => {
                let intersect_node = IntersectNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create IntersectNode: {}",
                        e
                    ))
                })?;
                PlanNodeEnum::Intersect(intersect_node)
            }
            crate::binder::bound::SetOperationKind::Minus => {
                let minus_node = MinusNode::new(left_root, right_root).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create MinusNode: {}", e))
                })?;
                PlanNodeEnum::Minus(minus_node)
            }
        };

        let tail = left_plan.tail().clone().unwrap_or(final_node.clone());
        Ok(SubPlan::new(Some(final_node), Some(tail)))
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
