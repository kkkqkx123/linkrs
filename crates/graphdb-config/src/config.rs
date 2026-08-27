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
//! use graphdb_config::Config;
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
//! use graphdb_config::{Config, EmbeddedConfig};
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
pub use common::parallel::ParallelConfig;
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

#[cfg(feature = "vector-qdrant")]
use vector_client::VectorClientConfig;

/// Global configuration aggregator
///
/// This is the main configuration structure that combines all configuration sections.
/// Use [`Config::default()`] to create a default configuration, or [`Config::load()`] to load from a file.
///
/// # Examples
///
/// ```rust
/// use graphdb_config::Config;
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
    #[cfg(feature = "vector")]
    #[serde(default)]
    pub vector: VectorConfig,

    /// Fulltext search configuration
    #[serde(default)]
    pub fulltext: FulltextConfig,
}

/// Vector search engine kind
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorEngineKind {
    /// Built-in local engine (WAL-backed, in-process)
    #[default]
    Local,
    /// External Qdrant server (requires `vector-qdrant` feature)
    Qdrant,
}

/// IVF settings for the local vector engine (raw TOML surface).
///
/// The values mirror `vector_search::IvfConfig`; the conversion lives in
/// graphdb-api so that graphdb-config stays free of vector-search types.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct IvfSettings {
    /// Whether collections are promoted to an IVF index automatically once
    /// they exceed `min_build_points`. Off by default until benchmarks
    /// justify turning it on.
    #[serde(default)]
    pub auto_promotion: bool,
    /// Cluster count; `0` = auto (`sqrt(live)` clamped to [1, 4096]).
    #[serde(default)]
    pub lists: u32,
    /// Minimum live points before an index is built.
    #[serde(default = "default_ivf_min_build_points")]
    pub min_build_points: u64,
    /// Training / drift-check sample cap.
    #[serde(default = "default_ivf_sample_limit")]
    pub sample_limit: usize,
    /// k-means iteration cap.
    #[serde(default = "default_ivf_kmeans_max_iter")]
    pub kmeans_max_iter: u32,
    /// Drift ratio above which a rebuild is scheduled.
    #[serde(default = "default_ivf_drift_threshold")]
    pub drift_threshold: f64,
    /// Upserts accumulated before the next drift check.
    #[serde(default = "default_ivf_drift_check_interval")]
    pub drift_check_interval: u64,
    /// Default nprobe when a query does not set one.
    #[serde(default = "default_ivf_nprobe")]
    pub default_nprobe: usize,
    /// Upper bound for multi-round probe widening on filtered searches;
    /// `0` = engine default (capped by the list count only).
    #[serde(default)]
    pub max_probes: usize,
}

impl Default for IvfSettings {
    fn default() -> Self {
        Self {
            auto_promotion: false,
            lists: 0,
            min_build_points: 100_000,
            sample_limit: 65_536,
            kmeans_max_iter: 10,
            drift_threshold: 0.10,
            drift_check_interval: 25_000,
            default_nprobe: 8,
            max_probes: 0,
        }
    }
}

fn default_ivf_min_build_points() -> u64 {
    100_000
}

fn default_ivf_sample_limit() -> usize {
    65_536
}

fn default_ivf_kmeans_max_iter() -> u32 {
    10
}

fn default_ivf_drift_threshold() -> f64 {
    0.10
}

fn default_ivf_drift_check_interval() -> u64 {
    25_000
}

fn default_ivf_nprobe() -> usize {
    8
}

/// HNSW settings for the local vector engine (raw TOML surface).
///
/// The values mirror `vector_search::HnswConfig`; the conversion lives in
/// graphdb-api so that graphdb-config stays free of vector-search types.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct HnswSettings {
    /// Number of bidirectional links per layer above the ground layer
    /// (the ground layer uses `2 * m`).
    #[serde(default = "default_hnsw_m")]
    pub m: usize,
    /// Size of the dynamic candidate list during graph construction.
    #[serde(default = "default_hnsw_ef_construct")]
    pub ef_construct: usize,
    /// Live points below which a collection stays on exact scan; `0` =
    /// engine default.
    #[serde(default)]
    pub full_scan_threshold: usize,
    /// Default search-list size at query time (`ef_search`); `0` = engine
    /// default. Per-query `SearchMode::KNN.ef_search` overrides it.
    #[serde(default)]
    pub ef_search: usize,
    /// Cap on iterative-scan expansion rounds for filtered searches; `0` =
    /// engine default.
    #[serde(default)]
    pub iterative_max_rounds: usize,
    /// Cumulative cap on nodes visited across iterative-scan rounds; `0` =
    /// unbounded.
    #[serde(default)]
    pub max_scan_tuples: u64,
}

