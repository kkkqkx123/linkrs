//! WITH Statement Planner
//!
//! Query planning for queries that handle the WITH statement

use crate::binder::BoundStatement;
use crate::planning::plan::core::{
    next_node_id,
    nodes::{FilterNode, ProjectNode, StartNode},
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::plan_combiner::{
    logical_start_root, wrap_logical_filter, wrap_logical_project_with,
};
use linkrs_core::YieldColumn;
use std::sync::Arc;

/// WITH Statement Planner
/// Responsible for converting the WITH statement into an execution plan.
#[derive(Debug, Clone)]
pub struct WithPlanner;

impl WithPlanner {
    /// Create a new WITH planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for WithPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let with_stmt = match bound {
            BoundStatement::With(w) => w,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the WITH".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let mut yield_columns: Vec<YieldColumn> = with_stmt
            .items
            .iter()
            .map(|item| {
                crate::binder::expr_converter::bound_projection_to_yield_column(item, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)
            })
            .collect::<Result<Vec<_>, PlannerError>>()?;

        // Compile expression-level EXISTS / IN in WITH assignments (mirrors
        // `transform` and return's bound path): the planned subqueries ride
        // on the Project node and execute per row. Plain expressions are
        // untouched, so existing WITH queries are unaffected.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        let mut id_alloc = exists_planner::SubqueryIdAllocator::new();
        let mut yield_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        for col in &mut yield_columns {
            let subqueries = exists_planner::plan_contextual_subqueries(
                &mut col.expression,
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
                &mut id_alloc,
            )?;
            yield_subqueries.extend(subqueries);
        }

        let start_node = StartNode::new();
        // A CTE fixpoint replaces the single empty seed row: the WITH items
        // project over the CTE output. Fixpoint plans stay physical-only
        // (no logical mirror) by design.
        let (mut current_node, seed_node, physical_only) = if with_stmt.ctes.is_empty() {
            let seed = PlanNodeEnum::Start(start_node.clone());
            (seed.clone(), seed, false)
        } else {
            if with_stmt.ctes.len() > 1 {
                return Err(PlannerError::PlanGenerationFailed(
                    "Only a single CTE per WITH statement is supported".to_string(),
                ));
            }
            let fixpoint = self.plan_bound_cte_fixpoint(&with_stmt.ctes[0], ctx)?;
            let seed = PlanNodeEnum::RecursiveCte(fixpoint);
            (seed.clone(), seed, true)
        };
        let mut current_logical: LogicalNodeEnum = logical_start_root();

        let project_node = ProjectNode::new(current_node.clone(), yield_columns.clone())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?
            .with_subqueries(yield_subqueries.clone());
        current_node = PlanNodeEnum::Project(project_node);
        current_logical = wrap_logical_project_with(
            current_logical,
            yield_columns,
            yield_subqueries,
            false,
            current_node.col_names().to_vec(),
        );

        if let Some(ref condition) = with_stmt.condition {
            let mut ctx_expr =
                crate::binder::expr_converter::bound_expr_to_contextual(condition, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)?;
            let where_subqueries = exists_planner::plan_contextual_subqueries(
                &mut ctx_expr,
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
                &mut id_alloc,
            )?;
            let filter_node = FilterNode::new(current_node.clone(), ctx_expr.clone())
                .map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?
                .with_subqueries(where_subqueries);
            current_node = PlanNodeEnum::Filter(filter_node);
            current_logical =
                wrap_logical_filter(current_logical, ctx_expr, current_node.col_names().to_vec());
        }

        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(seed_node),
            // Fixpoint plans stay physical-only: no logical mirror exists
            // for the iteration, so none is attached here.
            logical_root: if physical_only {
                None
            } else {
                Some(current_logical)
            },
        };
        Ok(sub_plan)
    }
}

impl WithPlanner {
    /// Plan one bound CTE into a [`RecursiveCteNode`].
    ///
    /// The anchor binds without the CTE name visible; the step was bound
    /// with it visible so CTE-labeled patterns scan the working table.
    /// V1 requires a single-column anchor (the working-table row is one
    /// value per iteration row).
    fn plan_bound_cte_fixpoint(
        &self,
        cte: &crate::binder::bound::BoundCteDef,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<
        crate::planning::plan::core::nodes::control_flow::control_flow_node::RecursiveCteNode,
        PlannerError,
    > {
        let anchor_plan = Self::plan_bound_substatement(&cte.anchor, ctx)?;
        let anchor_root = anchor_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(format!(
                "CTE '{}' anchor plan has no root node",
                cte.name
            ))
        })?;
        let step_root = cte
            .step
            .as_ref()
            .map(|step| {
                Self::plan_bound_substatement(step, ctx).and_then(|plan| {
                    plan.root().clone().ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(format!(
                            "CTE '{}' step plan has no root node",
                            cte.name
                        ))
                    })
                })
            })
            .transpose()?;
        Self::build_fixpoint_node(&cte.name, anchor_root, step_root)
    }

    /// Plan an arbitrary bound sub-statement (CTE anchor/step) through the
    /// same planner dispatch as top-level statements.
    fn plan_bound_substatement(
        bound_stmt: &BoundStatement,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let mut planner = crate::planning::planner::PlannerEnum::from_bound_statement(bound_stmt)
            .ok_or_else(|| {
            PlannerError::NoSuitablePlanner(format!(
                "No planner for CTE branch bound statement: {}",
                bound_stmt.kind()
            ))
        })?;
        // PlanContext::new leaves ast empty on purpose: the CTE branch is a
        // sub-statement with its own text, while ctx.ast parses the whole
        // WITH statement, so it must not leak into the branch planner.
        let sub_ctx =
            crate::planning::context::PlanContext::new(bound_stmt, ctx.qctx.clone(), ctx.metadata);
        planner.plan_bound(&sub_ctx)
    }

    /// Assemble a [`RecursiveCteNode`] from planned anchor/step roots.
    ///
    /// V1 requires a single-column anchor (the working-table row carries one
    /// value per iteration row) and column-identical step output.
    fn build_fixpoint_node(
        cte_name: &str,
        anchor_root: PlanNodeEnum,
        step_root: Option<PlanNodeEnum>,
    ) -> Result<
        crate::planning::plan::core::nodes::control_flow::control_flow_node::RecursiveCteNode,
        PlannerError,
    > {
        use crate::planning::plan::core::nodes::control_flow::control_flow_node::RecursiveCteNode;

        let anchor_cols = anchor_root.col_names().to_vec();
        if anchor_cols.len() != 1 {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "CTE '{}' anchor must project exactly one column (found {}); multi-column CTEs are not supported",
                cte_name,
                anchor_cols.len()
            )));
        }
        if let Some(ref step) = step_root {
            let step_cols = step.col_names().to_vec();
            if step_cols != anchor_cols {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "CTE '{}' step output columns {:?} must match anchor columns {:?}",
                    cte_name, step_cols, anchor_cols
                )));
            }
        }

        let mut node = RecursiveCteNode::new(
            next_node_id(),
            cte_name.to_string(),
            anchor_root,
            step_root,
            crate::cte::DEFAULT_RECURSIVE_CTE_MAX_ITERATIONS,
        );
        node.set_col_names(anchor_cols);
        Ok(node)
    }
}

impl Default for WithPlanner {
    fn default() -> Self {
        Self::new()
    }
}
