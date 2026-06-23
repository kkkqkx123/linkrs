//! YIELD Statement Planner
//!
//! Query planning for processing the YIELD statement

use crate::core::YieldColumn;
use crate::query::parser::ast::stmt::{OrderDirection, Stmt, YieldItem, YieldStmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{ArgumentNode, DedupNode, FilterNode, LimitNode, ProjectNode, SortNode},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
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

    /// Extract the YieldStmt from the Stmt.
    fn extract_yield_stmt(&self, stmt: &Stmt) -> Result<YieldStmt, PlannerError> {
        match stmt {
            Stmt::Yield(yield_stmt) => Ok(yield_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the YIELD".to_string(),
            )),
        }
    }

    /// Convert YieldItem to YieldColumn
    fn convert_yield_item_to_yield_column(
        &self,
        item: &YieldItem,
        _validated: &ValidatedStatement,
    ) -> YieldColumn {
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
    }
}

impl Planner for YieldPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Check the semantic information.
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("YIELD quoted tags: {:?}", referenced_tags);
        }

        let referenced_properties = &validation_info.semantic_info.referenced_properties;
        if !referenced_properties.is_empty() {
            log::debug!("YIELD referenced properties: {:?}", referenced_properties);
        }

        let yield_stmt = self.extract_yield_stmt(validated.stmt())?;

        // Create a parameter node as the input.
        let arg_node = ArgumentNode::new(next_node_id(), "yield_input");
        let mut current_node = PlanNodeEnum::Argument(arg_node.clone());

        let yield_columns: Vec<YieldColumn> = yield_stmt
            .items
            .iter()
            .map(|item| self.convert_yield_item_to_yield_column(item, validated))
            .collect();

        // Create a projection node.
        let project_node = ProjectNode::new(current_node.clone(), yield_columns).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
        })?;
        current_node = PlanNodeEnum::Project(project_node);

        // If there is a WHERE clause, create a filtering node.
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

        // If deduplication is required, create a deduplication node.
        if yield_stmt.distinct {
            let dedup_node = DedupNode::new(current_node.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create DedupNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Dedup(dedup_node);
        }

        // If there is an ORDER BY clause, create a sorting node.
        if let Some(order_by) = &yield_stmt.order_by {
            let sort_items: Vec<crate::query::planning::plan::core::nodes::SortItem> = order_by
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
                    crate::query::planning::plan::core::nodes::SortItem::new(expression, direction)
                })
                .collect();
            let sort_node = SortNode::new(current_node.clone(), sort_items).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create SortNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Sort(sort_node);
        }

        // If there is a SKIP clause, create a restriction node.
        if let Some(skip) = yield_stmt.skip {
            let limit_node = LimitNode::new(current_node.clone(), skip as i64, 0).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        // If there is a LIMIT clause, create a limit node.
        if let Some(limit) = yield_stmt.limit {
            let limit_node =
                LimitNode::new(current_node.clone(), 0, limit as i64).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!("Failed to create LimitNode: {}", e))
                })?;
            current_node = PlanNodeEnum::Limit(limit_node);
        }

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Argument(arg_node)));

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
