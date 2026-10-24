use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use gpui_kit::App;
use serde::{Deserialize, Serialize};

/// Theme preference persisted across runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeModeSetting {
    #[default]
    System,
    Light,
    Dark,
}

/// Most recent connection attempt, used to prefill the login form.
/// Passwords are never persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastConnection {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl LastConnection {
    pub fn label(&self) -> String {
        format!("{}:{} ({})", self.host, self.port, self.user)
    }
}

/// Cap on how many distinct connections and recent queries are persisted.
const MAX_RECENT_CONNECTIONS: usize = 8;
const MAX_QUERY_HISTORY: usize = 50;

/// Window position and size in logical pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Persisted application settings (`gpui.toml` under the system config dir).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub theme_mode: Option<ThemeModeSetting>,
    pub last_connection: Option<LastConnection>,
    pub window: Option<WindowGeometry>,
    /// Distinct recent connections, newest first, without passwords.
    #[serde(default)]
    pub recent_connections: Vec<LastConnection>,
    /// Recently executed SQL scripts, newest first.
    #[serde(default)]
    pub query_history: Vec<String>,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        ProjectDirs::from("", "", "linkrs").map(|dirs| dirs.config_dir().join("gpui.toml"))
    }

    fn dock_layout_path() -> Option<PathBuf> {
        ProjectDirs::from("", "", "linkrs").map(|dirs| dirs.config_dir().join("dock_layout.json"))
    }

    /// Load settings from disk, falling back to defaults when missing or invalid.
    pub fn load() -> Self {
        Self::path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| toml::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Atomically write settings to disk. Failures are logged and ignored so a
    /// read-only config dir never blocks startup or shutdown.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let Ok(raw) = toml::to_string_pretty(self) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                log::warn!("failed to create config dir {}: {err}", parent.display());
                return;
            }
        }
        let tmp = path.with_extension("toml.tmp");
        if let Err(err) = fs::write(&tmp, raw) {
            log::warn!("failed to write settings {}: {err}", tmp.display());
            return;
        }
        if let Err(err) = fs::rename(&tmp, &path) {
            log::warn!("failed to replace settings {}: {err}", path.display());
        }
    }

    /// Persist a dock layout dump. Same failure semantics as [`Self::save`].
    pub fn save_dock_layout(json: &str) {
        let Some(path) = Self::dock_layout_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                log::warn!("failed to create config dir {}: {err}", parent.display());
                return;
            }
        }
        let tmp = path.with_extension("json.tmp");
        if let Err(err) = fs::write(&tmp, json) {
            log::warn!("failed to write dock layout {}: {err}", tmp.display());
            return;
        }
        if let Err(err) = fs::rename(&tmp, &path) {
            log::warn!("failed to replace dock layout {}: {err}", path.display());
        }
    }

    pub fn load_dock_layout() -> Option<String> {
        Self::dock_layout_path().and_then(|path| fs::read_to_string(path).ok())
    }
}

/// App-wide access to the current settings snapshot.
pub struct SettingsCell(Settings);

impl gpui_kit::Global for SettingsCell {}

pub fn init_global(settings: Settings, cx: &mut App) {
    cx.set_global(SettingsCell(settings));
}

pub fn global(cx: &App) -> Settings {
    cx.global::<SettingsCell>().0.clone()
}

pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let cell = cx.global_mut::<SettingsCell>();
    f(&mut cell.0);
    cell.0.save();
}

/// Record a successful connection: becomes `last_connection` and moves to the
/// front of `recent_connections` (deduplicated, capped, never persisted with a
/// password).
pub fn record_connection(cx: &mut App, host: &str, port: u16, user: &str) {
    update(cx, |settings| {
        let entry = LastConnection {
            host: host.to_string(),
            port,
            user: user.to_string(),
        };
        settings.last_connection = Some(entry.clone());
        settings
            .recent_connections
            .iter()
            .position(|existing| existing == &entry)
            .map(|ix| settings.recent_connections.remove(ix));
        settings.recent_connections.insert(0, entry);
        settings.recent_connections.truncate(MAX_RECENT_CONNECTIONS);
    });
}

/// Record an executed script in the local query history (deduplicated, capped).
pub fn record_query(cx: &mut App, sql: &str) {
    let sql = sql.trim();
    if sql.is_empty() {
        return;
    }
    update(cx, |settings| {
        settings
            .query_history
            .iter()
            .position(|existing| existing == sql)
            .map(|ix| settings.query_history.remove(ix));
        settings.query_history.insert(0, sql.to_string());
        settings.query_history.truncate(MAX_QUERY_HISTORY);
    });
}
