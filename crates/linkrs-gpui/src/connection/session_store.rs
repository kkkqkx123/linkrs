use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use linkrs_client::{ClientError, SessionManager};

/// Process-wide holder of the client session. The `SessionManager` is shared
/// with tokio-side request tasks through a `tokio::sync::Mutex` (guards are
/// held across awaits of real HTTP calls); UI state mirrors only snapshots,
/// so the UI thread never locks it.
pub struct SessionStore {
    manager: Arc<tokio::sync::Mutex<SessionManager>>,
    /// Keeps the background health-check loop alive for the whole app run.
    _heartbeat: Option<Task<()>>,
}

impl Global for SessionStore {}

impl SessionStore {
    pub fn new(host: &str, port: u16) -> Result<Self, ClientError> {
        Ok(Self {
            manager: Arc::new(tokio::sync::Mutex::new(SessionManager::new_http(
                host, port,
            )?)),
            _heartbeat: None,
        })
    }

    pub fn init(store: Self, cx: &mut App) {
        cx.set_global(store);
    }

    pub fn manager(cx: &App) -> Arc<tokio::sync::Mutex<SessionManager>> {
        cx.global::<SessionStore>().manager.clone()
    }

    pub fn set_heartbeat(&mut self, task: Task<()>) {
        self._heartbeat = Some(task);
    }
}
