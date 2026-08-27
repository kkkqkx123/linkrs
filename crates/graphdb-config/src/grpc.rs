//! gRPC server configuration

use serde::{Deserialize, Serialize};

/// gRPC server configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GrpcConfig {
    /// Whether to enable gRPC server
    pub enabled: bool,
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
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9669,
            max_connections: 100,
            max_request_size: 10 * 1024 * 1024,  // 10MB
            max_response_size: 10 * 1024 * 1024, // 10MB
            keepalive_interval_secs: 30,
            keepalive_timeout_secs: 10,
            connection_timeout_secs: 10,
            request_timeout_secs: 60,
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
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.max_request_size, 10 * 1024 * 1024);
        assert_eq!(config.max_response_size, 10 * 1024 * 1024);
        assert_eq!(config.keepalive_interval_secs, 30);
        assert_eq!(config.keepalive_timeout_secs, 10);
        assert_eq!(config.connection_timeout_secs, 10);
        assert_eq!(config.request_timeout_secs, 60);
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
}
