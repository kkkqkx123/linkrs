//! Authentication configuration

use serde::{Deserialize, Serialize};
use std::fmt;

/// Authorization configuration
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Whether to enable authorization
    pub enable_authorize: bool,
    /// Maximum failed login attempts (0 means unlimited)
    pub failed_login_attempts: u32,
    /// Session idle timeout (seconds, 0 disables idle reclamation)
    pub session_idle_timeout_secs: u64,
    /// Whether to force changing the default password (on first login)
    pub force_change_default_password: bool,
    /// Default username
    pub default_username: String,
    /// Bcrypt cost factor for password hashing (4-12, higher is slower but safer)
    #[serde(default = "default_bcrypt_cost")]
    pub bcrypt_cost: u32,
    /// Bootstrap password for `default_username`, resolved at startup.
    ///
    /// Never deserialized from, or serialized to, the configuration file:
    /// [`crate::Config::resolve_bootstrap_password`] fills it from the
    /// environment or a generated owner-only secret file.
    #[serde(skip)]
    pub bootstrap_password: String,
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
            bcrypt_cost: default_bcrypt_cost(),
            bootstrap_password: String::new(),
        }
    }
}

/// Redact the bootstrap password wherever the configuration is printed.
impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("enable_authorize", &self.enable_authorize)
            .field("failed_login_attempts", &self.failed_login_attempts)
            .field("session_idle_timeout_secs", &self.session_idle_timeout_secs)
            .field(
                "force_change_default_password",
                &self.force_change_default_password,
            )
            .field("default_username", &self.default_username)
            .field("bcrypt_cost", &self.bcrypt_cost)
            .field("bootstrap_password", &"***")
            .finish()
    }
}

impl AuthConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.default_username.is_empty() {
            return Err("Default username cannot be empty".to_string());
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
        assert_eq!(config.bcrypt_cost, 12);
        assert!(config.bootstrap_password.is_empty());
    }

    #[test]
    fn test_auth_config_debug_redacts_bootstrap_password() {
        let config = AuthConfig {
            bootstrap_password: "super-secret".to_string(),
            ..Default::default()
        };
        let rendered = format!("{:?}", config);
        assert!(
            !rendered.contains("super-secret"),
            "debug output leaked the bootstrap password: {rendered}"
        );
        assert!(
            rendered.contains("bootstrap_password: \"***\""),
            "{rendered}"
        );
    }

    #[test]
    fn test_bootstrap_password_is_not_serialized() {
        let config = AuthConfig {
            bootstrap_password: "super-secret".to_string(),
            ..Default::default()
        };
        let toml_text = toml::to_string(&config).expect("serialize auth config");
        assert!(
            !toml_text.contains("bootstrap_password"),
            "auth config must never persist the bootstrap password: {toml_text}"
        );
        let round_tripped: AuthConfig =
            toml::from_str(&toml_text).expect("deserialize auth config");
        assert!(round_tripped.bootstrap_password.is_empty());
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
