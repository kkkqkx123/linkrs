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
use crate::parser::ast::{LookupStmt, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::access::{IndexLimit, IndexScanNode, ScanType};
use crate::planning::plan::logical::logical_nodes::access::{
    LogicalScanEdgesNode, LogicalScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalFilterNode, LogicalProjectNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::types::ContextualExpression;
use graphdb_core::Expression;
use graphdb_core::Value;
use std::sync::Arc;

pub use crate::planning::plan::core::nodes::{
    ArgumentNode, DedupNode, FilterNode, GetEdgesNode, GetVerticesNode, InnerJoinNode, ProjectNode,
    ScanEdgesNode, ScanVerticesNode,
};
pub use crate::planning::plan::core::PlanNodeEnum;

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

        let is_edge = matches!(
            lookup_stmt.target,
            crate::parser::ast::LookupTarget::Edge(_)
        );
        self.plan_lookup(validated, qctx, None, is_edge, 0)
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
        let where_ctx = lookup.where_clause.as_ref().map(|wc| {
            crate::binder::expr_converter::bound_expr_to_contextual(
                &wc.condition,
                validated.expr_context(),
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e))
        }).transpose()?;

        // Use metadata for index selection (same logic as transform_with_metadata)
        let (selected_index, tag_id) = if let Some(metadata_context) = metadata {
            let selected = Self::find_suitable_index(
                metadata_context,
                &target_name,
                is_edge,
                &where_ctx,
            );
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
        self.plan_lookup_bound(lookup, qctx, validated, selected_index.as_ref(), is_edge, tag_id, where_ctx)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Lookup(_))
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

    fn plan_lookup(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        selected_index: Option<&IndexMetadata>,
        is_edge: bool,
        tag_id: i32,
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
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

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

        // Extract the tag/edge name from the LOOKUP target for col_names
        let target_name = match &lookup_stmt.target {
            crate::parser::ast::LookupTarget::Tag(name) => name.clone(),
            crate::parser::ast::LookupTarget::Edge(name) => name.clone(),
            crate::parser::ast::LookupTarget::Unspecified(name) => name.clone(),
        };

        // 2. When an index was selected, extract the scan limits from WHERE.
        let (scan_limits, scan_type) = if let Some(index) = selected_index {
            let limits = Self::extract_scan_limits_from_where(
                &lookup_stmt.where_clause,
                std::slice::from_ref(&index.field_name),
            );
            let scan_type = if limits.len() == 1 && limits[0].scan_type == ScanType::Unique {
                // Single equality condition: use index point lookup
                ScanType::Unique
            } else {
                // Multiple conditions or range queries: use Range
                ScanType::Range
            };
            (limits, scan_type)
        } else {
            (Vec::new(), ScanType::Full)
        };

        // 3. Create the appropriate scan node. With an index the lookup uses
        // a unified IndexScan node; otherwise it falls back to a full scan of
        // the tag/edge, with WHERE filtering applied by a Filter node above.
        //
        // A parallel pure logical tree is built alongside (the index scan is
        // a physical choice that the logical representation drops); it is
        // attached to the SubPlan so the compiler can construct the
        // LogicalPlan natively.
        let mut current_node: PlanNodeEnum = match selected_index {
            Some(index) => {
                let mut index_scan_node = IndexScanNode::new(
                    space_id,
                    tag_id,
                    index.index_id,
                    index.index_name.clone(),
                    target_name.clone(),
                    scan_type,
                );

                index_scan_node.set_scan_limits(scan_limits);
                // Set col_names so the output layout has a named slot that
                // YIELD/Filter expressions like `person.name` can resolve via TagProperty.
                // No return_columns are set: the scan fetches full rows so the
                // WHERE Filter above can evaluate any property.
                index_scan_node.set_col_names(vec![target_name.clone()]);

                // Set limit from yield clause
                if let Some(ref yield_clause) = lookup_stmt.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        index_scan_node.set_limit(limit_clause.count as i64);
                    }
                }

                PlanNodeEnum::IndexScan(index_scan_node)
            }
            None if is_edge => {
                let mut edge_scan_node = ScanEdgesNode::new(space_id, &target_name);
                edge_scan_node.set_col_names(vec![target_name.clone()]);

                if let Some(ref yield_clause) = lookup_stmt.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        edge_scan_node.set_limit(limit_clause.count as i64);
                    }
                }

                PlanNodeEnum::ScanEdges(edge_scan_node)
            }
            None => {
                let mut vertex_scan_node = ScanVerticesNode::new(space_id, &space_name);
                vertex_scan_node.set_col_names(vec![target_name.clone()]);
                vertex_scan_node.set_tag(&target_name);

                if let Some(ref yield_clause) = lookup_stmt.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        vertex_scan_node.set_limit(limit_clause.count as i64);
                    }
                }

                PlanNodeEnum::ScanVertices(vertex_scan_node)
            }
        };

        // Pure logical mirror of the scan (index scans are a physical choice
        // and map to a tagged vertex scan in the logical representation).
        let limit_from_yield = lookup_stmt
            .yield_clause
            .as_ref()
            .and_then(|yc| yc.limit.as_ref().map(|limit| limit.count as i64));
        let mut logical_root = if is_edge {
            LogicalNodeEnum::ScanEdges(LogicalScanEdgesNode {
                id: next_node_id(),
                space_id,
                edge_type: Some(target_name.clone()),
                expression: None,
                limit: limit_from_yield,
                projected_properties: vec![],
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: vec![],
            })
        } else {
            LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
                id: next_node_id(),
                space_id,
                space_name: space_name.clone(),
                tag: Some(target_name.clone()),
                expression: None,
                limit: limit_from_yield,
                projected_properties: vec![],
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: vec![],
            })
        };

        if let Some(ref condition) = lookup_stmt.where_clause {
            let filter_node = FilterNode::new(current_node, condition.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create FilterNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Filter(filter_node);

            let logical_filter = LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                condition: condition.clone(),
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            };
            logical_root = LogicalNodeEnum::Filter(logical_filter);
        }

        if lookup_stmt.yield_clause.is_some() {
            let yield_columns = Self::build_yield_columns(lookup_stmt, validated)?;
            let project_node =
                ProjectNode::new(current_node, yield_columns.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create ProjectNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Project(project_node);

            let logical_project = LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                columns: yield_columns.clone(),
                output_var: None,
                col_names: yield_columns.iter().map(|col| col.alias.clone()).collect(),
                column_types: vec![],
            };
            logical_root = LogicalNodeEnum::Project(logical_project);
        }

        let arg_node = ArgumentNode::new(0, "lookup_input");
        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Argument(arg_node)),
            logical_root: Some(logical_root),
        };

        Ok(sub_plan)
    }

    /// Build a lookup plan from a bound statement (plan_bound path).
    fn plan_lookup_bound(
        &self,
        lookup: &crate::binder::bound::BoundLookupStatement,
        qctx: Arc<QueryContext>,
        validated: &ValidatedStatement,
        selected_index: Option<&IndexMetadata>,
        is_edge: bool,
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

        // Extract scan limits from WHERE when an index is available
        let (scan_limits, scan_type) = if let Some(index) = selected_index {
            let limits = if let Some(ref where_ctx) = where_ctx {
                Self::extract_scan_limits_from_where(
                    &Some(where_ctx.clone()),
                    std::slice::from_ref(&index.field_name),
                )
            } else {
                Vec::new()
            };
            let scan_type = if limits.len() == 1 && limits[0].scan_type == ScanType::Unique {
                ScanType::Unique
            } else {
                ScanType::Range
            };
            (limits, scan_type)
        } else {
            (Vec::new(), ScanType::Full)
        };

        // Build physical scan node
        let mut current_node: PlanNodeEnum = match selected_index {
            Some(index) => {
                let mut index_scan_node = IndexScanNode::new(
                    space_id,
                    tag_id,
                    index.index_id,
                    index.index_name.clone(),
                    target_name.clone(),
                    scan_type,
                );
                index_scan_node.set_scan_limits(scan_limits);
                index_scan_node.set_col_names(vec![target_name.clone()]);
                if let Some(ref yield_clause) = lookup.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        index_scan_node.set_limit(limit_clause.count as i64);
                    }
                }
                PlanNodeEnum::IndexScan(index_scan_node)
            }
            None if is_edge => {
                let mut edge_scan_node = ScanEdgesNode::new(space_id, &target_name);
                edge_scan_node.set_col_names(vec![target_name.clone()]);
                if let Some(ref yield_clause) = lookup.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        edge_scan_node.set_limit(limit_clause.count as i64);
                    }
                }
                PlanNodeEnum::ScanEdges(edge_scan_node)
            }
            None => {
                let mut vertex_scan_node = ScanVerticesNode::new(space_id, &space_name);
                vertex_scan_node.set_col_names(vec![target_name.clone()]);
                vertex_scan_node.set_tag(&target_name);
                if let Some(ref yield_clause) = lookup.yield_clause {
                    if let Some(ref limit_clause) = yield_clause.limit {
                        vertex_scan_node.set_limit(limit_clause.count as i64);
                    }
                }
                PlanNodeEnum::ScanVertices(vertex_scan_node)
            }
        };

        // Build logical mirror
        let limit_from_yield = lookup
            .yield_clause
            .as_ref()
            .and_then(|yc| yc.limit.as_ref().map(|limit| limit.count as i64));
        let mut logical_root = if is_edge {
            LogicalNodeEnum::ScanEdges(LogicalScanEdgesNode {
                id: next_node_id(),
                space_id,
                edge_type: Some(target_name.clone()),
                expression: None,
                limit: limit_from_yield,
                projected_properties: vec![],
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: vec![],
            })
        } else {
            LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
                id: next_node_id(),
                space_id,
                space_name: space_name.clone(),
                tag: Some(target_name.clone()),
                expression: None,
                limit: limit_from_yield,
                projected_properties: vec![],
                output_var: None,
                col_names: vec![target_name.clone()],
                column_types: vec![],
            })
        };

        // Add filter node if WHERE clause present
        if let Some(ref condition) = where_ctx {
            let filter_node = FilterNode::new(current_node, condition.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create FilterNode: {}", e))
            })?;
            current_node = PlanNodeEnum::Filter(filter_node);

            let logical_filter = LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                condition: condition.clone(),
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            };
            logical_root = LogicalNodeEnum::Filter(logical_filter);
        }

        // Add project node if YIELD clause present
        if lookup.yield_clause.is_some() {
            let yield_columns = self.build_yield_columns_from_bound(lookup, validated)?;
            let project_node =
                ProjectNode::new(current_node, yield_columns.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create ProjectNode: {}",
                        e
                    ))
                })?;
            current_node = PlanNodeEnum::Project(project_node);

            let logical_project = LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                columns: yield_columns.clone(),
                output_var: None,
                col_names: yield_columns.iter().map(|col| col.alias.clone()).collect(),
                column_types: vec![],
            };
            logical_root = LogicalNodeEnum::Project(logical_project);
        }

        let arg_node = ArgumentNode::new(0, "lookup_input");
        let sub_plan = SubPlan {
            root: Some(current_node),
            tail: Some(PlanNodeEnum::Argument(arg_node)),
            logical_root: Some(logical_root),
        };

        Ok(sub_plan)
    }

    /// Build YIELD columns from bound statement (plan_bound path)
    fn build_yield_columns_from_bound(
        &self,
        lookup: &crate::binder::bound::BoundLookupStatement,
        validated: &ValidatedStatement,
    ) -> Result<Vec<graphdb_core::YieldColumn>, PlannerError> {
        let mut columns = Vec::new();

        if let Some(ref yield_clause) = lookup.yield_clause {
            for item in &yield_clause.items {
                let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                    &item.expression,
                    validated.expr_context(),
                )
                .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
                columns.push(graphdb_core::YieldColumn {
                    expression: ctx_expr,
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
        }

        if columns.is_empty() {
            let expr = Expression::Variable("_vertex".to_string());
            let meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
            let id = validated.expr_context().register_expression(meta);
            let ctx_expr = ContextualExpression::new(id, validated.expr_context().clone());
            columns.push(graphdb_core::YieldColumn {
                expression: ctx_expr,
                alias: "result".to_string(),
                is_matched: false,
            });
        }

        Ok(columns)
    }

    /// Construct the YIELD column (legacy transform path)
    fn build_yield_columns(
        lookup_stmt: &LookupStmt,
        validated: &ValidatedStatement,
    ) -> Result<Vec<graphdb_core::YieldColumn>, PlannerError> {
        let mut columns = Vec::new();

        if let Some(ref yield_clause) = lookup_stmt.yield_clause {
            for item in &yield_clause.items {
                columns.push(graphdb_core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
        }

        if columns.is_empty() {
            let expr = Expression::Variable("_vertex".to_string());
            let meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
            let id = validated.expr_context().register_expression(meta);
            let ctx_expr = graphdb_core::types::ContextualExpression::new(
                id,
                validated.expr_context().clone(),
            );
            columns.push(graphdb_core::YieldColumn {
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
