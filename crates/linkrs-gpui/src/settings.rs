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

/// Row density for tables and cards, mirroring the web studio toggle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Dense,
}

impl Density {
    pub fn is_dense(self) -> bool {
        matches!(self, Self::Dense)
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Comfortable => Self::Dense,
            Self::Dense => Self::Comfortable,
        }
    }
}

/// Interface language persisted across runs. English is the default;
/// Chinese strings live in `messages/*.json` so no non-English text
/// appears in Rust sources.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    En,
    Zh,
}

/// Cap on how many distinct connections, history entries and favorites are persisted.
const MAX_RECENT_CONNECTIONS: usize = 8;
const MAX_QUERY_HISTORY: usize = 50;
const MAX_QUERY_FAVORITES: usize = 30;

/// Execution path of a recorded statement, mirroring the frontend history model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryPath {
    #[default]
    Materialized,
    Stream,
}

/// Console execution preference, mirroring the frontend console store:
/// run through the materialized endpoint, the SSE streaming endpoint,
/// or decide per script (multi-statement scripts stream, single
/// statements compare the planner row estimate against the threshold).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionPreference {
    #[default]
    Materialized,
    Stream,
    Auto,
}

/// Default row threshold for automatic stream routing.
pub fn default_auto_stream_threshold() -> u64 {
    1000
}

/// One executed statement with its outcome, newest first in `query_history`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHistoryItem {
    pub sql: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub path: HistoryPath,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

/// A named saved query with an optional preferred execution path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryFavoriteItem {
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub preferred_path: Option<HistoryPath>,
    #[serde(default)]
    pub created_at_ms: u64,
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duty| duty.as_millis() as u64)
        .unwrap_or(0)
}

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
    /// Recently executed statements with outcomes, newest first.
    #[serde(default)]
    pub query_history: Vec<QueryHistoryItem>,
    /// Named saved queries.
    #[serde(default)]
    pub favorites: Vec<QueryFavoriteItem>,
    /// Console execution preference, persisted across restarts.
    #[serde(default)]
    pub execution_preference: ExecutionPreference,
    /// Row threshold for automatic stream routing.
    #[serde(default = "default_auto_stream_threshold")]
    pub auto_stream_threshold: u64,
    /// Interface language, persisted across restarts.
    #[serde(default)]
    pub locale: Locale,
    /// Table/card density preference.
    #[serde(default)]
    pub density: Density,
    /// Expanded sidebar width in logical pixels.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    /// Whether the sidebar starts collapsed to its icon rail.
    #[serde(default)]
    pub sidebar_collapsed: bool,
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

/// Record one executed statement in the local query history (deduplicated by
/// statement text with newest first, capped). Both materialized and streaming
/// runs are recorded, including failures with their error codes.
pub fn record_query(cx: &mut App, item: QueryHistoryItem) {
    if item.sql.trim().is_empty() {
        return;
    }
    update(cx, |settings| {
        settings
            .query_history
            .iter()
            .position(|existing| existing.sql == item.sql)
            .map(|ix| settings.query_history.remove(ix));
        settings.query_history.insert(0, item);
        settings.query_history.truncate(MAX_QUERY_HISTORY);
    });
}

/// Drop all local query history entries.
pub fn clear_history(cx: &mut App) {
    update(cx, |settings| {
        settings.query_history.clear();
    });
}

/// Save the current editor content as a named favorite. Names must be unique
/// (case-insensitive) and non-empty, mirroring the frontend validation.
pub fn add_favorite(
    cx: &mut App,
    name: &str,
    sql: &str,
    preferred_path: Option<HistoryPath>,
) -> Result<(), String> {
    let name = name.trim();
    let sql = sql.trim();
    if name.is_empty() {
        return Err("Favorite name is required.".to_string());
    }
    if sql.is_empty() {
        return Err("Favorite query is required.".to_string());
    }
    let mut outcome = Ok(());
    update(cx, |settings| {
        if settings
            .favorites
            .iter()
            .any(|existing| existing.name.to_lowercase() == name.to_lowercase())
        {
            outcome = Err("A favorite with this name already exists.".to_string());
            return;
        }
        if settings.favorites.len() >= MAX_QUERY_FAVORITES {
            outcome = Err("Favorite limit reached.".to_string());
            return;
        }
        settings.favorites.push(QueryFavoriteItem {
            name: name.to_string(),
            sql: sql.to_string(),
            preferred_path,
            created_at_ms: now_ms(),
        });
    });
    outcome
}

/// Remove the favorite with the given name (case-insensitive).
pub fn remove_favorite(cx: &mut App, name: &str) {
    update(cx, |settings| {
        settings
            .favorites
            .iter()
            .position(|existing| existing.name.to_lowercase() == name.to_lowercase())
            .map(|ix| settings.favorites.remove(ix));
    });
}
