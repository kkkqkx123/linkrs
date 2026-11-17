//! LOOKUP Statement Planner
//! Planning for handling the Nebula LOOKUP queries
//!
//! ## Explanation of the improvements
//!
//! Unified import path
//! Improve the expression parsing mechanism.
//! Add logic for selecting attribute indexes.
//! Use IndexSelector to automatically select the optimal index.

use crate::binder::BoundStatement;
use crate::metadata::{IndexMetadata, MetadataContext};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::access::{IndexLimit, ScanType};
use crate::planning::plan::logical::logical_nodes::access::{
    IndexHint, LogicalScanEdgesNode, LogicalScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalFilterNode, LogicalProjectNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::projection_util;
use crate::QueryContext;
use linkrs_core::types::operators::BinaryOperator;
use linkrs_core::types::ContextualExpression;
use linkrs_core::Expression;
use linkrs_core::Value;
use std::sync::Arc;

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
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let lookup = match bound {
            BoundStatement::Lookup(l) => l,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "LookupPlanner requires the Lookup statement.".to_string(),
                ));
            }
        };

        let target_name = match &lookup.target {
            crate::binder::bound::BoundLookupTarget::Tag(name) => name.clone(),
            crate::binder::bound::BoundLookupTarget::Edge(name) => name.clone(),
        };

        let is_edge = matches!(
            &lookup.target,
            crate::binder::bound::BoundLookupTarget::Edge(_)
        );

        // Convert bound where clause to ContextualExpression for index selection
        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let where_ctx = lookup
            .where_clause
            .as_ref()
            .map(|wc| {
                crate::binder::expr_converter::bound_expr_to_contextual(&wc.condition, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)
            })
            .transpose()?;

        // Use metadata for index selection (same logic as transform_with_metadata)
        let (selected_index, tag_id) = if let Some(metadata_context) = metadata {
            let selected =
                Self::find_suitable_index(metadata_context, &target_name, is_edge, &where_ctx);
            let tag_id = if is_edge {
                0
            } else {
                metadata_context
                    .get_tag_metadata(&target_name)
                    .map(|meta| meta.tag_id as i32)
                    .unwrap_or(0)
            };
            (selected, tag_id)
        } else {
            (None, 0)
        };

        // Build the plan using bound data
        self.plan_lookup_bound(lookup, qctx, selected_index.as_ref(), tag_id, where_ctx)
    }
}

impl LookupPlanner {
    /// Select an index whose first field matches a WHERE condition on the
    /// target tag/edge. Returns `None` when no suitable index exists, in
    /// which case the caller falls back to a full scan.
    fn find_suitable_index(
        metadata_context: &MetadataContext,
        target_name: &str,
        is_edge: bool,
        where_clause: &Option<ContextualExpression>,
    ) -> Option<IndexMetadata> {
        let where_expr = where_clause.as_ref()?.get_expression()?;
        for index in metadata_context.get_all_indexes() {
            if index.is_edge != is_edge || index.tag_name != target_name {
                continue;
            }
            if index.field_name.is_empty() {
                continue;
            }
            let mut limits = Vec::new();
            Self::extract_conditions(
                &where_expr,
                std::slice::from_ref(&index.field_name),
                &mut limits,
            );
            if !limits.is_empty() {
                log::debug!(
                    "LOOKUP using index '{}' on {} '{}'",
                    index.index_name,
                    if is_edge { "edge" } else { "tag" },
                    target_name
                );
                return Some(index.clone());
            }
        }
        None
    }

