//! YIELD Statement Planner
//!
//! Query planning for processing the YIELD statement

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::core::nodes::{
    DedupNode, FilterNode, LimitNode, ProjectNode, SortNode, StartNode,
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
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

        let project_node = ProjectNode::new(current_node.clone(), yield_columns).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
        })?;
        current_node = PlanNodeEnum::Project(project_node);

        if let Some(where_clause) = &yield_stmt.where_clause {
            let filter_node =
                FilterNode::new(current_node.clone(), where_clause.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Filter(filter_node);
        }

        if yield_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
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
            let sort_node = SortNode::new(current_node.clone(), sort_items).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create SortNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Sort(sort_node);
        }

        if let Some(ref skip) = yield_stmt.skip {
            let limit_node =
                LimitNode::new(current_node.clone(), skip.count as i64, 0).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        if let Some(ref limit) = yield_stmt.limit {
            let limit_node =
                LimitNode::new(current_node.clone(), 0, limit.count as i64).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Start(start_node)));
        Ok(sub_plan)
    }

    fn plan_bound(
        &mut self,
        bound: &BoundStatement,
        _qctx: Arc<QueryContext>,
        _metadata: Option<&crate::metadata::MetadataContext>,
        _validated: &ValidatedStatement,
    ) -> Result<SubPlan, PlannerError> {
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

        let project_node = ProjectNode::new(current_node.clone(), yield_columns).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
        })?;
        current_node = PlanNodeEnum::Project(project_node);

        if let Some(where_clause) = &yield_stmt.where_clause {
            let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                where_clause,
                &expr_ctx,
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
            let filter_node =
                FilterNode::new(current_node.clone(), condition).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Filter(filter_node);
        }

        if yield_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
        }

        if let Some(order_by) = &yield_stmt.order_by {
            let sort_items: Vec<crate::planning::plan::core::nodes::SortItem> = order_by
                .iter()
                .map(|item| {
                    let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                        &item.expression,
                        &expr_ctx,
                    )
                    .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
                    let raw_expr = ctx_expr
                        .expression()
                        .map(|e| e.inner().clone())
                        .unwrap_or_else(|| {
                            graphdb_core::Expression::Variable(
                                ctx_expr.to_expression_string(),
                            )
                        });
                    Ok(crate::planning::plan::core::nodes::SortItem::new(
                        raw_expr,
                        item.direction.into(),
                    ))
                })
                .collect::<Result<Vec<_>, PlannerError>>()?;
            let sort_node = SortNode::new(current_node.clone(), sort_items).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create SortNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Sort(sort_node);
        }

        if let Some(skip) = &yield_stmt.skip {
            let limit_node =
                LimitNode::new(current_node.clone(), skip.count as i64, 0).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        if let Some(limit) = &yield_stmt.limit {
            let limit_node =
                LimitNode::new(current_node.clone(), 0, limit.count as i64).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Start(start_node)));
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
