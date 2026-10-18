//! Fulltext search HTTP handler.
//!
//! Exposes the FulltextApi search method as a dedicated REST endpoint,
//! mirroring the vector search endpoint for transport-level symmetry.

use axum::{
    extract::{Extension, State},
    response::Json as JsonResponse,
    Json,
};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_fulltext::SearchResult;

/// Fulltext search request body.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct FulltextSearchRequestBody {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

/// Fulltext search response body.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct FulltextSearchResponse {
    pub results: Vec<FulltextSearchHit>,
    pub total_hits: usize,
}

/// One fulltext search hit.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct FulltextSearchHit {
    pub doc_id: String,
    pub score: f32,
    #[serde(default)]
    pub highlights: Option<Vec<String>>,
}

#[utoipa::path(
    post,
    operation_id = "post_v1_fulltext_search",
    path = "/v1/fulltext/search",
    tag = "Fulltext",
    request_body = FulltextSearchRequestBody,
    responses(
        (status = 200, body = FulltextSearchResponse, description = "Fulltext search results"),
        (status = 500, description = "Internal error")
    )
)]
/// Search a fulltext index.
pub async fn search<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<FulltextSearchRequestBody>,
) -> Result<JsonResponse<FulltextSearchResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let fulltext_api = graph_service.fulltext_api();

    if let Some(fulltext_api) = fulltext_api {
        let results = fulltext_api
            .search(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                &request.query,
                request.limit,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let total_hits = results.len();
        let hits: Vec<FulltextSearchHit> = results
            .into_iter()
            .map(|r: SearchResult| {
                let doc_id = r
                    .doc_id
                    .to_string()
                    .map_err(|e| HttpError::InternalError(e))?;
                Ok(FulltextSearchHit {
                    doc_id,
                    score: r.score,
                    highlights: r.highlights,
                })
            })
            .collect::<Result<Vec<_>, HttpError>>()?;

        Ok(JsonResponse(FulltextSearchResponse {
            results: hits,
            total_hits,
        }))
    } else {
        Err(HttpError::InternalError(
            "Fulltext API is not available".to_string(),
        ))
    }
}
