use axum::{
    extract::{Extension, Json, State},
    http::StatusCode,
    response::Json as JsonResponse,
};
use graphdb_wire::meta::{AuthMeResponse, LoginRequest, LoginResponse, LogoutRequest};
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

    let permission_manager = graph_service.get_permission_manager();
    let roles: Vec<String> = permission_manager
        .list_user_roles(&request.username)
        .into_iter()
        .map(|(_, role)| role.to_string())
        .collect();
    let display = permission_manager
        .highest_role(&request.username)
        .map(|role| role.to_string());

    Ok(JsonResponse(LoginResponse {
        session_id,
        username: request.username,
        expires_at: None,
        role: display.clone(),
        display_role: display,
        roles,
    }))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_auth_me",
    path = "/v1/auth/me",
    tag = "Auth",
    responses(
        (status = 200, body = AuthMeResponse, description = "Current user"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn me<
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
) -> Result<JsonResponse<AuthMeResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let session = graph_service
        .get_session_manager()
        .find_session(session_id)
        .ok_or_else(|| HttpError::unauthorized("Session not found"))?;
    let username = session.user();
    let permission_manager = graph_service.get_permission_manager();
    let roles: Vec<String> = permission_manager
        .list_user_roles(&username)
        .into_iter()
        .map(|(_, role)| role.to_string())
        .collect();
    let display = permission_manager
        .highest_role(&username)
        .map(|role| role.to_string());
    Ok(JsonResponse(AuthMeResponse {
        username,
        role: display.clone(),
        display_role: display,
        roles,
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
