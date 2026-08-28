//! Vector Search Data Access Plan Nodes
//!
//! This module defines plan nodes for vector search data access operations.

use crate::parser::ast::vector::VectorQueryExpr;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, ZeroInputNode};
use serde::{Deserialize, Serialize};
#[cfg(feature = "vector")]
pub use vector_search::types::VectorFilter;

#[cfg(not(feature = "vector"))]
#[derive(Debug, Clone)]
pub struct VectorFilter;

/// Output field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputField {
    pub name: String,
    pub alias: Option<String>,
}

/// Parameters for creating a vector search node
#[derive(Debug, Clone)]
pub struct VectorSearchParams {
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query: VectorQueryExpr,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
    pub limit: usize,
    pub offset: usize,
    pub output_fields: Vec<OutputField>,
    pub metadata_version: u64,
    /// Static payload-index acceleration hints derived from the filter by
    /// the planner (explain output / engine-side planning input).
    pub payload_index_hints: Vec<PayloadIndexHint>,
}

/// One `must` condition the planner classified as payload-index acceleratable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadIndexHint {
    /// Payload field of the condition.
    pub field: String,
    /// Index kind expected to serve it at execution time.
    pub index_kind: PayloadIndexKind,
}

/// Kinds of per-field payload indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadIndexKind {
    /// Equality (`Match` / `MatchAny`) via a posting-list map index.
    Map,
    /// Numeric equality or range via a sorted-value index.
    Numeric,
}

impl VectorSearchParams {
    pub fn new(
        index_name: String,
        space_id: u64,
        tag_name: String,
        field_name: String,
        query: VectorQueryExpr,
    ) -> Self {
        Self {
            index_name,
            space_id,
            tag_name,
            field_name,
            query,
            threshold: None,
            filter: None,
            limit: 10,
            offset: 0,
            output_fields: Vec::new(),
            metadata_version: 0,
            payload_index_hints: Vec::new(),
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = Some(threshold);
        self
    }

    pub fn with_filter(mut self, filter: Option<VectorFilter>) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    pub fn with_output_fields(mut self, fields: Vec<OutputField>) -> Self {
        self.output_fields = fields;
        self
    }

    pub fn with_metadata_version(mut self, version: u64) -> Self {
        self.metadata_version = version;
        self
    }

    /// Attach planner-computed payload index hints.
    pub fn with_payload_index_hints(mut self, hints: Vec<PayloadIndexHint>) -> Self {
        self.payload_index_hints = hints;
        self
    }
}

/// Vector search plan node
#[derive(Debug, Clone)]
pub struct VectorSearchNode {
    id: i64,
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query: VectorQueryExpr,
    pub threshold: Option<f32>,
    /// Vector filter for payload filtering (e.g., WHERE clause conditions)
    pub filter: Option<VectorFilter>,
    pub limit: usize,
    pub offset: usize,
    pub output_fields: Vec<OutputField>,
    /// Metadata version for validation (0 if not tracked)
    pub metadata_version: u64,
    /// Planner-computed payload index acceleration hints (explain output).
    pub payload_index_hints: Vec<PayloadIndexHint>,
}

impl VectorSearchNode {
    pub fn new(params: VectorSearchParams) -> Self {
        Self {
            id: next_node_id(),
            index_name: params.index_name,
            space_id: params.space_id,
            tag_name: params.tag_name,
            field_name: params.field_name,
            query: params.query,
            threshold: params.threshold,
            filter: params.filter,
            limit: params.limit,
            offset: params.offset,
            output_fields: params.output_fields,
            metadata_version: params.metadata_version,
            payload_index_hints: params.payload_index_hints,
        }
    }

    /// One-line explain summary of the pre-filter plan:
    /// accelerated conditions and the must conditions left as post-filters.
    pub fn explain_filter(&self) -> String {
        let Some(filter) = self.filter.as_ref() else {
            return "filter: none".to_string();
        };
        let must_count = filter.must.as_ref().map_or(0, Vec::len);
        if must_count == 0 {
            return "filter: no acceleratable must conditions".to_string();
        }
        format!(
            "pre_filter_indexes: [{}], post_filter_conditions: {}",
            self.payload_index_hints
                .iter()
                .map(|h| format!(
                    "{}={}",
                    h.field,
                    match h.index_kind {
                        PayloadIndexKind::Map => "MapIndex",
                        PayloadIndexKind::Numeric => "NumericIndex",
                    }
                ))
                .collect::<Vec<_>>()
                .join(", "),
            must_count - self.payload_index_hints.len()
        )
    }
}

impl PlanNode for VectorSearchNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "VectorSearch"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
    }

    fn output_var(&self) -> Option<&str> {
        None
    }

    fn col_names(&self) -> &[String] {
        &[]
    }

    fn set_output_var(&mut self, _var: String) {}

    fn set_col_names(&mut self, _names: Vec<String>) {}

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::VectorSearch(self)
    }
}

