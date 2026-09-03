//! Immutable configuration for sink (data modification) operators.

use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

/// Copy target type for COPY FROM
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget {
    Vertex(String),
    Edge(String),
}

/// Immutable config for sink (data modification) operators.
#[derive(Debug, Clone)]
pub enum SinkSpec {
    InsertVertices {
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
        /// Property column names for each tag, aligned with `tags`.
        tag_property_names: Vec<Vec<String>>,
        if_not_exists: bool,
    },
    InsertEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
        if_not_exists: bool,
    },
    UpdateVertices {
        space_name: String,
        tag_name: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
    },
    UpdateEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
    },
    DeleteVertices {
        space_name: String,
        vertex_id_col: String,
    },
    DeleteEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
    },
    PipeDeleteVertices {
        space_name: String,
        vertex_id_col: String,
    },
    PipeDeleteEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
    },
    DeleteTags {
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
    },
    CopyFrom {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: u8,
        batch_size: usize,
    },
    CopyTo {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: u8,
    },
}