impl Default for HnswSettings {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construct: 100,
            full_scan_threshold: 0,
            ef_search: 0,
            iterative_max_rounds: 0,
            max_scan_tuples: 0,
        }
    }
}

fn default_hnsw_m() -> usize {
    16
}

fn default_hnsw_ef_construct() -> usize {
    100
}

/// Quantization settings for the local vector engine (raw TOML surface).
///
/// Mirrors `vector_search::QuantizationConfig` / Qdrant quantization builders;
/// the conversion lives in `graphdb-api` so that `graphdb-config` stays free
/// of `vector-search` types.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
pub struct QuantizationSettings {
    /// Quantization type: `scalar` | `product` | `binary` | `none` (default disabled)
    #[serde(default)]
    pub quantization_type: Option<String>,
    /// Scalar only: quantile in (0,1], default 0.99
    #[serde(default)]
    pub quantile: Option<f32>,
    /// Product only: compression `x4`/`x8`/`x16`/`x32`/`x64`, default `x4`
    #[serde(default)]
    pub compression: Option<String>,
    /// Whether quantized vectors stay in RAM
    #[serde(default)]
    pub always_ram: Option<bool>,
    /// Master switch: when false (default) quantization is disabled even if
    /// a type is set. Set to true to enable.
    #[serde(default)]
    pub enabled: bool,
}

/// Local vector engine configuration
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct LocalVectorConfig {
    /// Directory for the local vector engine; defaults to
    /// `<database.storage_path>/vector`.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// HNSW settings (local engine only). This is the default ANN tier.
    #[serde(default)]
    pub hnsw: Option<HnswSettings>,
    /// IVF settings (local engine only, opt-in alternative tier).
    #[serde(default)]
    pub ivf: Option<IvfSettings>,
    /// Quantization settings (local engine only, optional per-collection
    /// default when a collection is created without explicit quantization).
    #[serde(default)]
    pub quantization: Option<QuantizationSettings>,
}

/// Vector search configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VectorConfig {
    /// Master switch for vector search
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Active engine kind
    #[serde(default)]
    pub engine: VectorEngineKind,
    /// Local engine configuration
    #[serde(default)]
    pub local: LocalVectorConfig,
    /// Qdrant client configuration (only available with `vector-qdrant` feature)
    #[cfg(feature = "vector-qdrant")]
    #[serde(default)]
    pub qdrant: VectorClientConfig,
}

