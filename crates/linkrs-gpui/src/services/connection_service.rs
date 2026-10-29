use std::time::Duration;

use gpui_kit::{App, AsyncApp, SharedString, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;
use crate::state::{AppState, ConnectionStatus};

/// How often the background loop probes the server while a session is live.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Snapshot of the session after a successful login.
pub struct ConnectedProfile {
    pub user: SharedString,
    pub space: Option<SharedString>,
    pub session_id: Option<i64>,
}

/// Connect (and authenticate) against `host:port`. Runs on the tokio runtime;
/// the password never leaves this call.
pub fn connect(
    cx: &App,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> Task<Result<ConnectedProfile, ClientError>> {
    let manager = SessionStore::manager(cx);
    let host = host.to_string();
    let user = user.to_string();
    let password = password.to_string();

    runtime::spawn(cx, async move {
        let mut guard = manager.lock().await;
        guard
            .connect_with_host(&host, port, &user, &password)
            .await?;
        let session = guard.session();
        let space = session.and_then(|s| s.current_space.clone());
        let session_id = session.map(|s| s.session_id);
        let user_name = session
            .map(|s| s.username.clone())
            .unwrap_or_else(|| user.clone());
        Ok(ConnectedProfile {
            user: user_name.into(),
            space: space.map(Into::into),
            session_id,
        })
    })
}

/// End the session server-side. A missing session counts as disconnected.
pub fn disconnect(cx: &App) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let mut guard = manager.lock().await;
        match guard.disconnect().await {
            Err(ClientError::NotConnected) => Ok(()),
            other => other,
        }
    })
}

/// Switch the server-side session to another graph space.
pub fn switch_space(cx: &App, space: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    runtime::spawn(cx, async move {
        let mut guard = manager.lock().await;
        guard.switch_space(&space).await
    })
}

/// Probe server health. `Ok(false)` and transport errors both mean unhealthy.
pub fn health(cx: &App) -> Task<bool> {
    let manager = SessionStore::manager(cx);
    cx.spawn(async move |cx| {
        let task = cx.update(|cx| {
            runtime::spawn(cx, async move {
                let guard = manager.lock().await;
                guard.health_check().await
            })
        });
        task.await.unwrap_or(false)
    })
}

/// Whether an error means the session id is no longer valid and the UI should
/// drop back to the login view.
pub fn is_session_expired(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::AuthError(_) | ClientError::Http { status: 401, .. }
    )
}

/// Spawn the one-per-app heartbeat loop: while the UI believes a session is
/// live, probe the server and flip the status bar to red on failure.
pub fn start_heartbeat(cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx: &mut AsyncApp| loop {
        cx.background_executor().timer(HEARTBEAT_INTERVAL).await;

        let live =
            cx.update(|cx| AppState::global(cx).read(cx).status == ConnectionStatus::Connected);
        if !live {
            continue;
        }

        let healthy = cx.update(|cx| health(cx)).await;
        cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| state.set_unhealthy(!healthy, cx));
        });
    })
}

/// Best-effort local invalidation used when the server reports the session id
/// is no longer valid. The server-side logout is attempted but ignored.
pub fn invalidate_session(cx: &mut App) {
    let manager = SessionStore::manager(cx);
    let _ = runtime::spawn(cx, async move {
        let mut guard = manager.lock().await;
        let _ = guard.disconnect().await;
        Ok(())
    });
    AppState::global(cx).update(cx, |state, cx| state.set_disconnected(cx));
}
