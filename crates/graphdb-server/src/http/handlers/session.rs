use axum::{
    extract::{Extension, Json, Path, State},
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
    operation_id = "post_v1_sessions",
    path = "/v1/sessions",
    tag = "Session",
    request_body = CreateSessionRequest,
    responses(
        (status = 200, body = SessionResponse, description = "Session created"),
        (status = 401, description = "Invalid credentials"),
        (status = 403, description = "Account locked"),
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
    let graph_service = state.server.get_graph_service();

    // Verify account is not disabled (checked regardless of auth mode).
    if graph_service.is_user_locked(&request.username) {
        return Err(HttpError::forbidden(format!(
            "account {} is locked",
            request.username
        )));
    }

    // When authentication is active this endpoint must verify the password
    // before issuing a session — the handler no longer bypasses the
    // authenticator.
    if !graph_service.is_auth_disabled() {
        let password = request
            .password
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| HttpError::unauthorized("password required"))?;

        let _ = graph_service
            .authenticate(&request.username, password)
            .await
            .map_err(|message| {
                let lowered = message.to_lowercase();
                if lowered.contains("locked") {
                    HttpError::forbidden(message)
                } else {
                    HttpError::unauthorized(message)
                }
            })?;
    }

    let session_manager = state.server.get_session_manager();
    let session = session_manager
        .create_session(request.username.clone(), request.client_ip)
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
    operation_id = "get_v1_sessions",
    path = "/v1/sessions",
    tag = "Session",
    responses(
        (status = 200, body = serde_json::Value, description = "Session list"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list_sessions<
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
    super::authz::require_admin_session(&state, session_id)?;
    let sessions = state.server.get_graph_service().list_sessions().await;
    let items: Vec<serde_json::Value> = sessions
        .into_iter()
        .map(|info| {
            serde_json::json!({
                "session_id": info.session_id,
                "username": info.user_name,
                "space_name": info.space_name,
                "graph_addr": info.graph_addr,
                "active_queries": info.active_queries,
            })
        })
        .collect();
    Ok(JsonResponse(serde_json::json!({ "sessions": items })))
}

#[utoipa::path(
    get,
    path = "/v1/sessions/{id}",
    tag = "Session",
    params(("id" = i64, Path, description = "Session id")),
    responses(
        (status = 200, body = serde_json::Value, description = "Session details"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Not found")
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
    Extension(caller_session_id): Extension<i64>,
    Path(session_id): Path<i64>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let caller = super::authz::find_session(&state, caller_session_id)?;
    let graph_service = state.server.get_graph_service();
    let session_manager = state.server.get_session_manager();
    let session = session_manager
        .find_session(session_id)
        .ok_or_else(|| HttpError::NotFound("Session not found".to_string()))?;
    let is_admin = graph_service
        .get_permission_manager()
        .is_admin(&caller.user());
    if !is_admin && session.user() != caller.user() {
        return Err(HttpError::forbidden("Admin permission required"));
    }

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
    Extension(caller_session_id): Extension<i64>,
    Path(session_id): Path<i64>,
) -> Result<StatusCode, HttpError> {
    let caller = super::authz::find_session(&state, caller_session_id)?;
    state
        .server
        .get_graph_service()
        .kill_session(session_id, &caller.user())
        .await
        .map_err(|e| {
            let msg = e.to_string().to_lowercase();
            if msg.contains("not found") {
                HttpError::NotFound(e.to_string())
            } else if msg.contains("permission") {
                HttpError::forbidden(e.to_string())
            } else {
                HttpError::BadRequest(e.to_string())
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}
