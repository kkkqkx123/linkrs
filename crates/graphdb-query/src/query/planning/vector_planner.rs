//! Vector Search Planner
//!
//! This module contains the planner for vector search operations.

use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::query::metadata::MetadataContext;
use crate::query::parser::ast::vector::{
    CreateVectorIndex, DropVectorIndex, LookupVector, MatchVector, SearchVectorStatement,
    VectorYieldClause,
};
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::search::vector::data_access::{
    OutputField, PayloadIndexHint, PayloadIndexKind, VectorLookupNode, VectorMatchNode,
    VectorSearchNode,
};
use crate::query::planning::plan::core::nodes::search::vector::management::{
    CreateVectorIndexNode, CreateVectorIndexParams, DropVectorIndexNode,
};
use crate::query::planning::plan::core::nodes::search::vector::VectorSearchParams;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use vector_search::types::{ConditionType, FilterCondition, RangeCondition, VectorFilter};

/// Vector search planner
#[derive(Debug, Clone, Default)]
pub struct VectorSearchPlanner {
    /// Metadata context for pre-resolved metadata (optional for backward compatibility)
    metadata_context: Option<Arc<MetadataContext>>,
}

impl VectorSearchPlanner {
    pub fn new() -> Self {
        Self {
            metadata_context: None,
        }
    }

    /// Create a new vector search planner with metadata context
    pub fn with_metadata_context(metadata_context: Arc<MetadataContext>) -> Self {
        Self {
            metadata_context: Some(metadata_context),
        }
    }
}

impl Planner for VectorSearchPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let space_id = qctx.space_id().unwrap_or(0);

        match stmt {
            Stmt::CreateVectorIndex(create) => {
                self.transform_create_vector_index(create, &space_name, space_id)
            }
            Stmt::DropVectorIndex(drop) => self.transform_drop_vector_index(drop, &space_name),
            Stmt::SearchVector(search) => self.transform_search_vector(search, space_id),
            Stmt::LookupVector(lookup) => {
                self.transform_lookup_vector(lookup, space_id, &space_name)
            }
            Stmt::MatchVector(match_stmt) => self.transform_match_vector(match_stmt, space_id),
            _ => Err(PlannerError::PlanGenerationFailed(
                "Not a vector search statement".to_string(),
            )),
        }
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(
            stmt,
            Stmt::CreateVectorIndex(_)
                | Stmt::DropVectorIndex(_)
                | Stmt::SearchVector(_)
                | Stmt::LookupVector(_)
                | Stmt::MatchVector(_)
        )
    }

    fn transform_with_metadata(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let space_id = qctx.space_id().unwrap_or(0);

        match stmt {
            Stmt::CreateVectorIndex(create) => {
                self.transform_create_vector_index(create, &space_name, space_id)
            }
            Stmt::DropVectorIndex(drop) => {
                self.transform_drop_vector_index_with_metadata(drop, &space_name, metadata_context)
            }
            Stmt::SearchVector(search) => {
                self.transform_search_vector_with_metadata(search, space_id, metadata_context)
            }
            Stmt::LookupVector(lookup) => self.transform_lookup_vector_with_metadata(
                lookup,
                space_id,
                &space_name,
                metadata_context,
            ),
            Stmt::MatchVector(match_stmt) => {
                self.transform_match_vector_with_metadata(match_stmt, space_id, metadata_context)
            }
            _ => Err(PlannerError::PlanGenerationFailed(
                "Not a vector search statement".to_string(),
            )),
        }
    }
}

