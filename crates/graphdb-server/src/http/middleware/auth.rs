use crate::http::state::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
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
        + StorageOperationContextOps
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

    let session = state.server.get_session_manager().find_session(session_id);

    let session = match session {
        Some(session) => session,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    if state
        .server
        .get_graph_service()
        .is_user_locked(&session.user())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }

    if state
        .server
        .get_graph_service()
        .must_change_password(&session.user())
    {
        let path = request.uri().path();
        let query_allowed = path == "/v1/query" || path.starts_with("/v1/query/");
        if !query_allowed {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    request.extensions_mut().insert(session_id);

    Ok(next.run(request).await)
}
