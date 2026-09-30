//! Sync pipeline observability: outbox, generations, transport.
use super::core::StatsManager;
use super::metric_type::MetricType;

/// Statistics Manager
///
/// Unified management of query metrics, query profiling and error statistics.
#[derive(Debug, Clone, Copy)]
pub struct OutboxState {
    pub pending: u64,
    pub retries: u64,
    pub dead_lettered: u64,
    pub leased: u64,
    pub oldest_event_age_ms: u64,
    pub frontier_lag: u64,
    pub degraded: bool,
}

impl StatsManager {
    pub fn record_sync_operation(&self, latency_ms: u64, success: bool) {
        self.add_value(MetricType::SyncOperations);
        self.add_value_with_amount(MetricType::SyncLatencyMs, latency_ms);
        if !success {
            self.add_value(MetricType::SyncErrors);
        }
    }

    pub fn record_sync_error(&self) {
        self.add_value(MetricType::SyncErrors);
    }

    pub fn set_sync_queue_depth(&self, depth: u64) {
        self.set_value(MetricType::SyncQueueDepth, depth);
    }

    pub fn record_outbox_state(&self, state: OutboxState) {
        self.set_value(MetricType::OutboxPending, state.pending);
        self.set_value(MetricType::OutboxRetryCount, state.retries);
        self.set_value(MetricType::OutboxDeadLetterCount, state.dead_lettered);
        self.set_value(MetricType::OutboxLeasedCount, state.leased);
        self.set_value(
            MetricType::OutboxOldestEventAgeMs,
            state.oldest_event_age_ms,
        );
        self.set_value(MetricType::OutboxFrontierLag, state.frontier_lag);
        self.set_value(MetricType::OutboxDegraded, u64::from(state.degraded));
    }

    pub fn record_target_frontier_lag(&self, target: &str, lag: u64) {
        // Per-target lag uses the `target:{name}` namespace so it never
        // collides with `space_{id}` buckets. Both global and per-target
        // values are point-in-time gauges and use overwrite semantics.
        self.set_value(MetricType::TargetFrontierLag, lag);
        self.set_space_metric_with_amount(
            &Self::target_key(target),
            MetricType::TargetFrontierLag,
            lag,
        );
    }

    pub fn record_generation_build(&self) {
        self.add_value(MetricType::GenerationBuildCount);
    }

    pub fn record_generation_publish(&self) {
        self.add_value(MetricType::GenerationPublishCount);
    }

    pub fn record_generation_rebuild_failure(&self) {
        self.add_value(MetricType::GenerationRebuildFailures);
    }

    pub fn record_inconsistent_index_count(&self, count: u64) {
        self.set_value(MetricType::InconsistentIndexCount, count);
    }

    pub fn record_rebuild_phase_latency(&self, target: &str, phase: &str, latency_ms: u64) {
        // `rebuild:{target}:{phase}` namespace plus an ops count so means
        // are computable.
        self.add_value(MetricType::RebuildPhaseOps);
        self.add_value_with_amount(MetricType::RebuildPhaseLatencyMs, latency_ms);
        let key = Self::rebuild_key(target, phase);
        self.add_space_metric(&key, MetricType::RebuildPhaseOps);
        self.add_space_metric_with_amount(&key, MetricType::RebuildPhaseLatencyMs, latency_ms);
    }

    pub fn record_split(&self, success: bool) {
        if success {
            self.add_value(MetricType::SplitCount);
        } else {
            self.add_value(MetricType::SplitFailures);
        }
    }

    pub fn record_reclaimed_index_files(&self, count: u64) {
        self.add_value_with_amount(MetricType::ReclaimedIndexFiles, count);
    }

    pub fn set_manifest_state(&self, active_readers: u64, retired_generations: u64) {
        self.set_value(MetricType::ManifestActiveReaders, active_readers);
        self.set_value(MetricType::ManifestRetiredGenerations, retired_generations);
    }

    pub fn record_fence_failure(&self) {
        self.add_value(MetricType::FenceFailures);
    }

    pub fn record_transport_latency(&self, latency_ms: u64) {
        // Sum plus count so the mean is computable.
        self.add_value(MetricType::TransportOps);
        self.add_value_with_amount(MetricType::TransportLatencyMs, latency_ms);
    }

    pub fn record_materializer_latency(&self, latency_ms: u64) {
        self.add_value(MetricType::MaterializerOps);
        self.add_value_with_amount(MetricType::MaterializerLatencyMs, latency_ms);
    }

    pub fn set_snapshot_lag(&self, lag: u64) {
        self.set_value(MetricType::SnapshotLag, lag);
    }

    // Index scan/write timing is covered by `record_index_operation`
    // (per-space isolated); the global-only variants are removed to avoid
    // implying a second uncovered path.
}
