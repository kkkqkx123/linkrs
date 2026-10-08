//! Embedded runtime configuration

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Synchronization mode for embedded database
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Complete synchronization: Every write operation is immediately synchronized to disk
    Full,
    /// Normal synchronization, periodic synchronization (default)
    #[default]
    Normal,
    /// Asynchronous mode: The operating system determines when to synchronize
    Off,
}

impl SyncMode {
    /// Check if this is full sync mode
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Check if this is normal sync mode
    pub fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }

    /// Check if this is off sync mode
    pub fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }
}

/// Runtime configuration for embedded database
///
/// Controls the runtime behavior of the embedded database instance.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RuntimeConfig {
    /// Database path; `None` indicates in-memory mode
    pub path: Option<PathBuf>,

    /// Cache size (MB)
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,

    /// Default timeout
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Should WAL (Write-Ahead Logging) be enabled?
    #[serde(default = "default_true")]
    pub enable_wal: bool,

    /// Synchronization mode
    #[serde(default)]
    pub sync_mode: SyncMode,

    /// Is it read-only?
    #[serde(default)]
    pub read_only: bool,

    /// Should it be created if it does not exist?
    #[serde(default = "default_true")]
    pub create_if_missing: bool,

    /// Maximum number of open files
    #[serde(default = "default_max_open_files")]
    pub max_open_files: usize,

    /// Background worker threads (0 = auto-detect)
    #[serde(default)]
    pub worker_threads: usize,
}

fn default_cache_size() -> usize {
    64
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_true() -> bool {
    true
}

fn default_max_open_files() -> usize {
    100
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            path: None,
            cache_size_mb: default_cache_size(),
            default_timeout_secs: default_timeout_secs(),
            enable_wal: true,
            sync_mode: SyncMode::default(),
            read_only: false,
            create_if_missing: true,
            max_open_files: default_max_open_files(),
            worker_threads: 0,
        }
    }
}

impl RuntimeConfig {
    /// Create a configuration for in-memory database
    pub fn memory() -> Self {
        Self {
            path: None,
            cache_size_mb: default_cache_size(),
            default_timeout_secs: default_timeout_secs(),
            enable_wal: true,
            sync_mode: SyncMode::Normal,
            read_only: false,
            create_if_missing: true,
            max_open_files: default_max_open_files(),
            worker_threads: 0,
        }
    }

    /// Create a file database configuration
    pub fn file(path: impl AsRef<Path>) -> Self {
        Self {
            path: Some(path.as_ref().to_path_buf()),
            cache_size_mb: default_cache_size(),
            default_timeout_secs: default_timeout_secs(),
            enable_wal: true,
            sync_mode: SyncMode::Normal,
            read_only: false,
            create_if_missing: true,
            max_open_files: default_max_open_files(),
            worker_threads: 0,
        }
    }

    /// Set the cache size
    pub fn with_cache_size(mut self, size_mb: usize) -> Self {
        self.cache_size_mb = size_mb;
        self
    }

    /// Set the default timeout value
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout_secs = timeout.as_secs();
        self
    }

    /// Set whether to enable WAL (Write-Ahead Logging)
    pub fn with_wal(mut self, enable: bool) -> Self {
        self.enable_wal = enable;
        self
    }

    /// Set the synchronization mode
    pub fn with_sync_mode(mut self, mode: SyncMode) -> Self {
        self.sync_mode = mode;
        self
    }

    /// Set whether the database should be read-only
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Set whether the database should be created if it doesn't exist
    pub fn with_create_if_missing(mut self, create: bool) -> Self {
        self.create_if_missing = create;
        self
    }

    /// Set the maximum number of open files
    pub fn with_max_open_files(mut self, max: usize) -> Self {
        self.max_open_files = max;
        self
    }

    /// Set the number of worker threads (0 = auto-detect)
    pub fn with_worker_threads(mut self, threads: usize) -> Self {
        self.worker_threads = threads;
        self
    }

    /// Check whether it is in memory mode
    pub fn is_memory(&self) -> bool {
        self.path.is_none()
    }

    /// Get the database path
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Get the cache size (in bytes)
    pub fn cache_size_bytes(&self) -> usize {
        self.cache_size_mb * 1024 * 1024
    }

    /// Get the default timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.default_timeout_secs)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.cache_size_mb == 0 {
            return Err("Cache size must be greater than 0".to_string());
        }

        if self.default_timeout_secs == 0 {
            return Err("Default timeout must be greater than 0".to_string());
        }

        if self.max_open_files == 0 {
            return Err("Max open files must be greater than 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_default() {
        let config = RuntimeConfig::default();
        assert!(config.is_memory());
        assert_eq!(config.cache_size_mb, 64);
        assert_eq!(config.default_timeout_secs, 30);
        assert!(config.enable_wal);
        assert_eq!(config.sync_mode, SyncMode::Normal);
        assert!(!config.read_only);
        assert!(config.create_if_missing);
        assert_eq!(config.max_open_files, 100);
        assert_eq!(config.worker_threads, 0);
    }

    #[test]
    fn test_runtime_config_memory() {
        let config = RuntimeConfig::memory();
        assert!(config.is_memory());
        assert!(config.path.is_none());
    }

    #[test]
    fn test_runtime_config_file() {
        let config = RuntimeConfig::file("/tmp/test.db");
        assert!(!config.is_memory());
        assert_eq!(config.path(), Some(Path::new("/tmp/test.db")));
    }

    #[test]
    fn test_runtime_config_chain_builder() {
        let config = RuntimeConfig::memory()
            .with_cache_size(128)
            .with_timeout(Duration::from_secs(60))
            .with_wal(false)
            .with_sync_mode(SyncMode::Full)
            .with_read_only(true)
            .with_max_open_files(200)
            .with_worker_threads(4);

        assert_eq!(config.cache_size_mb, 128);
        assert_eq!(config.default_timeout_secs, 60);
        assert!(!config.enable_wal);
        assert_eq!(config.sync_mode, SyncMode::Full);
        assert!(config.read_only);
        assert_eq!(config.max_open_files, 200);
        assert_eq!(config.worker_threads, 4);
    }

    #[test]
    fn test_runtime_config_cache_size_bytes() {
        let config = RuntimeConfig::memory().with_cache_size(64);
        assert_eq!(config.cache_size_bytes(), 64 * 1024 * 1024);
    }

    #[test]
    fn test_runtime_config_timeout() {
        let config = RuntimeConfig::memory().with_timeout(Duration::from_secs(60));
        assert_eq!(config.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn test_runtime_config_validate() {
        let config = RuntimeConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = RuntimeConfig {
            cache_size_mb: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_sync_mode() {
        assert_eq!(SyncMode::default(), SyncMode::Normal);
        assert!(SyncMode::Normal.is_normal());
        assert!(SyncMode::Full.is_full());
        assert!(SyncMode::Off.is_off());
    }
}
