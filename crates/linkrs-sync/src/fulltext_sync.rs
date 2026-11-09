use std::sync::Arc;

use linkrs_fulltext::manager::FulltextIndexManager;
use linkrs_metrics::StatsManager;

pub struct FulltextSyncCoordinator {
    fulltext_manager: Arc<FulltextIndexManager>,
    stats_manager: Option<Arc<StatsManager>>,
}

impl std::fmt::Debug for FulltextSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("FulltextSyncCoordinator");
        d.field("has_stats", &self.stats_manager.is_some());
        d.finish_non_exhaustive()
    }
}

impl FulltextSyncCoordinator {
    pub fn new(fulltext_manager: Arc<FulltextIndexManager>) -> Self {
        Self {
            fulltext_manager,
            stats_manager: None,
        }
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn fulltext_manager(&self) -> &Arc<FulltextIndexManager> {
        &self.fulltext_manager
    }
}

#[derive(Debug, Default)]
pub struct RecoveryResult {
    pub total: usize,
    pub recovered: usize,
    pub failed: usize,
    pub total_attempted: usize,
}

impl RecoveryResult {
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn recovered(&self) -> usize {
        self.recovered
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    pub fn is_complete(&self) -> bool {
        self.failed == 0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FulltextCoordinatorError {
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Operation not supported: {0}")]
    NotSupported(String),

    #[error("Recovery error: {0}")]
    RecoveryError(String),

    #[error("Search error: {0}")]
    SearchError(String),
}
