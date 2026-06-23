use serde::{Deserialize, Serialize};

use crate::utils::error::{CliError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub connection: ConnectionConfig,
    #[serde(default)]
    pub output: OutputConfig,
    #[serde(default)]
    pub editor: EditorConfig,
    #[serde(default)]
    pub history: HistoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    #[serde(default = "default_host")]
    pub default_host: String,
    #[serde(default = "default_port")]
    pub default_port: u16,
    #[serde(default = "default_user")]
    pub default_user: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub pager: Option<String>,
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
    #[serde(default = "default_null_string")]
    pub null_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    #[serde(default = "default_editor")]
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_file")]
    pub file: String,
    #[serde(default = "default_max_size")]
    pub max_size: usize,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_user() -> String {
    "root".to_string()
}
fn default_format() -> String {
    "table".to_string()
}
fn default_max_rows() -> usize {
    1000
}
fn default_null_string() -> String {
    "NULL".to_string()
}
fn default_editor() -> String {
    "vim".to_string()
}
fn default_history_file() -> String {
    "~/.graphdb/cli_history".to_string()
}
fn default_max_size() -> usize {
    1000
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            default_host: default_host(),
            default_port: default_port(),
            default_user: default_user(),
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: default_format(),
            pager: None,
            max_rows: default_max_rows(),
            null_string: default_null_string(),
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            command: default_editor(),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            file: default_history_file(),
            max_size: default_max_size(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)
                .map_err(|e| CliError::config(format!("Failed to parse config: {}", e)))?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path();

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::config(format!("Failed to serialize config: {}", e)))?;
        std::fs::write(&config_path, content)?;

        Ok(())
    }

    pub fn config_path() -> std::path::PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        home.join(".graphdb").join("cli.toml")
    }
}
