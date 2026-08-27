//! Bootstrap configuration

use serde::{Deserialize, Serialize};

/// Bootstrap configuration
///
/// Controls initial database setup and single-user mode.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BootstrapConfig {
    /// Whether to automatically create the default Space
    pub auto_create_default_space: bool,
    /// Default Space name
    pub default_space_name: String,
    /// Single-user mode (skip authentication, always use the default user)
    pub single_user_mode: bool,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            auto_create_default_space: true,
            default_space_name: "default".to_string(),
            single_user_mode: false,
        }
    }
}

impl BootstrapConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.default_space_name.is_empty() {
            return Err("Default space name cannot be empty".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bootstrap_config_default() {
        let config = BootstrapConfig::default();
        assert!(config.auto_create_default_space);
        assert_eq!(config.default_space_name, "default");
        assert!(!config.single_user_mode);
    }

    #[test]
    fn test_bootstrap_config_validate() {
        let config = BootstrapConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = BootstrapConfig {
            default_space_name: String::new(),
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
