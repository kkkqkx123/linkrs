//! Web Management Middleware
//!
//! Provides authentication and authorization middleware for web management APIs

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::WebState;

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
) -> Result<Response, StatusCode> {
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
        return Ok(next.run(request).await);
    }

    let session_id = request
        .headers()
        .get("X-Session-ID")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let valid = web_state
        .core_state
        .server
        .get_session_manager()
        .find_session(session_id)
        .is_some();

    if !valid {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Store session_id in request extensions for handlers to use
    request.extensions_mut().insert(session_id);

    Ok(next.run(request).await)
}
