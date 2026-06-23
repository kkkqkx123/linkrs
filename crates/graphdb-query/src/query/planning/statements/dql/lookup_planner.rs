//! LOOKUP Statement Planner
//! Planning for handling the Nebula LOOKUP queries
//!
//! ## Explanation of the improvements
//!
//! Unified import path
//! Improve the expression parsing mechanism.
//! Add logic for selecting attribute indexes.
//! Use IndexSelector to automatically select the optimal index.

use crate::core::types::operators::BinaryOperator;
use crate::core::types::ContextualExpression;
use crate::core::types::Index;
use crate::core::value::NullType;
use crate::core::Expression;
use crate::query::parser::ast::{LookupStmt, Stmt};
use crate::query::planning::plan::core::nodes::access::{
    EdgeIndexScanNode, IndexLimit, IndexScanNode, ScanType,
};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

pub use crate::query::planning::plan::core::nodes::{
    ArgumentNode, DedupNode, FilterNode, GetEdgesNode, GetVerticesNode, HashInnerJoinNode,
    ProjectNode,
};
pub use crate::query::planning::plan::core::PlanNodeEnum;

/// LOOKUP Query Planner
/// Responsible for converting the LOOKUP statement into an execution plan.
#[derive(Debug, Clone)]
pub struct LookupPlanner {}

impl LookupPlanner {
    /// Create a new LOOKUP planner.
    pub fn new() -> Self {
        Self {}
    }
}

impl Planner for LookupPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let lookup_stmt = match validated.stmt() {
            Stmt::Lookup(lookup_stmt) => lookup_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "LookupPlanner requires the Lookup statement.".to_string(),
                ));
            }
        };

        let space_id = qctx.space_id().unwrap_or(1);

        if space_id == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "Invalid space ID: 0".to_string(),
            ));
        }

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // 1. Check the optimization suggestions.
        for hint in &validation_info.optimization_hints {
            log::debug!("LOOKUP Optimization Tip: {:?}", hint);
        }

        // 2. Check the index suggestions.
        let mut selected_index: Option<Index> = None;
        let mut scan_limits: Vec<crate::query::planning::plan::core::nodes::access::IndexLimit> =
            Vec::new();
        let mut scan_type = ScanType::Full;

        if !validation_info.index_hints.is_empty() {
            let hint = &validation_info.index_hints[0];
            log::debug!("LOOKUP using index hints: {:?}", hint);

            let index_fields: Vec<crate::core::types::IndexField> = hint
                .columns
                .iter()
                .map(|col| {
                    crate::core::types::IndexField::new(
                        col.clone(),
                        crate::core::Value::Null(NullType::Null),
                        true,
                    )
                })
                .collect();

            selected_index = Some(Index {
                id: 1,
                name: hint.index_name.clone(),
                space_id,
                schema_name: hint.table_name.clone(),
                fields: index_fields,
                properties: hint.columns.clone(),
                index_type: crate::core::types::IndexType::TagIndex,
                status: crate::core::types::IndexStatus::Active,
                is_unique: false,
                comment: None,
                partial_condition: None,
            });

            // Extract filter values from WHERE clause
            scan_limits =
                Self::extract_scan_limits_from_where(&lookup_stmt.where_clause, &hint.columns);
            scan_type = if scan_limits.is_empty() {
                ScanType::Full
            } else if scan_limits.len() == 1 && scan_limits[0].scan_type == ScanType::Unique {
                // Single equality condition: use index point lookup
                ScanType::Unique
            } else {
                // Multiple conditions or range queries: use Range
                ScanType::Range
            };
        }

        // 3. If there is no index suggestion, obtain the list of available indexes.
        if selected_index.is_none() {
            let available_indexes: Vec<Index> = vec![];

            // Use a simple heuristic to select the index (choose the first available index).
            if !available_indexes.is_empty() {
                let index = available_indexes.first().cloned();
                selected_index = index;
                scan_type = ScanType::Range;
            }
        }

        let index_id = selected_index.as_ref().map(|idx| idx.id).unwrap_or(0);

        // Get index_name and schema_name from index_hint if available
        let (index_name, schema_name) = if let Some(hint) = validation_info.index_hints.first() {
            let name = if hint.index_name.is_empty() {
                selected_index
                    .as_ref()
                    .map(|idx| idx.name.clone())
                    .unwrap_or_default()
            } else {
                hint.index_name.clone()
            };
            (name, hint.table_name.clone())
        } else {
            (String::new(), String::new())
        };

        // Check if this is an edge lookup
        let is_edge = validation_info
            .index_hints
            .first()
            .map(|h| h.is_edge)
            .unwrap_or(false);

        // 4. Create the appropriate scan node based on whether it's an edge or tag lookup
        let mut current_node: PlanNodeEnum = if is_edge {
            let mut edge_index_scan_node =
                EdgeIndexScanNode::new(space_id, &schema_name, &index_name);
            edge_index_scan_node.set_scan_type(scan_type);
            edge_index_scan_node.set_scan_limits(scan_limits);

            // Set limit from yield clause
            if let Some(ref yield_clause) = lookup_stmt.yield_clause {
                if let Some(ref limit_clause) = yield_clause.limit {
                    edge_index_scan_node.set_limit(limit_clause.count as i64);
                }
            }

            PlanNodeEnum::EdgeIndexScan(edge_index_scan_node)
        } else {
            let mut index_scan_node =
                IndexScanNode::new(space_id, 0, index_id, index_name, schema_name, scan_type);

            // 5. Setting scan limitations and the columns to be returned
            index_scan_node.set_scan_limits(scan_limits);

            // 5.1 Set limit from yield clause
            if let Some(ref yield_clause) = lookup_stmt.yield_clause {
                if let Some(ref limit_clause) = yield_clause.limit {
                    index_scan_node.set_limit(limit_clause.count as i64);
                }
            }

            PlanNodeEnum::IndexScan(index_scan_node)
        };

        if let Some(ref condition) = lookup_stmt.where_clause {
            let filter_node = FilterNode::new(current_node, condition.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create FilterNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Filter(filter_node);
        }

        if lookup_stmt.yield_clause.is_some() {
            let yield_columns = Self::build_yield_columns(lookup_stmt, validated)?;
            let project_node = ProjectNode::new(current_node, yield_columns).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Project(project_node);
        }

        let arg_node = ArgumentNode::new(0, "lookup_input");
        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Argument(arg_node)),
        };

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Lookup(_))
    }
}

