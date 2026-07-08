use crate::api::server::http::state::AppState;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http::StatusCode;

pub async fn auth_middleware<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let session_id = request
        .headers()
        .get("X-Session-ID")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let valid = state
        .server
        .get_session_manager()
        .find_session(session_id)
        .is_some();

    if !valid {
        return Err(StatusCode::UNAUTHORIZED);
    }

    request.extensions_mut().insert(session_id);

    Ok(next.run(request).await)
}
