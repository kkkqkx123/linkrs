//! Storage engine observability: I/O, bloom filters and space health.
use super::core::StatsManager;
use super::metric_type::MetricType;

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