    /// Build a lookup plan from a bound statement (plan_bound path).
    fn plan_lookup_bound(
        &self,
        lookup: &crate::binder::bound::BoundLookupStatement,
        qctx: Arc<QueryContext>,
        selected_index: Option<&IndexMetadata>,
        tag_id: i32,
        where_ctx: Option<ContextualExpression>,
    ) -> Result<SubPlan, PlannerError> {
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        if space_id == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "Invalid space ID: 0".to_string(),
            ));
        }

        let target_name = match &lookup.target {
            crate::binder::bound::BoundLookupTarget::Tag(name) => name.clone(),
            crate::binder::bound::BoundLookupTarget::Edge(name) => name.clone(),
        };
        let is_edge = matches!(
            &lookup.target,
            crate::binder::bound::BoundLookupTarget::Edge(_)
        );

        // When an index was selected, stamp it onto the logical scan as an
        // index hint so the logical-to-physical mapping rebuilds an
        // IndexScan instead of a full storage scan.
        let index_hint: Option<IndexHint> = selected_index.and_then(|index| {
            let where_expr = where_ctx.as_ref()?.get_expression()?;
            let mut limits = Vec::new();
            Self::extract_conditions(
                &where_expr,
                std::slice::from_ref(&index.field_name),
                &mut limits,
            );
            if limits.is_empty() {
                return None;
            }
            let scan_type = if limits.len() == 1 && limits[0].scan_type == ScanType::Unique {
                ScanType::Unique
            } else {
                ScanType::Range
            };
            Some(IndexHint::new(
                index.index_name.clone(),
                target_name.clone(),
                tag_id,
                index.index_id,
                scan_type.as_str().to_string(),
            ))
        });

        // The scan never carries a pre-filled row cap: row limits lower to
        // an explicit Limit node above the projection chain so filtering
        // runs before truncation. The limit pushdown optimizer decides
        // whether a trailing Limit folds back into the scan.
        let mut logical_root = if is_edge {
            LogicalNodeEnum::ScanEdges(LogicalScanEdgesNode {
                id: next_node_id(),
                space_id,
                edge_type: Some(target_name.clone()),
                expression: None,
                limit: None,
                projected_properties: vec![],
                index_hint: index_hint.clone(),
                estimated_cardinality: None,
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: projection_util::unknown_column_types(1),
            })
        } else {
            LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
                id: next_node_id(),
                space_id,
                space_name: space_name.clone(),
                tag: Some(target_name.clone()),
                expression: None,
                limit: None,
                projected_properties: vec![],
                index_hint,
                estimated_cardinality: None,
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: projection_util::unknown_column_types(1),
            })
        };

        // LOOKUP WHERE filters storage rows before projection.
        if let Some(ref condition) = where_ctx {
            logical_root = LogicalNodeEnum::Filter(LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                condition: condition.clone(),
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            });
        }

        // YIELD items project first; the embedded row filter lowers right
        // above the projection (the statement-level LOOKUP WHERE stays
        // below it), followed by DISTINCT / ORDER BY / SKIP / LIMIT.
        // Sampling stays an explicit planning error.
        if let Some(ref yield_clause) = lookup.yield_clause {
            if yield_clause.sample.is_some() {
                return Err(PlannerError::PlanGenerationFailed(
                    "LOOKUP SAMPLE is not supported yet".to_string(),
                ));
            }
            let yield_columns = self.build_yield_columns_from_bound(lookup)?;
            let column_types = projection_util::project_column_types(&yield_columns);
            let col_names: Vec<String> =
                yield_columns.iter().map(|col| col.alias.clone()).collect();
            logical_root = LogicalNodeEnum::Project(LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                columns: yield_columns,
                subqueries: Vec::new(),
                has_folded_expressions: false,
                output_var: None,
                col_names: col_names.clone(),
                column_types,
            });

            let expr_ctx = Arc::new(
                linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
            );
            logical_root =
                crate::planning::statements::clauses::yield_planner::apply_bound_tail_logical(
                    logical_root,
                    yield_clause,
                    &expr_ctx,
                    &col_names,
                )?;
        }

        Ok(SubPlan::from_logical_root(logical_root))
    }

    /// Build YIELD columns from bound statement (plan_bound path)
    fn build_yield_columns_from_bound(
        &self,
        lookup: &crate::binder::bound::BoundLookupStatement,
    ) -> Result<Vec<linkrs_core::YieldColumn>, PlannerError> {
        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let mut columns = Vec::new();

        if let Some(ref yield_clause) = lookup.yield_clause {
            for item in &yield_clause.items {
                let column = crate::binder::expr_converter::bound_projection_to_yield_column(
                    item, &expr_ctx,
                )
                .map_err(PlannerError::PlanGenerationFailed)?;
                columns.push(column);
            }
        }

        if columns.is_empty() {
            let expr = Expression::Variable("_vertex".to_string());
            let meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
            let id = expr_ctx.register_expression(meta);
            let ctx_expr = ContextualExpression::new(id, expr_ctx.clone());
            columns.push(linkrs_core::YieldColumn {
                expression: ctx_expr,
                alias: "result".to_string(),
            });
        }

        Ok(columns)
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
                            None::<Value>,
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
                            None::<Value>,
                            true,
                            false,
                        ));
                    }
                }
                BinaryOperator::LessThanOrEqual => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            None::<Value>,
                            Some(val),
                            false,
                            true,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(col, Some(val), None::<Value>, true, true));
                    }
                }
                BinaryOperator::GreaterThan => {
                    if let Some((col, val)) = Self::extract_comparison(left, right, index_columns) {
                        limits.push(IndexLimit::range(
                            col,
                            Some(val),
                            None::<Value>,
                            false,
                            false,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            None::<Value>,
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
                            None::<Value>,
                            true,
                            false,
                        ));
                    } else if let Some((col, val)) =
                        Self::extract_comparison(right, left, index_columns)
                    {
                        limits.push(IndexLimit::range(
                            col,
                            None::<Value>,
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
    ) -> Option<(String, Value)> {
        let col_name = Self::extract_property_name(left)?;
        if !index_columns.iter().any(|c| c == &col_name) {
            return None;
        }
        let value = Self::extract_literal_value(right)?;
        Some((col_name, value))
    }

    fn extract_property_name(expr: &Expression) -> Option<String> {
        match expr {
            Expression::TagProperty { property, .. } => Some(property.clone()),
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

    fn extract_literal_value(expr: &Expression) -> Option<Value> {
        match expr {
            Expression::Literal(value) => Some(value.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::bound::{BoundOrderByItem, BoundProjectionClause, BoundProjectionItem};
    use crate::planning::context::PlanContext;

    fn test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(crate::QueryRequestContext::new("LOOKUP".to_string()));
        let space_info = linkrs_core::types::SpaceInfo {
            space_id: 1,
            space_name: "test".to_string(),
            ..Default::default()
        };
        Arc::new(
            crate::QueryContext::builder(rctx)
                .with_space_info(space_info)
                .build(),
        )
    }

    fn var_expr(name: &str) -> crate::binder::bound::BoundExpression {
        crate::binder::bound::BoundExpression::Variable(
            name.to_string(),
            linkrs_core::DataType::Unknown,
        )
    }

    fn prop_expr(name: &str, property: &str) -> crate::binder::bound::BoundExpression {
        crate::binder::bound::BoundExpression::Property {
            object: Box::new(var_expr(name)),
            property: property.to_string(),
            value_type: linkrs_core::DataType::Unknown,
        }
    }

    fn yield_clause(
        distinct: bool,
        order: bool,
        skip: Option<usize>,
        limit: Option<usize>,
        sample: bool,
    ) -> crate::binder::bound::BoundProjectionClause {
        let order_by = if order {
            Some(vec![BoundOrderByItem {
                expression: prop_expr("Person", "name"),
                direction: linkrs_core::types::OrderDirection::Asc,
            }])
        } else {
            None
        };
        BoundProjectionClause {
            items: vec![BoundProjectionItem {
                expression: prop_expr("Person", "name"),
                alias: Some("name".to_string()),
            }],
            distinct,
            where_clause: None,
            order_by,
            limit: limit.map(|count| crate::parser::ast::LimitClause {
                span: crate::parser::ast::Span::default(),
                count,
            }),
            skip: skip.map(|count| crate::parser::ast::SkipClause {
                span: crate::parser::ast::Span::default(),
                count,
            }),
            sample: if sample {
                Some(crate::parser::ast::SampleClause {
                    span: crate::parser::ast::Span::default(),
                    count: 1,
                    percentage: None,
                })
            } else {
                None
            },
            having: None,
        }
    }

    use crate::planning::plan::core::nodes::operation::sort_node::LimitNode;

    fn plan_lookup(yield_clause: Option<BoundProjectionClause>) -> Result<SubPlan, PlannerError> {
        let bound = BoundStatement::Lookup(crate::binder::bound::BoundLookupStatement {
            target: crate::binder::bound::BoundLookupTarget::Tag("Person".to_string()),
            where_clause: None,
            yield_clause,
        });
        let qctx = test_query_context();
        let ctx = PlanContext::without_metadata(&bound, qctx);
        LookupPlanner::new().plan_bound(&ctx)
    }

    #[test]
    fn full_yield_chain_orders_limit_over_sort_over_dedup_over_project() {
        let plan = plan_lookup(Some(yield_clause(true, true, Some(5), Some(10), false)))
            .expect("lookup with full yield clause plans");
        let root = plan.logical_root.expect("lookup carries a logical root");
        let LogicalNodeEnum::Limit(limit) = root else {
            panic!("root must be Limit, got {:?}", root);
        };
        assert_eq!(limit.offset, 5);
        assert_eq!(limit.count, 10);
        let LogicalNodeEnum::Sort(sort) = limit.input.as_deref().expect("limit has input") else {
            panic!("limit input must be Sort");
        };
        assert_eq!(sort.sort_items.len(), 1);
        let LogicalNodeEnum::Dedup(_) = sort.input.as_deref().expect("sort has input") else {
            panic!("sort input must be Dedup");
        };
        // Walk through the dedup node to the projection below it.
        let dedup_input = match sort.input.as_deref().expect("sort has input") {
            LogicalNodeEnum::Dedup(dedup) => dedup.input.as_deref().expect("dedup has input"),
            other => panic!("sort input must be Dedup, got {:?}", other),
        };
        let LogicalNodeEnum::Project(project) = dedup_input else {
            panic!("dedup input must be Project");
        };
        assert_eq!(project.col_names, vec!["name".to_string()]);
        let LogicalNodeEnum::ScanVertices(scan) =
            project.input.as_deref().expect("project has input")
        else {
            panic!("project input must be ScanVertices");
        };
        assert!(
            scan.limit.is_none(),
            "scan must not pre-fill the yield limit"
        );
    }

    #[test]
    fn skip_only_uses_unbounded_count() {
        let plan = plan_lookup(Some(yield_clause(false, false, Some(3), None, false)))
            .expect("lookup with skip-only plans");
        let root = plan.logical_root.expect("lookup carries a logical root");
        let LogicalNodeEnum::Limit(limit) = root else {
            panic!("root must be Limit, got {:?}", root);
        };
        assert_eq!(limit.offset, 3);
        assert_eq!(limit.count, LimitNode::UNBOUNDED);
    }

    #[test]
    fn sample_is_a_planning_error() {
        let err = plan_lookup(Some(yield_clause(false, false, None, None, true)))
            .expect_err("lookup sample must fail");
        assert!(
            matches!(err, PlannerError::PlanGenerationFailed(_)),
            "sample must raise a planning error, got {:?}",
            err
        );
    }

    #[test]
    fn row_filter_sits_between_project_and_dedup() {
        use linkrs_core::types::operators::BinaryOperator;
        let mut clause = yield_clause(true, false, None, None, false);
        clause.where_clause = Some(crate::binder::bound::BoundExpression::BinaryOp {
            left: Box::new(prop_expr("Person", "age")),
            op: BinaryOperator::GreaterThan,
            right: Box::new(crate::binder::bound::BoundExpression::Literal(
                linkrs_core::Value::Int(18),
                linkrs_core::DataType::Int,
            )),
            return_type: linkrs_core::DataType::Bool,
        });
        let plan = plan_lookup(Some(clause)).expect("lookup with row filter plans");
        let root = plan.logical_root.expect("lookup carries a logical root");
        let LogicalNodeEnum::Dedup(dedup) = root else {
            panic!("root must be Dedup, got {:?}", root);
        };
        let LogicalNodeEnum::Filter(_) = dedup.input.as_deref().expect("dedup has input") else {
            panic!("dedup input must be the row filter");
        };
        let filter_input = match dedup.input.as_deref().expect("dedup has input") {
            LogicalNodeEnum::Filter(filter) => filter.input.as_deref().expect("filter has input"),
            other => panic!("dedup input must be Filter, got {:?}", other),
        };
        assert!(
            matches!(filter_input, LogicalNodeEnum::Project(_)),
            "row filter input must be Project, got {:?}",
            filter_input
        );
    }
}
