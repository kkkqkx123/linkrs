//! Configuration Management
//!
//! Unified configuration management for different usage patterns.
//!
//! # Module Structure
//!
//! The configuration system is organized into three main modules:
//!
//! - **common**: Configuration shared across all usage patterns (database, storage, logging, etc.)
//! - **server**: Server-specific configuration (gRPC, HTTP, auth, telemetry, etc.) - requires `server` feature
//! - **embedded**: Embedded-specific configuration (runtime settings) - requires `embedded` feature
//!
//! # Usage
//!
//! ## Server Mode
//!
//! ```rust,no_run
//! use graphdb_config::config::Config;
//!
//! // Load from file
//! let config = Config::load("config.toml").expect("Failed to load config");
//!
//! // Or create default
//! let config = Config::default();
//! ```
//!
//! ## Embedded Mode
//!
//! ```rust,ignore
//! use graphdb_config::config::{Config, EmbeddedConfig};
//!
//! let mut config = Config::default();
//! config.embedded.runtime.cache_size_mb = 128;
//! ```

pub mod common;
pub mod embedded;
pub mod logging;
pub mod server;

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub use common::*;
pub use embedded::*;
pub use server::*;

// Re-export commonly used types for backward compatibility
pub use common::database::DatabaseConfig;
pub use common::log::LogConfig;
pub use common::monitoring::{MonitoringConfig, SlowQueryLogConfig};
pub use common::optimizer::{OptimizerConfig, OptimizerRulesConfig};
pub use common::storage::{
    CompressionAlgorithm, QueryResourceConfig, StorageConfig, StorageEngine,
};
pub use common::transaction::TransactionConfig;

#[cfg(feature = "server")]
pub use server::auth::AuthConfig;
#[cfg(feature = "server")]
pub use server::bootstrap::BootstrapConfig;
#[cfg(feature = "server")]
pub use server::connection_pool::ConnectionPoolConfig;
#[cfg(feature = "server")]
pub use server::grpc::GrpcConfig;
#[cfg(feature = "server")]
pub use server::http::HttpServerConfig;
#[cfg(feature = "server")]
pub use server::security::{AuditConfig, PasswordPolicyConfig, SecurityConfig, SslConfig};

#[cfg(feature = "qdrant")]
use vector_client::VectorClientConfig;

/// Global configuration aggregator
///
/// This is the main configuration structure that combines all configuration sections.
/// Use [`Config::default()`] to create a default configuration, or [`Config::load()`] to load from a file.
///
/// # Examples
///
/// ```rust
/// use graphdb_config::config::Config;
///
/// // Create default configuration
/// let config = Config::default();
///
/// // Access configuration sections
/// println!("Database port: {}", config.common.database.port);
/// #[cfg(feature = "server")]
/// println!("gRPC enabled: {}", config.server.grpc.enabled);
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    /// Common configuration (always available)
    #[serde(flatten)]
    pub common: CommonConfig,

    /// Server-specific configuration (only available with `server` feature)
    #[cfg(feature = "server")]
    #[serde(default)]
    pub server: ServerConfig,

    /// Embedded-specific configuration (only available with `embedded` feature)
    #[cfg(feature = "embedded")]
    #[serde(default)]
    pub embedded: EmbeddedConfig,

    /// Vector search configuration
    #[cfg(feature = "qdrant")]
    #[serde(default)]
    pub vector: VectorClientConfig,

    /// Fulltext search configuration
    #[serde(default)]
    pub fulltext: FulltextConfig,
}

