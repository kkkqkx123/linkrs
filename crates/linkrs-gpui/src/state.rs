use gpui_kit::{App, AppContext as _, Entity, Global, SharedString};

use crate::services::graph_service::Neighborhood;
use crate::settings::Locale;

/// Connection lifecycle as displayed in the status bar and by view switching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    /// Server became unreachable while a session was believed to be live.
    Unhealthy,
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
    pub locale: Locale,
    /// Latest console result import for the graph panel, with the
    /// generation that published it.
    pub graph_drop: Option<(u64, Neighborhood)>,
    pub graph_generation: u64,
}

pub struct AppStateGlobal(Entity<AppState>);

impl Global for AppStateGlobal {}

/// Client capability checked at panel entries, mirroring the web studio
/// role gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    ManageUsers,
    Write,
    AlterSchema,
    DropSpace,
    ManageConfig,
    ManageBatch,
}

impl AppState {
    pub fn init(cx: &mut App) {
        let locale = crate::settings::global(cx).locale;
        let entity = cx.new(|_| Self {
            status: ConnectionStatus::Disconnected,
            user: None,
            space: None,
            session_id: None,
            roles: Vec::new(),
            locale,
            graph_drop: None,
            graph_generation: 0,
        });
        cx.set_global(AppStateGlobal(entity));
    }

    /// Switch the interface language. `Entity::update` notifies observers,
    /// so observing views re-render with the new catalogue.
    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
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

    /// Whether the signed-in identity holds a client capability.
    /// Mirrors the web studio `can()` gates: user and session management,
    /// schema and config writes and batch jobs stay admin-only, plain
    /// querying stays open to operators, and viewers are read-only.
    /// Disconnected sessions hold no capability.
    pub fn can(&self, capability: Capability) -> bool {
        if self.status != ConnectionStatus::Connected {
            return false;
        }
        let admin = self.ui_role() == "admin";
        let operator = self.ui_role() != "viewer";
        match capability {
            Capability::ManageUsers => admin,
            Capability::Write => operator,
            Capability::AlterSchema => admin,
            Capability::DropSpace => admin,
            Capability::ManageConfig => admin,
            Capability::ManageBatch => admin,
        }
    }

    /// Publish a console result import for the graph panel. The generation
    /// counter lets every observer detect a fresh drop independently.
    pub fn push_graph_drop(&mut self, drop: Neighborhood, cx: &mut gpui_kit::Context<Self>) {
        self.graph_generation = self.graph_generation.wrapping_add(1);
        self.graph_drop = Some((self.graph_generation, drop));
        cx.notify();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_with(roles: &[&str]) -> AppState {
        AppState {
            status: ConnectionStatus::Connected,
            user: Some("tester".into()),
            space: None,
            session_id: Some(1),
            roles: roles.iter().map(|role| role.to_string()).collect(),
            locale: Locale::En,
            graph_drop: None,
            graph_generation: 0,
        }
    }

    #[test]
    fn admin_holds_every_capability() {
        let state = connected_with(&["ADMIN"]);
        for capability in [
            Capability::ManageUsers,
            Capability::Write,
            Capability::AlterSchema,
            Capability::DropSpace,
            Capability::ManageConfig,
            Capability::ManageBatch,
        ] {
            assert!(state.can(capability), "{capability:?} granted to admin");
        }
    }

    #[test]
    fn operator_keeps_write_only() {
        let state = connected_with(&["USER"]);
        assert!(state.can(Capability::Write));
        assert!(!state.can(Capability::ManageUsers));
        assert!(!state.can(Capability::ManageBatch));
        assert!(!state.can(Capability::AlterSchema));
        assert!(!state.can(Capability::DropSpace));
        assert!(!state.can(Capability::ManageConfig));
    }

    #[test]
    fn viewer_is_read_only() {
        let state = connected_with(&["GUEST"]);
        assert!(!state.can(Capability::Write));
        assert!(!state.can(Capability::ManageBatch));
    }

    #[test]
    fn disconnected_holds_nothing() {
        let mut state = connected_with(&["GOD"]);
        state.status = ConnectionStatus::Disconnected;
        assert!(!state.can(Capability::Write));
        assert!(!state.can(Capability::ManageUsers));
    }
}
