//! Web Module Data Models

use serde::{Deserialize, Serialize};

pub mod metadata;
pub mod schema;

/// Standard API response wrapper
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ApiError>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// Pagination parameters
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct PaginationParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_offset")]
    pub offset: usize,
}

pub(crate) fn default_limit() -> usize {
    20
}

pub(crate) fn default_offset() -> usize {
    0
}

/// Paginated response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

impl<T> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total: i64, limit: usize, offset: usize) -> Self {
        Self {
            items,
            total,
            limit,
            offset,
        }
    }
}

/// Vertex page with schema overlay.
///
/// Runtime rows stay plain JSON values; the schema exposes them as maps of
/// the restricted attribute value so documents show candidate branches
/// instead of an unknown value.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VertexPage {
    #[schema(value_type = Vec<std::collections::HashMap<String, linkrs_wire::query::CellValue>>)]
    pub items: Vec<serde_json::Value>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

/// Edge page with schema overlay.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EdgePage {
    #[schema(value_type = Vec<std::collections::HashMap<String, linkrs_wire::query::CellValue>>)]
    pub items: Vec<serde_json::Value>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

/// Vertex detail with schema overlay.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VertexDetailData {
    #[schema(value_type = linkrs_wire::query::CellValue)]
    pub vertex: serde_json::Value,
}

/// Edge detail with schema overlay.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EdgeDetailData {
    #[schema(value_type = linkrs_wire::query::CellValue)]
    pub edge: serde_json::Value,
}

/// Neighbor list with schema overlay.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NeighborListData {
    pub vid: String,
    pub space: String,
    pub direction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_type: Option<String>,
    #[schema(value_type = Vec<linkrs_wire::query::CellValue>)]
    pub neighbors: Vec<serde_json::Value>,
}
