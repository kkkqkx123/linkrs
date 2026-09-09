//! WITH Statement Planner
//!
//! Query planning for queries that handle the WITH statement

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::{OrderDirection, ReturnItem, Stmt, WithStmt};
use crate::planning::plan::core::{
    next_node_id,
    nodes::{DedupNode, FilterNode, LimitNode, ProjectNode, SortNode, StartNode},
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::plan_combiner::{
    logical_start_root, wrap_logical_dedup, wrap_logical_filter, wrap_logical_limit,
    wrap_logical_project, wrap_logical_sort,
};
use crate::QueryContext;
use graphdb_core::YieldColumn;
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

    /// Extract the WithStmt from the Stmt.
    fn extract_with_stmt(&self, stmt: &Stmt) -> Result<WithStmt, PlannerError> {
        match stmt {
            Stmt::With(with_stmt) => Ok(with_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the WITH".to_string(),
            )),
        }
    }

    /// Convert “ReturnItem” to “YieldColumn”.
    fn convert_return_item_to_yield_column(
        &self,
        item: &ReturnItem,
        _validated: &ValidatedStatement,
    ) -> YieldColumn {
        let (expression, alias) = match item {
            ReturnItem::Expression { expression, alias } => (expression.clone(), alias.clone()),
        };
        let alias = alias.unwrap_or_else(|| {
            expression
                .get_expression()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "_".to_string())
        });
        YieldColumn {
            expression,
            alias,
            is_matched: false,
        }
    }
}

