use std::time::Duration;

use gpui_kit::{App, AsyncApp, SharedString, Task};
use linkrs_client::ClientError;

use crate::connection::{credentials, SessionStore};
use crate::runtime;
use crate::settings;
use crate::state::{AppState, ConnectionStatus};

/// How often the background loop probes the server while a session is live.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Consecutive failed probes before the status bar turns red. A single probe
/// can fail on a transient network blip, so we require a short streak.
const HEARTBEAT_FAILURE_THRESHOLD: u32 = 3;

/// Snapshot of the session after a successful login.
pub struct ConnectedProfile {
    pub user: SharedString,
    pub space: Option<SharedString>,
    pub session_id: Option<i64>,
    pub roles: Vec<String>,
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
        // Refresh identity via `me` so roles track server truth instead of
        // only the login response payload.
        let me = guard.client().auth_me().await.ok();
        let session = guard.session();
        let space = session.and_then(|s| s.current_space.clone());
        let session_id = session.map(|s| s.session_id);
        let (user_name, roles) = match me {
            Some(me) => (me.username, me.roles),
            None => (
                session
                    .map(|s| s.username.clone())
                    .unwrap_or_else(|| user.clone()),
                session.map(|s| s.roles.clone()).unwrap_or_default(),
            ),
        };
        Ok(ConnectedProfile {
            user: user_name.into(),
            space: space.map(Into::into),
            session_id,
            roles,
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

/// Spawn the one-per-app heartbeat loop: while a session is live (Connected or
/// already Unhealthy), probe the server and flip the status bar on a sustained
/// failure streak. Probing continues while Unhealthy so a recovering server is
/// detected and the status can return to Connected.
pub fn start_heartbeat(cx: &mut App) -> Task<()> {
    let mut consecutive_failures: u32 = 0;
    cx.spawn(async move |cx: &mut AsyncApp| loop {
        cx.background_executor().timer(HEARTBEAT_INTERVAL).await;

        let session_live = cx.update(|cx| {
            let status = AppState::global(cx).read(cx).status;
            status == ConnectionStatus::Connected || status == ConnectionStatus::Unhealthy
        });
        if !session_live {
            continue;
        }

        let healthy = cx.update(|cx| health(cx)).await;
        if healthy {
            consecutive_failures = 0;
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1);
        }
        let offline = consecutive_failures >= HEARTBEAT_FAILURE_THRESHOLD;
        cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| state.set_unhealthy(offline, cx));
        });
    })
}

/// Handle a session the server no longer accepts (401 / auth error).
///
/// When the endpoint has a password saved in the OS keychain, a single silent
/// re-login is attempted and the previous space is restored. Without a stored
/// credential, or after a short failure budget is exhausted, the client drops
/// to the sign-in view. Concurrent 401s share one in-flight attempt so a burst
/// of failing requests cannot trigger a login storm.
pub fn invalidate_session(cx: &mut App) {
    let reconnect = SessionStore::reconnect(cx);
    if !reconnect.try_start() {
        return;
    }

    let last = settings::global(cx).last_connection.clone();
    let Some(last) = last else {
        reconnect.abort();
        drop_to_login(cx);
        return;
    };
    let space = AppState::global(cx).read(cx).space.clone();
    let executor = cx.background_executor().clone();

    cx.spawn(async move |cx: &mut AsyncApp| {
        // Keychain I/O is blocking; keep it off the UI thread.
        let load_host = last.host.clone();
        let load_user = last.user.clone();
        let load_port = last.port;
        let password: Option<String> = executor
            .spawn(async move { credentials::load(&load_host, load_port, &load_user) })
            .await;

        let Some(password) = password else {
            reconnect.abort();
            cx.update(|cx: &mut App| drop_to_login(cx));
            return;
        };

        let outcome = cx
            .update(|cx: &mut App| connect(cx, &last.host, last.port, &last.user, &password))
            .await;

        match outcome {
            Ok(mut profile) => {
                // A fresh login starts with no selected space; restore the one
                // that was active when the session expired.
                if let Some(ref saved) = space {
                    let restored = cx.update(|cx: &mut App| switch_space(cx, saved)).await;
                    if restored.is_ok() {
                        profile.space = Some(saved.clone());
                    }
                }
                reconnect.reset();
                cx.update(|cx: &mut App| {
                    let locale = crate::i18n::locale(cx);
                    AppState::global(cx).update(cx, |state, cx| {
                        state.set_connected(
                            profile.user,
                            profile.space,
                            profile.session_id,
                            profile.roles,
                            cx,
                        );
                        // The old session (and any explicit transactions bound
                        // to it) is gone. Tell the user to re-check uncommitted
                        // work; we cannot detect it because the client does not
                        // track transaction ids and the expired id cannot be
                        // queried from the server.
                        state.push_notice(crate::i18n::t("status.reconnectedNotice", locale), cx);
                    });
                });
            }
            Err(error) => {
                log::warn!("silent re-login failed: {}", error);
                if reconnect.finish_failure() {
                    cx.update(|cx: &mut App| drop_to_login(cx));
                }
            }
        }
    })
    .detach();
}

/// Tear down the local session and return to the sign-in view. Shared by the
/// re-login fallback paths; the server-side logout is best-effort and ignored.
fn drop_to_login(cx: &mut App) {
    // Clear any partial failure streak so a later manual login starts fresh.
    SessionStore::reconnect(cx).reset();
    let manager = SessionStore::manager(cx);
    drop(runtime::spawn(cx, async move {
        let mut guard = manager.lock().await;
        let _ = guard.disconnect().await;
        Ok(())
    }));
    AppState::global(cx).update(cx, |state, cx| state.set_disconnected(cx));
}
