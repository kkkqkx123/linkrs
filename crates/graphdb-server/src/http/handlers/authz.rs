use std::sync::Arc;

use crate::client::{ClientSession, Session};
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Maximum number of transient sessions we will auto-create during auth-disabled
/// single-user mode. The background reclaimer will still evict idle ones.
const AUTH_DISABLED_AUTO_CREATE: bool = true;

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
    // Auth-disabled mode (enable_authorize = false or single_user_mode = true):
    // treat session_id == 0 as a placeholder for the default identity. We look
    // up or lazily create the default session so downstream handlers never
    // see a missing session while still reusing the normal session-lifecycle
    // machinery.
    if session_id == 0 {
        let graph_service = state.server.get_graph_service();
        if graph_service.is_auth_disabled() && AUTH_DISABLED_AUTO_CREATE {
            let default_username = graph_service.auth_config().default_username.clone();
            if let Some(existing) = state
                .server
                .get_session_manager()
                .find_session_from_cache(0)
            {
                return Ok(existing);
            }
            let session = Session {
                session_id: 0,
                user_name: default_username,
                space_name: None,
                graph_addr: Some("127.0.0.1".to_string()),
                timezone: None,
            };
            return Ok(ClientSession::new(session));
        }
    }

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
