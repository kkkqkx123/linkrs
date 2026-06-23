//! Configuration Management Module
//!
//! Provides configuration management for embedded database, separated from embedded_api.rs

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Database configuration
///
/// Used to configure the behavior of the embedded GraphDB database.
///
/// # Example
///
/// ```rust
/// use graphdb::api::embedded::DatabaseConfig;
///
// Configuration for the in-memory database
/// let config = DatabaseConfig::memory();
///
// File database configuration
/// let config = DatabaseConfig::file("/path/to/db");
///
// Chain configuration
/// let config = DatabaseConfig::memory()
///     .with_cache_size(128)
///     .with_timeout(Duration::from_secs(60));
/// ```
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Database path; “None” indicates the in-memory mode.
    pub path: Option<PathBuf>,
    /// Cache size (MB)
    pub cache_size_mb: usize,
    /// Default timeout
    pub default_timeout: Duration,
    /// Should WAL (Write-Ahead Logging) be enabled?
    pub enable_wal: bool,
    /// Synchronous mode
    pub sync_mode: SyncMode,
    /// Is it read-only?
    pub read_only: bool,
    /// Should it be created if it does not exist?
    pub create_if_missing: bool,
}

/// Synchronization Mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Complete synchronization: Every write operation is immediately synchronized to the disk (the safest method, but also the slowest).
    Full,
    /// Normal synchronization, periodic synchronization (balancing)
    #[default]
    Normal,
    /// Asynchronous mode: The operating system determines when to perform synchronization (the fastest option, but it carries certain risks).
    Off,
}

impl DatabaseConfig {
    /// Create a configuration for the in-memory database.
    pub fn memory() -> Self {
        Self {
            path: None,
            cache_size_mb: 64,
            default_timeout: Duration::from_secs(30),
            enable_wal: true,
            sync_mode: SyncMode::Normal,
            read_only: false,
            create_if_missing: true,
        }
    }

    /// Create a file database configuration.
    pub fn file(path: impl AsRef<Path>) -> Self {
        Self {
            path: Some(path.as_ref().to_path_buf()),
            cache_size_mb: 64,
            default_timeout: Duration::from_secs(30),
            enable_wal: true,
            sync_mode: SyncMode::Normal,
            read_only: false,
            create_if_missing: true,
        }
    }

    /// Creating a configuration using a path (an easy method)
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::file(path)
    }

    /// Set the cache size
    pub fn with_cache_size(mut self, size_mb: usize) -> Self {
        self.cache_size_mb = size_mb;
        self
    }

    /// Set the default timeout value
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set whether to enable WAL (Write-Ahead Logging).
    pub fn with_wal(mut self, enable: bool) -> Self {
        self.enable_wal = enable;
        self
    }

    /// Set the synchronization mode.
    pub fn with_sync_mode(mut self, mode: SyncMode) -> Self {
        self.sync_mode = mode;
        self
    }

    /// Set whether the content should be read-only only.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Should a setting be created if it does not exist?
    pub fn with_create_if_missing(mut self, create: bool) -> Self {
        self.create_if_missing = create;
        self
    }

    /// Check whether it is in memory mode.
    pub fn is_memory(&self) -> bool {
        self.path.is_none()
    }

    /// Obtain the database path
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Get the cache size (in bytes)
    pub fn cache_size_bytes(&self) -> usize {
        self.cache_size_mb * 1024 * 1024
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self::memory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DatabaseConfig::default();
        assert!(config.is_memory());
        assert_eq!(config.cache_size_mb, 64);
        assert_eq!(config.default_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_memory_config() {
        let config = DatabaseConfig::memory();
        assert!(config.is_memory());
        assert!(config.path.is_none());
    }

    #[test]
    fn test_file_config() {
        let config = DatabaseConfig::file("/tmp/test.db");
        assert!(!config.is_memory());
        assert_eq!(config.path(), Some(Path::new("/tmp/test.db")));
    }

    #[test]
    fn test_chain_config() {
        let config = DatabaseConfig::memory()
            .with_cache_size(128)
            .with_timeout(Duration::from_secs(60))
            .with_wal(false)
            .with_sync_mode(SyncMode::Full);

        assert_eq!(config.cache_size_mb, 128);
        assert_eq!(config.default_timeout, Duration::from_secs(60));
        assert!(!config.enable_wal);
        assert_eq!(config.sync_mode, SyncMode::Full);
    }

    #[test]
    fn test_cache_size_bytes() {
        let config = DatabaseConfig::memory().with_cache_size(64);
        assert_eq!(config.cache_size_bytes(), 64 * 1024 * 1024);
    }

    #[test]
    fn test_sync_mode_default() {
        let mode = SyncMode::default();
        assert_eq!(mode, SyncMode::Normal);
    }
}
