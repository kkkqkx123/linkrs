use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MigrationConfig {
    pub batch_size: usize,
    pub lock_ttl_secs: u64,
    pub checkpoint_dir: Option<PathBuf>,
    pub lock_path: Option<PathBuf>,
    /// Minimum free disk space required on the checkpoint directory
    /// filesystem before execution starts. 0 disables the preflight check.
    pub min_free_bytes: u64,
    /// Bounded stall for the schema-switch drain window. A timeout aborts
    /// the migration instead of stretching the write stall.
    pub drain_timeout_ms: u64,
    /// Directory for pre-migration backups of destructive steps.
    pub backup_dir: Option<PathBuf>,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            lock_ttl_secs: 300,
            checkpoint_dir: None,
            lock_path: None,
            min_free_bytes: 0,
            drain_timeout_ms: 5000,
            backup_dir: None,
        }
    }
}

impl MigrationConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    pub fn with_lock_ttl(mut self, ttl_secs: u64) -> Self {
        self.lock_ttl_secs = ttl_secs;
        self
    }

    pub fn with_checkpoint_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.checkpoint_dir = Some(dir.into());
        self
    }

    pub fn with_lock_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.lock_path = Some(path.into());
        self
    }

    pub fn with_min_free_bytes(mut self, min_free_bytes: u64) -> Self {
        self.min_free_bytes = min_free_bytes;
        self
    }

    pub fn with_drain_timeout_ms(mut self, drain_timeout_ms: u64) -> Self {
        self.drain_timeout_ms = drain_timeout_ms;
        self
    }

    pub fn with_backup_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.backup_dir = Some(dir.into());
        self
    }
}
