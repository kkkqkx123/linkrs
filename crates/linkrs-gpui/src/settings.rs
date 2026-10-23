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
