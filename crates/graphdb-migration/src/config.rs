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
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            lock_ttl_secs: 300,
            checkpoint_dir: None,
            lock_path: None,
            min_free_bytes: 0,
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
}
