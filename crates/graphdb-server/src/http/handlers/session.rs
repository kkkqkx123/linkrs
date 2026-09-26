use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::Json as JsonResponse,
};
use graphdb_wire::meta::{CreateSessionRequest, SessionResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    post,
    path = "/v1/sessions",
    tag = "Session",
    request_body = CreateSessionRequest,
    responses(
        (status = 200, body = SessionResponse, description = "Session created"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn create<
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
    Json(request): Json<CreateSessionRequest>,
) -> Result<JsonResponse<SessionResponse>, HttpError> {
    let session_manager = state.server.get_session_manager();
    let session = session_manager
        .create_session(request.username, request.client_ip)
        .await
        .map_err(|e| HttpError::BadRequest(format!("Failed to create session: {}", e)))?;

    Ok(JsonResponse(SessionResponse {
        session_id: session.id(),
        username: session.user(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH")
            .as_secs(),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/sessions/{id}",
    tag = "Session",
    params(("id" = i64, Path, description = "Session id")),
    responses(
        (status = 200, body = serde_json::Value, description = "Session details"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_session<
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
    Path(session_id): Path<i64>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let session_manager = state.server.get_session_manager();
    let session = session_manager
        .find_session(session_id)
        .ok_or_else(|| HttpError::NotFound("Session not found".to_string()))?;

    Ok(JsonResponse(serde_json::json!({
        "session_id": session.id(),
        "username": session.user(),
        "space_name": session.space_name(),
        "graph_addr": session.graph_addr(),
        "timezone": session.timezone(),
    })))
}

#[utoipa::path(
    delete,
    path = "/v1/sessions/{id}",
    tag = "Session",
    params(("id" = i64, Path, description = "Session id")),
    responses(
        (status = 204, description = "Session deleted"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn delete_session<
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
    Path(session_id): Path<i64>,
) -> Result<StatusCode, HttpError> {
    let session_manager = state.server.get_session_manager();
    session_manager.remove_session(session_id).await;
    Ok(StatusCode::NO_CONTENT)
}
