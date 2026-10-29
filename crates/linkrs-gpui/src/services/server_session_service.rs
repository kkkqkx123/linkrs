use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// List server-side sessions.
pub fn list(cx: &App) -> Task<Result<Vec<serde_json::Value>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_server_sessions().await
    })
}

/// Get one server-side session.
pub fn get(cx: &App, session_id: i64) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_server_session(session_id).await
    })
}

/// Terminate a server-side session.
pub fn kill(cx: &App, session_id: i64) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().kill_server_session(session_id).await
    })
}

/// Get the current authenticated identity.
pub fn me(cx: &App) -> Task<Result<linkrs_wire::meta::AuthMeResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().auth_me().await
    })
}
