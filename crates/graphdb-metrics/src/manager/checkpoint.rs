//! Checkpoint and dirty-page observability.
use std::collections::HashMap;
use std::time::Duration;

use super::core::StatsManager;
use super::metric_type::MetricType;

/// Reason a checkpoint was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointTriggerReason {
    WalSizeExceeded,
    TimeSinceLastCheckpoint,
    Explicit,
}

impl StatsManager {
    /// Record a successful checkpoint operation.
    pub fn record_checkpoint_success(
        &self,
        duration: Duration,
        bytes_flushed: u64,
        wal_files_truncated: u64,
    ) {
        self.add_value(MetricType::CheckpointSuccessCount);
        self.add_value_with_amount(
            MetricType::CheckpointDurationUs,
            duration.as_micros() as u64,
        );
        self.add_value_with_amount(MetricType::CheckpointDataFlushedBytes, bytes_flushed);
        self.add_value_with_amount(MetricType::CheckpointWalFilesTruncated, wal_files_truncated);
    }

    /// Record a failed checkpoint operation.
    pub fn record_checkpoint_failure(&self) {
        self.add_value(MetricType::CheckpointFailureCount);
    }

    /// Record a checkpoint trigger event.
    pub fn record_checkpoint_trigger(&self, reason: CheckpointTriggerReason, age: Duration) {
        self.add_value(MetricType::CheckpointTriggerCount);
        self.add_value_with_amount(MetricType::CheckpointTriggerAgeSecs, age.as_secs());
        match reason {
            CheckpointTriggerReason::WalSizeExceeded => {
                self.add_value(MetricType::CheckpointTriggeredByWalSize);
            }
            CheckpointTriggerReason::TimeSinceLastCheckpoint => {
                self.add_value(MetricType::CheckpointTriggeredByInterval);
            }
            CheckpointTriggerReason::Explicit => {
                self.add_value(MetricType::CheckpointTriggeredExplicit);
            }
        }
    }

    /// Record a deduplicated checkpoint request.
    pub fn record_checkpoint_deduplicated(&self) {
        self.add_value(MetricType::CheckpointRequestsDeduplicated);
    }

    /// Record a checkpoint request that found another checkpoint in progress.
    pub fn record_checkpoint_blocked(&self) {
        self.add_value(MetricType::CheckpointRequestsBlocked);
    }

    /// Get all checkpoint-related metrics.
    pub fn get_checkpoint_metrics(&self) -> HashMap<MetricType, u64> {
        let mut out = HashMap::new();
        for mt in [
            MetricType::CheckpointTriggerCount,
            MetricType::CheckpointSuccessCount,
            MetricType::CheckpointFailureCount,
            MetricType::CheckpointDurationUs,
            MetricType::CheckpointDataFlushedBytes,
            MetricType::CheckpointWalFilesTruncated,
            MetricType::CheckpointTriggerAgeSecs,
            MetricType::CheckpointTriggeredByWalSize,
            MetricType::CheckpointTriggeredByInterval,
            MetricType::CheckpointTriggeredExplicit,
            MetricType::CheckpointRequestsDeduplicated,
            MetricType::CheckpointRequestsBlocked,
            MetricType::CheckpointStrategyIncremental,
            MetricType::CheckpointStrategyHybrid,
            MetricType::CheckpointStrategyFull,
            MetricType::CheckpointIncrementalDurationUs,
            MetricType::CheckpointIncrementalBytesFlushed,
        ] {
            if let Some(v) = self.get_value(mt) {
                out.insert(mt, v);
            }
        }
        out
    }

    pub fn record_dirty_pages(&self, dirty: u64, total: u64) {
        self.set_value(MetricType::DirtyPagesCount, dirty);
        self.set_value(MetricType::DirtyPagesTotal, total);
        let ratio_permille = if total == 0 {
            0
        } else {
            (dirty as f64 / total as f64 * 1000.0) as u64
        };
        self.set_value(MetricType::DirtyPageRatioPermille, ratio_permille);
    }

    pub fn record_checkpoint_strategy_by_name(&self, strategy: &str) {
        match strategy {
            "incremental" => self.add_value(MetricType::CheckpointStrategyIncremental),
            "hybrid" => self.add_value(MetricType::CheckpointStrategyHybrid),
            "full" => self.add_value(MetricType::CheckpointStrategyFull),
            _ => {}
        }
    }

    pub fn record_incremental_checkpoint(&self, duration: Duration, bytes: u64) {
        self.add_value_with_amount(
            MetricType::CheckpointIncrementalDurationUs,
            duration.as_micros() as u64,
        );
        self.add_value_with_amount(MetricType::CheckpointIncrementalBytesFlushed, bytes);
    }

    pub fn get_dirty_page_metrics(&self) -> HashMap<MetricType, u64> {
        let mut out = HashMap::new();
        for mt in [
            MetricType::DirtyPagesTotal,
            MetricType::DirtyPagesCount,
            MetricType::DirtyPageRatioPermille,
        ] {
            if let Some(v) = self.get_value(mt) {
                out.insert(mt, v);
            }
        }
        out
    }
}
