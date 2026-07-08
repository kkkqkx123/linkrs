//! Data Browser Handlers
//!
//! Provides data browsing APIs:
//! - Browse vertices by tag
//! - Browse edges by edge type
//! - Data filtering and pagination

use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::api::server::web::{
    error::{WebError, WebResult},
    models::{ApiResponse, PaginatedResponse, PaginationParams},
    WebState,
};
use crate::core::Value;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Get or create a session for the current request
/// Returns a session ID that can be used with graph_service.execute()
async fn get_or_create_session_id<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    web_state: &WebState<S>,
) -> Result<i64, WebError> {
    let session_manager = web_state.core_state.server.get_session_manager();

    // Try to find an existing anonymous session or create a new one
    // In a production system, this would use authenticated session from request context
    let session = session_manager
        .create_session("anonymous".to_string(), "127.0.0.1".to_string())
        .await
        .map_err(|e| WebError::Internal(format!("Failed to create session: {}", e)))?;

    Ok(session.id())
}

/// Create data browser routes (without state)
pub fn create_routes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
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
#[derive(Debug, Deserialize)]
pub struct DataFilterParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,
    /// Property filter (e.g., "age>18")
    pub filter: Option<String>,
    /// Sort field
    pub sort_by: Option<String>,
    /// Sort order
    pub sort_order: Option<String>,
}

/// List vertices by tag
async fn list_vertices_by_tag<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path((space_name, tag_name)): Path<(String, String)>,
    Query(params): Query<DataFilterParams>,
) -> WebResult<Json<ApiResponse<PaginatedResponse<serde_json::Value>>>> {
    // Get or create session for this request
    let session_id = get_or_create_session_id(&web_state).await?;

    let graph_service = web_state.core_state.server.get_graph_service();

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
        "USE {}; MATCH (v:{}) RETURN COUNT(v) as total{}",
        space_name, tag_name, filter_clause
    );

    let total = match graph_service.execute(session_id, &count_query).await {
        Ok(crate::query::executor::ExecutionResult::DataSet(ds)) => ds
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|val| match val {
                Value::BigInt(c) => Some(*c),
                _ => None,
            })
            .unwrap_or(0),
        _ => 0,
    };

    // Then, get paginated data
    let query = format!(
        "USE {}; MATCH (v:{}) RETURN v{}{} SKIP {} LIMIT {}",
        space_name,
        tag_name,
        filter_clause,
        sort_clause,
        params.pagination.offset,
        params.pagination.limit
    );

    let result = match graph_service.execute(session_id, &query).await {
        Ok(exec_result) => {
            // Convert ExecutionResult to JSON values
            let rows: Vec<serde_json::Value> = match exec_result {
                crate::query::executor::ExecutionResult::DataSet(ds) => ds
                    .rows
                    .iter()
                    .filter_map(|row| row.first())
                    .map(|val| serde_json::json!({"vertex": val}))
                    .collect(),
                _ => vec![],
            };

            Ok::<_, WebError>(PaginatedResponse::new(
                rows,
                total,
                params.pagination.limit,
                params.pagination.offset,
            ))
        }
        Err(e) => Err(WebError::Query(format!("Failed to list vertices: {}", e))),
    };

    Ok(Json(ApiResponse::success(result?)))
}

/// List edges by type
async fn list_edges_by_type<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path((space_name, edge_name)): Path<(String, String)>,
    Query(params): Query<DataFilterParams>,
) -> WebResult<Json<ApiResponse<PaginatedResponse<serde_json::Value>>>> {
    // Get or create session for this request
    let session_id = get_or_create_session_id(&web_state).await?;

    let graph_service = web_state.core_state.server.get_graph_service();

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
        "USE {}; MATCH ()-[e:{}]->() RETURN COUNT(e) as total{}",
        space_name, edge_name, filter_clause
    );

    let total = match graph_service.execute(session_id, &count_query).await {
        Ok(crate::query::executor::ExecutionResult::DataSet(ds)) => ds
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|val| match val {
                Value::BigInt(c) => Some(*c),
                _ => None,
            })
            .unwrap_or(0),
        _ => 0,
    };

    // Then, get paginated data
    let query = format!(
        "USE {}; MATCH ()-[e:{}]->() RETURN e{}{} SKIP {} LIMIT {}",
        space_name,
        edge_name,
        filter_clause,
        sort_clause,
        params.pagination.offset,
        params.pagination.limit
    );

    let result = match graph_service.execute(session_id, &query).await {
        Ok(exec_result) => {
            // Convert ExecutionResult to JSON values
            let rows: Vec<serde_json::Value> = match exec_result {
                crate::query::executor::ExecutionResult::DataSet(ds) => ds
                    .rows
                    .iter()
                    .filter_map(|row| row.first())
                    .map(|val| serde_json::json!({"edge": val}))
                    .collect(),
                _ => vec![],
            };

            Ok::<_, WebError>(PaginatedResponse::new(
                rows,
                total,
                params.pagination.limit,
                params.pagination.offset,
            ))
        }
        Err(e) => Err(WebError::Query(format!("Failed to list edges: {}", e))),
    };

    Ok(Json(ApiResponse::success(result?)))
}
