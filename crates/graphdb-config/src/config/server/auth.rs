//! Authentication configuration

use serde::{Deserialize, Serialize};

/// Authorization configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthConfig {
    /// Whether to enable authorization
    pub enable_authorize: bool,
    /// Maximum failed login attempts (0 means unlimited)
    pub failed_login_attempts: u32,
    /// Session idle timeout (seconds)
    pub session_idle_timeout_secs: u64,
    /// Whether to force changing the default password (on first login)
    pub force_change_default_password: bool,
    /// Default username
    pub default_username: String,
    /// Default password (used only on first start or in single-user mode)
    pub default_password: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enable_authorize: true,
            failed_login_attempts: 5,
            session_idle_timeout_secs: 3600,
            force_change_default_password: true,
            default_username: "root".to_string(),
            default_password: "root".to_string(),
        }
    }
}

impl AuthConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.default_username.is_empty() {
            return Err("Default username cannot be empty".to_string());
        }

        if self.default_password.is_empty() {
            return Err("Default password cannot be empty".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(config.enable_authorize);
        assert_eq!(config.failed_login_attempts, 5);
        assert_eq!(config.session_idle_timeout_secs, 3600);
        assert!(config.force_change_default_password);
        assert_eq!(config.default_username, "root");
        assert_eq!(config.default_password, "root");
    }

    #[test]
    fn test_auth_config_validate() {
        let config = AuthConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = AuthConfig {
            default_username: String::new(),
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
