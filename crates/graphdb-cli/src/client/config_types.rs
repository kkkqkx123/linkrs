//! Server configuration types

use serde::Deserialize;

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub version: String,
    pub sections: Vec<ConfigSection>,
}

/// Configuration section
#[derive(Debug, Clone)]
pub struct ConfigSection {
    pub name: String,
    pub description: Option<String>,
    pub items: Vec<ConfigItem>,
}

/// Configuration item
#[derive(Debug, Clone)]
pub struct ConfigItem {
    pub key: String,
    pub value: serde_json::Value,
    pub default_value: Option<serde_json::Value>,
    pub description: Option<String>,
    pub mutable: bool,
}

/// Server configuration response (internal)
#[derive(Debug, Deserialize)]
pub(crate) struct ServerConfigResponse {
    pub version: String,
    pub sections: Vec<ConfigSectionData>,
}

/// Configuration section data (internal)
#[derive(Debug, Deserialize)]
pub(crate) struct ConfigSectionData {
    pub name: String,
    pub description: Option<String>,
    pub items: Vec<ConfigItemData>,
}

/// Configuration item data (internal)
#[derive(Debug, Deserialize)]
pub(crate) struct ConfigItemData {
    pub key: String,
    pub value: serde_json::Value,
    pub default_value: Option<serde_json::Value>,
    pub description: Option<String>,
    pub mutable: bool,
}
