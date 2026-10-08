use crate::http::state::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Structured failure codes returned by the auth middleware.
pub const CODE_UNAUTHENTICATED: &str = "unauthenticated";
pub const CODE_ACCOUNT_LOCKED: &str = "account_locked";
pub const CODE_PASSWORD_CHANGE_REQUIRED: &str = "password_change_required";

fn deny(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message }, "status": status.as_u16() })),
    )
        .into_response()
}

/// Whether a must-change account may call this path.
///
/// Query execution stays available so the password can be rotated via
/// `CHANGE PASSWORD`; the admin reset endpoint plus the auth self-service
/// endpoints stay available so management-plane flows are not deadlocked.
fn must_change_allowed(path: &str) -> bool {
    if path == "/v1/query" || path.starts_with("/v1/query/") {
        return true;
    }
    if path == "/v1/auth/me" || path == "/v1/auth/logout" {
        return true;
    }
    if path.starts_with("/v1/users/") && path.ends_with("/password") {
        return true;
    }
    false
}

pub async fn auth_middleware<
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
    mut request: Request,
    next: Next,
) -> Response {
    let graph_service = state.server.get_graph_service();

    // Skip every session check when the operator has explicitly disabled
    // authentication (either via enable_authorize = false or single_user_mode = true).
    // This matches the local-deployment "no login needed" contract. We still
    // inject a placeholder session_id so downstream handlers never have to
    // deal with an Option type.
    if graph_service.is_auth_disabled() {
        request.extensions_mut().insert(0i64);
        return next.run(request).await;
    }

    let session_id = match request
        .headers()
        .get("X-Session-ID")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
    {
        Some(id) => id,
        None => {
            return deny(
                StatusCode::UNAUTHORIZED,
                CODE_UNAUTHENTICATED,
                "missing or invalid session id".to_string(),
            );
        }
    };

    let session = match state.server.get_session_manager().find_session(session_id) {
        Some(session) => session,
        None => {
            return deny(
                StatusCode::UNAUTHORIZED,
                CODE_UNAUTHENTICATED,
                "session not found or expired".to_string(),
            );
        }
    };

    if state
        .server
        .get_graph_service()
        .is_user_locked(&session.user())
    {
        return deny(
            StatusCode::FORBIDDEN,
            CODE_ACCOUNT_LOCKED,
            format!("account {} is locked", session.user()),
        );
    }

    if state
        .server
        .get_graph_service()
        .must_change_password(&session.user())
        && !must_change_allowed(request.uri().path())
    {
        return deny(
            StatusCode::FORBIDDEN,
            CODE_PASSWORD_CHANGE_REQUIRED,
            "password change required before other operations".to_string(),
        );
    }

    request.extensions_mut().insert(session_id);

    next.run(request).await
}
