//! Checkpoint and dirty-page observability.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use super::core::StatsManager;
use super::metric_type::MetricType;

/// Checkpoint summary for handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSnapshot {
    pub trigger_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub avg_duration_us: f64,
    pub data_flushed_bytes: u64,
    pub wal_files_truncated: u64,
    pub triggered_by_wal_size: u64,
    pub triggered_by_interval: u64,
    pub triggered_explicit: u64,
    pub deduplicated: u64,
    pub blocked: u64,
}

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

    pub fn checkpoint_snapshot(&self) -> CheckpointSnapshot {
        let v = |m: MetricType| self.get_value(m).unwrap_or(0);
        let success = v(MetricType::CheckpointSuccessCount);
        CheckpointSnapshot {
            trigger_count: v(MetricType::CheckpointTriggerCount),
            success_count: success,
            failure_count: v(MetricType::CheckpointFailureCount),
            avg_duration_us: if success == 0 {
                0.0
            } else {
                v(MetricType::CheckpointDurationUs) as f64 / success as f64
            },
            data_flushed_bytes: v(MetricType::CheckpointDataFlushedBytes),
            wal_files_truncated: v(MetricType::CheckpointWalFilesTruncated),
            triggered_by_wal_size: v(MetricType::CheckpointTriggeredByWalSize),
            triggered_by_interval: v(MetricType::CheckpointTriggeredByInterval),
            triggered_explicit: v(MetricType::CheckpointTriggeredExplicit),
            deduplicated: v(MetricType::CheckpointRequestsDeduplicated),
            blocked: v(MetricType::CheckpointRequestsBlocked),
        }
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
