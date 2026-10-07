use axum::{
    extract::{Extension, State},
    response::Json as JsonResponse,
};
use graphdb_wire::meta::{UserListItem, UserListResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    get,
    operation_id = "get_v1_users",
    path = "/v1/users",
    tag = "Auth",
    responses(
        (status = 200, body = UserListResponse, description = "User list"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list<
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
) -> Result<JsonResponse<UserListResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let session = graph_service
        .get_session_manager()
        .find_session(session_id)
        .ok_or_else(|| HttpError::unauthorized("Session not found"))?;
    let username = session.user();
    let permission_manager = graph_service.get_permission_manager();
    if !permission_manager.is_admin(&username) {
        return Err(HttpError::forbidden("Admin permission required"));
    }
    let mut users: Vec<UserListItem> = permission_manager
        .list_all_users()
        .into_iter()
        .map(|(name, roles)| {
            let display = roles
                .iter()
                .map(|(_, role)| *role)
                .min_by_key(|role| role.to_byte())
                .map(|role| role.to_string());
            UserListItem {
                username: name,
                role: display,
                status: None,
                last_active: None,
            }
        })
        .collect();
    users.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(JsonResponse(UserListResponse { users }))
}