impl Config {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from a specific file path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the configuration file (TOML format)
    ///
    /// # Returns
    ///
    /// * `Ok(Config)` - Successfully loaded configuration
    /// * `Err(Box<dyn Error>)` - Error reading or parsing the file
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use graphdb_config::config::Config;
    ///
    /// let config = Config::load("config.toml").expect("Failed to load config");
    /// ```
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let content = fs::read_to_string(path)?;
        let default_value: toml::Value = toml::from_str(&toml::to_string(&Config::default())?)?;
        let file_value: toml::Value = toml::from_str(&content)?;
        let merged_value = Self::merge_toml_values(default_value, file_value);
        let mut config: Config = toml::from_str(&toml::to_string(&merged_value)?)?;
        config.resolve_relative_paths(base_dir)?;
        Ok(config)
    }

    /// Load configuration from the default user configuration directory.
    pub fn load_user_config() -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_user_config_named("config.toml")
    }

    /// Load configuration from the user configuration directory with a custom file name.
    pub fn load_user_config_named(file_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config_dir = Self::user_config_dir()?;
        Self::load(config_dir.join(file_name))
    }

    /// Save configuration to file
    ///
    /// # Arguments
    ///
    /// * `path` - Path to save the configuration file (TOML format)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use graphdb_config::config::Config;
    ///
    /// let config = Config::default();
    /// config.save("config.toml").expect("Failed to save config");
    /// ```
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn user_config_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
        if let Ok(dir) = env::var("GRAPHDB_CONFIG_DIR") {
            return Ok(PathBuf::from(dir));
        }

        if let Some(dir) = dirs::config_dir() {
            return Ok(dir.join("graphdb"));
        }

        Err("Failed to determine user configuration directory".into())
    }

    fn resolve_relative_paths(
        &mut self,
        base_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let storage_path = self.common.database.storage_path.clone();
        self.common.database.storage_path = Self::resolve_string_path(base_dir, &storage_path)?;

        let log_dir = self.common.log.dir.clone();
        self.common.log.dir = Self::resolve_string_path(base_dir, &log_dir)?;

        let slow_query_log_file = self.common.monitoring.slow_query_log.log_file_path.clone();
        self.common.monitoring.slow_query_log.log_file_path =
            Self::resolve_string_path(base_dir, &slow_query_log_file)?;

        self.fulltext.index_path = Self::resolve_path_buf(base_dir, &self.fulltext.index_path)?;

        #[cfg(feature = "server")]
        {
            let static_dir = self.server.http.static_dir.clone();
            self.server.http.static_dir = Self::resolve_optional_string_path(base_dir, static_dir)?;

            let https_cert_file = self.server.http.https_cert_file.clone();
            self.server.http.https_cert_file =
                Self::resolve_optional_string_path(base_dir, https_cert_file)?;

            let https_key_file = self.server.http.https_key_file.clone();
            self.server.http.https_key_file =
                Self::resolve_optional_string_path(base_dir, https_key_file)?;

            let ssl_cert_file = self.server.security.ssl.cert_file.clone();
            if !self.server.security.ssl.cert_file.is_empty() {
                self.server.security.ssl.cert_file =
                    Self::resolve_string_path(base_dir, &ssl_cert_file)?;
            }

            let ssl_key_file = self.server.security.ssl.key_file.clone();
            if !self.server.security.ssl.key_file.is_empty() {
                self.server.security.ssl.key_file =
                    Self::resolve_string_path(base_dir, &ssl_key_file)?;
            }

            let ssl_ca_file = self.server.security.ssl.ca_file.clone();
            self.server.security.ssl.ca_file =
                Self::resolve_optional_string_path(base_dir, ssl_ca_file)?;

            let audit_log_file = self.server.security.audit.log_file.clone();
            self.server.security.audit.log_file =
                Self::resolve_string_path(base_dir, &audit_log_file)?;
        }

        #[cfg(feature = "embedded")]
        {
            let runtime_path = self.embedded.runtime.path.clone();
            self.embedded.runtime.path = Self::resolve_optional_path_buf(base_dir, runtime_path)?;
        }

        Ok(())
    }

    fn resolve_string_path(
        base_dir: &Path,
        path_value: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        Ok(Self::resolve_path_buf(base_dir, Path::new(path_value))?
            .to_string_lossy()
            .into_owned())
    }

    #[cfg(feature = "server")]
    fn resolve_optional_string_path(
        base_dir: &Path,
        path_value: Option<String>,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        path_value
            .map(|path| Self::resolve_string_path(base_dir, &path))
            .transpose()
    }

    fn merge_toml_values(base: toml::Value, overlay: toml::Value) -> toml::Value {
        match (base, overlay) {
            (toml::Value::Table(mut base_table), toml::Value::Table(overlay_table)) => {
                for (key, overlay_value) in overlay_table {
                    let merged_value = match base_table.remove(&key) {
                        Some(base_value) => Self::merge_toml_values(base_value, overlay_value),
                        None => overlay_value,
                    };
                    base_table.insert(key, merged_value);
                }
                toml::Value::Table(base_table)
            }
            (_, overlay_value) => overlay_value,
        }
    }

    fn resolve_path_buf(
        base_dir: &Path,
        path_value: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        if path_value.is_absolute() {
            return Ok(path_value.to_path_buf());
        }

        let path_text = path_value.to_string_lossy();
        if let Some(relative_path) = path_text.strip_prefix('~') {
            let home_dir = dirs::home_dir().ok_or("Failed to get user home directory")?;
            let relative_path = relative_path
                .strip_prefix('/')
                .or_else(|| relative_path.strip_prefix('\\'))
                .unwrap_or(relative_path);
            return Ok(home_dir.join(relative_path));
        }

        Ok(base_dir.join(path_value))
    }

    #[cfg(feature = "embedded")]
    fn resolve_optional_path_buf(
        base_dir: &Path,
        path_value: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        path_value
            .map(|path| Self::resolve_path_buf(base_dir, &path))
            .transpose()
    }

    /// Validate all configurations
    pub fn validate(&self) -> Result<(), String> {
        self.common.validate()?;
        #[cfg(feature = "server")]
        self.server.validate()?;
        #[cfg(feature = "embedded")]
        self.embedded.validate()?;
        Ok(())
    }

    // ========== Convenience Methods ==========

    /// Get log level
    pub fn log_level(&self) -> &str {
        &self.common.log.level
    }

    /// Get log directory
    pub fn log_dir(&self) -> &str {
        &self.common.log.dir
    }

    /// Get log file name
    pub fn log_file(&self) -> &str {
        &self.common.log.file
    }

    /// Get host address
    pub fn host(&self) -> &str {
        &self.common.database.host
    }

    /// Get port
    pub fn port(&self) -> u16 {
        self.common.database.port
    }

    /// Get gRPC port (server mode only)
    #[cfg(feature = "server")]
    pub fn grpc_port(&self) -> u16 {
        self.server.grpc.port
    }

    /// Get gRPC configuration (server mode only)
    #[cfg(feature = "server")]
    pub fn grpc(&self) -> &GrpcConfig {
        &self.server.grpc
    }

    /// Check if gRPC is enabled (server mode only)
    #[cfg(feature = "server")]
    pub fn grpc_enabled(&self) -> bool {
        self.server.grpc.enabled
    }

    /// Get storage path
    pub fn storage_path(&self) -> &str {
        &self.common.database.storage_path
    }

    /// Get maximum connections
    pub fn max_connections(&self) -> usize {
        self.common.database.max_connections
    }

    /// Get transaction timeout
    pub fn transaction_timeout(&self) -> u64 {
        self.common.transaction.default_timeout
    }

    /// Get maximum concurrent transactions
    pub fn max_concurrent_transactions(&self) -> usize {
        self.common.transaction.max_concurrent_transactions
    }

    /// Get slow query log configuration
    pub fn slow_query_log(&self) -> &SlowQueryLogConfig {
        &self.common.monitoring.slow_query_log
    }

    /// Get slow query config for StatsManager
    pub fn to_slow_query_config(&self) -> crate::core::stats::SlowQueryConfig {
        self.common.monitoring.slow_query_log.to_slow_query_config()
    }

    /// Get storage configuration
    pub fn storage(&self) -> &StorageConfig {
        &self.common.storage
    }

    /// Get query resource configuration
    pub fn query_resource(&self) -> &QueryResourceConfig {
        &self.common.query_resource
    }

    /// Check if vector search is enabled
    pub fn is_vector_enabled(&self) -> bool {
        #[cfg(feature = "qdrant")]
        {
            self.vector.enabled
        }
        #[cfg(not(feature = "qdrant"))]
        {
            false
        }
    }

    /// Get vector client configuration (only available with `qdrant` feature)
    #[cfg(feature = "qdrant")]
    pub fn vector_config(&self) -> &VectorClientConfig {
        &self.vector
    }
}