impl LookupPlanner {
    /// Construct the YIELD column
    fn build_yield_columns(
        lookup_stmt: &LookupStmt,
        validated: &ValidatedStatement,
    ) -> Result<Vec<crate::core::YieldColumn>, PlannerError> {
        let mut columns = Vec::new();

        if let Some(ref yield_clause) = lookup_stmt.yield_clause {
            for item in &yield_clause.items {
                columns.push(crate::core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
        }

        if columns.is_empty() {
            let expr = Expression::Variable("_vertex".to_string());
            let meta = crate::core::types::expr::ExpressionMeta::new(expr);
            let id = validated.expr_context().register_expression(meta);
            let ctx_expr =
                crate::core::types::ContextualExpression::new(id, validated.expr_context().clone());
            columns.push(crate::core::YieldColumn {
                expression: ctx_expr,
                alias: "result".to_string(),
                is_matched: false,
            });
        }

        Ok(columns)
    }

    /// Extract scan limits from WHERE clause
    fn extract_scan_limits_from_where(
        where_clause: &Option<ContextualExpression>,
        index_columns: &[String],
    ) -> Vec<IndexLimit> {
        let mut limits = Vec::new();

        let Some(ref where_expr) = where_clause else {
            return limits;
        };

        let Some(expr) = where_expr.get_expression() else {
            return limits;
        };

        Self::extract_conditions(&expr, index_columns, &mut limits);

        limits
    }

    fn extract_conditions(
        expr: &Expression,
        index_columns: &[String],
        limits: &mut Vec<IndexLimit>,
    ) {
        if let Expression::Binary { left, op, right } = expr {
            match op {
                BinaryOperator::Equal => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::equal(col, val));
                    }
                }
                BinaryOperator::NotEqual => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val.clone()),
                            Some(val),
                            true,
                            true,
                        ));
                    }
                }
                BinaryOperator::LessThan => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            None::<String>,
                            Some(val),
                            false,
                            false,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val),
                            None::<String>,
                            true,
                            false,
                        ));
                    }
                }
                BinaryOperator::LessThanOrEqual => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            None::<String>,
                            Some(val),
                            false,
                            true,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val),
                            None::<String>,
                            true,
                            true,
                        ));
                    }
                }
                BinaryOperator::GreaterThan => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val),
                            None::<String>,
                            false,
                            false,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            None::<String>,
                            Some(val),
                            false,
                            false,
                        ));
                    }
                }
                BinaryOperator::GreaterThanOrEqual => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val),
                            None::<String>,
                            true,
                            false,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            None::<String>,
                            Some(val),
                            false,
                            true,
                        ));
                    }
                }
                BinaryOperator::And => {
                    Self::extract_conditions(left, index_columns, limits);
                    Self::extract_conditions(right, index_columns, limits);
                }
                _ => {}
            }
        }
    }

    fn extract_comparison(
        left: &Expression,
        right: &Expression,
        index_columns: &[String],
    ) -> Option<(String, String)> {
        let col_name = Self::extract_property_name(left)?;
        if !index_columns.iter().any(|c| c == &col_name) {
            return None;
        }
        let value = Self::extract_literal_value(right)?;
        Some((col_name, value))
    }

    fn extract_property_name(expr: &Expression) -> Option<String> {
        match expr {
            Expression::Property { property, .. } => Some(property.clone()),
            Expression::Variable(name) => {
                if name.contains('.') {
                    let parts: Vec<&str> = name.split('.').collect();
                    parts.last().map(|s| s.to_string())
                } else {
                    Some(name.clone())
                }
            }
            _ => None,
        }
    }

    fn extract_literal_value(expr: &Expression) -> Option<String> {
        match expr {
            Expression::Literal(value) => match value {
                crate::core::Value::String(s) => Some(s.clone()),
                crate::core::Value::Int(i) => Some(i.to_string()),
                crate::core::Value::BigInt(i) => Some(i.to_string()),
                crate::core::Value::Float(f) => Some(f.to_string()),
                crate::core::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Analyzing the YIELD expression
    fn _parse_yield_expression(name: &str) -> Result<Expression, PlannerError> {
        if name.contains(".") {
            let parts: Vec<&str> = name.split(".").collect();
            if parts.len() == 2 {
                return Ok(Expression::Property {
                    object: Box::new(Expression::Variable(parts[0].to_string())),
                    property: parts[1].to_string(),
                });
            }
        }

        Ok(Expression::Variable(name.to_string()))
    }
}

impl Default for LookupPlanner {
    fn default() -> Self {
        Self::new()
    }
}
