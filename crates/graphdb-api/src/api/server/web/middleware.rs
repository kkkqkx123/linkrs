//! Web Management Middleware
//!
//! Provides authentication and authorization middleware for web management APIs

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::api::server::web::WebState;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Web authentication middleware
///
/// Validates the session ID from the X-Session-ID header
/// and ensures the session is active in the session manager.
pub async fn web_auth_middleware<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
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