impl VectorSearchPlanner {
    fn transform_create_vector_index(
        &self,
        create: &CreateVectorIndex,
        space_name: &str,
        space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        let schema_name = if create.schema_name.is_empty() {
            space_name.to_string()
        } else {
            create.schema_name.clone()
        };

        let mut params = CreateVectorIndexParams::new(
            create.index_name.clone(),
            schema_name,
            create.schema_name.clone(),
            create.field_name.clone(),
            create.config.vector_size,
            create.config.distance,
            space_id,
        )
        .with_hnsw_m(create.config.hnsw_m)
        .with_hnsw_ef_construct(create.config.hnsw_ef_construct)
        .with_quantization(
            create.config.quantization,
            create.config.quantile,
            create.config.compression,
            create.config.always_ram,
        );
        if create.if_not_exists {
            params = params.with_if_not_exists();
        }
        let node = CreateVectorIndexNode::new(params);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_drop_vector_index(
        &self,
        drop: &DropVectorIndex,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let node = DropVectorIndexNode::new(
            drop.index_name.clone(),
            space_name.to_string(),
            drop.if_exists,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform DROP VECTOR INDEX with pre-resolved metadata.
    ///
    /// The coordinator's `drop_vector_index` API is addressed by
    /// `(space_id, tag_name, field_name)` instead of the logical index name,
    /// so the location is resolved here from index metadata. A missing index
    /// yields `IndexNotFound` unless `IF EXISTS` was given, in which case the
    /// node keeps an empty location and the executor turns it into a no-op
    /// status row.
    fn transform_drop_vector_index_with_metadata(
        &self,
        drop: &DropVectorIndex,
        space_name: &str,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        let node = match metadata_context.get_index_metadata(&drop.index_name) {
            Some(index_metadata) => DropVectorIndexNode::new(
                drop.index_name.clone(),
                space_name.to_string(),
                drop.if_exists,
            )
            .with_location(
                index_metadata.space_id,
                index_metadata.tag_name.clone(),
                index_metadata.field_name.clone(),
            ),
            None => {
                if drop.if_exists {
                    DropVectorIndexNode::new(
                        drop.index_name.clone(),
                        space_name.to_string(),
                        drop.if_exists,
                    )
                } else {
                    return Err(PlannerError::IndexNotFound(drop.index_name.clone()));
                }
            }
        };

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform SEARCH VECTOR statement into execution plan
    ///
    /// # Architecture Note
    /// This method now pre-resolves index metadata during the planning phase.
    /// The metadata_context is used to look up tag_name and field_name from the index_name.
    /// This allows for early error detection and better query optimization.
    fn transform_search_vector(
        &self,
        search: &SearchVectorStatement,
        space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        // Parse output fields from yield clause
        let output_fields = self.parse_output_fields(&search.yield_clause);

        // Convert WHERE clause to VectorFilter
        let filter = match search
            .where_clause
            .as_ref()
            .map(|where_clause| self.convert_where_clause_to_filter(where_clause))
        {
            Some(filter) => filter?,
            None => None,
        };

        // Pre-resolve tag_name and field_name from metadata context if available
        let (tag_name, field_name) = if let Some(ref metadata_context) = self.metadata_context {
            // Try to get index metadata from context
            if let Some(index_metadata) = metadata_context.get_index_metadata(&search.index_name) {
                (
                    index_metadata.tag_name.clone(),
                    index_metadata.field_name.clone(),
                )
            } else {
                // Metadata not pre-resolved, use empty strings (executor will resolve)
                (String::new(), String::new())
            }
        } else {
            // No metadata context, use empty strings (backward compatibility)
            (String::new(), String::new())
        };

        let node = self.build_vector_search_node(
            search,
            space_id,
            tag_name,
            field_name,
            filter,
            output_fields,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_lookup_vector(
        &self,
        lookup: &LookupVector,
        _space_id: u64,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let schema_name = if lookup.schema_name.is_empty() {
            space_name.to_string()
        } else {
            lookup.schema_name.clone()
        };

        let yield_fields = self.parse_output_fields(&lookup.yield_clause);

        // Pre-resolve tag_name and field_name from metadata context if
        // available; otherwise leave empty (executor will report a clear error).
        let (tag_name, field_name, resolved_space_id) =
            if let Some(ref metadata_context) = self.metadata_context {
                match metadata_context.get_index_metadata(&lookup.index_name) {
                    Some(index_metadata) => (
                        index_metadata.tag_name.clone(),
                        index_metadata.field_name.clone(),
                        index_metadata.space_id,
                    ),
                    None => (String::new(), String::new(), 0),
                }
            } else {
                (String::new(), String::new(), 0)
            };

        let node = VectorLookupNode::new(
            schema_name,
            lookup.index_name.clone(),
            lookup.query.clone(),
            yield_fields,
            lookup.limit.unwrap_or(10),
        )
        .with_metadata(resolved_space_id, tag_name, field_name);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_match_vector(
        &self,
        match_stmt: &MatchVector,
        _space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        let yield_fields = self.parse_output_fields(&match_stmt.yield_clause);

        let node = VectorMatchNode::new(
            match_stmt.pattern.clone(),
            match_stmt.vector_condition.field.clone(),
            match_stmt.vector_condition.query.clone(),
            match_stmt.vector_condition.threshold,
            yield_fields,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Parse output fields from yield clause
    fn parse_output_fields(&self, yield_clause: &Option<VectorYieldClause>) -> Vec<OutputField> {
        yield_clause
            .as_ref()
            .map(|yield_clause| {
                yield_clause
                    .items
                    .iter()
                    .map(|item| OutputField {
                        name: item
                            .expr
                            .get_expression()
                            .and_then(|inner| self.expression_to_field_name(&inner))
                            .unwrap_or_else(|| item.expr.to_expression_string()),
                        alias: item.alias.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Build VectorSearchNode with common parameters
    fn build_vector_search_node(
        &self,
        search: &SearchVectorStatement,
        space_id: u64,
        tag_name: String,
        field_name: String,
        filter: Option<VectorFilter>,
        output_fields: Vec<OutputField>,
    ) -> VectorSearchNode {
        let hints = filter
            .as_ref()
            .map(|f| self.classify_payload_index_hints(f))
            .unwrap_or_default();
        VectorSearchNode::new(
            VectorSearchParams::new(
                search.index_name.clone(),
                space_id,
                tag_name,
                field_name,
                search.query.clone(),
            )
            .with_threshold(search.threshold.unwrap_or(0.0))
            .with_filter(filter)
            .with_limit(search.limit.as_ref().map(|l| l.count).unwrap_or(10))
            .with_offset(search.skip.as_ref().map(|s| s.count).unwrap_or(0))
            .with_output_fields(output_fields)
            .with_payload_index_hints(hints),
        )
    }

    /// Static payload-index classification of a filter's `must` conditions.
    ///
    /// Conditions on equality-comparables (`Match` / `MatchAny`) map to the
    /// `MapIndex` kind and numeric ranges to the `NumericIndex` kind; every
    /// other condition stays a post-filter. The execution engine performs
    /// the actual index lookup — this annotation only documents which
    /// conditions *can* be served by an index if one is declared.
    fn classify_payload_index_hints(&self, filter: &VectorFilter) -> Vec<PayloadIndexHint> {
        let Some(must) = filter.must.as_ref() else {
            return Vec::new();
        };
        must.iter()
            .filter_map(|cond| {
                let kind = match &cond.condition {
                    ConditionType::Match { .. } | ConditionType::MatchAny { .. } => {
                        PayloadIndexKind::Map
                    }
                    ConditionType::Range(_) => PayloadIndexKind::Numeric,
                    _ => return None,
                };
                Some(PayloadIndexHint {
                    field: cond.field.clone(),
                    index_kind: kind,
                })
            })
            .collect()
    }

    /// Convert a contextual WHERE expression to VectorFilter
    ///
    /// This method transforms the contextual expression into a VectorFilter that can
    /// be used by the vector search engine (e.g., Qdrant).
    ///
    /// Returns `Ok(None)` when the WHERE clause carries no expression, and an
    /// error when the expression uses constructs that cannot be represented as
    /// a vector payload filter — silently dropping the predicate would widen
    /// the result set, so unsupported forms must fail loudly.
    fn convert_where_clause_to_filter(
        &self,
        where_clause: &ContextualExpression,
    ) -> Result<Option<VectorFilter>, PlannerError> {
        match where_clause.get_expression() {
            Some(expr) => self.convert_expression_to_filter(&expr).map(Some),
            None => Ok(None),
        }
    }

    /// Recursively convert an Expression to VectorFilter
    fn convert_expression_to_filter(
        &self,
        expr: &Expression,
    ) -> Result<VectorFilter, PlannerError> {
        let unsupported = |detail: String| Err(PlannerError::UnsupportedVectorFilter(detail));
        match expr {
            Expression::Binary { left, op, right } => match op {
                BinaryOperator::And => {
                    let left_filter = self.convert_expression_to_filter(left)?;
                    let right_filter = self.convert_expression_to_filter(right)?;

                    // Merge filters: AND means both conditions must be met
                    Ok(self.merge_filters_must(left_filter, right_filter))
                }
                BinaryOperator::Or => {
                    let left_filter = self.convert_expression_to_filter(left)?;
                    let right_filter = self.convert_expression_to_filter(right)?;

                    // Merge filters: OR means either condition can be met
                    Ok(self.merge_filters_should(left_filter, right_filter))
                }
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::LessThan
                | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterThanOrEqual => {
                    self.convert_comparison_to_filter(left, op, right)
                }
                other => unsupported(format!(
                    "unsupported operator in vector search WHERE clause: {}",
                    other
                )),
            },
            Expression::Unary {
                op: UnaryOperator::Not,
                operand,
            } => {
                let inner_filter = self.convert_expression_to_filter(operand)?;
                // Negate the filter: must_not
                Ok(self.negate_filter(inner_filter))
            }
            _ => unsupported(
                "vector search WHERE clause only supports comparisons combined with AND/OR/NOT"
                    .to_string(),
            ),
        }
    }

    /// Convert a comparison expression to VectorFilter
    fn convert_comparison_to_filter(
        &self,
        left: &Expression,
        op: &BinaryOperator,
        right: &Expression,
    ) -> Result<VectorFilter, PlannerError> {
        let field = self.expression_to_field_name(left).ok_or_else(|| {
            PlannerError::UnsupportedVectorFilter(
                "left side of a vector filter comparison must be a variable or property"
                    .to_string(),
            )
        })?;
        let value = match right {
            Expression::Literal(value) => value,
            _ => {
                return Err(PlannerError::UnsupportedVectorFilter(format!(
                    "right side of a vector filter comparison must be a literal, got {:?}",
                    right
                )))
            }
        };
        let value_str = self.value_to_string(value).ok_or_else(|| {
            PlannerError::UnsupportedVectorFilter(format!(
                "literal value {:?} cannot be used in a vector filter",
                value
            ))
        })?;

        let condition = match op {
            BinaryOperator::Equal => {
                FilterCondition::new(field, ConditionType::Match { value: value_str })
            }
            BinaryOperator::NotEqual => {
                // For Not Equal, we use must_not with Match
                let filter = VectorFilter::new().must_not(FilterCondition::new(
                    field,
                    ConditionType::Match { value: value_str },
                ));
                return Ok(filter);
            }
            BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual => {
                // Range condition; non-numeric literals are rejected instead of dropped.
                let range = self.create_range_condition(op, &value_str).ok_or_else(|| {
                    PlannerError::UnsupportedVectorFilter(format!(
                        "range comparison requires a numeric literal, got {:?}",
                        value_str
                    ))
                })?;
                FilterCondition::new(field, ConditionType::Range(range))
            }
            _ => {
                return Err(PlannerError::UnsupportedVectorFilter(format!(
                    "unsupported comparison operator in vector filter: {}",
                    op
                )))
            }
        };

        Ok(VectorFilter::new().must(condition))
    }

    /// Convert an expression to a payload field name
    fn expression_to_field_name(&self, expr: &Expression) -> Option<String> {
        match expr {
            Expression::Variable(name) => Some(name.clone()),
            Expression::Property { object, property } => {
                let object_name = match object.as_ref() {
                    Expression::Variable(name) => name.clone(),
                    _ => return None,
                };
                Some(format!("{}.{}", object_name, property))
            }
            _ => None,
        }
    }

    /// Create RangeCondition from comparison operator and value
    fn create_range_condition(
        &self,
        op: &BinaryOperator,
        value_str: &str,
    ) -> Option<RangeCondition> {
        let mut range = RangeCondition::new();

        match op {
            BinaryOperator::LessThan => {
                range.lt = Some(value_str.parse().ok()?);
            }
            BinaryOperator::LessThanOrEqual => {
                range.lte = Some(value_str.parse().ok()?);
            }
            BinaryOperator::GreaterThan => {
                range.gt = Some(value_str.parse().ok()?);
            }
            BinaryOperator::GreaterThanOrEqual => {
                range.gte = Some(value_str.parse().ok()?);
            }
            _ => return None,
        }

        Some(range)
    }

    /// Convert core::Value to String for filter conditions
    fn value_to_string(&self, value: &crate::core::Value) -> Option<String> {
        match value {
            crate::core::Value::String(s) => Some(s.to_string()),
            crate::core::Value::Int(i) => Some(i.to_string()),
            crate::core::Value::Float(f) => Some(f.to_string()),
            crate::core::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Merge two filters with AND logic (must)
    fn merge_filters_must(&self, left: VectorFilter, right: VectorFilter) -> VectorFilter {
        let mut result = VectorFilter::new();

        // Add all must conditions from left
        if let Some(must) = left.must {
            for condition in must {
                result = result.must(condition);
            }
        }

        // Add all must conditions from right
        if let Some(must) = right.must {
            for condition in must {
                result = result.must(condition);
            }
        }

        // Add all must_not conditions from left
        if let Some(must_not) = left.must_not {
            for condition in must_not {
                result = result.must_not(condition);
            }
        }

        // Add all must_not conditions from right
        if let Some(must_not) = right.must_not {
            for condition in must_not {
                result = result.must_not(condition);
            }
        }

        result
    }

    /// Merge two filters with OR logic (should)
    fn merge_filters_should(&self, left: VectorFilter, right: VectorFilter) -> VectorFilter {
        let mut result = VectorFilter::new();

        // Add all must conditions from left as should
        if let Some(must) = left.must {
            for condition in must {
                result = result.should(condition);
            }
        }

        // Add all must conditions from right as should
        if let Some(must) = right.must {
            for condition in must {
                result = result.should(condition);
            }
        }

        result
    }

    /// Negate a filter (convert to must_not)
    fn negate_filter(&self, filter: VectorFilter) -> VectorFilter {
        let mut result = VectorFilter::new();

        // Convert must to must_not
        if let Some(must) = filter.must {
            for condition in must {
                result = result.must_not(condition);
            }
        }

        // Convert must_not to must
        if let Some(must_not) = filter.must_not {
            for condition in must_not {
                result = result.must(condition);
            }
        }

        result
    }

    /// Transform SEARCH VECTOR with pre-resolved metadata
    fn transform_search_vector_with_metadata(
        &self,
        search: &SearchVectorStatement,
        space_id: u64,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Parse output fields from yield clause
        let output_fields = self.parse_output_fields(&search.yield_clause);

        // Convert WHERE clause to VectorFilter
        let filter = match search
            .where_clause
            .as_ref()
            .map(|where_clause| self.convert_where_clause_to_filter(where_clause))
        {
            Some(filter) => filter?,
            None => None,
        };

        // Pre-resolve tag_name and field_name from metadata context
        let (tag_name, field_name) = match metadata_context.get_index_metadata(&search.index_name) {
            Some(index_metadata) => (
                index_metadata.tag_name.clone(),
                index_metadata.field_name.clone(),
            ),
            None => {
                return Err(PlannerError::IndexNotFound(search.index_name.clone()));
            }
        };

        let node = self.build_vector_search_node(
            search,
            space_id,
            tag_name,
            field_name,
            filter,
            output_fields,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform LOOKUP VECTOR with pre-resolved metadata
    fn transform_lookup_vector_with_metadata(
        &self,
        lookup: &LookupVector,
        _space_id: u64,
        space_name: &str,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // LOOKUP VECTOR executes through the same search path as SEARCH
        // VECTOR, so the index location must be fully resolved here.
        let (resolved_space_id, tag_name, field_name) =
            match metadata_context.get_index_metadata(&lookup.index_name) {
                Some(index_metadata) => (
                    index_metadata.space_id,
                    index_metadata.tag_name.clone(),
                    index_metadata.field_name.clone(),
                ),
                None => {
                    return Err(PlannerError::IndexNotFound(lookup.index_name.clone()));
                }
            };

        let schema_name = if lookup.schema_name.is_empty() {
            space_name.to_string()
        } else {
            lookup.schema_name.clone()
        };

        let yield_fields = self.parse_output_fields(&lookup.yield_clause);

        let node = VectorLookupNode::new(
            schema_name,
            lookup.index_name.clone(),
            lookup.query.clone(),
            yield_fields,
            lookup.limit.unwrap_or(10),
        )
        .with_metadata(resolved_space_id, tag_name, field_name);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform MATCH VECTOR with pre-resolved metadata
    fn transform_match_vector_with_metadata(
        &self,
        match_stmt: &MatchVector,
        space_id: u64,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate that the field exists in metadata context if index info is available
        // Note: MatchVector uses direct field reference rather than index name
        // so we perform a basic validation that the field is not empty
        if match_stmt.vector_condition.field.is_empty() {
            return Err(PlannerError::InvalidOperation(
                "Vector field name cannot be empty".to_string(),
            ));
        }

        let yield_fields = self.parse_output_fields(&match_stmt.yield_clause);

        // Try to find vector index metadata for the field
        let mut resolved_space_id = space_id;
        let mut resolved_tag_name = String::new();
        let mut resolved_field_name = String::new();

        // Look for a vector index that matches the field
        if let Some(index_metadata) = metadata_context
            .find_vector_index_by_field(space_id, &match_stmt.vector_condition.field)
        {
            resolved_space_id = index_metadata.space_id;
            resolved_tag_name = index_metadata.tag_name.clone();
            resolved_field_name = index_metadata.field_name.clone();
        }

        let node = VectorMatchNode::new(
            match_stmt.pattern.clone(),
            match_stmt.vector_condition.field.clone(),
            match_stmt.vector_condition.query.clone(),
            match_stmt.vector_condition.threshold,
            yield_fields,
        )
        .with_metadata(resolved_space_id, resolved_tag_name, resolved_field_name);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::{create_contextual_expression, Expression};
    use crate::core::types::span::Span;
    use crate::core::value::Value;
    use crate::query::parser::ast::vector::{
        VectorIndexConfig, VectorYieldClause, VectorYieldItem,
    };

    #[test]
    fn test_vector_search_planner_new() {
        let planner = VectorSearchPlanner::new();
        assert!(planner.metadata_context.is_none());
    }

    #[test]
    fn test_vector_search_planner_with_metadata() {
        let metadata_context = Arc::new(MetadataContext::new());
        let planner = VectorSearchPlanner::with_metadata_context(metadata_context);
        assert!(planner.metadata_context.is_some());
    }

    #[test]
    fn test_match_planner() {
        let planner = VectorSearchPlanner::new();

        let create_stmt = Stmt::CreateVectorIndex(CreateVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            schema_name: "tag".to_string(),
            field_name: "vec".to_string(),
            config: VectorIndexConfig::new(
                128,
                crate::query::parser::ast::vector::VectorDistance::Cosine,
            ),
            if_not_exists: false,
        });
        assert!(planner.match_planner(&create_stmt));

        let drop_stmt = Stmt::DropVectorIndex(DropVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            if_exists: false,
        });
        assert!(planner.match_planner(&drop_stmt));
    }

    #[test]
    fn test_parse_output_fields() {
        let planner = VectorSearchPlanner::new();

        // Test with None
        let fields = planner.parse_output_fields(&None);
        assert!(fields.is_empty());

        // Test with Some
        let yield_clause = VectorYieldClause {
            items: vec![
                VectorYieldItem {
                    expr: create_contextual_expression(Expression::variable("field1")),
                    alias: Some("f1".to_string()),
                },
                VectorYieldItem {
                    expr: create_contextual_expression(Expression::variable("field2")),
                    alias: None,
                },
            ],
        };
        let fields = planner.parse_output_fields(&Some(yield_clause));
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "field1");
        assert_eq!(fields[0].alias, Some("f1".to_string()));
        assert_eq!(fields[1].name, "field2");
        assert_eq!(fields[1].alias, None);
    }

    #[test]
    fn test_value_to_string() {
        let planner = VectorSearchPlanner::new();

        assert_eq!(
            planner.value_to_string(&crate::core::Value::string("test")),
            Some("test".to_string())
        );
        assert_eq!(
            planner.value_to_string(&crate::core::Value::Int(42)),
            Some("42".to_string())
        );
        assert_eq!(
            planner.value_to_string(&crate::core::Value::Float(std::f32::consts::PI)),
            Some(format!("{}", std::f32::consts::PI))
        );
        assert_eq!(
            planner.value_to_string(&crate::core::Value::Bool(true)),
            Some("true".to_string())
        );
    }

    fn comparison(field: &str, op: BinaryOperator, value: Expression) -> Expression {
        Expression::Binary {
            left: Box::new(Expression::variable(field)),
            op,
            right: Box::new(value),
        }
    }

    #[test]
    fn filter_conversion_accepts_supported_forms() {
        let planner = VectorSearchPlanner::new();

        let equal = comparison(
            "status",
            BinaryOperator::Equal,
            Expression::Literal(Value::string("active")),
        );
        let filter = planner.convert_expression_to_filter(&equal).unwrap();
        assert!(filter.must.is_some());

        let range = comparison(
            "age",
            BinaryOperator::GreaterThanOrEqual,
            Expression::Literal(Value::Int(18)),
        );
        let filter = planner.convert_expression_to_filter(&range).unwrap();
        assert!(filter.must.is_some());

        let conjunction = Expression::Binary {
            left: Box::new(equal),
            op: BinaryOperator::And,
            right: Box::new(range),
        };
        planner
            .convert_expression_to_filter(&conjunction)
            .expect("AND of supported comparisons must convert");

        let negation = Expression::Unary {
            op: UnaryOperator::Not,
            operand: Box::new(comparison(
                "status",
                BinaryOperator::Equal,
                Expression::Literal(Value::string("inactive")),
            )),
        };
        let filter = planner.convert_expression_to_filter(&negation).unwrap();
        assert!(filter.must_not.is_some() || filter.must.is_some());
    }

    #[test]
    fn filter_conversion_rejects_unsupported_operator() {
        let planner = VectorSearchPlanner::new();

        let like = comparison(
            "name",
            BinaryOperator::Like,
            Expression::Literal(Value::string("%a%")),
        );
        let error = planner
            .convert_expression_to_filter(&like)
            .expect_err("LIKE must be rejected instead of silently dropped");
        assert!(matches!(error, PlannerError::UnsupportedVectorFilter(_)));

        let list = comparison(
            "status",
            BinaryOperator::In,
            Expression::Literal(Value::string("a")),
        );
        let error = planner
            .convert_expression_to_filter(&list)
            .expect_err("IN must be rejected instead of silently dropped");
        assert!(matches!(error, PlannerError::UnsupportedVectorFilter(_)));
    }

    #[test]
    fn filter_conversion_rejects_non_literal_rhs() {
        let planner = VectorSearchPlanner::new();

        let variable_rhs = comparison("age", BinaryOperator::Equal, Expression::variable("limit"));
        let error = planner
            .convert_expression_to_filter(&variable_rhs)
            .expect_err("non-literal right side must be rejected");
        assert!(matches!(error, PlannerError::UnsupportedVectorFilter(_)));

        let non_field_lhs = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let error = planner
            .convert_expression_to_filter(&non_field_lhs)
            .expect_err("literal left side must be rejected");
        assert!(matches!(error, PlannerError::UnsupportedVectorFilter(_)));
    }

    #[test]
    fn filter_conversion_rejects_non_numeric_range_literal() {
        let planner = VectorSearchPlanner::new();

        let range = comparison(
            "age",
            BinaryOperator::LessThan,
            Expression::Literal(Value::string("abc")),
        );
        let error = planner
            .convert_expression_to_filter(&range)
            .expect_err("non-numeric range literal must be rejected instead of dropped");
        assert!(matches!(error, PlannerError::UnsupportedVectorFilter(_)));
    }

    #[test]
    fn payload_index_hints_classify_must_conditions() {
        use vector_search::types::ConditionType as CT;

        let filter = VectorFilter::new()
            .must(FilterCondition::new(
                "color",
                CT::Match {
                    value: "red".to_string(),
                },
            ))
            .must(FilterCondition::new(
                "price",
                CT::Range(RangeCondition::new().lt(100.0)),
            ));
        let hints = planner_hints(&filter);
        assert_eq!(
            hints,
            vec![
                PayloadIndexHint {
                    field: "color".to_string(),
                    index_kind: PayloadIndexKind::Map,
                },
                PayloadIndexHint {
                    field: "price".to_string(),
                    index_kind: PayloadIndexKind::Numeric,
                },
            ]
        );

        // Unsupported conditions (IsEmpty) are never hinted.
        let filter = VectorFilter::new().must(FilterCondition::new("tag", CT::IsEmpty));
        assert!(planner_hints(&filter).is_empty());
    }

    fn planner_hints(filter: &VectorFilter) -> Vec<PayloadIndexHint> {
        VectorSearchPlanner::new().classify_payload_index_hints(filter)
    }
}
