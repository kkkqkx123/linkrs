use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Json as JsonResponse,
    Json,
};
use linkrs_wire::meta::{
    CreateUserRequest, GrantRoleRequest, ResetPasswordRequest, RevokeRoleRequest, UserListItem,
    UserListResponse,
};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

fn session_user<S>(state: &AppState<S>, session_id: i64) -> Result<String, HttpError>
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
{
    let graph_service = state.server.get_graph_service();
    let session = graph_service
        .get_session_manager()
        .find_session(session_id)
        .ok_or_else(|| HttpError::unauthorized("Session not found"))?;
    Ok(session.user())
}

fn map_admin_error(message: String) -> HttpError {
    let lower = message.to_lowercase();
    if lower.contains("not found") {
        HttpError::not_found(message)
    } else if lower.contains("already exists") {
        HttpError::Conflict(message)
    } else if lower.contains("permission") {
        HttpError::forbidden(message)
    } else {
        HttpError::bad_request(message)
    }
}

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
    let users: Vec<UserListItem> = graph_service
        .list_users_detailed()
        .into_iter()
        .map(|detail| UserListItem {
            username: detail.username,
            role: detail.role,
            status: Some(detail.status),
            last_active: detail.last_active,
        })
        .collect();
    Ok(JsonResponse(UserListResponse { users }))
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users",
    path = "/v1/users",
    tag = "Auth",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 409, description = "Conflict"),
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
    Extension(session_id): Extension<i64>,
    Json(request): Json<CreateUserRequest>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_create_user(&caller, &request.username, &request.password)
        .map_err(map_admin_error)?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users_name_password",
    path = "/v1/users/{name}/password",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    request_body = ResetPasswordRequest,
    responses(
        (status = 204, description = "Password reset"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn reset_password<
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
    Path(name): Path<String>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_reset_password(&caller, Some(session_id), &name, &request.password)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users_name_enable",
    path = "/v1/users/{name}/enable",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    responses(
        (status = 204, description = "User enabled"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn enable<
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
    Path(name): Path<String>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_set_user_enabled(&caller, Some(session_id), &name, true)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users_name_disable",
    path = "/v1/users/{name}/disable",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    responses(
        (status = 204, description = "User disabled"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn disable<
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
    Path(name): Path<String>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_set_user_enabled(&caller, Some(session_id), &name, false)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users_name_grant",
    path = "/v1/users/{name}/grant",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    request_body = GrantRoleRequest,
    responses(
        (status = 204, description = "Role granted"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn grant<
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
    Path(name): Path<String>,
    Json(request): Json<GrantRoleRequest>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_grant_role(&caller, &name, &request.space, &request.role)
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    operation_id = "post_v1_users_name_revoke",
    path = "/v1/users/{name}/revoke",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    request_body = RevokeRoleRequest,
    responses(
        (status = 204, description = "Role revoked"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn revoke<
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
    Path(name): Path<String>,
    Json(request): Json<RevokeRoleRequest>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_revoke_role(&caller, &name, &request.space)
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    operation_id = "delete_v1_users_name",
    path = "/v1/users/{name}",
    tag = "Auth",
    params(("name" = String, Path, description = "Username")),
    responses(
        (status = 204, description = "User deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn drop_user<
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
    Path(name): Path<String>,
) -> Result<StatusCode, HttpError> {
    let caller = session_user(&state, session_id)?;
    state
        .server
        .get_graph_service()
        .admin_drop_user(&caller, &name)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}
