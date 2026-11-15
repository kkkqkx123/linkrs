//! HTTP server configuration

use serde::{Deserialize, Serialize};

/// HTTP server configuration
///
/// `[http]` owns the HTTP listener binding. Defaults bind loopback; reaching
/// the instance from another host requires an explicit bind address.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct HttpServerConfig {
    /// Whether to enable HTTP server
    pub enabled: bool,
    /// Interface the HTTP listener binds to
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
    /// Static file serving directory. Empty (None) disables static hosting
    /// and keeps the pure-API behavior.
    pub static_dir: Option<String>,
    /// Fallback page served for extension-less unknown paths when static
    /// hosting is enabled and `static_fallback` is true.
    #[serde(default = "default_static_index")]
    pub static_index: String,
    /// Rewrite extension-less unknown paths to `static_index` (single-page
    /// application behavior). Missing fingerprinted assets always stay 404.
    #[serde(default = "default_static_fallback")]
    pub static_fallback: bool,
    /// Cache lifetime in seconds for fingerprinted immutable assets
    /// (paths under `/_app/immutable/`).
    #[serde(default = "default_static_immutable_max_age_secs")]
    pub static_immutable_max_age_secs: u64,
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
            bind_address: "127.0.0.1".to_string(),
            port: 9758,
            request_timeout_secs: 60,
            max_request_size: 10 * 1024 * 1024, // 10MB
            cors_enabled: true,
            cors_allowed_origins: Vec::new(),
            static_dir: None,
            static_index: default_static_index(),
            static_fallback: default_static_fallback(),
            static_immutable_max_age_secs: default_static_immutable_max_age_secs(),
            https_enabled: false,
            https_cert_file: None,
            https_key_file: None,
        }
    }
}

/// Default fallback page for static hosting.
fn default_static_index() -> String {
    "index.html".to_string()
}

/// Default single-page application fallback behavior.
fn default_static_fallback() -> bool {
    true
}

/// Default cache lifetime for fingerprinted immutable assets: one year.
fn default_static_immutable_max_age_secs() -> u64 {
    31_556_952
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

        self.validate_static()?;

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

    /// Whether static frontend hosting is enabled.
    ///
    /// A configured non-blank `static_dir` turns hosting on; everything
    /// else keeps the pure-API behavior.
    pub fn is_static_enabled(&self) -> bool {
        self.static_dir
            .as_deref()
            .is_some_and(|dir| !dir.trim().is_empty())
    }

    /// Validate the static hosting companion fields.
    ///
    /// Directory existence is checked at router construction time (so the
    /// error names the resolved path); this only rejects combinations that
    /// are wrong regardless of the filesystem.
    fn validate_static(&self) -> Result<(), String> {
        if self
            .static_dir
            .as_deref()
            .is_some_and(|dir| dir.trim().is_empty())
        {
            return Err(
                "http.static_dir must not be blank; remove it to disable static hosting"
                    .to_string(),
            );
        }
        if self.is_static_enabled() && self.static_fallback && self.static_index.trim().is_empty() {
            return Err(
                "http.static_index must not be empty when static fallback is enabled".to_string(),
            );
        }
        Ok(())
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
        assert_eq!(config.bind_address, "127.0.0.1");
        assert!(config.cors_enabled);
        assert!(config.cors_allowed_origins.is_empty());
        assert!(!config.is_static_enabled());
        assert_eq!(config.static_index, "index.html");
        assert!(config.static_fallback);
        assert_eq!(config.static_immutable_max_age_secs, 31_556_952);
        assert!(!config.https_enabled);
        assert!(!config.is_https_configured());
    }

    #[test]
    fn test_static_hosting_defaults_to_disabled() {
        let mut config = HttpServerConfig::default();
        assert!(!config.is_static_enabled());
        assert!(config.validate().is_ok());

        config.static_dir = Some("web".to_string());
        assert!(config.is_static_enabled());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_static_hosting_rejects_blank_dir_and_index() {
        let mut config = HttpServerConfig::default();
        config.static_dir = Some("   ".to_string());
        assert!(!config.is_static_enabled());
        assert!(config.validate().is_err());

        let mut config = HttpServerConfig::default();
        config.static_dir = Some("web".to_string());
        config.static_index = "  ".to_string();
        assert!(config.validate().is_err());

        let mut config = HttpServerConfig::default();
        config.static_dir = Some("web".to_string());
        config.static_fallback = false;
        config.static_index = String::new();
        assert!(config.validate().is_ok());
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
