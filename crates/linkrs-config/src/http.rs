//! HTTP server configuration

use serde::{Deserialize, Serialize};

/// HTTP server configuration
///
/// Configures the HTTP/REST API server behavior.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HttpServerConfig {
    /// Whether to enable HTTP server
    pub enabled: bool,
    /// Bind address
    pub bind_address: String,
    /// Port number
    pub port: u16,
    /// Request timeout (seconds)
    pub request_timeout_secs: u64,
    /// Maximum request body size (bytes)
    pub max_request_size: usize,
    /// CORS settings
    pub cors_enabled: bool,
    /// Allowed CORS origins for strict mode. Empty keeps the permissive
    /// local-development behavior; production must list exact origins.
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Static file serving directory
    pub static_dir: Option<String>,
    /// Enable HTTPS
    pub https_enabled: bool,
    /// HTTPS certificate file path
    pub https_cert_file: Option<String>,
    /// HTTPS private key file path
    pub https_key_file: Option<String>,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "0.0.0.0".to_string(),
            port: 9758,
            request_timeout_secs: 60,
            max_request_size: 10 * 1024 * 1024, // 10MB
            cors_enabled: true,
            cors_allowed_origins: Vec::new(),
            static_dir: None,
            https_enabled: false,
            https_cert_file: None,
            https_key_file: None,
        }
    }
}

impl HttpServerConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("HTTP port cannot be 0".to_string());
        }

        if self.max_request_size == 0 {
            return Err("Max request size must be greater than 0".to_string());
        }

        for origin in &self.cors_allowed_origins {
            Self::validate_origin(origin)?;
        }

        if self.https_enabled {
            if self.https_cert_file.is_none() {
                return Err(
                    "HTTPS certificate file must be specified when HTTPS is enabled".to_string(),
                );
            }
            if self.https_key_file.is_none() {
                return Err("HTTPS key file must be specified when HTTPS is enabled".to_string());
            }
        }

        Ok(())
    }

    /// Check if HTTPS is properly configured
    pub fn is_https_configured(&self) -> bool {
        self.https_enabled && self.https_cert_file.is_some() && self.https_key_file.is_some()
    }

    /// Whether strict CORS mode applies: enabled with a non-empty whitelist.
    pub fn is_strict_cors(&self) -> bool {
        self.cors_enabled && !self.cors_allowed_origins.is_empty()
    }

    /// Validate one whitelist entry: `http(s)://host[:port]` without path.
    fn validate_origin(origin: &str) -> Result<(), String> {
        let rest = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
            .ok_or_else(|| {
                format!(
                    "Invalid CORS origin '{}': scheme must be http or https",
                    origin
                )
            })?;
        if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(format!(
                "Invalid CORS origin '{}': expected http(s)://host[:port] without path",
                origin
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_server_config_default() {
        let config = HttpServerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.port, 9758);
        assert_eq!(config.bind_address, "0.0.0.0");
        assert!(config.cors_enabled);
        assert!(config.cors_allowed_origins.is_empty());
        assert!(!config.https_enabled);
        assert!(!config.is_https_configured());
    }

    #[test]
    fn test_http_server_config_validate() {
        let config = HttpServerConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = HttpServerConfig {
            port: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_http_server_config_https_validation() {
        let config = HttpServerConfig {
            https_enabled: true,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = HttpServerConfig {
            https_enabled: true,
            https_cert_file: Some("cert.pem".to_string()),
            https_key_file: Some("key.pem".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(config.is_https_configured());
    }

    #[test]
    fn test_cors_origin_validation() {
        let mut config = HttpServerConfig::default();
        assert!(config.validate().is_ok());
        assert!(!config.is_strict_cors());

        config.cors_allowed_origins = vec!["https://console.example.com".to_string()];
        assert!(config.validate().is_ok());
        assert!(config.is_strict_cors());

        for bad in [
            "console.example.com",
            "https://console.example.com/app",
            "ftp://console.example.com",
            "https://",
        ] {
            config.cors_allowed_origins = vec![bad.to_string()];
            assert!(config.validate().is_err(), "origin should fail: {bad}");
        }
    }
}