// ========== Backward Compatibility Field Access ==========
//
// These implementations provide backward compatibility by allowing
// direct field access like `config.database` instead of `config.common.database`

impl std::ops::Deref for Config {
    type Target = CommonConfig;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.common
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.common.database.host, "127.0.0.1");
        assert_eq!(config.common.database.port, 9758);
        assert_eq!(config.common.log.level, "info");
        assert_eq!(config.common.optimizer.max_iteration_rounds, 5);
        #[cfg(feature = "server")]
        assert_eq!(config.server.grpc.port, 9669);
        #[cfg(feature = "server")]
        assert!(config.server.grpc.enabled);
    }

    #[test]
    fn test_config_load_save() {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");

        let config = Config::default();
        let toml_content =
            toml::to_string_pretty(&config).expect("Failed to serialize config to TOML");
        temp_file
            .write_all(toml_content.as_bytes())
            .expect("Failed to write TOML content to temporary file");

        let loaded_config =
            Config::load(temp_file.path()).expect("Failed to load config from temporary file");
        assert_eq!(
            config.common.database.host,
            loaded_config.common.database.host
        );
        assert_eq!(
            config.common.database.port,
            loaded_config.common.database.port
        );
        assert_eq!(config.common.log.level, loaded_config.common.log.level);
    }

    #[test]
    fn test_nested_config_load() {
        let config_content = r#"
[database]
host = "0.0.0.0"
port = 8080
storage_path = "/tmp/graphdb"
max_connections = 100

[transaction]
default_timeout = 60
max_concurrent_transactions = 500

[log]
level = "debug"
dir = "/var/log/graphdb"
file = "graphdb"
max_file_size = 104857600
max_files = 10

[storage]
engine = "propertygraph"
compression = "zstd"
compression_level = 5

[query_resource]
max_concurrent_queries = 50
max_memory_per_query = 1073741824
"#;

        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");

        let config = Config::load(temp_file.path()).expect("Failed to load config");

        assert_eq!(config.common.database.host, "0.0.0.0");
        assert_eq!(config.common.database.port, 8080);
        assert_eq!(config.common.transaction.default_timeout, 60);
        assert_eq!(config.common.transaction.max_concurrent_transactions, 500);
        assert_eq!(config.common.log.level, "debug");
        assert_eq!(
            config.common.storage.compression,
            CompressionAlgorithm::Zstd
        );
        assert_eq!(config.common.storage.compression_level, 5);
        assert_eq!(config.common.query_resource.max_concurrent_queries, 50);
    }

    #[test]
    fn test_config_load_resolves_relative_paths_from_file_directory() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_dir = temp_dir.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");

        let config_content = r#"
