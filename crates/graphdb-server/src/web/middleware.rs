//! Web Management Middleware
//!
//! Provides authentication and authorization middleware for web management APIs

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::WebState;

fn deny(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message }, "status": status.as_u16() })),
    )
        .into_response()
}

/// Web authentication middleware
///
/// Validates the session ID from the X-Session-ID header
/// and ensures the session is active in the session manager.
pub async fn web_auth_middleware<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    mut request: Request,
    next: Next,
) -> Response {
    // Align with the server-wide auth-disabled contract: skip every check
    // when enable_authorize = false or single_user_mode = true so the web
    // console is reachable without a login round-trip on local deployments.
    if web_state
        .core_state
        .server
        .get_graph_service()
        .is_auth_disabled()
    {
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
                "unauthenticated",
                "missing or invalid session id".to_string(),
            );
        }
    };

    let session = match web_state
        .core_state
        .server
        .get_session_manager()
        .find_session(session_id)
    {
        Some(session) => session,
        None => {
            return deny(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "session not found or expired".to_string(),
            );
        }
    };

    if web_state
        .core_state
        .server
        .get_graph_service()
        .is_user_locked(&session.user())
    {
        return deny(
            StatusCode::FORBIDDEN,
            "account_locked",
            format!("account {} is locked", session.user()),
        );
    }

    // Store session_id in request extensions for handlers to use
    request.extensions_mut().insert(session_id);

    next.run(request).await
}
