use std::time::Duration;

use crate::search::SyncFailurePolicy;

/// Batch processing configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of operations before auto-commit
    pub batch_size: usize,
    /// Time interval for automatic batch commits
    pub flush_interval: Duration,
    /// Maximum number of operations that can be buffered
    pub max_buffer_size: usize,
    /// Whether to enable persistence for batch operations
    pub enable_persistence: bool,
    /// Path for persistence storage
    pub persistence_path: Option<std::path::PathBuf>,
    /// Policy for handling sync failures
    pub failure_policy: SyncFailurePolicy,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            flush_interval: Duration::from_secs(1),
            max_buffer_size: 10000,
            enable_persistence: false,
            persistence_path: None,
            failure_policy: SyncFailurePolicy::FailOpen,
        }
    }
}

impl BatchConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    pub fn with_max_buffer_size(mut self, size: usize) -> Self {
        self.max_buffer_size = size;
        self
    }

    pub fn with_persistence(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.enable_persistence = path.is_some();
        self.persistence_path = path;
        self
    }

    pub fn with_failure_policy(mut self, policy: SyncFailurePolicy) -> Self {
        self.failure_policy = policy;
        self
    }
}

impl From<crate::sync::SyncConfig> for BatchConfig {
    fn from(old: crate::sync::SyncConfig) -> Self {
        Self {
            batch_size: old.batch_size,
            flush_interval: Duration::from_millis(old.commit_interval_ms),
            max_buffer_size: old.queue_size,
            enable_persistence: false,
            persistence_path: None,
            failure_policy: old.failure_policy,
        }
    }
}
