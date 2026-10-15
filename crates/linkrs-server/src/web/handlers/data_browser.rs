//! Data Browser Handlers
//!
//! Provides data browsing APIs:
//! - Browse vertices by tag
//! - Browse edges by edge type
//! - Data filtering and pagination

use axum::{
    extract::{Extension, Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::WebResult,
    handlers::common::execute_in_space,
    models::{ApiResponse, PaginatedResponse},
    WebState,
};
use linkrs_core::Value;

/// Create data browser routes (without state)
pub fn create_routes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>() -> Router<WebState<S>> {
    Router::new()
        .route(
            "/spaces/{name}/tags/{tag_name}/vertices",
            get(list_vertices_by_tag),
        )
        .route(
            "/spaces/{name}/edge-types/{edge_name}/edges",
            get(list_edges_by_type),
        )
}

/// Filter parameters for data browsing
///
/// The pagination fields are declared inline rather than flattened into
/// `PaginationParams`: the query-string deserializer buffers flattened fields
/// as text, which rejects `limit=10` as `invalid type: string, expected usize`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DataFilterParams {
    #[serde(default = "crate::web::models::default_limit")]
    pub limit: usize,
    #[serde(default = "crate::web::models::default_offset")]
    pub offset: usize,
    /// Property filter (e.g., "age>18")
    pub filter: Option<String>,
    /// Sort field
    pub sort_by: Option<String>,
    /// Sort order
    pub sort_order: Option<String>,
}

/// List vertices by tag
#[utoipa::path(
    get,
    path = "/api/v1/data/spaces/{name}/tags/{tag_name}/vertices",
    tag = "WebData",
    params(
        ("name" = String, Path, description = "Space name"),
        ("tag_name" = String, Path, description = "Tag name"),
        ("limit" = usize, Query, description = "Page size"),
        ("offset" = usize, Query, description = "Page offset"),
        ("filter" = Option<String>, Query, description = "Property filter expression"),
        ("sort_by" = Option<String>, Query, description = "Sort field"),
        ("sort_order" = Option<String>, Query, description = "Sort order")
    ),
    responses(
        (status = 200, description = "Vertex page", body = ApiResponse<PaginatedResponse<serde_json::Value>>),
        (status = 500, description = "Internal error")
    )
)]
async fn list_vertices_by_tag<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Extension(caller_session_id): Extension<i64>,
    Path((space_name, tag_name)): Path<(String, String)>,
    Query(params): Query<DataFilterParams>,
) -> WebResult<Json<ApiResponse<PaginatedResponse<serde_json::Value>>>> {
    // Build filter clause for both count and data queries
    let filter_clause = params
        .filter
        .as_ref()
        .map(|f| format!(" WHERE {}", f))
        .unwrap_or_default();
    let sort_clause = params
        .sort_by
        .as_ref()
        .map(|s| {
            let order = params
                .sort_order
                .clone()
                .unwrap_or_else(|| "ASC".to_string());
            format!(" ORDER BY {} {}", s, order)
        })
        .unwrap_or_default();

    // First, get total count
    let count_query = format!(
        "MATCH (v:{}) RETURN COUNT(v) as total{}",
        tag_name, filter_clause
    );

    let total = execute_in_space(&web_state, caller_session_id, &space_name, &count_query)
        .await?
        .first_value()
        .and_then(|val| match val {
            Value::BigInt(c) => Some(*c),
            _ => None,
        })
        .unwrap_or(0);

    // Then, get paginated data
    let query = format!(
        "MATCH (v:{}) RETURN v{}{} SKIP {} LIMIT {}",
        tag_name,
        filter_clause,
        sort_clause,
        params.offset,
        params.limit
    );

    let result = execute_in_space(&web_state, caller_session_id, &space_name, &query).await?;
    // Convert QueryResult to JSON values
    let rows: Vec<serde_json::Value> = result
        .first_column_values()
        .map(|val| serde_json::json!({"vertex": val}))
        .collect();

    Ok(Json(ApiResponse::success(PaginatedResponse::new(
        rows,
        total,
        params.limit,
        params.offset,
    ))))
}

/// List edges by type
#[utoipa::path(
    get,
    path = "/api/v1/data/spaces/{name}/edge-types/{edge_name}/edges",
    tag = "WebData",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge_name" = String, Path, description = "Edge type name"),
        ("limit" = usize, Query, description = "Page size"),
        ("offset" = usize, Query, description = "Page offset"),
        ("filter" = Option<String>, Query, description = "Property filter expression"),
        ("sort_by" = Option<String>, Query, description = "Sort field"),
        ("sort_order" = Option<String>, Query, description = "Sort order")
    ),
    responses(
        (status = 200, description = "Edge page", body = ApiResponse<PaginatedResponse<serde_json::Value>>),
        (status = 500, description = "Internal error")
    )
)]
async fn list_edges_by_type<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Extension(caller_session_id): Extension<i64>,
    Path((space_name, edge_name)): Path<(String, String)>,
    Query(params): Query<DataFilterParams>,
) -> WebResult<Json<ApiResponse<PaginatedResponse<serde_json::Value>>>> {
    // Build filter clause for both count and data queries
    let filter_clause = params
        .filter
        .as_ref()
        .map(|f| format!(" WHERE {}", f))
        .unwrap_or_default();
    let sort_clause = params
        .sort_by
        .as_ref()
        .map(|s| {
            let order = params
                .sort_order
                .clone()
                .unwrap_or_else(|| "ASC".to_string());
            format!(" ORDER BY {} {}", s, order)
        })
        .unwrap_or_default();

    // First, get total count
    let count_query = format!(
        "MATCH ()-[e:{}]->() RETURN COUNT(e) as total{}",
        edge_name, filter_clause
    );

    let total = execute_in_space(&web_state, caller_session_id, &space_name, &count_query)
        .await?
        .first_value()
        .and_then(|val| match val {
            Value::BigInt(c) => Some(*c),
            _ => None,
        })
        .unwrap_or(0);

    // Then, get paginated data
    let query = format!(
        "MATCH ()-[e:{}]->() RETURN e{}{} SKIP {} LIMIT {}",
        edge_name,
        filter_clause,
        sort_clause,
        params.offset,
        params.limit
    );

    let result = execute_in_space(&web_state, caller_session_id, &space_name, &query).await?;
    // Convert QueryResult to JSON values
    let rows: Vec<serde_json::Value> = result
        .first_column_values()
        .map(|val| serde_json::json!({"edge": val}))
        .collect();

    Ok(Json(ApiResponse::success(PaginatedResponse::new(
        rows,
        total,
        params.limit,
        params.offset,
    ))))
}
