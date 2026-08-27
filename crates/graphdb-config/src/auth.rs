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
    /// Bcrypt cost factor for password hashing (4-12, higher is slower but safer)
    #[serde(default = "default_bcrypt_cost")]
    pub bcrypt_cost: u32,
}

/// Default bcrypt cost factor (bcrypt::DEFAULT_COST)
fn default_bcrypt_cost() -> u32 {
    12
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
            bcrypt_cost: default_bcrypt_cost(),
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

        if !(4..=12).contains(&self.bcrypt_cost) {
            return Err("Bcrypt cost must be between 4 and 12".to_string());
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
        assert_eq!(config.bcrypt_cost, 12);
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

        let invalid_cost = AuthConfig {
            bcrypt_cost: 3,
            ..Default::default()
        };
        assert!(invalid_cost.validate().is_err());
    }
}
