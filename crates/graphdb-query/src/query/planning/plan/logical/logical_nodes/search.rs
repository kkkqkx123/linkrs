//! Logical search nodes: FulltextSearch, FulltextLookup, MatchFulltext, VectorSearch, VectorLookup, VectorMatch.

use crate::define_logical_plan_node;
use crate::query::parser::ast::fulltext::{
    FulltextMatchCondition, FulltextQueryExpr, FulltextYieldClause, OrderClause, WhereClause,
};

define_logical_plan_node! {
    pub struct LogicalFulltextSearchNode {
        index_name: String,
        query: FulltextQueryExpr,
        yield_clause: Option<FulltextYieldClause>,
        where_clause: Option<WhereClause>,
        order_clause: Option<OrderClause>,
        limit: Option<usize>,
        offset: Option<usize>,
        space_id: u64,
        tag_name: String,
        field_name: String,
    }
    enum: FulltextSearch
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalFulltextLookupNode {
        schema_name: String,
        index_name: String,
        query: String,
        yield_clause: Option<FulltextYieldClause>,
        limit: Option<usize>,
        space_id: u64,
        tag_name: String,
        field_name: String,
    }
    enum: FulltextLookup
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalMatchFulltextNode {
        pattern: String,
        fulltext_condition: FulltextMatchCondition,
        yield_clause: Option<FulltextYieldClause>,
        space_id: u64,
        tag_name: String,
        field_name: String,
    }
    enum: MatchFulltext
    input: ZeroInputNode
}

#[cfg(feature = "qdrant")]
mod vector {
    use crate::define_logical_plan_node;
    use crate::query::parser::ast::vector::VectorQueryExpr;
    use crate::query::planning::plan::core::nodes::search::vector::data_access::{
        OutputField, VectorFilter,
    };

    define_logical_plan_node! {
        pub struct LogicalVectorSearchNode {
            index_name: String,
            space_id: u64,
            tag_name: String,
            field_name: String,
            query: VectorQueryExpr,
            threshold: Option<f32>,
            filter: Option<VectorFilter>,
            limit: usize,
            offset: usize,
            output_fields: Vec<OutputField>,
            metadata_version: u64,
        }
        enum: VectorSearch
        input: ZeroInputNode
    }

    define_logical_plan_node! {
        pub struct LogicalVectorLookupNode {
            schema_name: String,
            index_name: String,
            query: VectorQueryExpr,
            yield_fields: Vec<OutputField>,
            limit: usize,
        }
        enum: VectorLookup
        input: ZeroInputNode
    }

    define_logical_plan_node! {
        pub struct LogicalVectorMatchNode {
            pattern: String,
            field: String,
            query: VectorQueryExpr,
            threshold: Option<f32>,
            yield_fields: Vec<OutputField>,
            space_id: u64,
            tag_name: String,
            field_name: String,
        }
        enum: VectorMatch
        input: ZeroInputNode
    }
}

#[cfg(feature = "qdrant")]
pub use vector::*;
