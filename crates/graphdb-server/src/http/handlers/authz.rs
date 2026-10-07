use std::sync::Arc;

use crate::client::ClientSession;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

pub fn find_session<S>(
    state: &AppState<S>,
    session_id: i64,
) -> Result<Arc<ClientSession>, HttpError>
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
    state
        .server
        .get_session_manager()
        .find_session(session_id)
        .ok_or_else(|| HttpError::unauthorized("Session not found"))
}

pub fn require_admin_session<S>(
    state: &AppState<S>,
    session_id: i64,
) -> Result<Arc<ClientSession>, HttpError>
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
    let session = find_session(state, session_id)?;
    let graph_service = state.server.get_graph_service();
    if !graph_service
        .get_permission_manager()
        .is_admin(&session.user())
    {
        return Err(HttpError::forbidden("Admin permission required"));
    }
    Ok(session)
}

pub fn require_schema_write_session<S>(
    state: &AppState<S>,
    session_id: i64,
    space_name: &str,
) -> Result<Arc<ClientSession>, HttpError>
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
    let session = find_session(state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let space_id = graph_service
        .get_storage_space_id(space_name)
        .ok_or_else(|| HttpError::not_found(format!("Space '{}' not found", space_name)))?;
    let checker = graph_service.permission_checker();
    checker
        .can_write_schema(&session, space_id)
        .map_err(|e| HttpError::forbidden(e.to_string()))?;
    Ok(session)
}

pub fn require_session_owner_or_admin<S>(
    state: &AppState<S>,
    caller_session_id: i64,
    target_session_id: i64,
) -> Result<Arc<ClientSession>, HttpError>
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
    let caller = find_session(state, caller_session_id)?;
    if caller_session_id == target_session_id {
        return Ok(caller);
    }
    let graph_service = state.server.get_graph_service();
    if graph_service
        .get_permission_manager()
        .is_admin(&caller.user())
    {
        return Ok(caller);
    }
    let target = find_session(state, target_session_id).ok();
    if let Some(target) = target {
        if target.user() == caller.user() {
            return Ok(caller);
        }
    }
    Err(HttpError::forbidden("Admin permission required"))
}

pub fn require_space_read<S>(
    state: &AppState<S>,
    session_id: i64,
    space_id: i64,
) -> Result<Arc<ClientSession>, HttpError>
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
    let session = find_session(state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let checker = graph_service.permission_checker();
    checker
        .can_read_data(&session, space_id)
        .map_err(|e| HttpError::forbidden(e.to_string()))?;
    Ok(session)
}

pub fn require_space_write<S>(
    state: &AppState<S>,
    session_id: i64,
    space_id: i64,
) -> Result<Arc<ClientSession>, HttpError>
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
    let session = find_session(state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let checker = graph_service.permission_checker();
    checker
        .can_write_data(&session, space_id)
        .map_err(|e| HttpError::forbidden(e.to_string()))?;
    Ok(session)
}
