use gpui_kit::{App, AppContext as _, Entity, Global, SharedString};

/// Connection lifecycle as displayed in the status bar and by view switching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    /// Server became unreachable while a session was believed to be live.
    Unhealthy,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Not connected",
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
            Self::Unhealthy => "Unhealthy",
        }
    }
}

/// Global UI state shared across views. The `SessionManager` itself lives in
/// [`crate::connection::SessionStore`]; this entity only mirrors what the UI
/// renders so views can observe it without touching the client stack.
pub struct AppState {
    pub status: ConnectionStatus,
    pub user: Option<SharedString>,
    pub space: Option<SharedString>,
    pub session_id: Option<i64>,
    pub roles: Vec<String>,
}

pub struct AppStateGlobal(Entity<AppState>);

impl Global for AppStateGlobal {}

impl AppState {
    pub fn init(cx: &mut App) {
        let entity = cx.new(|_| Self {
            status: ConnectionStatus::Disconnected,
            user: None,
            space: None,
            session_id: None,
            roles: Vec::new(),
        });
        cx.set_global(AppStateGlobal(entity));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<AppStateGlobal>().0.clone()
    }

    pub fn set_connecting(&mut self, cx: &mut gpui_kit::Context<Self>) {
        self.status = ConnectionStatus::Connecting;
        cx.notify();
    }

    pub fn set_connected(
        &mut self,
        user: SharedString,
        space: Option<SharedString>,
        session_id: Option<i64>,
        roles: Vec<String>,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        self.status = ConnectionStatus::Connected;
        self.user = Some(user);
        self.space = space;
        self.session_id = session_id;
        self.roles = roles;
        cx.notify();
    }

    pub fn set_disconnected(&mut self, cx: &mut gpui_kit::Context<Self>) {
        self.status = ConnectionStatus::Disconnected;
        self.user = None;
        self.space = None;
        self.session_id = None;
        self.roles.clear();
        cx.notify();
    }

    pub fn set_space(&mut self, space: Option<SharedString>, cx: &mut gpui_kit::Context<Self>) {
        self.space = space;
        cx.notify();
    }

    /// Highest privilege UI role derived from the backend role names.
    /// Unknown or empty roles fall back to the read-only role.
    pub fn ui_role(&self) -> &'static str {
        let mut best = 0;
        for role in &self.roles {
            let rank = match role.to_uppercase().as_str() {
                "GOD" | "ADMIN" => 2,
                "DBA" | "USER" => 1,
                _ => 0,
            };
            best = best.max(rank);
        }
        match best {
            2 => "admin",
            1 => "operator",
            _ => "viewer",
        }
    }

    /// Whether the signed-in identity may open the management panels.
    /// Mirrors the web studio rule where only the admin role manages
    /// users and sessions.
    pub fn can_manage(&self) -> bool {
        self.status == ConnectionStatus::Connected && self.ui_role() == "admin"
    }

    pub fn set_unhealthy(&mut self, unhealthy: bool, cx: &mut gpui_kit::Context<Self>) {
        if self.status == ConnectionStatus::Connected && unhealthy {
            self.status = ConnectionStatus::Unhealthy;
            cx.notify();
        } else if self.status == ConnectionStatus::Unhealthy && !unhealthy {
            self.status = ConnectionStatus::Connected;
            cx.notify();
        }
    }
}
