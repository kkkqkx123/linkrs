//! Security configuration

use serde::{Deserialize, Serialize};

/// SSL/TLS configuration
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
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
pub struct AuditConfig {
    /// Enable audit logging
    pub enabled: bool,
    /// Audit log file path
    pub log_file: String,
    /// Log successful operations
    pub log_success: bool,
    /// Log failed operations
    pub log_failure: bool,
    /// Log query content
    pub log_query_content: bool,
    /// Maximum log file size (MB)
    pub max_file_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: u32,
}

impl AuditConfig {
    /// Create default configuration
    pub fn new() -> Self {
        Self {
            enabled: false,
            log_file: "logs/audit.log".to_string(),
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

        if self.max_file_size_mb == 0 {
            return Err("Max file size must be greater than 0".to_string());
        }

        if self.max_files == 0 {
            return Err("Max files must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Password policy configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
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
        if self.min_length < 6 {
            return Err("Minimum password length must be at least 6".to_string());
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
        assert_eq!(config.log_file, "logs/audit.log");
        assert!(config.log_success);
        assert!(config.log_failure);
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
    fn test_security_config_default() {
        let config = SecurityConfig::default();
        assert!(!config.is_ssl_configured());
        assert!(!config.is_audit_enabled());
        assert!(config.validate().is_ok());
    }
}
