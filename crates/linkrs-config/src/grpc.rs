//! gRPC server configuration

use serde::{Deserialize, Serialize};

use crate::security::SslConfig;

/// gRPC server configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GrpcConfig {
    /// Whether to enable gRPC server
    pub enabled: bool,
    /// Interface the gRPC listener binds to
    pub bind_address: String,
    /// gRPC server port
    pub port: u16,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Maximum request message size (bytes)
    pub max_request_size: usize,
    /// Maximum response message size (bytes)
    pub max_response_size: usize,
    /// Keepalive interval (seconds, 0 to disable)
    pub keepalive_interval_secs: u64,
    /// Keepalive timeout (seconds, 0 to disable)
    pub keepalive_timeout_secs: u64,
    /// Connection timeout (seconds)
    pub connection_timeout_secs: u64,
    /// Request timeout (seconds, 0 to disable)
    pub request_timeout_secs: u64,
    /// TLS material for the gRPC listener. Leave disabled for loopback
    /// deployments; a production listener must supply cert and key.
    #[serde(default)]
    pub tls: SslConfig,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "127.0.0.1".to_string(),
            port: 9669,
            max_connections: 100,
            max_request_size: 10 * 1024 * 1024,  // 10MB
            max_response_size: 10 * 1024 * 1024, // 10MB
            keepalive_interval_secs: 30,
            keepalive_timeout_secs: 10,
            connection_timeout_secs: 10,
            request_timeout_secs: 60,
            tls: SslConfig::default(),
        }
    }
}

impl GrpcConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("gRPC port cannot be 0".to_string());
        }

        if self.max_connections == 0 {
            return Err("Max connections must be greater than 0".to_string());
        }

        if self.max_request_size == 0 {
            return Err("Max request size must be greater than 0".to_string());
        }

        if self.max_response_size == 0 {
            return Err("Max response size must be greater than 0".to_string());
        }

        self.tls.validate()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_config_default() {
        let config = GrpcConfig::default();
        assert!(config.enabled);
        assert_eq!(config.port, 9669);
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.max_request_size, 10 * 1024 * 1024);
        assert_eq!(config.max_response_size, 10 * 1024 * 1024);
        assert_eq!(config.keepalive_interval_secs, 30);
        assert_eq!(config.keepalive_timeout_secs, 10);
        assert_eq!(config.connection_timeout_secs, 10);
        assert_eq!(config.request_timeout_secs, 60);
        assert!(!config.tls.enabled);
    }

    #[test]
    fn test_grpc_config_validate() {
        let config = GrpcConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = GrpcConfig {
            port: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        let invalid_config = GrpcConfig {
            max_connections: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_grpc_tls_requires_cert_and_key() {
        let mut config = GrpcConfig::default();
        assert!(config.validate().is_ok());

        config.tls = SslConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        config.tls = SslConfig {
            enabled: true,
            cert_file: "cert.pem".to_string(),
            key_file: "key.pem".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }
}
