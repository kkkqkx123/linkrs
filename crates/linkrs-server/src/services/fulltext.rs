//! Fulltext search services shared by both transports.

use linkrs_fulltext::SearchResult;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::authz::require_space_read;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

fn default_limit() -> usize {
    10
}

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

/// Search a fulltext index.
pub(crate) async fn search_core<
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
    request: FulltextSearchRequestBody,
) -> Result<FulltextSearchResponse, HttpError> {
    require_space_read(state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let fulltext_api = graph_service.fulltext_api();

    let fulltext_api = fulltext_api
        .ok_or_else(|| HttpError::InternalError("Fulltext API is not available".to_string()))?;

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

    Ok(FulltextSearchResponse {
        results: hits,
        total_hits,
    })
}