fn default_true() -> bool {
    true
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: VectorEngineKind::Local,
            local: LocalVectorConfig::default(),
            #[cfg(feature = "vector-qdrant")]
            qdrant: VectorClientConfig::disabled(),
        }
    }
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
    /// use graphdb_config::Config;
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
    /// use graphdb_config::Config;
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

        #[cfg(feature = "vector")]
        {
            let default_dir = PathBuf::from(&self.common.database.storage_path).join("vector");
            let data_dir = self.vector.local.data_dir.clone().unwrap_or(default_dir);
            self.vector.local.data_dir = Some(Self::resolve_path_buf(base_dir, &data_dir)?);
        }

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

    /// Get columnar fast-path configuration
    pub fn columnar(&self) -> &ColumnarConfig {
        &self.common.columnar
    }

    /// Check if vector search is enabled
    pub fn is_vector_enabled(&self) -> bool {
        #[cfg(feature = "vector")]
        {
            match self.vector.engine {
                VectorEngineKind::Local => self.vector.enabled,
                #[cfg(feature = "vector-qdrant")]
                VectorEngineKind::Qdrant => self.vector.enabled && self.vector.qdrant.enabled,
                #[cfg(not(feature = "vector-qdrant"))]
                VectorEngineKind::Qdrant => false,
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            false
        }
    }

    /// Check if the local vector engine is the active backend
    pub fn is_local_vector(&self) -> bool {
        #[cfg(feature = "vector")]
        {
            self.vector.enabled && self.vector.engine == VectorEngineKind::Local
        }
        #[cfg(not(feature = "vector"))]
        {
            false
        }
    }

    /// Get the active vector engine kind
    pub fn vector_engine(&self) -> Option<VectorEngineKind> {
        #[cfg(feature = "vector")]
        {
            self.vector.enabled.then_some(self.vector.engine)
        }
        #[cfg(not(feature = "vector"))]
        {
            None
        }
    }

    /// Get vector search configuration (only available with `vector` feature)
    #[cfg(feature = "vector")]
    pub fn vector_config(&self) -> &VectorConfig {
        &self.vector
    }

    /// Resolved data directory for the local vector engine.
    #[cfg(feature = "vector")]
    pub fn vector_data_dir(&self) -> PathBuf {
        self.vector
            .local
            .data_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.common.database.storage_path).join("vector"))
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
    fn test_parallel_config_load() {
        let config_content = r#"
[parallel]
enabled = true
workers = 4
min_rows_per_partition = 20000
max_partitions = 4
max_buffered_chunks = 8
vertex_id_start = 0
vertex_id_end = 100000
"#;

        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");

        let config = Config::load(temp_file.path()).expect("Failed to load config");

        assert!(config.common.parallel.enabled);
        assert_eq!(config.common.parallel.workers, 4);
        assert_eq!(config.common.parallel.min_rows_per_partition, 20_000);
        assert_eq!(config.common.parallel.max_partitions, 4);
        assert_eq!(config.common.parallel.max_buffered_chunks, 8);
        assert_eq!(config.common.parallel.vertex_id_range(), Some(0..100_000));
    }

    #[test]
    fn test_parallel_config_defaults_when_absent() {
        let config_content = r#"
[database]
host = "0.0.0.0"
"#;

        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");

        let config = Config::load(temp_file.path()).expect("Failed to load config");

        // Absent [parallel] section must keep the safe defaults.
        assert!(!config.common.parallel.enabled);
        assert_eq!(config.common.parallel.workers, 1);
        assert!(config.common.parallel.vertex_id_range().is_none());
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

    #[cfg(feature = "vector-qdrant")]
    #[test]
    fn test_vector_section_deserializes_qdrant() {
        let toml = r#"
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/graphdb"
max_connections = 10

[vector]
enabled = true
engine = "qdrant"

[vector.qdrant]
enabled = true

[vector.qdrant.connection]
host = "localhost"
port = 6334
http_port = 6333
use_tls = false
connect_timeout_secs = 5

[vector.qdrant.timeout]
request_timeout_secs = 30
search_timeout_secs = 10
upsert_timeout_secs = 30
"#;
        let config: Config = toml::from_str(toml).expect("vector section should deserialize");
        assert!(config.is_vector_enabled());
        assert_eq!(config.vector_engine(), Some(VectorEngineKind::Qdrant));
        assert_eq!(
            config.vector_data_dir(),
            std::path::PathBuf::from("data/graphdb/vector")
        );
        assert!(config.vector.qdrant.enabled);
        assert_eq!(config.vector.qdrant.connection.host, "localhost");
        assert_eq!(config.vector.qdrant.connection.port, 6334);
    }

    #[cfg(feature = "vector-qdrant")]
    #[test]
    fn parse_vector_client_config_standalone() {
        let toml = r#"
enabled = true

[connection]
host = "localhost"
port = 6334
http_port = 6333
use_tls = false
connect_timeout_secs = 5

[timeout]
request_timeout_secs = 30
search_timeout_secs = 10
upsert_timeout_secs = 30
"#;
        let c: VectorClientConfig = toml::from_str(toml).expect("vc parse");
        assert_eq!(c.connection.host, "localhost");
    }
}
