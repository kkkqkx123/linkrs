use serde::{Deserialize, Serialize};
use std::time::Duration;

use graphdb_embedding::EmbeddingConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EngineType {
    #[default]
    Qdrant,
}

/// Wire transport used to reach a Qdrant service.
///
/// Both transports may be compiled in at once; this selects the one used at
/// runtime. The default follows the historical behavior (gRPC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QdrantTransport {
    #[default]
    Grpc,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorClientConfig {
    pub enabled: bool,
    #[serde(default)]
    pub engine: EngineType,
    pub connection: ConnectionConfig,
    pub timeout: TimeoutConfig,
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
}

impl VectorClientConfig {
    pub fn new(engine: EngineType) -> Self {
        Self {
            enabled: true,
            engine,
            connection: ConnectionConfig::default(),
            timeout: TimeoutConfig::default(),
            embedding: None,
        }
    }

    pub fn qdrant() -> Self {
        Self::new(EngineType::Qdrant)
    }

    pub fn qdrant_local(host: &str, grpc_port: u16, http_port: u16) -> Self {
        Self {
            enabled: true,
            engine: EngineType::Qdrant,
            connection: ConnectionConfig {
                host: host.to_string(),
                port: grpc_port,
                use_tls: false,
                api_key: None,
                connect_timeout_secs: 5,
                http_port: Some(http_port),
                transport: QdrantTransport::default(),
            },
            timeout: TimeoutConfig::default(),
            embedding: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            engine: EngineType::Qdrant,
            connection: ConnectionConfig::default(),
            timeout: TimeoutConfig::default(),
            embedding: None,
        }
    }

    pub fn with_connection(mut self, connection: ConnectionConfig) -> Self {
        self.connection = connection;
        self
    }

    pub fn with_timeout(mut self, timeout: TimeoutConfig) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_embedding(mut self, embedding: EmbeddingConfig) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

impl Default for VectorClientConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
    pub api_key: Option<String>,
    pub connect_timeout_secs: u64,
    pub http_port: Option<u16>,
    /// Which wire transport to use; requires the matching feature to be
    /// compiled in.
    #[serde(default)]
    pub transport: QdrantTransport,
}

impl ConnectionConfig {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            use_tls: false,
            api_key: None,
            connect_timeout_secs: 5,
            http_port: None,
            transport: QdrantTransport::default(),
        }
    }

    pub fn localhost(port: u16) -> Self {
        Self::new("localhost", port)
    }

    pub fn qdrant_local(grpc_port: u16, http_port: u16) -> Self {
        Self {
            host: "localhost".to_string(),
            port: grpc_port,
            use_tls: false,
            api_key: None,
            connect_timeout_secs: 5,
            http_port: Some(http_port),
            transport: QdrantTransport::default(),
        }
    }

    pub fn with_tls(mut self) -> Self {
        self.use_tls = true;
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Select the wire transport used to reach Qdrant.
    pub fn with_transport(mut self, transport: QdrantTransport) -> Self {
        self.transport = transport;
        self
    }

    pub fn to_url(&self) -> String {
        let scheme = if self.use_tls { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }

    pub fn to_grpc_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        // The default transport is gRPC (see [`QdrantTransport`]), so the
        // default port follows Qdrant's gRPC convention. The HTTP engine
        // falls back to its own conventional 6333 when `http_port` is unset.
        Self::localhost(6334)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    pub request_timeout_secs: u64,
    pub search_timeout_secs: u64,
    pub upsert_timeout_secs: u64,
}

impl TimeoutConfig {
    pub fn new(request: u64, search: u64, upsert: u64) -> Self {
        Self {
            request_timeout_secs: request,
            search_timeout_secs: search,
            upsert_timeout_secs: upsert,
        }
    }

    pub fn request_duration(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    pub fn search_duration(&self) -> Duration {
        Duration::from_secs(self.search_timeout_secs)
    }

    pub fn upsert_duration(&self) -> Duration {
        Duration::from_secs(self.upsert_timeout_secs)
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self::new(30, 60, 30)
    }
}

// Simple validation methods
impl VectorClientConfig {
    /// Validate configuration
    ///
    /// Returns Ok(()) if valid, Err(message) if invalid
    pub fn validate(&self) -> Result<(), String> {
        self.connection.validate()?;
        self.timeout.validate()?;
        Ok(())
    }
}

impl ConnectionConfig {
    /// Validate connection configuration
    /// u16 can't be greater than 65535, so only check 0
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("connection.host cannot be empty".to_string());
        }
        if self.port == 0 {
            return Err("connection.port must not be 0".to_string());
        }
        if self.connect_timeout_secs == 0 {
            return Err("connection.connect_timeout_secs must be greater than 0".to_string());
        }
        if self.http_port == Some(0) {
            return Err("connection.http_port must not be 0".to_string());
        }
        Ok(())
    }
}

