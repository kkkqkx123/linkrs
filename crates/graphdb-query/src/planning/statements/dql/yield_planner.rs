//! YIELD Statement Planner
//!
//! Query planning for processing the YIELD statement

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::core::nodes::{
    DedupNode, FilterNode, LimitNode, ProjectNode, SortNode, StartNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::plan_combiner::{
    logical_start_root, wrap_logical_dedup, wrap_logical_filter, wrap_logical_limit,
    wrap_logical_project, wrap_logical_sort,
};
use crate::QueryContext;
use graphdb_core::types::graph_schema::OrderDirection;
use graphdb_core::YieldColumn;
use std::sync::Arc;

/// YIELD Statement Planner
/// Responsible for converting the YIELD statement into an execution plan.
#[derive(Debug, Clone)]
pub struct YieldPlanner;

impl YieldPlanner {
    /// Create a new YIELD planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for YieldPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let yield_stmt = match validated.stmt() {
            Stmt::Yield(yield_stmt) => yield_stmt,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the YIELD".to_string(),
                ));
            }
        };

        let start_node = StartNode::new();
        let mut current_node = PlanNodeEnum::Start(start_node.clone());
        let mut current_logical: LogicalNodeEnum = logical_start_root();

        let yield_columns: Vec<YieldColumn> = yield_stmt
            .items
            .iter()
            .map(|item| {
                let expression = item.expression.clone();
                let alias = item.alias.clone().unwrap_or_else(|| {
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
            })
            .collect();

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

        if let Some(where_clause) = &yield_stmt.where_clause {
            let filter_node =
                FilterNode::new(current_node.clone(), where_clause.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Filter(filter_node);
            current_logical = wrap_logical_filter(
                current_logical,
                where_clause.clone(),
                current_node.col_names().to_vec(),
            );
        }

        if yield_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
            current_logical =
                wrap_logical_dedup(current_logical, current_node.col_names().to_vec());
        }

        if let Some(order_by) = &yield_stmt.order_by {
            let sort_items: Vec<crate::planning::plan::core::nodes::SortItem> = order_by
                .items
                .iter()
                .map(|item| {
                    let direction = match item.direction {
                        crate::parser::ast::stmt::OrderDirection::Asc => OrderDirection::Asc,
                        crate::parser::ast::stmt::OrderDirection::Desc => OrderDirection::Desc,
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

        if let Some(ref skip) = yield_stmt.skip {
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

        if let Some(ref limit) = yield_stmt.limit {
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

        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Start(start_node)),
            logical_root: Some(current_logical),
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
        let yield_stmt = match bound {
            BoundStatement::Yield(y) => y,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the YIELD".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let start_node = StartNode::new();
        let mut current_node = PlanNodeEnum::Start(start_node.clone());
        let mut current_logical: LogicalNodeEnum = logical_start_root();

        let yield_columns: Vec<YieldColumn> = yield_stmt
            .items
            .iter()
            .map(|item| {
                let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                    &item.expression,
                    &expr_ctx,
                )
                .unwrap_or_else(|_| {
                    let ctx = Arc::new(
                        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                    );
                    let id = ctx.register_expression(
                        graphdb_core::types::expr::ExpressionMeta::new(
                            graphdb_core::Expression::Variable("_".to_string()),
                        ),
                    );
                    graphdb_core::types::ContextualExpression::new(id, ctx)
                });
                let alias = item.alias.clone().unwrap_or_else(|| "_".to_string());
                YieldColumn {
                    expression: ctx_expr,
                    alias,
                    is_matched: false,
                }
            })
            .collect();

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

        if let Some(where_clause) = &yield_stmt.where_clause {
            let condition =
                crate::binder::expr_converter::bound_expr_to_contextual(where_clause, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)?;
            let filter_node =
                FilterNode::new(current_node.clone(), condition.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Filter(filter_node);
            current_logical = wrap_logical_filter(
                current_logical,
                condition,
                current_node.col_names().to_vec(),
            );
        }

        if yield_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
            current_logical =
                wrap_logical_dedup(current_logical, current_node.col_names().to_vec());
        }

        if let Some(order_by) = &yield_stmt.order_by {
            let sort_items: Vec<crate::planning::plan::core::nodes::SortItem> = order_by
                .iter()
                .map(|item| {
                    let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                        &item.expression,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    let raw_expr = ctx_expr
                        .expression()
                        .map(|e| e.inner().clone())
                        .unwrap_or_else(|| {
                            graphdb_core::Expression::Variable(ctx_expr.to_expression_string())
                        });
                    Ok(crate::planning::plan::core::nodes::SortItem::new(
                        raw_expr,
                        item.direction,
                    ))
                })
                .collect::<Result<Vec<_>, PlannerError>>()?;
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

        if let Some(skip) = &yield_stmt.skip {
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

        if let Some(limit) = &yield_stmt.limit {
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

        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Start(start_node)),
            logical_root: Some(current_logical),
        };
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Yield(_))
    }
}

impl Default for YieldPlanner {
    fn default() -> Self {
        Self::new()
    }
}
