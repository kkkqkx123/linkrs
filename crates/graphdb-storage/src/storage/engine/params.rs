use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::Value;
use crate::storage::edge::EdgeStrategy;
use crate::storage::types::StoragePropertyDef;

/// Parameters for creating an edge type
pub struct CreateEdgeTypeParams<'a> {
    pub name: &'a str,  // Storage name (space_id:edge:user_name)
    pub user_name: &'a str,  // User-visible name (e.g., KNOWS)
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,
    pub ie_strategy: EdgeStrategy,
}

/// Parameters for edge operations.
pub struct EdgeOperationParams {
    pub edge_label: LabelId,
    pub src_label: LabelId,
    pub src_id: VertexId,
    pub dst_label: LabelId,
    pub dst_id: VertexId,
    pub rank: i64,
}

/// Parameters for insert_edge operation.
pub struct InsertEdgeParams<'a> {
    pub edge_label: LabelId,
    pub src_label: LabelId,
    pub src_id: VertexId,
    pub dst_label: LabelId,
    pub dst_id: VertexId,
    pub rank: i64,
    pub properties: &'a [(String, Value)],
    pub ts: Timestamp,
}