impl ZeroInputNode for VectorSearchNode {}

/// Lookup vector plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorLookupNode {
    id: i64,
    pub schema_name: String,
    pub index_name: String,
    pub query: VectorQueryExpr,
    pub yield_fields: Vec<OutputField>,
    pub limit: usize,
    /// Pre-resolved space id from index metadata (0 when unresolved).
    pub space_id: u64,
    /// Pre-resolved tag name from index metadata (empty when unresolved).
    pub tag_name: String,
    /// Pre-resolved field name from index metadata (empty when unresolved).
    pub field_name: String,
}

impl VectorLookupNode {
    pub fn new(
        schema_name: String,
        index_name: String,
        query: VectorQueryExpr,
        yield_fields: Vec<OutputField>,
        limit: usize,
    ) -> Self {
        Self {
            id: next_node_id(),
            schema_name,
            index_name,
            query,
            yield_fields,
            limit,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    /// Attach the index location resolved from metadata during planning so
    /// the lookup executes through the same `(space_id, tag, field)` address
    /// as a vector search.
    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }
}

impl PlanNode for VectorLookupNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "VectorLookup"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
    }

    fn output_var(&self) -> Option<&str> {
        None
    }

    fn col_names(&self) -> &[String] {
        &[]
    }

    fn set_output_var(&mut self, _var: String) {}

    fn set_col_names(&mut self, _names: Vec<String>) {}

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::VectorLookup(self)
    }
}

impl ZeroInputNode for VectorLookupNode {}

/// Match vector plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMatchNode {
    id: i64,
    pub pattern: String,
    pub field: String,
    pub query: VectorQueryExpr,
    pub threshold: Option<f32>,
    pub yield_fields: Vec<OutputField>,
    /// Pre-resolved space_id from metadata context
    pub space_id: u64,
    /// Pre-resolved tag_name from index metadata
    pub tag_name: String,
    /// Pre-resolved field_name from index metadata
    pub field_name: String,
}

impl VectorMatchNode {
    pub fn new(
        pattern: String,
        field: String,
        query: VectorQueryExpr,
        threshold: Option<f32>,
        yield_fields: Vec<OutputField>,
    ) -> Self {
        Self {
            id: next_node_id(),
            pattern,
            field,
            query,
            threshold,
            yield_fields,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }
}

impl PlanNode for VectorMatchNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "VectorMatch"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
    }

    fn output_var(&self) -> Option<&str> {
        None
    }

    fn col_names(&self) -> &[String] {
        &[]
    }

    fn set_output_var(&mut self, _var: String) {}

    fn set_col_names(&mut self, _names: Vec<String>) {}

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::VectorMatch(self)
    }
}

impl ZeroInputNode for VectorMatchNode {}

impl MemoryEstimatable for VectorLookupNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.schema_name.capacity()
            + self.index_name.capacity()
            + self
                .yield_fields
                .iter()
                .map(|f| {
                    std::mem::size_of::<OutputField>()
                        + f.name.capacity()
                        + f.alias.as_ref().map_or(0, |a| a.capacity())
                })
                .sum::<usize>()
    }
}

impl MemoryEstimatable for VectorMatchNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.pattern.capacity()
            + self.field.capacity()
            + self
                .yield_fields
                .iter()
                .map(|f| {
                    std::mem::size_of::<OutputField>()
                        + f.name.capacity()
                        + f.alias.as_ref().map_or(0, |a| a.capacity())
                })
                .sum::<usize>()
    }
}

impl MemoryEstimatable for VectorSearchNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.index_name.capacity()
            + self.tag_name.capacity()
            + self.field_name.capacity()
            + self
                .output_fields
                .iter()
                .map(|f| {
                    std::mem::size_of::<OutputField>()
                        + f.name.capacity()
                        + f.alias.as_ref().map_or(0, |a| a.capacity())
                })
                .sum::<usize>()
    }
}
