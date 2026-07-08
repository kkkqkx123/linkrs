//! Schema Extension Models

use serde::{Deserialize, Serialize};

/// Space detail response
#[derive(Debug, Serialize)]
pub struct SpaceDetail {
    pub id: u64,
    pub name: String,
    pub vid_type: String,
    pub partition_num: i32,
    pub replica_factor: i32,
    pub comment: Option<String>,
    pub created_at: i64,
    pub statistics: SpaceStatistics,
}

/// Space statistics
#[derive(Debug, Serialize)]
pub struct SpaceStatistics {
    pub tag_count: i64,
    pub edge_type_count: i64,
    pub index_count: i64,
    pub estimated_vertex_count: i64,
    pub estimated_edge_count: i64,
}

/// Tag summary
#[derive(Debug, Serialize)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub property_count: i64,
    pub index_count: i64,
    pub created_at: i64,
}

/// Tag detail
#[derive(Debug, Serialize)]
pub struct TagDetail {
    pub id: i64,
    pub name: String,
    pub properties: Vec<PropertyDef>,
    pub indexes: Vec<IndexInfo>,
    pub created_at: i64,
}

/// Edge type summary
#[derive(Debug, Serialize)]
pub struct EdgeTypeSummary {
    pub id: i64,
    pub name: String,
    pub property_count: i64,
    pub index_count: i64,
    pub created_at: i64,
}

/// Edge type detail
#[derive(Debug, Serialize)]
pub struct EdgeTypeDetail {
    pub id: i64,
    pub name: String,
    pub properties: Vec<PropertyDef>,
    pub indexes: Vec<IndexInfo>,
    pub created_at: i64,
}

/// Property definition
#[derive(Debug, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
}

/// Index information
#[derive(Debug, Serialize)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub index_type: String, // INDEX, UNIQUE, FULLTEXT
    pub fields: Vec<String>,
    pub status: String, // ACTIVE, BUILDING, FAILED
    pub progress: Option<i32>,
    pub created_at: i64,
}

/// Create index request
#[derive(Debug, Deserialize)]
pub struct CreateIndexRequest {
    pub name: String,
    pub index_type: String,
    pub entity_type: String, // TAG or EDGE
    pub entity_name: String,
    pub fields: Vec<String>,
    pub comment: Option<String>,
}

/// Update tag request
#[derive(Debug, Deserialize)]
pub struct UpdateTagRequest {
    pub add_properties: Option<Vec<PropertyDef>>,
    pub drop_properties: Option<Vec<String>>,
}

/// Update edge type request
#[derive(Debug, Deserialize)]
pub struct UpdateEdgeTypeRequest {
    pub add_properties: Option<Vec<PropertyDef>>,
    pub drop_properties: Option<Vec<String>>,
}
