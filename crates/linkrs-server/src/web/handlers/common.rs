//! Shared helpers for the web management handlers.
//!
//! The management handlers run graph statements on behalf of the console. A
//! statement is parsed as a single unit, so the target space is bound on a
//! request-scoped session instead of being prepended to the statement text.

use std::sync::Arc;

use linkrs_api::api_core::QueryResult;
use linkrs_core::types::{SpaceInfo, SpaceSummary};
use linkrs_core::DataType;

use crate::client::ClientSession;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{error::WebError, WebState};

/// Execute one statement with the request routed to `space_name`.
///
/// `caller_session_id` is the session the auth middleware attached to the
/// request: the console's own session, or the auth-disabled placeholder.
pub(crate) async fn execute_in_space<S>(
    web_state: &WebState<S>,
    caller_session_id: i64,
    space_name: &str,
    stmt: &str,
) -> Result<QueryResult, WebError>
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
    let session = request_session(web_state, caller_session_id).await?;
    bind_space(web_state, &session, space_name)?;
    web_state
        .core_state
        .server
        .get_graph_service()
        .execute(session.id(), stmt)
        .await
        .map_err(|e| WebError::Query(e.to_string()))
}

/// Open a session for the identity this request runs as.
///
/// A real caller session runs under its own user, so the permission gate sees
/// that user's roles. The auth-disabled placeholder runs under the configured
/// default identity — the same account the auth-disabled login path hands out.
/// The session is scoped to the request, so binding its space cannot leak into
/// another request or mutate the caller's own session.
async fn request_session<S>(
    web_state: &WebState<S>,
    caller_session_id: i64,
) -> Result<Arc<ClientSession>, WebError>
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
    let session_manager = web_state.core_state.server.get_session_manager();
    let username = match session_manager.find_session(caller_session_id) {
        Some(caller) => caller.user(),
        None => web_state
            .core_state
            .server
            .get_graph_service()
            .auth_config()
            .default_username
            .clone(),
    };
    session_manager
        .create_session(username, "127.0.0.1".to_string())
        .await
        .map_err(|e| WebError::Internal(format!("Failed to create session: {}", e)))
}

/// Route the session to `space_name`, erroring when the space is unknown.
fn bind_space<S>(
    web_state: &WebState<S>,
    session: &ClientSession,
    space_name: &str,
) -> Result<(), WebError>
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
    let space = resolve_space(web_state, space_name)?;
    session.set_space(SpaceSummary::from(&space));
    Ok(())
}

/// Look up `space_name`, reporting an unknown space as not found.
fn resolve_space<S>(web_state: &WebState<S>, space_name: &str) -> Result<SpaceInfo, WebError>
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
    web_state
        .core_state
        .server
        .get_storage()
        .read()
        .get_space(space_name)
        .map_err(|e| WebError::Storage(e.to_string()))?
        .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))
}

/// Render `vid` as an id literal for the space's id type.
///
/// Id comparisons in the engine are typed: a space with integer ids only
/// matches an integer literal, and a string space only a quoted one.
pub(crate) fn vid_literal<S>(
    web_state: &WebState<S>,
    space_name: &str,
    vid: &str,
) -> Result<String, WebError>
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
    let space = resolve_space(web_state, space_name)?;
    Ok(match space.vid_type {
        DataType::String => format!("\"{}\"", vid.replace('\\', "\\\\").replace('"', "\\\"")),
        _ => vid.to_string(),
    })
}
