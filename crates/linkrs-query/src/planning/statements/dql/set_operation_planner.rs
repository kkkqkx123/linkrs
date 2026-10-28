//! Set Operation Planner
//!
//! Query planning for set operation statements such as UNION, UNION ALL, INTERSECT, and MINUS.
//!
//! ## Recursive Planning
//!
//! This planner supports recursive planning of nested set operations.
//! Each side of a set operation can be another set operation or a regular query.

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{IntersectNode, MinusNode, UnionNode};
use crate::planning::plan::logical::logical_nodes::graph_ops::{
    LogicalIntersectNode, LogicalMinusNode, LogicalUnionNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerEnum, PlannerError};

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

    /// Build the sub-context for one side of a set operation.
    fn side_context<'b>(
        ctx: &'b crate::planning::context::PlanContext<'b>,
        side: &'b BoundStatement,
        query_text: &str,
    ) -> crate::planning::context::PlanContext<'b> {
        let mut rctx = (*ctx.qctx.request_context_arc()).clone();
        rctx.query = query_text.to_string();
        let mut qctx = crate::QueryContext::new(std::sync::Arc::new(rctx));
        if let Some(space) = ctx.qctx.space_info() {
            qctx.set_space_info(space.clone());
        }
        // ast stays None on purpose: this side carries its own query text
        // (`rctx.query` above), while `ctx.ast` parses the whole set
        // operation, so it must not leak into the side planner.
        crate::planning::context::PlanContext {
            bound: side,
            qctx: std::sync::Arc::new(qctx),
            metadata: ctx.metadata,
            ast: None,
            join_order_stats: ctx.join_order_stats.clone(),
        }
    }

    fn side_query_texts(source: &str) -> (String, String) {
        if let Some((left, right)) = Self::split_set_operation(source) {
            return (left.trim().to_string(), right.trim().to_string());
        }
        (source.to_string(), source.to_string())
    }

    fn split_set_operation(source: &str) -> Option<(String, String)> {
        // Find the top-level set-operation keyword, respecting parentheses
        // and string literals so keywords inside patterns or strings do not
        // split the query.
        let chars: Vec<char> = source.chars().collect();
        let mut depth = 0usize;
        let mut in_string = false;
        let mut string_delim = '\'';
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if in_string {
                if c == string_delim {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            match c {
                '\'' | '"' | '`' => {
                    in_string = true;
                    string_delim = c;
                    i += 1;
                }
                '(' => {
                    depth += 1;
                    i += 1;
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
                _ => {
                    if depth == 0 {
                        for keyword in ["UNION ALL", "UNION", "INTERSECT", "MINUS"] {
                            let kw: Vec<char> = keyword.chars().collect();
                            if i + kw.len() <= chars.len()
                                && chars[i..i + kw.len()]
                                    .iter()
                                    .collect::<String>()
                                    .eq_ignore_ascii_case(keyword)
                            {
                                // Ensure keyword boundaries.
                                let before_ok = i == 0
                                    || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
                                let after_idx = i + kw.len();
                                let after_ok = after_idx == chars.len()
                                    || !(chars[after_idx].is_alphanumeric()
                                        || chars[after_idx] == '_');
                                if before_ok && after_ok {
                                    let left = chars[..i].iter().collect::<String>();
                                    let right = chars[after_idx..].iter().collect::<String>();
                                    return Some((left, right));
                                }
                            }
                        }
                    }
                    i += 1;
                }
            }
        }
        None
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

    /// Join two planned sides under one set operator, mirroring the physical
    /// join on the native logical trees when both sides carry one.
    fn combine(
        &self,
        left_plan: SubPlan,
        right_plan: SubPlan,
        operation: crate::binder::bound::SetOperationKind,
    ) -> Result<SubPlan, PlannerError> {
        self.validate_column_compatibility(&left_plan, &right_plan)?;

        let left_root = left_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_root = right_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root node".to_string())
        })?;

        let final_node = match operation {
            crate::binder::bound::SetOperationKind::Union => {
                let union_node = UnionNode::new(left_root, right_root, true).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create UnionNode: {}", e))
                })?;
                PlanNodeEnum::Union(union_node)
            }
            crate::binder::bound::SetOperationKind::UnionAll => {
                let union_node = UnionNode::new(left_root, right_root, false).map_err(|e| {
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

        let logical_root = match (
            left_plan.logical_root().cloned(),
            right_plan.logical_root().cloned(),
        ) {
            (Some(left), Some(right)) => {
                let col_names = final_node.col_names().to_vec();
                let mirror = match operation {
                    crate::binder::bound::SetOperationKind::Union => {
                        LogicalNodeEnum::Union(LogicalUnionNode {
                            id: next_node_id(),
                            deps: vec![left, right],
                            distinct: true,
                            output_var: None,
                            col_names,
                            column_types: vec![],
                        })
                    }
                    crate::binder::bound::SetOperationKind::UnionAll => {
                        LogicalNodeEnum::Union(LogicalUnionNode {
                            id: next_node_id(),
                            deps: vec![left, right],
                            distinct: false,
                            output_var: None,
                            col_names,
                            column_types: vec![],
                        })
                    }
                    crate::binder::bound::SetOperationKind::Intersect => {
                        LogicalNodeEnum::Intersect(LogicalIntersectNode {
                            id: next_node_id(),
                            deps: vec![left, right],
                            output_var: None,
                            col_names,
                            column_types: vec![],
                        })
                    }
                    crate::binder::bound::SetOperationKind::Minus => {
                        LogicalNodeEnum::Minus(LogicalMinusNode {
                            id: next_node_id(),
                            deps: vec![left, right],
                            output_var: None,
                            col_names,
                            column_types: vec![],
                        })
                    }
                };
                Some(mirror)
            }
            _ => None,
        };

        let tail = left_plan.tail().clone().unwrap_or(final_node.clone());
        Ok(SubPlan::from_dual(
            Some(final_node),
            Some(tail),
            logical_root,
        ))
    }
}

impl Planner for SetOperationPlanner {
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

        let (left_text, right_text) = Self::side_query_texts(ctx.qctx.query());
        let left_ctx = Self::side_context(ctx, &set_op.left, &left_text);
        let right_ctx = Self::side_context(ctx, &set_op.right, &right_text);
        let left_plan = self.plan_bound_subquery(&left_ctx, 1)?;
        let right_plan = self.plan_bound_subquery(&right_ctx, 1)?;

        self.combine(left_plan, right_plan, set_op.operation)
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
            _ => PlannerEnum::plan_substatement(ctx, ctx.bound),
        }
    }

    fn plan_bound_set_op(
        &mut self,
        set_op: &crate::binder::bound::BoundSetOperationStatement,
        ctx: &crate::planning::context::PlanContext<'_>,
        depth: usize,
    ) -> Result<SubPlan, PlannerError> {
        let (left_text, right_text) = Self::side_query_texts(ctx.qctx.query());
        let left_ctx = Self::side_context(ctx, &set_op.left, &left_text);
        let right_ctx = Self::side_context(ctx, &set_op.right, &right_text);
        let left_plan = self.plan_bound_subquery(&left_ctx, depth)?;
        let right_plan = self.plan_bound_subquery(&right_ctx, depth)?;

        self.combine(left_plan, right_plan, set_op.operation)
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
