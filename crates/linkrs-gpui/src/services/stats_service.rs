use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_wire::meta::SessionStatistics;

use crate::connection::SessionStore;
use crate::runtime;
use crate::state::AppState;

/// Session statistics for the current session.
pub fn session(cx: &App) -> Task<Result<SessionStatistics, ClientError>> {
    let manager = SessionStore::manager(cx);
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let client = manager.lock().await.client_clone();
        let id = session_id.ok_or(ClientError::NotConnected)?;
        client.get_session_statistics(id).await
    })
}

/// Search statistics.
pub fn search(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let client = manager.lock().await.client_clone();
        client.get_search_statistics().await
    })
}

/// Freeze statistics.
pub fn freeze(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let client = manager.lock().await.client_clone();
        client.get_freeze_statistics().await
    })
}

/// Trigger a freeze.
pub fn trigger_freeze(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let client = manager.lock().await.client_clone();
        client.trigger_freeze().await
    })
}

/// Migration statistics.
pub fn migration(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let client = manager.lock().await.client_clone();
        client.get_migration_statistics().await
    })
}
