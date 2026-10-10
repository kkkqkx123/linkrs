//! Security configuration

use serde::{Deserialize, Serialize};

/// SSL/TLS configuration
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SslConfig {
    /// Enable SSL/TLS
    pub enabled: bool,
    /// Certificate file path
    pub cert_file: String,
    /// Private key file path
    pub key_file: String,
    /// CA certificate file path (optional, for client verification)
    pub ca_file: Option<String>,
    /// Require client certificate verification
    pub require_client_cert: bool,
}

impl SslConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled {
            if self.cert_file.is_empty() {
                return Err("Certificate file must be specified when SSL is enabled".to_string());
            }
            if self.key_file.is_empty() {
                return Err("Key file must be specified when SSL is enabled".to_string());
            }
        }
        Ok(())
    }

    /// Check if SSL is properly configured
    pub fn is_configured(&self) -> bool {
        self.enabled && !self.cert_file.is_empty() && !self.key_file.is_empty()
    }
}

/// Audit log configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    /// Enable audit logging
    pub enabled: bool,
    /// Audit log file name or absolute path. A bare file name resolves
    /// against `[log].dir`.
    pub log_file: String,
    /// Log successful operations
    pub log_success: bool,
    /// Log failed operations
    pub log_failure: bool,
    /// Log query content
    pub log_query_content: bool,
    /// Maximum log file size in megabytes before rotation
    pub max_file_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: u32,
}

impl AuditConfig {
    /// Create default configuration
    pub fn new() -> Self {
        Self {
            enabled: false,
            log_file: "audit.log".to_string(),
            log_success: true,
            log_failure: true,
            log_query_content: false,
            max_file_size_mb: 100,
            max_files: 10,
        }
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.log_file.is_empty() {
            return Err("Audit log file path cannot be empty".to_string());
        }

        crate::validate_log_file_field(
            "security.audit.log_file",
            &self.log_file,
            self.max_file_size_mb,
            self.max_files,
        )?;

        Ok(())
    }
}

/// Password policy configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PasswordPolicyConfig {
    /// Minimum password length
    pub min_length: usize,
    /// Require uppercase letters
    pub require_uppercase: bool,
    /// Require lowercase letters
    pub require_lowercase: bool,
    /// Require digits
    pub require_digit: bool,
    /// Require special characters
    pub require_special: bool,
    /// Maximum password age (days, 0 = no expiration)
    pub max_age_days: u64,
    /// Password history size (0 = no history)
    pub history_size: usize,
}

/// Special characters recognized by the password policy.
///
/// Restricted to a common printable-ASCII symbol subset so strength
/// classification stays unambiguous across Unicode inputs.
pub const SPECIAL_CHARACTERS: &str = "!@#$%^&*()-_=+[]{}|;:',.<>?/`~\"\\";

impl Default for PasswordPolicyConfig {
    fn default() -> Self {
        Self {
            min_length: 8,
            require_uppercase: true,
            require_lowercase: true,
            require_digit: true,
            require_special: false,
            max_age_days: 0, // No expiration
            history_size: 0, // No history
        }
    }
}

impl PasswordPolicyConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.min_length < 8 {
            return Err("Minimum password length must be at least 8".to_string());
        }

        Ok(())
    }

    /// Check a plaintext password against the complexity rules.
    ///
    /// Covers length, enabled character classes, and the username-inclusion
    /// rule. Returns the first unsatisfied requirement so callers can
    /// surface a single actionable reason.
    pub fn validate_password(&self, username: &str, password: &str) -> Result<(), String> {
        if password.chars().count() < self.min_length {
            return Err(format!(
                "password must be at least {} characters long",
                self.min_length
            ));
        }
        if self.require_uppercase && !password.chars().any(|c| c.is_ascii_uppercase()) {
            return Err("password must contain at least one uppercase letter".to_string());
        }
        if self.require_lowercase && !password.chars().any(|c| c.is_ascii_lowercase()) {
            return Err("password must contain at least one lowercase letter".to_string());
        }
        if self.require_digit && !password.chars().any(|c| c.is_ascii_digit()) {
            return Err("password must contain at least one digit".to_string());
        }
        if self.require_special && !password.chars().any(|c| SPECIAL_CHARACTERS.contains(c)) {
            return Err("password must contain at least one special character".to_string());
        }
        if !username.is_empty() && password.to_lowercase().contains(&username.to_lowercase()) {
            return Err("password must not contain the username".to_string());
        }
        Ok(())
    }

    /// Check if password complexity requirements are configured
    pub fn has_complexity_requirements(&self) -> bool {
        self.require_uppercase
            || self.require_lowercase
            || self.require_digit
            || self.require_special
    }

    /// Check if password expiration is enabled
    pub fn has_expiration(&self) -> bool {
        self.max_age_days > 0
    }

    /// Check if password history is enabled
    pub fn has_history(&self) -> bool {
        self.history_size > 0
    }
}

