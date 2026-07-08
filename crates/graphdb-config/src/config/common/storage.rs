//! Storage configuration

use serde::{Deserialize, Serialize};

/// Storage engine type
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageEngine {
    /// PropertyGraph storage engine (columnar + CSR)
    #[default]
    PropertyGraph,
    /// RocksDB storage engine (future support)
    #[serde(rename = "rocksdb")]
    RocksDB,
}

impl std::fmt::Display for StorageEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PropertyGraph => write!(f, "propertygraph"),
            Self::RocksDB => write!(f, "rocksdb"),
        }
    }
}

/// Compression algorithm
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgorithm {
    /// No compression
    #[default]
    None,
    /// Zstandard compression
    Zstd,
}

impl std::fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Zstd => write!(f, "zstd"),
        }
    }
}

/// Storage configuration
///
/// Configures the storage engine behavior and performance characteristics.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageConfig {
    /// Storage engine type (propertygraph, rocksdb, etc.)
    #[serde(default)]
    pub engine: StorageEngine,

    /// Compression algorithm (none, lz4, zstd, snappy)
    #[serde(default)]
    pub compression: CompressionAlgorithm,

    /// Compression level (0-9, engine-dependent)
    #[serde(default = "default_compression_level")]
    pub compression_level: u32,

    /// Checkpoint interval (seconds, 0 = disabled)
    #[serde(default = "default_checkpoint_interval")]
    pub checkpoint_interval_secs: u64,

    /// Maximum database size (bytes, 0 = unlimited)
    #[serde(default)]
    pub max_db_size: u64,

    /// Enable automatic statistics collection
    #[serde(default = "default_true")]
    pub auto_statistics: bool,

    /// Statistics collection interval (seconds)
    #[serde(default = "default_statistics_interval")]
    pub statistics_interval_secs: u64,
}

fn default_compression_level() -> u32 {
    3
}

fn default_checkpoint_interval() -> u64 {
    300 // 5 minutes
}

fn default_statistics_interval() -> u64 {
    60 // 1 minute
}

fn default_true() -> bool {
    true
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            engine: StorageEngine::default(),
            compression: CompressionAlgorithm::default(),
            compression_level: default_compression_level(),
            checkpoint_interval_secs: default_checkpoint_interval(),
            max_db_size: 0, // Unlimited
            auto_statistics: true,
            statistics_interval_secs: default_statistics_interval(),
        }
    }
}

impl StorageConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.compression_level > 9 {
            return Err("Compression level must be between 0 and 9".to_string());
        }
        Ok(())
    }

    /// Check if compression is enabled
    pub fn is_compression_enabled(&self) -> bool {
        !matches!(self.compression, CompressionAlgorithm::None)
    }
}

/// Query resource configuration
///
/// Controls resource limits for query execution.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct QueryResourceConfig {
    /// Maximum memory per query (bytes, 0 = unlimited)
    #[serde(default)]
    pub max_memory_per_query: u64,

    /// Maximum concurrent queries (0 = unlimited)
    #[serde(default = "default_max_concurrent_queries")]
    pub max_concurrent_queries: usize,

    /// Query timeout (seconds, 0 = no timeout)
    #[serde(default)]
    pub query_timeout_secs: u64,

    /// Maximum result set size (0 = unlimited)
    #[serde(default)]
    pub max_result_size: usize,

    /// Maximum number of vertices to scan in a single query
    #[serde(default = "default_max_vertex_scan")]
    pub max_vertex_scan: usize,

    /// Maximum number of edges to scan in a single query
    #[serde(default = "default_max_edge_scan")]
    pub max_edge_scan: usize,
}

fn default_max_concurrent_queries() -> usize {
    100
}

fn default_max_vertex_scan() -> usize {
    1_000_000
}

fn default_max_edge_scan() -> usize {
    10_000_000
}

impl Default for QueryResourceConfig {
    fn default() -> Self {
        Self {
            max_memory_per_query: 0, // Unlimited
            max_concurrent_queries: default_max_concurrent_queries(),
            query_timeout_secs: 0, // No timeout
            max_result_size: 0,    // Unlimited
            max_vertex_scan: default_max_vertex_scan(),
            max_edge_scan: default_max_edge_scan(),
        }
    }
}

impl QueryResourceConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_concurrent_queries == 0 {
            return Err("Max concurrent queries must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Check if memory limit is enabled
    pub fn has_memory_limit(&self) -> bool {
        self.max_memory_per_query > 0
    }

    /// Check if query timeout is enabled
    pub fn has_timeout(&self) -> bool {
        self.query_timeout_secs > 0
    }

    /// Check if result size limit is enabled
    pub fn has_result_size_limit(&self) -> bool {
        self.max_result_size > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_config_default() {
        let config = StorageConfig::default();
        assert_eq!(config.engine, StorageEngine::PropertyGraph);
        assert_eq!(config.compression, CompressionAlgorithm::None);
        assert_eq!(config.compression_level, 3);
        assert_eq!(config.checkpoint_interval_secs, 300);
        assert!(config.auto_statistics);
    }

    #[test]
    fn test_storage_config_validate() {
        let config = StorageConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = StorageConfig {
            compression_level: 10,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_query_resource_config_default() {
        let config = QueryResourceConfig::default();
        assert_eq!(config.max_concurrent_queries, 100);
        assert_eq!(config.max_vertex_scan, 1_000_000);
        assert_eq!(config.max_edge_scan, 10_000_000);
        assert!(!config.has_memory_limit());
        assert!(!config.has_timeout());
    }

    #[test]
    fn test_query_resource_config_validate() {
        let config = QueryResourceConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = QueryResourceConfig {
            max_concurrent_queries: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_storage_engine_display() {
        assert_eq!(StorageEngine::PropertyGraph.to_string(), "propertygraph");
        assert_eq!(StorageEngine::RocksDB.to_string(), "rocksdb");
    }

    #[test]
    fn test_compression_algorithm_display() {
        assert_eq!(CompressionAlgorithm::None.to_string(), "none");
        assert_eq!(CompressionAlgorithm::Zstd.to_string(), "zstd");
    }
}
