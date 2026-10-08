//! Storage engine observability: I/O, bloom filters and space health.
use serde::{Deserialize, Serialize};

use super::core::StatsManager;
use super::metric_type::MetricType;

/// Storage subsystem snapshot for handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSnapshot {
    pub read_ops: u64,
    pub write_ops: u64,
    pub avg_read_latency_us: f64,
    pub avg_write_latency_us: f64,
    pub storage_errors: u64,
    pub index_memory_bytes: u64,
    pub tombstone_count: u64,
    pub tombstone_memory_bytes: u64,
    pub fragmentation_permille: u64,
    pub wasted_bytes: u64,
    pub bloom_queries: u64,
    pub bloom_hits: u64,
    pub bloom_hit_rate: f64,
    pub dirty_pages: u64,
    pub dirty_pages_total: u64,
    pub dirty_page_ratio_permille: u64,
    pub checkpoint_strategy_incremental: u64,
    pub checkpoint_strategy_hybrid: u64,
    pub checkpoint_strategy_full: u64,
    pub checkpoint_incremental_duration_us: u64,
    pub checkpoint_incremental_bytes_flushed: u64,
}

fn avg(total: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64
    }
}

impl StatsManager {
    pub fn record_storage_read(&self, latency_us: u64) {
        self.add_value(MetricType::StorageReadOps);
        self.add_value_with_amount(MetricType::StorageReadLatencyUs, latency_us);
    }

    /// Record a storage write with latency so means are computable.
    pub fn record_storage_write(&self, latency_us: u64) {
        self.add_value(MetricType::StorageWriteOps);
        self.add_value_with_amount(MetricType::StorageWriteLatencyUs, latency_us);
    }

    /// Record a storage error.
    pub fn record_storage_error(&self) {
        self.add_value(MetricType::StorageErrors);
    }

    /// Record a bloom pre-check query and whether it was positive.
    /// Hit rate derives from `BloomHits / BloomQueries`; permille is exported
    /// for dashboards via `BloomHitRatePermille`. The triple update holds
    /// `bloom_lock` so queries/hits/permille stay atomic under concurrency.
    pub fn record_bloom_query(&self, hit: bool) {
        let _guard = self.bloom_lock.lock();
        self.add_value(MetricType::BloomQueries);
        if hit {
            self.add_value(MetricType::BloomHits);
        }
        if let (Some(queries), Some(hits)) = (
            self.get_value(MetricType::BloomQueries),
            self.get_value(MetricType::BloomHits),
        ) {
            if queries > 0 {
                let permille = hits.saturating_mul(1000) / queries.max(1);
                self.set_value(MetricType::BloomHitRatePermille, permille);
            }
        }
    }

    /// Snapshot-export bloom counters from a shard runtime.
    /// Holds `bloom_lock` for the same atomicity as `record_bloom_query`.
    /// Callers must pass deltas since the last export, not cumulative
    /// totals, to avoid double counting.
    pub fn record_bloom_snapshot(&self, queries: u64, hits: u64) {
        let _guard = self.bloom_lock.lock();
        self.add_value_with_amount(MetricType::BloomQueries, queries);
        self.add_value_with_amount(MetricType::BloomHits, hits);
        let total_queries = self.get_value(MetricType::BloomQueries).unwrap_or(0);
        let total_hits = self.get_value(MetricType::BloomHits).unwrap_or(0);
        if total_queries > 0 {
            self.set_value(
                MetricType::BloomHitRatePermille,
                total_hits.saturating_mul(1000) / total_queries.max(1),
            );
        }
    }

    pub fn set_index_memory_usage(&self, bytes: u64) {
        self.set_value(MetricType::IndexMemoryUsage, bytes);
    }

    pub fn storage_snapshot(&self) -> StorageSnapshot {
        let v = |m: MetricType| self.get_value(m).unwrap_or(0);
        let read_ops = v(MetricType::StorageReadOps);
        let write_ops = v(MetricType::StorageWriteOps);
        let bloom_queries = v(MetricType::BloomQueries);
        let bloom_hits = v(MetricType::BloomHits);
        StorageSnapshot {
            read_ops,
            write_ops,
            avg_read_latency_us: avg(v(MetricType::StorageReadLatencyUs), read_ops),
            avg_write_latency_us: avg(v(MetricType::StorageWriteLatencyUs), write_ops),
            storage_errors: v(MetricType::StorageErrors),
            index_memory_bytes: v(MetricType::IndexMemoryUsage),
            tombstone_count: v(MetricType::TombstoneCount),
            tombstone_memory_bytes: v(MetricType::TombstoneMemoryBytes),
            fragmentation_permille: v(MetricType::FragmentationRatioPermille),
            wasted_bytes: v(MetricType::TopologyWastedBytes),
            bloom_queries,
            bloom_hits,
            bloom_hit_rate: if bloom_queries == 0 {
                0.0
            } else {
                bloom_hits as f64 / bloom_queries as f64
            },
            dirty_pages: v(MetricType::DirtyPagesCount),
            dirty_pages_total: v(MetricType::DirtyPagesTotal),
            dirty_page_ratio_permille: v(MetricType::DirtyPageRatioPermille),
            checkpoint_strategy_incremental: v(MetricType::CheckpointStrategyIncremental),
            checkpoint_strategy_hybrid: v(MetricType::CheckpointStrategyHybrid),
            checkpoint_strategy_full: v(MetricType::CheckpointStrategyFull),
            checkpoint_incremental_duration_us: v(MetricType::CheckpointIncrementalDurationUs),
            checkpoint_incremental_bytes_flushed: v(MetricType::CheckpointIncrementalBytesFlushed),
        }
    }

    /// Record tombstone statistics for MVCC observability
    pub fn record_tombstone_stats(
        &self,
        count: u64,
        memory_bytes: u64,
        oldest_ts_min: Option<u32>,
        newest_ts_max: Option<u32>,
        active_snapshots: u64,
    ) {
        self.set_value(MetricType::TombstoneCount, count);
        self.set_value(MetricType::TombstoneMemoryBytes, memory_bytes);
        self.set_value(MetricType::TombstoneActiveSnapshots, active_snapshots);

        if let Some(ts) = oldest_ts_min {
            self.set_value(MetricType::TombstoneOldestTsMin, ts as u64);
        }
        if let Some(ts) = newest_ts_max {
            self.set_value(MetricType::TombstoneNewestTsMax, ts as u64);
        }
    }

    /// Record storage fragmentation on the single wasted-share caliber.
    pub fn record_fragmentation_stats(&self, ratio: f32, wasted_bytes: u64) {
        let clamped = ratio.clamp(0.0, 1.0);
        self.set_value(
            MetricType::FragmentationRatioPermille,
            (clamped * 1000.0) as u64,
        );
        self.set_value(MetricType::TopologyWastedBytes, wasted_bytes);
    }
}
