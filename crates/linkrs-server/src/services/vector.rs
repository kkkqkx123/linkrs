//! Vector scroll/count services shared by both transports.

use serde::{Deserialize, Serialize};
use simvec::VectorFilter;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::authz::require_space_read;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

fn default_limit() -> usize {
    10
}

/// Parse the gRPC `SearchFilter.expression` wire string.
///
/// The wire carries a JSON-encoded `VectorFilter` with the same shape as the
/// HTTP search filter. There is no expression language; arbitrary strings
/// fail with the underlying JSON error.
#[allow(dead_code)]
pub(crate) fn parse_vector_filter_expression(expression: &str) -> Result<VectorFilter, String> {
    serde_json::from_str(expression).map_err(|e| {
        format!("filter.expression must be a JSON VectorFilter with the same shape as the HTTP search filter: {e}")
    })
}

/// Single vector search result
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorSearchResult {
    pub doc_id: String,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Scroll request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ScrollRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub offset: Option<String>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
}

/// Scroll response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScrollResponse {
    pub points: Vec<VectorSearchResult>,
    pub next_offset: Option<String>,
}

/// Paginated scroll over vector points.
pub(crate) async fn scroll_core<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
    session_id: i64,
    request: ScrollRequest,
) -> Result<ScrollResponse, HttpError> {
    require_space_read(state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    let vector_api = vector_api
        .ok_or_else(|| HttpError::InternalError("Vector API is not available".to_string()))?;

    let (points, next_offset) = vector_api
        .scroll(linkrs_api::api_core::vector_api::ScrollQuery {
            space_id: request.space_id,
            tag_name: &request.tag_name,
            field_name: &request.field_name,
            limit: request.limit,
            offset: request.offset.as_deref(),
            with_payload: request.with_payload,
            with_vector: request.with_vector,
        })
        .await
        .map_err(|e| HttpError::InternalError(e.to_string()))?;

    let results: Vec<VectorSearchResult> = points
        .into_iter()
        .map(|p| VectorSearchResult {
            doc_id: p.id.to_string(),
            score: 0.0,
            vector: if request.with_vector.unwrap_or(false) {
                Some(p.vector)
            } else {
                None
            },
            payload: p.payload.map(|pay| pay.into_iter().collect()),
        })
        .collect();

    Ok(ScrollResponse {
        points: results,
        next_offset,
    })
}

/// Count the points stored in a vector index.
pub(crate) async fn count_core<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
    session_id: i64,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
) -> Result<u64, HttpError> {
    require_space_read(state, session_id, space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    let vector_api = vector_api
        .ok_or_else(|| HttpError::InternalError("Vector API is not available".to_string()))?;

    vector_api
        .count(space_id, tag_name, field_name)
        .await
        .map_err(|e: linkrs_api::api_core::error::CoreError| {
            HttpError::InternalError(e.to_string())
        })
}
