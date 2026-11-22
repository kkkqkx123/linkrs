//! Configuration Management Module
//!
//! Provides configuration management for embedded database, separated from embedded_api.rs

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use linkrs_config::{ParallelConfig, SyncMode, VectorEngineKind};

/// Database configuration
///
/// Used to configure the behavior of the embedded Linkrs database.
///
/// # Example
///
/// ```rust
/// use linkrs_api::embedded::DatabaseConfig;
///
/// # use std::time::Duration;
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
    /// Create one final checkpoint on explicit close. Off by default.
    pub checkpoint_on_close: bool,
    /// Whether each checkpoint also creates a full snapshot copy. Snapshots
    /// double checkpoint storage and are optional for read-mostly benchmarks.
    pub enable_snapshots: bool,
    /// Maximum number of snapshot copies to retain when snapshots are enabled.
    pub snapshot_retention_count: usize,
    /// Vector subsystem configuration.
    pub vector: VectorConfig,
    /// Query-internal parallel execution settings. Defaults keep embedded
    /// execution serial.
    pub parallel: ParallelConfig,
}

/// Vector subsystem configuration for the embedded database.
///
/// Defaults to the local engine with a data directory derived from the
/// database path (`<db_file>_vector`). In-memory databases have no on-disk
/// home, so their vectors stay disabled unless an explicit directory is set.
///
/// This is a thin front over [`linkrs_config::VectorConfig`]: it names the
/// same engine enum and the same Qdrant parameters the server reads from
/// TOML, and hands the resolved form to the one assembler both paths share.
#[derive(Debug, Clone)]
pub struct VectorConfig {
    /// Master switch for the vector subsystem.
    pub enabled: bool,
    /// Engine selection.
    pub engine: VectorEngineKind,
    /// Explicit storage directory for the local engine;
    /// derived from the database path when unset.
    pub local_data_dir: Option<PathBuf>,
    /// Parameters for the remote engine. Only read when
    /// [`Self::engine`] is [`VectorEngineKind::Qdrant`], where it also
    /// carries the engine's own enable switch.
    #[cfg(feature = "vector-qdrant")]
    pub qdrant: linkrs_config::QdrantConfig,
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: VectorEngineKind::Local,
            local_data_dir: None,
            #[cfg(feature = "vector-qdrant")]
            qdrant: linkrs_config::QdrantConfig::default(),
        }
    }
}

impl VectorConfig {
    /// Resolve the shared engine configuration this surface describes.
    ///
    /// `None` means the vector subsystem is deliberately off: the master
    /// switch is off, or an in-memory database has no directory to put an
    /// engine in. A contradictory engine selection is *not* folded into
    /// `None` here — it stays in the configuration so the assembler can report
    /// it instead of silently serving a database without vector search.
    pub(crate) fn to_engine_config(
        &self,
        db_path: Option<&Path>,
    ) -> Option<linkrs_config::VectorConfig> {
        if !self.enabled {
            return None;
        }
        let mut engine_config = linkrs_config::VectorConfig {
            enabled: true,
            engine: self.engine,
            ..linkrs_config::VectorConfig::default()
        };
        match self.engine {
            VectorEngineKind::Local => {
                let data_dir = match (&self.local_data_dir, db_path) {
                    (Some(dir), _) => dir.clone(),
                    (None, Some(path)) => default_local_vector_dir(path),
                    // In-memory databases have no on-disk home; vectors stay
                    // off unless an explicit directory is configured.
                    (None, None) => return None,
                };
                engine_config.local.data_dir = Some(data_dir);
            }
            #[cfg(feature = "vector-qdrant")]
            VectorEngineKind::Qdrant => engine_config.qdrant = self.qdrant.clone(),
        }
        Some(engine_config)
    }
}

/// Derive the default local vector data directory from the database file path
/// (`<db_file>_vector` next to the database).
fn default_local_vector_dir(db_path: &Path) -> PathBuf {
    let mut name = db_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push("_vector");
    db_path.with_file_name(name)
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
            checkpoint_on_close: false,
            enable_snapshots: true,
            snapshot_retention_count: 10,
            vector: VectorConfig::default(),
            parallel: ParallelConfig::default(),
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
            checkpoint_on_close: false,
            enable_snapshots: true,
            snapshot_retention_count: 10,
            vector: VectorConfig::default(),
            parallel: ParallelConfig::default(),
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

    /// Whether checkpoints also create full snapshot copies.
    pub fn with_snapshots(mut self, enabled: bool) -> Self {
        self.enable_snapshots = enabled;
        self
    }

    /// Maximum number of snapshot copies to retain.
    pub fn with_snapshot_retention(mut self, count: usize) -> Self {
        self.snapshot_retention_count = count.max(1);
        self
    }

    /// Should a setting be created if it does not exist?
    pub fn with_create_if_missing(mut self, create: bool) -> Self {
        self.create_if_missing = create;
        self
    }

    /// Create one final checkpoint on explicit close.
    pub fn with_checkpoint_on_close(mut self, enabled: bool) -> Self {
        self.checkpoint_on_close = enabled;
        self
    }

    /// Apply query-internal parallel execution settings.
    pub fn with_parallel_config(mut self, parallel: ParallelConfig) -> Self {
        self.parallel = parallel;
        self
    }

    /// Enable or disable the vector subsystem.
    pub fn with_vector_enabled(mut self, enabled: bool) -> Self {
        self.vector.enabled = enabled;
        self
    }

    /// Set an explicit data directory for the local vector engine.
    pub fn with_vector_local_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.vector.local_data_dir = Some(dir.into());
        self
    }

    /// Select the remote Qdrant engine; requires the `vector-qdrant` feature.
    ///
    /// The engine's own `enabled` switch is forced on: selecting the engine
    /// here *is* the enablement, and the embedded surface has no
    /// `[vector.qdrant]` section where a half-configured engine could linger.
    /// Use [`Self::with_vector_enabled`] to turn the subsystem off.
    #[cfg(feature = "vector-qdrant")]
    pub fn with_vector_qdrant(mut self, mut config: linkrs_config::QdrantConfig) -> Self {
        config.enabled = true;
        self.vector.engine = VectorEngineKind::Qdrant;
        self.vector.qdrant = config;
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
        assert!(!config.checkpoint_on_close);
        assert!(config.enable_snapshots);
        assert_eq!(config.snapshot_retention_count, 10);
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