impl TimeoutConfig {
    /// Validate timeout configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.request_timeout_secs == 0 {
            return Err("timeout.request_timeout_secs must be greater than 0".to_string());
        }
        if self.search_timeout_secs == 0 {
            return Err("timeout.search_timeout_secs must be greater than 0".to_string());
        }
        if self.upsert_timeout_secs == 0 {
            return Err("timeout.upsert_timeout_secs must be greater than 0".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn test_connection_config_valid() {
        let config = ConnectionConfig::new("localhost", 6333);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_connection_config_empty_host() {
        let config = ConnectionConfig::new("", 6333);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_connection_config_invalid_port() {
        let config = ConnectionConfig::new("localhost", 0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_timeout_config_valid() {
        let config = TimeoutConfig::new(30, 60, 30);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_timeout_config_zero() {
        let config = TimeoutConfig::new(0, 60, 30);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_connection_config_tls() {
        let config = ConnectionConfig::localhost(6333).with_tls();
        assert!(config.use_tls);
        assert_eq!(config.to_url(), "https://localhost:6333");
    }

    #[test]
    fn test_connection_config_api_key() {
        let config = ConnectionConfig::localhost(6333).with_api_key("my-key");
        assert_eq!(config.api_key.as_deref(), Some("my-key"));
    }

    #[test]
    fn test_connection_config_to_grpc_url() {
        let config = ConnectionConfig::new("qdrant.example.com", 6334);
        assert_eq!(config.to_grpc_url(), "http://qdrant.example.com:6334");
    }

    #[test]
    fn test_connection_config_transport_selection() {
        let grpc = ConnectionConfig::localhost(6334);
        assert_eq!(grpc.transport, QdrantTransport::Grpc);

        let http = ConnectionConfig::localhost(6333).with_transport(QdrantTransport::Http);
        assert_eq!(http.transport, QdrantTransport::Http);
    }

    #[test]
    fn test_timeout_config_durations() {
        let config = TimeoutConfig::new(10, 20, 30);
        assert_eq!(
            config.request_duration(),
            std::time::Duration::from_secs(10)
        );
        assert_eq!(config.search_duration(), std::time::Duration::from_secs(20));
        assert_eq!(config.upsert_duration(), std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_vector_client_config_new() {
        let config = VectorClientConfig::new(EngineType::Qdrant);
        assert!(config.enabled);
        assert_eq!(config.engine, EngineType::Qdrant);
    }

    #[test]
    fn test_vector_client_config_qdrant_local() {
        let config = VectorClientConfig::qdrant_local("localhost", 6334, 6333);
        assert!(config.enabled);
        assert_eq!(config.connection.port, 6334);
        assert_eq!(config.connection.http_port, Some(6333));
    }

    #[test]
    fn test_vector_client_config_disabled() {
        let config = VectorClientConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_vector_client_config_validate_ok() {
        let config = VectorClientConfig::qdrant();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_vector_client_config_with_connection() {
        let conn = ConnectionConfig::new("other-host", 9999);
        let config = VectorClientConfig::qdrant().with_connection(conn);
        assert_eq!(config.connection.host, "other-host");
    }

    #[test]
    fn test_vector_client_config_with_timeout() {
        let to = TimeoutConfig::new(15, 30, 15);
        let config = VectorClientConfig::qdrant().with_timeout(to);
        assert_eq!(config.timeout.request_timeout_secs, 15);
    }

    #[test]
    fn test_connection_config_qdrant_local() {
        let config = ConnectionConfig::qdrant_local(6334, 6333);
        assert_eq!(config.port, 6334);
        assert_eq!(config.http_port, Some(6333));
    }

    #[test]
    fn test_connection_config_http_port_zero_invalid() {
        let config = ConnectionConfig {
            port: 6333,
            ..ConnectionConfig::localhost(6333)
        };
        // Manually set http_port = 0 to test validation
        let invalid = ConnectionConfig {
            http_port: Some(0),
            ..config
        };
        assert!(invalid.validate().is_err());
    }
}
