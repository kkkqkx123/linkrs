//! Fulltext search HTTP handler.
//!
//! The search logic and wire types live in [`crate::services::fulltext`];
//! this handler only maps the request and response.

use axum::{
    extract::{Extension, State},
    response::Json as JsonResponse,
    Json,
};

use crate::http::{error::HttpError, state::AppState};
use crate::services::fulltext::search_core;
pub use crate::services::fulltext::{
    FulltextSearchHit, FulltextSearchRequestBody, FulltextSearchResponse,
};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

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
    Ok(JsonResponse(
        search_core(&state, session_id, request).await?,
    ))
}
