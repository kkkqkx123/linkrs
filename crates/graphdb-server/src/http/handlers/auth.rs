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

#[utoipa::path(
    post,
    path = "/v1/auth/login",
    tag = "Auth",
    request_body = LoginRequest,
    responses(
        (status = 200, body = LoginResponse, description = "Login succeeded"),
        (status = 500, description = "Internal error")
    )
)]
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
    // Verify the password through the shared authenticator before creating
    // a session; failures surface as 401 instead of minting a session.
    let graph_service = state.server.get_graph_service();
    let session = graph_service
        .authenticate(&request.username, &request.password)
        .await
        .map_err(HttpError::unauthorized)?;

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

#[utoipa::path(
    post,
    path = "/v1/auth/logout",
    tag = "Auth",
    request_body = LogoutRequest,
    responses(
        (status = 204, description = "Logout succeeded"),
        (status = 500, description = "Internal error")
    )
)]
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