impl Planner for WithPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Check the semantic information.
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("WITH referenced tags: {:?}", referenced_tags);
        }

        let referenced_properties = &validation_info.semantic_info.referenced_properties;
        if !referenced_properties.is_empty() {
            log::debug!("WITH referenced properties: {:?}", referenced_properties);
        }

        let with_stmt = self.extract_with_stmt(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in WITH
        // assignments and the WITH WHERE condition are compiled here and
        // attached to the Project/Filter nodes; WITH ORDER BY items are still
        // refused at planning time with a precise error.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        let mut id_alloc = exists_planner::SubqueryIdAllocator::new();
        let mut yield_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let mut yield_columns: Vec<YieldColumn> = with_stmt
            .items
            .iter()
            .map(|item| self.convert_return_item_to_yield_column(item, validated))
            .collect();
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
        let mut where_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let where_clause = with_stmt.where_clause.clone().map(|mut condition| {
            let subqueries = exists_planner::plan_contextual_subqueries(
                &mut condition,
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
                &mut id_alloc,
            )?;
            where_subqueries = subqueries;
            Ok::<_, PlannerError>(condition)
        });
        let where_clause = match where_clause {
            Some(Ok(condition)) => Some(condition),
            Some(Err(error)) => return Err(error),
            None => None,
        };
        if let Some(order_by) = &with_stmt.order_by {
            for item in &order_by.items {
                if let Some(expr_meta) = item.expression.expression() {
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

        // A single empty row seeds a standalone WITH statement. A CTE
        // fixpoint replaces the seed: the WITH items project over the CTE
        // output. Fixpoint plans stay physical-only (no logical mirror).
        let start_node = StartNode::new();
        let (mut current_node, seed_node, physical_only) = if with_stmt.ctes.is_empty() {
            let seed = PlanNodeEnum::Start(start_node.clone());
            (seed.clone(), seed, false)
        } else {
            if with_stmt.ctes.len() > 1 {
                return Err(PlannerError::PlanGenerationFailed(
                    "Only a single CTE per WITH statement is supported".to_string(),
                ));
            }
            let fixpoint =
                self.plan_stmt_cte_fixpoint(&with_stmt.ctes[0], with_stmt.recursive, &qctx)?;
            let seed = PlanNodeEnum::RecursiveCte(fixpoint);
            (seed.clone(), seed, true)
        };
        let mut current_logical: LogicalNodeEnum = logical_start_root();

        // Create a projection node.
        let project_node = ProjectNode::new(current_node.clone(), yield_columns.clone())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?
            .with_subqueries(yield_subqueries);
        current_node = PlanNodeEnum::Project(project_node);
        current_logical = wrap_logical_project(
            current_logical,
            yield_columns,
            current_node.col_names().to_vec(),
        );

        // If there is a WHERE clause, create a filtering node.
        if let Some(where_clause) = where_clause {
            let filter_node = FilterNode::new(current_node.clone(), where_clause.clone())
                .map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?
                .with_subqueries(where_subqueries);
            current_node = PlanNodeEnum::Filter(filter_node);
            current_logical = wrap_logical_filter(
                current_logical,
                where_clause,
                current_node.col_names().to_vec(),
            );
        }

        // Handle recursive CTE. The fixpoint already seeds this plan
        // (see above); a bare `WITH RECURSIVE` without any CTE is rejected
        // here since the AST path never reaches the binder.
        if with_stmt.recursive && with_stmt.ctes.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "WITH RECURSIVE requires at least one CTE (`name AS (anchor UNION ALL step)`)"
                    .to_string(),
            ));
        }

        // If deduplication is required, create a deduplication node.
        if with_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
            current_logical =
                wrap_logical_dedup(current_logical, current_node.col_names().to_vec());
        }

        // If there is an ORDER BY clause, create a sorting node.
        if let Some(order_by) = &with_stmt.order_by {
            let sort_items: Vec<crate::planning::plan::core::nodes::SortItem> = order_by
                .items
                .iter()
                .map(|item| {
                    let direction = match item.direction {
                        OrderDirection::Asc => {
                            graphdb_core::types::graph_schema::OrderDirection::Asc
                        }
                        OrderDirection::Desc => {
                            graphdb_core::types::graph_schema::OrderDirection::Desc
                        }
                    };
                    let expression = item
                        .expression
                        .expression()
                        .map(|e| e.inner().clone())
                        .unwrap_or_else(|| {
                            graphdb_core::Expression::Variable(
                                item.expression.to_expression_string(),
                            )
                        });
                    crate::planning::plan::core::nodes::SortItem::new(expression, direction)
                })
                .collect();
            let sort_node =
                SortNode::new(current_node.clone(), sort_items.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create SortNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Sort(sort_node);
            current_logical = wrap_logical_sort(
                current_logical,
                sort_items,
                current_node.col_names().to_vec(),
            );
        }

        // If there is a SKIP clause, create a restriction node.
        if let Some(skip) = with_stmt.skip {
            let limit_node =
                LimitNode::new(current_node.clone(), skip.count as i64, 0).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
            current_logical = wrap_logical_limit(
                current_logical,
                skip.count as i64,
                0,
                current_node.col_names().to_vec(),
            );
        }

        // If there is a LIMIT clause, create a limit node.
        if let Some(limit) = with_stmt.limit {
            let limit_node =
                LimitNode::new(current_node.clone(), 0, limit.count as i64).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
            current_logical = wrap_logical_limit(
                current_logical,
                0,
                limit.count as i64,
                current_node.col_names().to_vec(),
            );
        }

        // Create a SubPlan
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

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let with_stmt = match bound {
            BoundStatement::With(w) => w,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the WITH".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let yield_columns: Vec<YieldColumn> = with_stmt
            .items
            .iter()
            .map(|item| {
                let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                    &item.expression,
                    &expr_ctx,
                )
                .map_err(PlannerError::PlanGenerationFailed)?;
                let alias = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| ctx_expr.to_expression_string());
                Ok(YieldColumn {
                    expression: ctx_expr,
                    alias,
                    is_matched: false,
                })
            })
            .collect::<Result<Vec<_>, PlannerError>>()?;

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

        let project_node =
            ProjectNode::new(current_node.clone(), yield_columns.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
        current_node = PlanNodeEnum::Project(project_node);
        current_logical = wrap_logical_project(
            current_logical,
            yield_columns,
            current_node.col_names().to_vec(),
        );

        if let Some(ref condition) = with_stmt.condition {
            let ctx_expr =
                crate::binder::expr_converter::bound_expr_to_contextual(condition, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)?;
            let filter_node =
                FilterNode::new(current_node.clone(), ctx_expr.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
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

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::With(_))
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
        let sub_ctx = crate::planning::context::PlanContext::new(
            bound_stmt,
            ctx.qctx.clone(),
            ctx.metadata,
            ctx.validated,
        );
        planner.plan_bound(&sub_ctx)
    }

    /// Plan one AST-level CTE into a [`RecursiveCteNode`] (legacy
    /// AST-transform path; mirrors [`Self::plan_bound_cte_fixpoint`]).
    fn plan_stmt_cte_fixpoint(
        &self,
        cte: &crate::parser::ast::stmt::CteDef,
        recursive: bool,
        qctx: &Arc<QueryContext>,
    ) -> Result<
        crate::planning::plan::core::nodes::control_flow::control_flow_node::RecursiveCteNode,
        PlannerError,
    > {
        let (anchor_stmt, step_stmt) = if recursive {
            let (anchor, step) = Self::split_recursive_body(&cte.name, &cte.body)?;
            (anchor, Some(step))
        } else {
            (cte.body.as_ref(), None)
        };
        let anchor_plan = Self::plan_stmt_substatement(anchor_stmt, qctx)?;
        let anchor_root = anchor_plan.root().clone().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(format!(
                "CTE '{}' anchor plan has no root node",
                cte.name
            ))
        })?;
        let step_root = step_stmt
            .map(|step| Self::plan_stmt_substatement(step, qctx))
            .transpose()?
            .map(|plan| {
                plan.root().clone().ok_or_else(|| {
                    PlannerError::PlanGenerationFailed(format!(
                        "CTE '{}' step plan has no root node",
                        cte.name
                    ))
                })
            })
            .transpose()?;
        Self::build_fixpoint_node(&cte.name, anchor_root, step_root)
    }

    /// Split a recursive CTE body into its `anchor UNION ALL step` branches.
    fn split_recursive_body<'a>(
        cte_name: &str,
        body: &'a Stmt,
    ) -> Result<(&'a Stmt, &'a Stmt), PlannerError> {
        use crate::parser::ast::stmt::SetOperationType;

        match body {
            Stmt::SetOperation(setop) if setop.op_type == SetOperationType::UnionAll => {
                Ok((setop.left.as_ref(), setop.right.as_ref()))
            }
            _ => Err(PlannerError::PlanGenerationFailed(format!(
                "Recursive CTE '{}' body must be `anchor UNION ALL step`",
                cte_name
            ))),
        }
    }

    /// Plan an arbitrary AST sub-statement (CTE anchor/step) through the
    /// same planner dispatch as top-level statements.
    fn plan_stmt_substatement(
        stmt: &Stmt,
        qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let mut planner = PlannerEnum::from_stmt_ref(stmt).ok_or_else(|| {
            PlannerError::NoSuitablePlanner("No planner for CTE branch statement".to_string())
        })?;
        let sub_ast = Arc::new(crate::parser::ast::stmt::Ast::new(
            stmt.clone(),
            Arc::new(
                graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
            ),
        ));
        let sub_validated = crate::binder::validation::ValidatedStatement::new(
            sub_ast,
            crate::binder::validation::ValidationInfo::new(),
        );
        planner.transform(&sub_validated, qctx.clone())
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