[database]
storage_path = "data/graphdb"
"#;

        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, config_content).expect("Failed to write config");

        let config = Config::load(&config_path).expect("Failed to load config");

        assert_eq!(
            config.common.database.storage_path,
            config_dir.join("data/graphdb").to_string_lossy()
        );
        assert_eq!(
            config.common.log.dir,
            config_dir.join("logs").to_string_lossy()
        );
        assert_eq!(
            config.common.monitoring.slow_query_log.log_file_path,
            config_dir.join("logs/slow_query.log").to_string_lossy()
        );
        assert_eq!(config.fulltext.index_path, config_dir.join("data/fulltext"));
    }

    #[test]
    fn test_load_user_config_named_uses_graphdb_config_dir() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_dir = temp_dir.path().join("user-config");
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");

        let config_content = r#"
[database]
storage_path = "storage"
"#;
        std::fs::write(config_dir.join("config.toml"), config_content)
            .expect("Failed to write config");

        let previous_dir = env::var("GRAPHDB_CONFIG_DIR").ok();
        env::set_var("GRAPHDB_CONFIG_DIR", &config_dir);

        let config =
            Config::load_user_config_named("config.toml").expect("Failed to load user config");
        assert_eq!(
            config.common.database.storage_path,
            config_dir.join("storage").to_string_lossy()
        );

        if let Some(value) = previous_dir {
            env::set_var("GRAPHDB_CONFIG_DIR", value);
        } else {
            env::remove_var("GRAPHDB_CONFIG_DIR");
        }
    }

    #[test]
    fn test_config_validate() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_backward_compatibility() {
        let config = Config::default();
        // Test Deref implementation
        assert_eq!(config.database.host, "127.0.0.1");
        assert_eq!(config.port(), 9758);
        assert_eq!(config.storage_path(), "data/graphdb");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_server_config() {
        let config = Config::default();
        assert!(config.server.grpc.enabled);
        assert!(config.server.http.enabled);
        assert!(config.server.auth.enable_authorize);
        assert_eq!(config.server.grpc.port, 9669);
        assert_eq!(config.server.http.port, 9758);
    }

    #[cfg(feature = "embedded")]
    #[test]
    fn test_embedded_config() {
        let config = Config::default();
        assert!(config.embedded.runtime.is_memory());
        assert_eq!(config.embedded.runtime.cache_size_mb, 64);
    }
}
