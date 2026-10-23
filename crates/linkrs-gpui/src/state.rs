use gpui_kit::{App, AppContext as _, Entity, Global, SharedString};

/// Connection lifecycle as displayed in the status bar. `Connecting` and
/// `Connected` are produced once the login view lands.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Not connected",
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
        }
    }
}

/// Global UI state shared across views.
pub struct AppState {
    pub status: ConnectionStatus,
    pub space: Option<SharedString>,
}

pub struct AppStateGlobal(Entity<AppState>);

impl Global for AppStateGlobal {}

impl AppState {
    pub fn init(cx: &mut App) {
        let entity = cx.new(|_| Self {
            status: ConnectionStatus::Disconnected,
            space: None,
        });
        cx.set_global(AppStateGlobal(entity));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<AppStateGlobal>().0.clone()
    }
}
