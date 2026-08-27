//! RETURN Statement Planner
//!
//! Query planning for statements that handle the RETURN command

use crate::core::YieldColumn;
use crate::parser::ast::stmt::{OrderDirection, ReturnItem, ReturnStmt, Stmt};
use crate::planning::plan::core::nodes::{
    DedupNode, LimitNode, ProjectNode, SortNode, StartNode,
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;
use std::sync::Arc;

/// RETURN statement planner
/// Responsible for converting the RETURN statement into an execution plan.
#[derive(Debug, Clone)]
pub struct ReturnPlanner;

impl ReturnPlanner {
    /// Create a new RETURN planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the ReturnStmt from the Stmt.
    fn extract_return_stmt(&self, stmt: &Stmt) -> Result<ReturnStmt, PlannerError> {
        match stmt {
            Stmt::Return(return_stmt) => Ok(return_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain a RETURN".to_string(),
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

impl Planner for ReturnPlanner {
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
            log::debug!("RETURN Referenced tags: {:?}", referenced_tags);
        }

        let referenced_properties = &validation_info.semantic_info.referenced_properties;
        if !referenced_properties.is_empty() {
            log::debug!("RETURN Referenced properties: {:?}", referenced_properties);
        }

        let return_stmt = self.extract_return_stmt(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in
        // RETURN expressions are compiled here and attached to the Project
        // node; RETURN ORDER BY items are still refused at planning time with
        // a precise error.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        let mut id_alloc = exists_planner::SubqueryIdAllocator::new();
        let mut yield_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let mut yield_columns: Vec<YieldColumn> = return_stmt
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
        if let Some(order_by) = &return_stmt.order_by {
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

        // A single empty row seeds a standalone RETURN statement.
        let start_node = StartNode::new();
        let mut current_node = PlanNodeEnum::Start(start_node.clone());

        // Create a projection node.
        let project_node = ProjectNode::new(current_node.clone(), yield_columns)
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?
            .with_subqueries(yield_subqueries);
        current_node = PlanNodeEnum::Project(project_node);

        // If deduplication is required, create a deduplication node.
        if return_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
        }

        // If there is an ORDER BY clause, create a sorting node.
        if let Some(order_by) = &return_stmt.order_by {
            let sort_items: Vec<crate::planning::plan::core::nodes::SortItem> = order_by
                .items
                .iter()
                .map(|item| {
                    let direction = match item.direction {
                        OrderDirection::Asc => {
                            crate::core::types::graph_schema::OrderDirection::Asc
                        }
                        OrderDirection::Desc => {
                            crate::core::types::graph_schema::OrderDirection::Desc
                        }
                    };
                    let expression = item
                        .expression
                        .expression()
                        .map(|e| e.inner().clone())
                        .unwrap_or_else(|| {
                            crate::core::Expression::Variable(
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

        // If there is a SKIP clause, create a restriction node.
        if let Some(skip) = return_stmt.skip {
            let limit_node =
                LimitNode::new(current_node.clone(), skip.count as i64, 0).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        // If there is a LIMIT clause, create a limit node.
        if let Some(limit) = return_stmt.limit {
            let limit_node =
                LimitNode::new(current_node.clone(), 0, limit.count as i64).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Start(start_node)));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Return(_))
    }
}

impl Default for ReturnPlanner {
    fn default() -> Self {
        Self::new()
    }
}