/// Security configuration aggregator
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// SSL/TLS configuration
    #[serde(default)]
    pub ssl: SslConfig,

    /// Audit logging configuration
    #[serde(default)]
    pub audit: AuditConfig,

    /// Password policy configuration
    #[serde(default)]
    pub password_policy: PasswordPolicyConfig,
}

impl SecurityConfig {
    /// Validate all security configurations
    pub fn validate(&self) -> Result<(), String> {
        self.ssl.validate()?;
        self.audit.validate()?;
        self.password_policy.validate()?;
        Ok(())
    }

    /// Check if SSL is properly configured
    pub fn is_ssl_configured(&self) -> bool {
        self.ssl.is_configured()
    }

    /// Check if audit logging is enabled
    pub fn is_audit_enabled(&self) -> bool {
        self.audit.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssl_config_default() {
        let config = SslConfig::default();
        assert!(!config.enabled);
        assert!(!config.is_configured());
    }

    #[test]
    fn test_ssl_config_validate() {
        let config = SslConfig {
            enabled: true,
            cert_file: "cert.pem".to_string(),
            key_file: "key.pem".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(config.is_configured());

        let config = SslConfig {
            enabled: true,
            cert_file: String::new(),
            key_file: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_audit_config_default() {
        let config = AuditConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.log_file, "audit.log");
        assert!(config.log_success);
        assert!(config.log_failure);
    }

    #[test]
    fn test_audit_config_rejects_relative_directory() {
        let config = AuditConfig::default();
        assert!(config.validate().is_ok());

        let nested = AuditConfig {
            log_file: "logs/audit.log".to_string(),
            ..Default::default()
        };
        assert!(nested.validate().is_err());

        let absolute = AuditConfig {
            log_file: "/var/log/linkrs/audit.log".to_string(),
            ..Default::default()
        };
        assert!(absolute.validate().is_ok());
    }

    #[test]
    fn test_password_policy_config_default() {
        let config = PasswordPolicyConfig::default();
        assert_eq!(config.min_length, 8);
        assert!(config.require_uppercase);
        assert!(config.require_lowercase);
        assert!(config.require_digit);
        assert!(!config.require_special);
        assert!(config.has_complexity_requirements());
        assert!(!config.has_expiration());
        assert!(!config.has_history());
    }

    #[test]
    fn test_password_policy_min_length_floor() {
        let weak_floor = PasswordPolicyConfig {
            min_length: 6,
            ..Default::default()
        };
        assert!(weak_floor.validate().is_err());
        let floor = PasswordPolicyConfig {
            min_length: 8,
            ..Default::default()
        };
        assert!(floor.validate().is_ok());
    }

    #[test]
    fn test_password_policy_validate_password() {
        let policy = PasswordPolicyConfig::default();
        assert!(policy.validate_password("alice", "Str0ngPass").is_ok());
        assert!(policy.validate_password("alice", "Short1").is_err());
        assert!(policy.validate_password("alice", "alllowercase1").is_err());
        assert!(policy.validate_password("alice", "ALLUPPERCASE1").is_err());
        assert!(policy.validate_password("alice", "NoDigitsHere").is_err());
        assert!(policy.validate_password("alice", "Alice2024X").is_err());
        let relaxed = PasswordPolicyConfig {
            require_special: true,
            ..Default::default()
        };
        assert!(relaxed.validate_password("bob", "Str0ngPass").is_err());
        assert!(relaxed.validate_password("bob", "Str0ngPass!").is_ok());
    }

    #[test]
    fn test_security_config_default() {
        let config = SecurityConfig::default();
        assert!(!config.is_ssl_configured());
        assert!(!config.is_audit_enabled());
        assert!(config.validate().is_ok());
    }
}
