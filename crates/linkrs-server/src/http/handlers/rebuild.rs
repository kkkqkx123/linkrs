//! Async index rebuild handlers (fulltext + vector).
//!
//! The task registry and rebuild drivers live in [`crate::services::rebuild`];
//! these handlers only map request/response shapes.

#[cfg(any(feature = "fulltext", feature = "vector"))]
use axum::{
    extract::{Extension, Path, State},
    response::Json as JsonResponse,
    Json,
};

#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::http::{error::HttpError, state::AppState};
#[cfg(feature = "fulltext")]
use crate::services::rebuild::{
    clear_fulltext_core, fulltext_rebuild_status_core, inconsistent_fulltext_core,
    list_fulltext_indexes_core, rebuild_fulltext_core,
};
#[cfg(feature = "vector")]
use crate::services::rebuild::{
    clear_vector_core, rebuild_vector_core, vector_rebuild_status_core,
};
#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

pub use crate::services::rebuild::{RebuildTaskRecord, RebuildTaskRegistry};

// ── Fulltext ──────────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/v1/fulltext/indexes/rebuild",
    tag = "Fulltext",
    request_body = linkrs_wire::fulltext::RebuildFulltextIndexRequest,
    responses(
        (status = 200, body = linkrs_wire::fulltext::RebuildFulltextIndexResponse, description = "Fulltext rebuild started"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Start an async fulltext index rebuild.
#[cfg(feature = "fulltext")]
pub async fn rebuild_fulltext<
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
    Json(request): Json<linkrs_wire::fulltext::RebuildFulltextIndexRequest>,
) -> Result<JsonResponse<linkrs_wire::fulltext::RebuildFulltextIndexResponse>, HttpError> {
    Ok(JsonResponse(
        rebuild_fulltext_core(&state, session_id, request).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/fulltext/rebuilds/{id}",
    tag = "Fulltext",
    params(("id" = String, Path, description = "Rebuild task id")),
    responses(
        (status = 200, body = linkrs_wire::fulltext::FulltextRebuildStatusResponse, description = "Fulltext rebuild status"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Poll a fulltext rebuild task, merging live driver progress while running.
#[cfg(feature = "fulltext")]
pub async fn fulltext_rebuild_status<
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
    Path(rebuild_id): Path<String>,
) -> Result<JsonResponse<linkrs_wire::fulltext::FulltextRebuildStatusResponse>, HttpError> {
    Ok(JsonResponse(
        fulltext_rebuild_status_core(&state, session_id, rebuild_id).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/fulltext/indexes/clear",
    tag = "Fulltext",
    request_body = linkrs_wire::fulltext::ClearFulltextIndexRequest,
    responses(
        (status = 200, body = linkrs_wire::fulltext::ClearFulltextIndexResponse, description = "Fulltext index cleared"),
        (status = 500, description = "Internal error")
    )
)]
/// Synchronously clear a fulltext index. Destructive: requires `force=true`.
#[cfg(feature = "fulltext")]
pub async fn clear_fulltext<
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
    Json(request): Json<linkrs_wire::fulltext::ClearFulltextIndexRequest>,
) -> Result<JsonResponse<linkrs_wire::fulltext::ClearFulltextIndexResponse>, HttpError> {
    Ok(JsonResponse(
        clear_fulltext_core(&state, session_id, request).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/fulltext/indexes/inconsistent",
    tag = "Fulltext",
    responses(
        (status = 200, body = serde_json::Value, description = "Inconsistent fulltext indexes"),
        (status = 500, description = "Internal error")
    )
)]
/// List fulltext indexes whose engine is in the `Inconsistent` state.
///
/// These indexes reject writes and need operator attention: trigger the
/// async online rebuild (`POST /v1/fulltext/indexes/rebuild`) or drop and
/// recreate the index. Read-only; safe to poll for alerting.
#[cfg(feature = "fulltext")]
pub async fn inconsistent_fulltext<
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    Ok(JsonResponse(
        inconsistent_fulltext_core(&state, session_id).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/fulltext/indexes",
    tag = "Fulltext",
    responses(
        (status = 200, body = serde_json::Value, description = "Fulltext indexes"),
        (status = 500, description = "Internal error")
    )
)]
/// List all fulltext indexes.
#[cfg(feature = "fulltext")]
pub async fn list_fulltext_indexes<
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    Ok(JsonResponse(
        list_fulltext_indexes_core(&state, session_id).await?,
    ))
}

// ── Vector ────────────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/v1/vector/indexes/rebuild",
    tag = "Vector",
    request_body = linkrs_wire::vector::RebuildVectorIndexRequest,
    responses(
        (status = 200, body = linkrs_wire::vector::RebuildVectorIndexResponse, description = "Vector rebuild started"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Start an async vector index rebuild.
#[cfg(feature = "vector")]
pub async fn rebuild_vector<
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
    Json(request): Json<linkrs_wire::vector::RebuildVectorIndexRequest>,
) -> Result<JsonResponse<linkrs_wire::vector::RebuildVectorIndexResponse>, HttpError> {
    Ok(JsonResponse(
        rebuild_vector_core(&state, session_id, request).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/vector/rebuilds/{id}",
    tag = "Vector",
    params(("id" = String, Path, description = "Rebuild task id")),
    responses(
        (status = 200, body = linkrs_wire::vector::VectorRebuildStatusResponse, description = "Vector rebuild status"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Poll a vector rebuild task, merging live driver progress while running.
#[cfg(feature = "vector")]
pub async fn vector_rebuild_status<
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
    Path(rebuild_id): Path<String>,
) -> Result<JsonResponse<linkrs_wire::vector::VectorRebuildStatusResponse>, HttpError> {
    Ok(JsonResponse(
        vector_rebuild_status_core(&state, session_id, rebuild_id).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/vector/indexes/clear",
    tag = "Vector",
    request_body = linkrs_wire::vector::ClearVectorIndexRequest,
    responses(
        (status = 200, body = linkrs_wire::vector::ClearVectorIndexResponse, description = "Vector index cleared"),
        (status = 500, description = "Internal error")
    )
)]
/// Synchronously clear a vector index. Destructive: requires `force=true`.
#[cfg(feature = "vector")]
pub async fn clear_vector<
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
    Json(request): Json<linkrs_wire::vector::ClearVectorIndexRequest>,
) -> Result<JsonResponse<linkrs_wire::vector::ClearVectorIndexResponse>, HttpError> {
    Ok(JsonResponse(
        clear_vector_core(&state, session_id, request).await?,
    ))
}
