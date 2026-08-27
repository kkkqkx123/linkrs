use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::Json as JsonResponse,
};
use graphdb_wire::meta::{LoginRequest, LoginResponse, LogoutRequest};
use log::info;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

pub async fn login<
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
    Json(request): Json<LoginRequest>,
) -> Result<JsonResponse<LoginResponse>, HttpError> {
    // TODO: Implement proper authentication with password verification
    // For now, accept any username/password and create a session

    let session_manager = state.server.get_session_manager();

    // Create a new session for the user
    let session = session_manager
        .create_session(request.username.clone(), "127.0.0.1".to_string())
        .await
        .map_err(|e| HttpError::InternalError(format!("Failed to create session: {}", e)))?;

    let session_id = session.id();
    info!(
        "Created session {} for user {}",
        session_id, request.username
    );

    Ok(JsonResponse(LoginResponse {
        session_id,
        username: request.username,
        expires_at: None,
    }))
}

pub async fn logout<
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
    Json(request): Json<LogoutRequest>,
) -> Result<StatusCode, HttpError> {
    let session_manager = state.server.get_session_manager();
    session_manager.remove_session(request.session_id).await;
    Ok(StatusCode::NO_CONTENT)
}
