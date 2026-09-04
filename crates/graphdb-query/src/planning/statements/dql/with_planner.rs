//! WITH Statement Planner
//!
//! Query planning for queries that handle the WITH statement

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::{OrderDirection, ReturnItem, Stmt, WithStmt};
use crate::planning::plan::core::{
    next_node_id,
    nodes::{DedupNode, FilterNode, LimitNode, LoopNode, ProjectNode, SortNode, StartNode},
};
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
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

        // A single empty row seeds a standalone WITH statement.
        let start_node = StartNode::new();
        let mut current_node = PlanNodeEnum::Start(start_node.clone());
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

        // Handle recursive CTE
        if with_stmt.recursive {
            // For recursive CTE, create a loop node for iterative expansion
            let expr_ctx =
                graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new();
            let expr_id =
                expr_ctx.register_expression(graphdb_core::types::ExpressionMeta::with_span(
                    graphdb_core::Expression::literal(true),
                    graphdb_core::types::Span::default(),
                ));
            let condition_expr = graphdb_core::types::ContextualExpression::new(
                expr_id,
                std::sync::Arc::new(expr_ctx),
            );
            let loop_node = LoopNode::new(next_node_id(), condition_expr.clone());
            current_node = PlanNodeEnum::Loop(loop_node);
            current_logical = LogicalNodeEnum::Loop(LogicalLoopNode {
                id: next_node_id(),
                condition: condition_expr,
                body: None,
                output_var: None,
                col_names: current_node.col_names().to_vec(),
                column_types: vec![],
            });
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
        let mut current_node = PlanNodeEnum::Start(start_node.clone());
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
            tail: Some(PlanNodeEnum::Start(start_node)),
            logical_root: Some(current_logical),
        };
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::With(_))
    }
}

impl Default for WithPlanner {
    fn default() -> Self {
        Self::new()
    }
}
